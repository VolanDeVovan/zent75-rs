//! Top-level keyboard state machine: config banks + engine + key processing + lighting +
//! configurator protocol. The platform (board or simulator) drives it:
//!
//! ```text
//! boot(storage)                       once, loads + validates the banks (repairs like stock)
//! scan(samples)                       every full scan (8 kHz), 88 averaged ADC samples
//! hid()                               current HID state; send reports when hid_generation() moves
//! vendor_in(report, storage, out)     a 1024-byte output report from the vendor interface;
//!                                     returns the reply length written to `out`, if any
//! vendor_poll(out)                    unsolicited frames (BE FD stream, BE C0 notice)
//! service(storage)                    deferred work: profile switches, full wipe
//! take_system_action()                bootloader / reboot requests for the platform
//! render_leds(now_ms, out)            one lighting frame (call every ≥ 5.8 ms)
//! set_usb_suspended / set_host_leds   bus state for the lighting
//! ```
//!
//! The dispatcher follows stock 0.0.18 `FUN_0000B812` (docs/re-config-rgb.md §3) with these
//! deliberate differences: writes never go past our shadows (the excess is dropped; stock
//! overwrote live RAM), reads outside the shadows return zeros (stock leaked stack), `BE 01`
//! is answered like `BE 04` (stock hangs), and the firmware-owned bank-1 words `soft_version` /
//! `cur_param_num` cannot be overwritten by `BE 03`.

use crate::config::{self, b1, Bank, Global, Profile, BANK1_SIZE, BANK2_SIZE};
use crate::defaults;
use crate::engine::Engine;
use crate::hid::HidState;
use crate::keys::KeyProcessor;
use crate::layout::{KEY_COUNT, LED_COUNT, PROFILE_COUNT, SLOT_COUNT};
use crate::lighting::Lighting;
use crate::protocol::{cmd, normalize, parse, Reply, Request, REPORT_SIZE, SOF};
use crate::storage::{Slot, Storage};

/// Firmware version reported in `soft_version`. The panel gates DKS/key groups on patch ≥ 18.
pub const VERSION: (u8, u8, u8) = (1, 0, 18);
/// [`VERSION`] as the `soft_version` word.
pub const VERSION_WORD: u32 = ((VERSION.0 as u32) << 16) | ((VERSION.1 as u32) << 8) | VERSION.2 as u32;

/// Heartbeat lifetime: the stream stops this many scans after the last `BE 00` (2 s, stock).
pub const HEARTBEAT_SCANS: u32 = 16000;
/// Stream cadence: one `BE FD` frame every 320 scans (25 Hz), like the stock firmware.
const STREAM_EVERY: u32 = 320;
/// Profile-switch cool-down set by every switch request (0.9 s at 8 kHz, stock).
pub const SWITCH_COOLDOWN_SCANS: u32 = 7200;
/// Full wipe (keycode 319): LEDs blink (60,60,60) 3 × 50 ms on/off, then reboot.
pub const WIPE_BLINK_SCANS: u32 = 2400;

/// Requests for the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAction {
    /// Quiesce and jump to the UF2 loader at 0x80020000.
    EnterBootloader,
    /// Watchdog reset.
    Reboot,
}

#[derive(Debug, Clone, Copy, Default)]
struct Pending {
    switch_to: Option<u8>,
    wipe: bool,
    reset_profile: bool,
    action: Option<SystemAction>,
    profile_notice: Option<u8>,
}

pub struct Keyboard {
    pub global: Global,
    pub profile: Profile,
    pub engine: Engine,
    pub keys: KeyProcessor,
    pub lighting: Lighting,
    serial: [u8; 16],
    heartbeat_left: u32,
    stream_ctr: u32,
    calibration: bool,
    pub diag_flag: bool,
    pending: Pending,
    hid_gen: u32,
    hid_last: HidState,
    scans: u32,
    switch_cooldown: u32,
    wipe_left: Option<u32>,
}

impl Keyboard {
    pub fn new(serial: [u8; 16]) -> Self {
        Self {
            global: Global::blank(),
            profile: Profile::blank(),
            engine: Engine::new(),
            keys: KeyProcessor::new(),
            lighting: Lighting::new(),
            serial,
            heartbeat_left: 0,
            stream_ctr: 0,
            calibration: false,
            diag_flag: false,
            pending: Pending::default(),
            hid_gen: 0,
            hid_last: HidState::default(),
            scans: 0,
            switch_cooldown: 0,
            wipe_left: None,
        }
    }

    /// Load the banks from storage. Missing/blank banks become the factory images and every
    /// bank goes through stock's validation + field-wise repair (stock `FUN_0000DD24`); a
    /// repaired bank is written back, a valid one is never touched.
    pub fn boot(&mut self, st: &mut impl Storage) {
        if st.load(Slot::Global, &mut self.global.0).is_err() {
            self.global = Global::blank();
        }
        if self.global.repair(VERSION_WORD) {
            let _ = st.store(Slot::Global, &self.global.0);
        }
        let p = self.global.active_profile();
        self.load_profile(st, p);
        self.apply_config();
    }

    /// Read profile `p` (stock `FUN_0001385C`): force/fix-up, validate, repair + write back.
    fn load_profile(&mut self, st: &mut impl Storage, p: usize) {
        if st.load(Slot::Profile(p as u8), &mut self.profile.0).is_err() {
            self.profile = Profile::blank();
        }
        if self.profile.load_fixup(p) {
            let _ = st.store(Slot::Profile(p as u8), &self.profile.0);
        }
    }

    /// Re-derive everything that depends on the config (stock `FUN_0000A6F8`): keys and
    /// lighting (the lighting reload also drops runtime-only speed changes).
    fn apply_config(&mut self) {
        self.engine.configure(&self.global, &self.profile);
        self.keys.configure(&self.profile);
        self.lighting.configure(&mut self.profile);
    }

    pub fn active_profile(&self) -> usize {
        self.global.active_profile()
    }

    // ------------------------------------------------------------------ scan path

    /// One full scan. `samples` are averaged ADC counts indexed by scan slot.
    pub fn scan(&mut self, samples: &[u16; SLOT_COUNT]) {
        self.scans = self.scans.wrapping_add(1);
        self.engine.scan(samples, &self.global, &self.profile, self.calibration);
        let ev = self.keys.update(&self.engine, &self.profile, &self.global);
        for e in ev.iter() {
            self.on_key_event(*e);
        }
        let h = *self.keys.hid();
        if h != self.hid_last {
            self.hid_last = h;
            self.hid_gen = self.hid_gen.wrapping_add(1);
        }
        self.lighting.on_scan(&self.keys, &self.profile);
        if self.heartbeat_left > 0 {
            self.heartbeat_left -= 1;
            if self.heartbeat_left == 0 {
                self.diag_flag = false; // stock clears the DA flag with the stream
            }
        }
        self.switch_cooldown = self.switch_cooldown.saturating_sub(1);
        if let Some(n) = self.wipe_left {
            let n = n.saturating_sub(1);
            self.lighting.set_wipe_blink(Some(((WIPE_BLINK_SCANS - n) / 400) % 2 == 0));
            if n == 0 {
                self.wipe_left = None;
                self.pending.action = Some(SystemAction::Reboot);
            } else {
                self.wipe_left = Some(n);
            }
        }
    }

    fn on_key_event(&mut self, e: crate::keys::Event) {
        use crate::keys::Event::*;
        match e {
            Bootloader => self.pending.action = Some(SystemAction::EnterBootloader),
            Reboot => self.pending.action = Some(SystemAction::Reboot),
            // keycode 319 held 5 s: erase bank 1 and all profiles, blink, reboot (stock)
            FactoryReset => self.pending.wipe = true,
            SwitchProfile(p) => self.request_switch(p, false),
            // key-driven changes are RAM only in stock (persisted by the next `BE 02`)
            ProfileChanged | ConfigChanged => self.apply_config(),
            Lighting(l) => self.lighting.command(l, &mut self.profile),
        }
    }

    /// Profile switch request: keys honour the cool-down, `BE C0` ignores it; both restart it.
    fn request_switch(&mut self, p: u8, ignore_cooldown: bool) {
        if (p as usize) >= PROFILE_COUNT || (!ignore_cooldown && self.switch_cooldown > 0) {
            return;
        }
        self.switch_cooldown = SWITCH_COOLDOWN_SCANS;
        self.pending.switch_to = Some(p);
    }

    /// Keycode 319 held 5 s: erase bank 1 and all profiles, blink, reboot (what the key
    /// layer's `Event::FactoryReset` does). Executed by the next [`Keyboard::service`].
    pub fn request_full_wipe(&mut self) {
        self.pending.wipe = true;
    }

    /// Keycode 305 (`INITSET`): reset the active profile to defaults and save it (like `BE EE`).
    /// Executed by the next [`Keyboard::service`].
    pub fn request_profile_reset(&mut self) {
        self.pending.reset_profile = true;
    }

    /// Changes whenever the HID state changes; the USB task sends reports on change (and the
    /// keyboard endpoint additionally every poll if it wants to).
    pub fn hid_generation(&self) -> u32 {
        self.hid_gen
    }
    pub fn hid(&self) -> &HidState {
        self.keys.hid()
    }
    pub fn set_host_leds(&mut self, leds: u8) {
        self.lighting.set_host_leds(leds);
    }

    // ------------------------------------------------------------------ lighting

    pub fn render_leds(&mut self, now_ms: u32, out: &mut [[u8; 3]; LED_COUNT]) {
        self.lighting.render(now_ms, &self.profile, &self.global, &self.keys, &self.engine, out);
    }

    // ------------------------------------------------------------------ protocol

    /// Handle one vendor output report. Returns the length of a reply written to `out`.
    pub fn vendor_in(&mut self, report: &[u8], st: &mut impl Storage, out: &mut [u8; REPORT_SIZE]) -> Option<usize> {
        let r = normalize(report);
        if r.first() == Some(&SOF) {
            // every frame that reaches the dispatcher keeps the LEDs awake (stock)
            self.lighting.note_activity();
        }
        let req = parse(r).ok()?;
        let len = u16::from_le_bytes([r[2], r[3]]) as usize;
        match req {
            Request::Heartbeat => {
                if len == 5 {
                    self.heartbeat_left = HEARTBEAT_SCANS;
                }
                None
            }
            Request::Save => {
                if len == 5 {
                    self.save_global(st);
                    self.save_profile(st);
                }
                None
            }
            Request::WriteRam { addr, data } => {
                let (bank, off) = config::locate_write(addr, data.len())?;
                match bank {
                    Bank::Global => self.write_global(off, data, st),
                    Bank::Profile => self.write_profile(off, data),
                    // live preview: RAM only, no ack (stock)
                    Bank::Preview => {
                        self.lighting.preview_write(off, data, &self.profile);
                        return None;
                    }
                }
                Some(Reply::new(out).simple(cmd::WRITE_RAM))
            }
            Request::ReadRam { addr, n } | Request::ReadFlash { addr, n } => {
                let c = if matches!(req, Request::ReadRam { .. }) { cmd::READ_RAM } else { cmd::READ_FLASH };
                let src: &[u8] = match config::locate_read(addr, n as usize) {
                    Some((Bank::Global, off)) => self.global.0.get(off..).unwrap_or(&[]),
                    Some((Bank::Profile, off)) => self.profile.0.get(off..).unwrap_or(&[]),
                    _ => &[],
                };
                Some(Reply::new(out).data(c, addr, n, |d| {
                    for (i, b) in d.iter_mut().enumerate() {
                        *b = src.get(i).copied().unwrap_or(0);
                    }
                }))
            }
            Request::Serial => (len == 5).then(|| Reply::new(out).serial(&self.serial)),
            Request::SwitchProfile(p) => {
                self.request_switch(p, true);
                None
            }
            Request::Diag(v) => {
                if v == 2 || v == 3 {
                    self.diag_flag = v == 3;
                }
                None
            }
            Request::ResetConfig => {
                if len == 5 {
                    self.reset_profile(st);
                }
                None
            }
            Request::Bootloader => {
                if len == 5 {
                    self.pending.action = Some(SystemAction::EnterBootloader);
                }
                None
            }
            Request::Calibration(on) => {
                self.calibration = on;
                self.engine.set_calibration(on);
                None
            }
            Request::Reboot => {
                if len == 5 {
                    self.pending.action = Some(SystemAction::Reboot);
                }
                None
            }
        }
    }

    /// Bank-1 save (stock `FUN_00015F5A`): store, then re-impose the fixed Fn shortcuts.
    fn save_global(&mut self, st: &mut impl Storage) {
        let _ = st.store(Slot::Global, &self.global.0);
        self.profile.apply_fixed_fn(self.active_profile());
    }

    fn save_profile(&mut self, st: &mut impl Storage) {
        let p = self.active_profile() as u8;
        let _ = st.store(Slot::Profile(p), &self.profile.0);
    }

    /// `BE 03` into bank 1: copied into the shadow, then saved and re-derived — unless any key
    /// now has `|bot − top| < 3000`, in which case the write is discarded (stock re-reads NOR;
    /// we restore the pre-write shadow, which is the same bytes). The ack follows the save.
    fn write_global(&mut self, off: usize, data: &[u8], st: &mut impl Storage) {
        let end = (off + data.len()).min(BANK1_SIZE);
        if off >= end {
            return; // entirely past the shadow (stock: preview area / live RAM)
        }
        // keep the overwritten bytes (≤ 512) for the discard case, not a copy of the bank
        let mut old = [0u8; 512];
        let n = end - off;
        old[..n].copy_from_slice(&self.global.0[off..end]);
        let words = &self.global.0[b1::SOFT_VERSION..b1::CUR_PARAM_NUM + 4];
        let mut owned = [0u8; 12];
        owned.copy_from_slice(words);
        self.global.0[off..end].copy_from_slice(&data[..n]);
        // firmware-owned words (deviation: stock lets the host overwrite them)
        self.global.0[b1::SOFT_VERSION..b1::CUR_PARAM_NUM + 4].copy_from_slice(&owned);
        if self.global.calibration_implausible() {
            self.global.0[off..end].copy_from_slice(&old[..n]);
            self.global.0[b1::SOFT_VERSION..b1::CUR_PARAM_NUM + 4].copy_from_slice(&owned);
            return;
        }
        self.save_global(st);
        self.apply_config();
    }

    /// `BE 03` into the profile: RAM only (saved by `BE 02`). Writes inside the SOCD / static
    /// colour window only reload the lighting, everything else re-derives keys + lighting.
    fn write_profile(&mut self, off: usize, data: &[u8]) {
        let end_req = off + data.len();
        let end = end_req.min(BANK2_SIZE);
        if off < end {
            self.profile.0[off..end].copy_from_slice(&data[..end - off]);
        }
        if off >= config::b2::LIGHT_WINDOW_START && end_req <= config::b2::LIGHT_WINDOW_END {
            self.lighting.configure(&mut self.profile);
        } else {
            self.apply_config();
        }
    }

    /// `BE EE` / keycode 305: defaults for the active profile only, saved, re-derived.
    fn reset_profile(&mut self, st: &mut impl Storage) {
        let p = self.active_profile();
        defaults::reset_profile(&mut self.profile, p);
        self.save_profile(st);
        self.apply_config();
    }

    /// Switch to profile `p` (stock `FUN_0001170E`). Unsaved RAM edits of the old profile are
    /// dropped. `cur_param_num` is *not* written to flash here — like stock, the next `BE 02`
    /// or bank-1 write persists it, so a power cycle returns to the last saved profile. The
    /// host is notified (also when `p` is already active) and the LEDs blink the profile colour.
    fn switch_profile(&mut self, st: &mut impl Storage, p: u8) {
        if p as usize != self.active_profile() {
            self.global.set_active_profile(p as usize);
            self.load_profile(st, p as usize);
            self.apply_config();
            self.keys.reset();
        }
        self.pending.profile_notice = Some(p);
        self.lighting.start_profile_blink(p);
    }

    /// Keycode 319: erase bank 1 and all profiles; the next boot is a first boot.
    fn wipe(&mut self, st: &mut impl Storage) {
        // blank images live in flash, not on the stack
        static BLANK1: [u8; BANK1_SIZE] = [0xFF; BANK1_SIZE];
        static BLANK2: [u8; BANK2_SIZE] = [0xFF; BANK2_SIZE];
        let _ = st.store(Slot::Global, &BLANK1);
        for p in 0..PROFILE_COUNT {
            let _ = st.store(Slot::Profile(p as u8), &BLANK2);
        }
        self.wipe_left = Some(WIPE_BLINK_SCANS);
    }

    /// Unsolicited frames: profile notice first, then the live stream while heartbeats arrive.
    pub fn vendor_poll(&mut self, out: &mut [u8; REPORT_SIZE]) -> Option<usize> {
        if let Some(p) = self.pending.profile_notice.take() {
            return Some(Reply::new(out).with_arg(cmd::SWITCH_PROFILE, p));
        }
        if self.heartbeat_left == 0 {
            return None;
        }
        if self.scans.wrapping_sub(self.stream_ctr) < STREAM_EVERY {
            return None;
        }
        self.stream_ctr = self.scans;
        Some(self.engine.stream_frame(out, self.diag_flag))
    }

    /// Deferred work that may touch flash. Call from a low-priority task.
    pub fn service(&mut self, st: &mut impl Storage) {
        if core::mem::take(&mut self.pending.wipe) && self.wipe_left.is_none() {
            self.wipe(st);
        }
        if core::mem::take(&mut self.pending.reset_profile) {
            self.reset_profile(st);
        }
        if let Some(p) = self.pending.switch_to.take() {
            self.switch_profile(st, p);
        }
    }

    pub fn take_system_action(&mut self) -> Option<SystemAction> {
        self.pending.action.take()
    }

    pub fn calibration_mode(&self) -> bool {
        self.calibration
    }
    pub fn keys_count() -> usize {
        KEY_COUNT
    }

    /// Current travel of `key` in µm (stable accessor; the engine internals may change).
    pub fn travel_um(&self, key: usize) -> u16 {
        self.engine.travel[key]
    }
    /// Filtered raw ADC count of `key`.
    pub fn raw(&self, key: usize) -> u16 {
        self.engine.raw[key]
    }
    /// True once the engine has its rest baseline.
    pub fn engine_ready(&self) -> bool {
        self.engine.ready()
    }
    /// USB bus suspended (or unconfigured): LEDs are blanked at the next frame, like stock.
    pub fn set_usb_suspended(&mut self, suspended: bool) {
        self.lighting.set_usb_suspended(suspended);
    }
    /// True while the `BE FD` stream is on (heartbeat seen within [`HEARTBEAT_SCANS`]).
    pub fn streaming(&self) -> bool {
        self.heartbeat_left > 0
    }
}
