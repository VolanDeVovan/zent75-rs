//! The one place for deliberate deviations from the stock 0.0.18 engine.
//!
//! Every vendor bug we fix is a field of [`Compat`] with the stock behaviour described next to
//! it. [`Compat::FIXED`] (the default) is what the firmware runs; [`Compat::STOCK`] reproduces
//! the vendor behaviour bit for bit so the differential tests against the emulated stock engine
//! (`tests/engine_*.rs`, `tests/keys_*.rs`) can replay traces with exact equality.
//!
//! Stock behaviours that are not reproduced because nothing observable depends on them:
//!
//! * the curve builder writes one point past the template (lands outside the table for types
//!   0/2/3);
//! * codes 121–200 (below the consumer report) and 225–250 / 256–300 (past the 16-byte keyboard
//!   report, onto the heartbeat / DA / gate flags) write outside the reports: they send nothing;
//! * the calibration-mode frame counter runs past 48 snapshots while USB is unconfigured.
//!
//! Deviations outside the engine are documented where they live: the dispatcher's in
//! `device/vendor.rs`, the bank repair's in `config.rs`, and Fn + Esc (5 s), which keeps the
//! calibration when it wipes the config (`Keyboard::wipe`; stock erases it too).

/// Runtime switches for the stock quirks. `false` = fixed behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compat {
    /// Stock: the IIR filter's error integrator `acc` (`0x92104`) is **one global variable**
    /// shared by all 88 slots and updated in processing order, so every key's filter is
    /// disturbed by its neighbours' noise. Fixed: one integrator per slot.
    pub shared_iir_acc: bool,
    /// Stock: `FUN_A6F8` files key object k in the per-step table at `slot2key[k]` instead of
    /// `key2slot[k]`, so a key is evaluated up to 10 steps away from its own sample and may see
    /// the previous scan's heights (one scan of extra latency). The same index mix-up puts the
    /// SOCD object into that step's list and indexes the activity threshold (`max − 100`) by the
    /// wrong slot. Fixed: every key is evaluated in the step that sampled it.
    pub object_step_quirk: bool,
    /// Stock: the auto-calibration clamp is evaluated as
    /// `min(max(min(d, swing+135), max(−135−swing, swing−380)), 380−swing)`. For any real swing
    /// (> 257 counts) the bounds are inverted and `d` always ends up `380 − swing`: the measured
    /// bottom only gates the update and the span shrinks by ~380 counts. Fixed: `d` is clamped
    /// into the band the constants describe, `[−swing−135, −swing+380]` (mirrored for sensors
    /// that rise when pressed), so auto-cal tracks the measured bottom within that band.
    pub autocal_inverted_clamp: bool,
    /// Stock: the auto-cal reference `act_ref` and the swing used by its ±600 gate are taken once
    /// at boot; later calibration writes do not update them, and every config apply resets the
    /// span to the stored calibration although `act_ref` keeps its tracked value. Fixed: both
    /// are re-derived when the stored calibration of a slot changes, and the span follows
    /// `act_ref` across config applies.
    pub autocal_ref_from_boot: bool,
    /// Stock: the power-up baseline is captured from whatever the sensor reads — a key held
    /// while the board starts is captured at its pressed level, the rest tracker rejects the
    /// released level (`|m − rest0| > 450`, `m − baseline > 40`) and the key reads travel 0
    /// until the next reboot ("travel stuck at 0"). Fixed: a quiet window whose mean sits more
    /// than [`HELD_RECOVER_COUNTS`] above the baseline (towards rest) re-captures baseline and
    /// power-up level of that slot, so the key recovers ~0.2 s after it is first released.
    pub held_key_baseline: bool,
}

impl Compat {
    /// What our firmware runs.
    pub const FIXED: Compat = Compat {
        shared_iir_acc: false,
        object_step_quirk: false,
        autocal_inverted_clamp: false,
        autocal_ref_from_boot: false,
        held_key_baseline: false,
    };
    /// Bit-exact stock 0.0.18 behaviour (differential tests).
    pub const STOCK: Compat = Compat {
        shared_iir_acc: true,
        object_step_quirk: true,
        autocal_inverted_clamp: true,
        autocal_ref_from_boot: true,
        held_key_baseline: true,
    };
}

impl Default for Compat {
    fn default() -> Self {
        Self::FIXED
    }
}

/// A quiet 512-scan window whose mean is this many ADC counts above the baseline (in the
/// released direction) re-captures the slot's baseline (fix for a key held at power-up). Far
/// beyond noise (dead zones are ~30 counts, the stock tracker only accepts −16..+40).
pub const HELD_RECOVER_COUNTS: i32 = 250;

/// Rapid-trigger guard constants per key (`botLen`, `botThr`, `topLen`, `topThr`), stock values.
/// `TopThr::Abs(650)` is the stock absolute height for Space/Backspace/Enter/LShift — very likely
/// meant as `max − 300` (a vendor slip), kept because it is what the keyboard has always done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopThr {
    /// `max_travel − n` µm.
    BelowMax(i32),
    /// Absolute height in µm.
    Abs(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guard {
    pub bot_len: i16,
    pub bot_thr: i16,
    pub top_len: i16,
    pub top_thr: TopThr,
}

pub const GUARD_DEFAULT: Guard = Guard { bot_len: 128, bot_thr: 100, top_len: 80, top_thr: TopThr::BelowMax(300) };

/// Special guards of `key_mode` 1 (applied by the config apply only when `return_th < 300`).
pub const GUARDS_RT: [(usize, Guard); 4] = [
    (76, Guard { bot_len: 440, bot_thr: 550, top_len: 440, top_thr: TopThr::Abs(650) }), // Space
    (28, Guard { bot_len: 160, bot_thr: 100, top_len: 120, top_thr: TopThr::Abs(650) }), // Backspace
    (57, Guard { bot_len: 160, bot_thr: 100, top_len: 120, top_thr: TopThr::Abs(650) }), // Enter
    (59, Guard { bot_len: 440, bot_thr: 100, top_len: 440, top_thr: TopThr::Abs(650) }), // LShift
];

/// Guards applied by keycode 304 (RT on): no `return_th` gate, LShift gets 160/100/440.
pub const GUARDS_304: [(usize, Guard); 4] = [
    (76, Guard { bot_len: 440, bot_thr: 550, top_len: 440, top_thr: TopThr::Abs(650) }),
    (28, Guard { bot_len: 160, bot_thr: 100, top_len: 120, top_thr: TopThr::Abs(650) }),
    (57, Guard { bot_len: 160, bot_thr: 100, top_len: 120, top_thr: TopThr::Abs(650) }),
    (59, Guard { bot_len: 160, bot_thr: 100, top_len: 440, top_thr: TopThr::Abs(650) }),
];

/// Guards of DKS records (no gate; the Space top threshold is `max − 550` here).
pub const GUARDS_DKS: [(usize, Guard); 4] = [
    (76, Guard { bot_len: 440, bot_thr: 550, top_len: 440, top_thr: TopThr::BelowMax(550) }),
    (28, Guard { bot_len: 160, bot_thr: 100, top_len: 120, top_thr: TopThr::BelowMax(300) }),
    (57, Guard { bot_len: 160, bot_thr: 100, top_len: 120, top_thr: TopThr::BelowMax(300) }),
    (59, Guard { bot_len: 440, bot_thr: 100, top_len: 440, top_thr: TopThr::BelowMax(300) }),
];

pub fn guard_for(table: &[(usize, Guard)], key: usize) -> Guard {
    table.iter().find(|(k, _)| *k == key).map(|(_, g)| *g).unwrap_or(GUARD_DEFAULT)
}

impl Guard {
    pub fn top_thr_for(&self, max: i32) -> i16 {
        match self.top_thr {
            TopThr::BelowMax(n) => (max - n) as i16,
            TopThr::Abs(v) => v as i16,
        }
    }
}
