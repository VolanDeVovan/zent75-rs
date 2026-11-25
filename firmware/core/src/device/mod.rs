//! The keyboard as one state machine: config banks + engine + key pipeline + lighting, plus the
//! configurator protocol ([`vendor`]). The platform (board or simulator) owns the hardware and
//! drives it:
//!
//! ```text
//! boot(serial, storage)               once: load + validate the banks (repairs like stock)
//! scan(samples)                       every full scan (8 kHz), 88 averaged ADC samples
//! hid() / hid_generation()            current input reports; send them when the generation moves
//! vendor_in(report, storage, out)     a 1024-byte output report from the vendor interface;
//!                                     returns the reply length written to `out`, if any
//! vendor_poll(out)                    unsolicited frames (BE FD stream, BE C0 notice)
//! service(storage)                    deferred work: profile switches, resets, the full wipe
//! take_system_action()                bootloader / reboot requests for the platform
//! render_leds(now_ms, out)            one lighting frame (call every ≥ 5.8 ms)
//! set_usb_suspended / set_host_leds   bus state for the lighting
//! second_tick(busy)                   once per second: stream statistics
//! ```
//!
//! `scan` is the only call made from the scan interrupt on the board; it never touches storage.
//! Everything that does (`boot`, `vendor_in`, `service`) takes the [`Storage`] as an argument, so
//! the platform decides where and when bytes are actually written.

mod vendor;

use crate::config::{Global, Profile};
use crate::defaults;
use crate::engine::Engine;
use crate::hid::HidReports;
use crate::keys::{self, KeyProcessor};
use crate::layout::{LED_COUNT, PROFILE_COUNT, SLOT_COUNT};
use crate::lighting::Lighting;
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
pub const SWITCH_COOLDOWN_SCANS: u32 = keys::SWITCH_COOLDOWN as u32;
/// Full wipe (Fn + Esc 5 s): LEDs blink (60,60,60) 3 × 50 ms on/off, then reboot.
pub const WIPE_BLINK_SCANS: u32 = 2400;
/// Half period of the wipe blink.
const WIPE_BLINK_HALF_SCANS: u32 = 400;

/// Requests for the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAction {
    /// Quiesce and jump to the UF2 loader at 0x80020000.
    EnterBootloader,
    /// Watchdog reset.
    Reboot,
}

/// Work requested from the scan path or the dispatcher and carried out by
/// [`Keyboard::service`] / the platform.
#[derive(Debug, Clone, Copy)]
struct Pending {
    switch_to: Option<u8>,
    wipe: bool,
    reset_profile: bool,
    action: Option<SystemAction>,
    profile_notice: Option<u8>,
}

pub struct Keyboard {
    global: Global,
    profile: Profile,
    engine: Engine,
    keys: KeyProcessor,
    lighting: Lighting,
    serial: [u8; 16],
    /// Scans until the `BE FD` stream stops (0 = off).
    heartbeat_left: u32,
    scans: u32,
    /// `scans` at the last stream frame.
    stream_mark: u32,
    pending: Pending,
    switch_cooldown: u32,
    /// Scans left of the wipe blink (then reboot).
    wipe_left: Option<u32>,
    storage_errors: u32,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Keyboard {
    /// `const` so the board can keep the keyboard in a `static` initialised at link time: the
    /// ~40 KB struct must never be built on its 16 KB stack.
    pub const fn new() -> Self {
        Self {
            global: Global::blank(),
            profile: Profile::blank(),
            engine: Engine::new(),
            keys: KeyProcessor::new(),
            lighting: Lighting::new(),
            serial: [0; 16],
            heartbeat_left: 0,
            scans: 0,
            stream_mark: 0,
            pending: Pending { switch_to: None, wipe: false, reset_profile: false, action: None, profile_notice: None },
            switch_cooldown: 0,
            wipe_left: None,
            storage_errors: 0,
        }
    }

    /// Load the banks from storage; `serial` is what `BE 1D` reports. Missing/blank banks become
    /// the factory images and every bank goes through stock's validation + field-wise repair
    /// (stock `FUN_0000DD24`); a repaired bank is written back, a valid one is never touched.
    pub fn boot(&mut self, serial: [u8; 16], st: &mut impl Storage) {
        self.serial = serial;
        if st.load(Slot::Global, &mut self.global.0).is_err() {
            self.global.0.fill(0xFF); // in place: no bank-sized temporary on the stack
        }
        if self.global.repair(VERSION_WORD) {
            self.save_global(st);
        }
        let p = self.global.active_profile();
        self.load_profile(st, p);
        self.apply_config();
    }

    /// Read profile `p` (stock `FUN_0001385C`): force/fix-up, validate, repair + write back.
    fn load_profile(&mut self, st: &mut impl Storage, p: usize) {
        if st.load(Slot::Profile(p as u8), &mut self.profile.0).is_err() {
            self.profile.0.fill(0xFF);
        }
        if self.profile.load_fixup(p) {
            self.save_profile(st);
        }
    }

    fn save_global(&mut self, st: &mut impl Storage) {
        if st.store(Slot::Global, &self.global.0).is_err() {
            self.storage_errors += 1;
        }
    }

    fn save_profile(&mut self, st: &mut impl Storage) {
        let slot = Slot::Profile(self.active_profile() as u8);
        if st.store(slot, &self.profile.0).is_err() {
            self.storage_errors += 1;
        }
    }

    /// Re-derive everything that depends on the config (stock `FUN_0000A6F8`): engine, keys and
    /// lighting (the lighting reload also drops runtime-only speed changes).
    fn apply_config(&mut self) {
        self.engine.configure(&self.global, &self.profile);
        self.keys.invalidate();
        self.lighting.configure(&mut self.profile);
    }

    // ------------------------------------------------------------------ state

    pub fn global(&self) -> &Global {
        &self.global
    }
    /// The active profile (RAM copy, including unsaved edits).
    pub fn profile(&self) -> &Profile {
        &self.profile
    }
    pub fn active_profile(&self) -> usize {
        self.global.active_profile()
    }
    pub fn engine(&self) -> &Engine {
        &self.engine
    }
    pub fn keys(&self) -> &KeyProcessor {
        &self.keys
    }
    pub fn lighting(&self) -> &Lighting {
        &self.lighting
    }
    /// True while the `BE FD` stream is on (heartbeat seen within [`HEARTBEAT_SCANS`]).
    pub fn streaming(&self) -> bool {
        self.heartbeat_left > 0
    }
    /// Stores the [`Storage`] refused or failed since boot. The RAM copy stays authoritative;
    /// the platform reports the details.
    pub fn storage_errors(&self) -> u32 {
        self.storage_errors
    }

    // ------------------------------------------------------------------ scan path

    /// One full scan. `samples` are averaged ADC counts indexed by scan slot.
    pub fn scan(&mut self, samples: &[u16; SLOT_COUNT]) {
        self.scans = self.scans.wrapping_add(1);
        self.engine.scan(samples);
        let events = self.keys.update(&self.engine, &self.profile);
        for e in events.iter() {
            self.on_key_event(*e);
        }
        self.lighting.on_scan(&self.keys);
        if self.heartbeat_left > 0 {
            self.heartbeat_left -= 1;
            if self.heartbeat_left == 0 {
                self.engine.set_diag(false); // stock clears the DA flag with the stream
            }
        }
        self.switch_cooldown = self.switch_cooldown.saturating_sub(1);
        if let Some(n) = self.wipe_left {
            let n = n.saturating_sub(1);
            self.lighting.set_wipe_blink(Some(((WIPE_BLINK_SCANS - n) / WIPE_BLINK_HALF_SCANS) % 2 == 0));
            if n == 0 {
                self.wipe_left = None;
                self.pending.action = Some(SystemAction::Reboot);
            } else {
                self.wipe_left = Some(n);
            }
        }
    }

    fn on_key_event(&mut self, e: keys::Event) {
        use keys::Event::*;
        match e {
            Bootloader => self.pending.action = Some(SystemAction::EnterBootloader),
            Reboot => self.pending.action = Some(SystemAction::Reboot),
            FactoryReset => self.request_full_wipe(),
            // level-triggered: repeats while held, like stock
            ResetProfile => self.pending.reset_profile = true,
            WinLock(on) => self.lighting.set_winlock(on),
            SwitchProfile(p) => self.request_switch(p, false),
            Lighting(l) => self.lighting.command(l, &mut self.profile),
        }
    }

    /// Profile switch request: keys honour the cool-down, `BE C0` ignores it; both restart it.
    fn request_switch(&mut self, p: u8, ignore_cooldown: bool) {
        if (p as usize) >= PROFILE_COUNT || (!ignore_cooldown && self.switch_cooldown > 0) {
            return;
        }
        self.switch_cooldown = SWITCH_COOLDOWN_SCANS;
        if ignore_cooldown {
            // `BE C0` also freezes the reports for 0.9 s (stock shares the key cool-down)
            self.keys.start_cooldown();
        }
        self.pending.switch_to = Some(p);
    }

    /// Fn + Esc held 5 s: erase the config (see [`Keyboard::wipe`]), blink, reboot. Carried out
    /// by the next [`Keyboard::service`].
    pub fn request_full_wipe(&mut self) {
        self.pending.wipe = true;
    }

    /// The input reports the host should have.
    pub fn hid(&self) -> &HidReports {
        self.keys.hid()
    }
    /// Advances whenever [`Keyboard::hid`] changed.
    pub fn hid_generation(&self) -> u32 {
        self.keys.hid_generation()
    }
    /// Keyboard LED output report from the host (Caps Lock indicator).
    pub fn set_host_leds(&mut self, leds: u8) {
        self.lighting.set_host_leds(leds);
    }
    /// USB bus suspended (or unconfigured): LEDs are blanked at the next frame, like stock.
    pub fn set_usb_suspended(&mut self, suspended: bool) {
        self.lighting.set_usb_suspended(suspended);
    }
    /// Call once per second with the scan-processing load in per mille (stream words 499–504).
    pub fn second_tick(&mut self, busy_permille: u16) {
        self.engine.second_tick(busy_permille);
    }

    // ------------------------------------------------------------------ lighting

    pub fn render_leds(&mut self, now_ms: u32, out: &mut [[u8; 3]; LED_COUNT]) {
        self.lighting.render(now_ms, &self.profile, &self.global, &self.engine, out);
    }

    // ------------------------------------------------------------------ deferred work

    /// Deferred work that may touch storage. Call from a low-priority context.
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

    /// `BE EE` / keycode 305: defaults for the active profile only, saved, re-derived.
    fn reset_profile(&mut self, st: &mut impl Storage) {
        let p = self.active_profile();
        defaults::reset_profile(&mut self.profile, p);
        self.save_profile(st);
        self.apply_config();
    }

    /// Switch to profile `p` (stock `FUN_0001170E`). Unsaved RAM edits of the old profile are
    /// dropped. `cur_param_num` is *not* written to storage here — like stock, the next `BE 02`
    /// or bank-1 write persists it, so a power cycle returns to the last saved profile. The
    /// host is notified (also when `p` is already active) and the LEDs blink the profile colour.
    fn switch_profile(&mut self, st: &mut impl Storage, p: u8) {
        if p as usize != self.active_profile() {
            self.global.set_active_profile(p as usize);
            self.load_profile(st, p as usize);
            self.apply_config();
        }
        self.pending.profile_notice = Some(p);
        self.lighting.start_profile_blink(p);
    }

    /// Fn + Esc held 5 s: erase all profiles and bank 1 — except the per-key calibration
    /// (`top_value`, `bot_value`) and switch types (`keytype_code`), which stock erases as well
    /// (a nasty trap on a daily driver: **deviation**). The next boot is otherwise a first boot.
    fn wipe(&mut self, st: &mut impl Storage) {
        // the blank image lives in read-only data, not on the stack
        static BLANK: Profile = Profile::blank();
        self.global.0[crate::config::b1::SOFT_VERSION..].fill(0xFF);
        self.save_global(st);
        for p in 0..PROFILE_COUNT {
            if st.store(Slot::Profile(p as u8), &BLANK.0).is_err() {
                self.storage_errors += 1;
            }
        }
        self.wipe_left = Some(WIPE_BLINK_SCANS);
    }
}
