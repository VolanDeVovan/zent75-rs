//! Top-level keyboard state machine: config banks + engine + key processing + lighting +
//! configurator protocol. The platform (board or simulator) drives it:
//!
//! ```text
//! boot(storage)                       once, loads the banks (or factory defaults)
//! scan(samples)                       every full scan (8 kHz), 88 averaged ADC samples
//! hid()                               current HID state; send reports when hid_generation() moves
//! vendor_in(report, storage, out)     a 1024-byte output report from the vendor interface;
//!                                     returns the reply length written to `out`, if any
//! vendor_poll(out)                    unsolicited frames (BE FD stream, BE C0 notice)
//! service(storage)                    deferred work: saves, key-triggered profile switches
//! take_system_action()                bootloader / reboot requests for the platform
//! render_leds(now_ms, out)            one lighting frame
//! ```

use crate::config::{self, Bank, Global, Profile, BANK1_SIZE, BANK2_SIZE};
use crate::defaults;
use crate::engine::Engine;
use crate::hid::HidState;
use crate::keys::KeyProcessor;
use crate::layout::{KEY_COUNT, LED_COUNT, PROFILE_COUNT, SLOT_COUNT};
use crate::lighting::Lighting;
use crate::protocol::{self, cmd, parse, Reply, Request, REPORT_SIZE};
use crate::storage::{Slot, Storage};

/// Firmware version reported in `soft_version`. The panel gates DKS/key groups on patch ≥ 18.
pub const VERSION: (u8, u8, u8) = (1, 0, 18);

/// Heartbeat lifetime: the stream stops this many scans after the last `BE 00` (≈ 3 s).
const HEARTBEAT_SCANS: u32 = 3 * 8000;
/// Stream cadence: one `BE FD` frame every 320 scans (25 Hz), like the stock firmware.
const STREAM_EVERY: u32 = 320;

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
    save_global: bool,
    save_profile: bool,
    switch_to: Option<u8>,
    reset_config: bool,
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
}

impl Keyboard {
    pub fn new(serial: [u8; 16]) -> Self {
        Self {
            global: Global::zeroed(),
            profile: Profile::zeroed(),
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
        }
    }

    /// Load the banks from storage, falling back to factory defaults.
    pub fn boot(&mut self, st: &mut impl Storage) {
        if st.load(Slot::Global, &mut self.global.0).is_err() {
            defaults::global(&mut self.global);
            let _ = st.store(Slot::Global, &self.global.0);
        }
        let (ma, mi, pa) = VERSION;
        self.global.set_version(ma, mi, pa);
        let p = self.global.active_profile();
        self.load_profile(st, p);
        self.apply_config();
    }

    fn load_profile(&mut self, st: &mut impl Storage, p: usize) {
        if st.load(Slot::Profile(p as u8), &mut self.profile.0).is_err() {
            defaults::profile(&mut self.profile, p);
            let _ = st.store(Slot::Profile(p as u8), &self.profile.0);
        }
    }

    /// Re-derive everything that depends on the config (stock `FUN_0000A6F8`).
    fn apply_config(&mut self) {
        self.engine.configure(&self.global, &self.profile);
        self.keys.configure(&self.profile);
        self.lighting.configure(&self.profile);
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
        self.lighting.on_scan(&self.keys);
        if self.heartbeat_left > 0 {
            self.heartbeat_left -= 1;
        }
    }

    fn on_key_event(&mut self, e: crate::keys::Event) {
        use crate::keys::Event::*;
        match e {
            Bootloader => self.pending.action = Some(SystemAction::EnterBootloader),
            Reboot => self.pending.action = Some(SystemAction::Reboot),
            FactoryReset => self.pending.reset_config = true,
            SwitchProfile(p) => self.pending.switch_to = Some(p),
            ProfileChanged | ConfigChanged => self.pending.save_profile = true,
            Lighting(l) => {
                self.lighting.command(l, &mut self.profile);
                self.pending.save_profile = true;
            }
        }
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
        self.lighting.render(now_ms, &self.profile, &self.keys, &self.engine, out);
    }

    // ------------------------------------------------------------------ protocol

    /// Handle one vendor output report. Returns the length of a reply written to `out`.
    pub fn vendor_in(&mut self, report: &[u8], st: &mut impl Storage, out: &mut [u8; REPORT_SIZE]) -> Option<usize> {
        let req = parse(report).ok()?;
        match req {
            Request::Heartbeat => {
                self.heartbeat_left = HEARTBEAT_SCANS;
                None
            }
            Request::Save => {
                let _ = st.store(Slot::Global, &self.global.0);
                let p = self.active_profile() as u8;
                let _ = st.store(Slot::Profile(p), &self.profile.0);
                None
            }
            Request::WriteRam { addr, data } => {
                let (bank, off) = config::locate(addr, data.len())?;
                self.write_mem(bank, off, data, addr, st);
                Some(Reply::new(out).simple(cmd::WRITE_RAM))
            }
            Request::ReadRam { addr, n } | Request::ReadFlash { addr, n } => {
                let (bank, off) = config::locate(addr, n as usize)?;
                let c = if matches!(req, Request::ReadRam { .. }) { cmd::READ_RAM } else { cmd::READ_FLASH };
                let src: &[u8] = match bank {
                    Bank::Global => &self.global.0,
                    Bank::Profile => &self.profile.0,
                };
                Some(Reply::new(out).data(c, addr, n, |d| {
                    for (i, b) in d.iter_mut().enumerate() {
                        *b = src.get(off + i).copied().unwrap_or(0);
                    }
                }))
            }
            Request::Serial => Some(Reply::new(out).serial(&self.serial)),
            Request::SwitchProfile(p) => {
                if (p as usize) < PROFILE_COUNT {
                    self.switch_profile(st, p);
                }
                None
            }
            Request::Diag(v) => {
                if v == 2 || v == 3 {
                    self.diag_flag = v == 3;
                }
                None
            }
            Request::ResetConfig => {
                self.reset_config(st);
                None
            }
            Request::Bootloader => {
                self.pending.action = Some(SystemAction::EnterBootloader);
                None
            }
            Request::Calibration(on) => {
                self.calibration = on;
                self.engine.set_calibration(on);
                None
            }
            Request::Reboot => {
                self.pending.action = Some(SystemAction::Reboot);
                None
            }
        }
    }

    fn write_mem(&mut self, bank: Bank, off: usize, data: &[u8], addr: u32, st: &mut impl Storage) {
        match bank {
            Bank::Global => {
                if addr == config::TRIGGER_BRIGHTNESS_A || addr == config::TRIGGER_BRIGHTNESS_B {
                    self.lighting.brightness_trigger(addr, data, &mut self.profile);
                    return;
                }
                let end = (off + data.len()).min(BANK1_SIZE);
                if off < end {
                    // soft_version and cur_param_num are firmware-owned.
                    let saved = self.global.0[config::b1::SOFT_VERSION..config::b1::SOFT_VERSION + 12].to_owned_array();
                    self.global.0[off..end].copy_from_slice(&data[..end - off]);
                    self.global.0[config::b1::SOFT_VERSION..config::b1::SOFT_VERSION + 12].copy_from_slice(&saved);
                    // Bank-1 writes save immediately (stock behaviour).
                    let _ = st.store(Slot::Global, &self.global.0);
                }
            }
            Bank::Profile => {
                let end = (off + data.len()).min(BANK2_SIZE);
                if off < end {
                    self.profile.0[off..end].copy_from_slice(&data[..end - off]);
                }
            }
        }
        self.apply_config();
    }

    fn switch_profile(&mut self, st: &mut impl Storage, p: u8) {
        // Unsaved RAM edits of the old profile are dropped, like the stock firmware.
        self.global.set_active_profile(p as usize);
        let _ = st.store(Slot::Global, &self.global.0);
        self.load_profile(st, p as usize);
        self.apply_config();
        self.keys.reset();
        self.pending.profile_notice = Some(p);
    }

    fn reset_config(&mut self, st: &mut impl Storage) {
        let p = self.active_profile();
        defaults::global_reset(&mut self.global);
        for i in 0..PROFILE_COUNT {
            defaults::profile(&mut self.profile, i);
            let _ = st.store(Slot::Profile(i as u8), &self.profile.0);
        }
        self.global.set_active_profile(0);
        let _ = p;
        let _ = st.store(Slot::Global, &self.global.0);
        defaults::profile(&mut self.profile, 0);
        self.apply_config();
        self.keys.reset();
        self.pending.profile_notice = Some(0);
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
        if core::mem::take(&mut self.pending.reset_config) {
            self.reset_config(st);
        }
        if let Some(p) = self.pending.switch_to.take() {
            self.switch_profile(st, p);
        }
        if core::mem::take(&mut self.pending.save_profile) {
            let p = self.active_profile() as u8;
            let _ = st.store(Slot::Profile(p), &self.profile.0);
        }
        if core::mem::take(&mut self.pending.save_global) {
            let _ = st.store(Slot::Global, &self.global.0);
        }
    }

    pub fn take_system_action(&mut self) -> Option<SystemAction> {
        self.pending.action.take()
    }

    pub fn calibration_mode(&self) -> bool {
        self.calibration
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
    pub fn keys_count() -> usize {
        KEY_COUNT
    }
}

trait ToOwnedArray {
    fn to_owned_array(&self) -> [u8; 12];
}
impl ToOwnedArray for [u8] {
    fn to_owned_array(&self) -> [u8; 12] {
        let mut a = [0u8; 12];
        a.copy_from_slice(&self[..12]);
        a
    }
}

#[allow(unused_imports)]
use protocol as _;
