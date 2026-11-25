//! Hall-sensor engine: averaged ADC samples → filtered raw → baseline → curve heights → travel.
//!
//! A faithful port of the stock 0.0.18 signal path (docs/re-engine.md §1–§6, §12, §13), integer
//! everywhere stock is integer and `f32` exactly where stock uses single-precision soft-float
//! (span and auto-cal fit). Vendor bugs we fix are switchable in [`compat::Compat`]; with
//! [`Compat::STOCK`] the engine matches the emulated stock firmware bit for bit
//! (`tests/engine_diff.rs`).
//!
//! The scan is processed in the stock order: 11 mux steps of 8 slots (crosstalk correction,
//! IIR filter, three curve heights per slot), then the step-10 bookkeeping (baseline capture,
//! activity, rest tracker, auto-cal, DA min/max). The key pipeline (`crate::keys`) evaluates the
//! key objects afterwards, step by step, through [`Engine::key_input`].
// Index loops mirror the stock per-slot / per-object tables on purpose.
#![allow(clippy::needless_range_loop)]

pub mod compat;
pub mod curves;
pub mod tables;

use crate::config::{Global, Profile};
use crate::layout::{KEY_COUNT, SLOT_COUNT, STEP_COUNT};
use crate::protocol::{Reply, REPORT_SIZE};
use compat::{Compat, HELD_RECOVER_COUNTS};
use curves::{type_of_code, Curves, TYPES};
use tables::{iir_step, DZ, NEIGHBOURS, OBJ_TO_SLOT, SLOT_TO_OBJ, XT_C1, XT_C2, XT_C3};

/// Scans before the engine starts (1.5 s).
pub const WARMUP_SCANS: u32 = 12000;
/// Scans per baseline window.
const WINDOW: u32 = 512;
/// Scans of calibration mode before snapshots start (1 s, counted once per boot).
const CAL_SETTLE_SCANS: u32 = 8000;
/// Snapshots per calibration frame; one every 8 scans.
const CAL_SNAPSHOTS: usize = 6;
/// Words of the normal `BE FD` frame.
pub const STREAM_WORDS: usize = 505;
/// Words of the calibration frame (6 × 83).
pub const CAL_WORDS: usize = CAL_SNAPSHOTS * KEY_COUNT;

const N: usize = SLOT_COUNT;
const IIR_BYPASS: i32 = 323;
const IIR_ACC_LIMIT: i32 = 0x70000;

/// Bank-1 / bank-2 offsets read by the engine.
const B1_TOP: usize = 0x000;
const B1_BOT: usize = 0x0B0;
const B1_TYPE: usize = 0x160;
const B2_AUTO_CALIB: usize = 0xC71;

#[inline]
fn rd16(b: &[u8], off: usize) -> u16 {
    match b.get(off..off + 2) {
        Some(s) => u16::from_le_bytes([s[0], s[1]]),
        None => 0,
    }
}

/// RISC-V `div`: truncating, `x/0 = −1`, `MIN/−1 = MIN`.
#[inline]
pub(crate) fn div_rv(a: i32, b: i32) -> i32 {
    if b == 0 { -1 } else { a.wrapping_div(b) }
}

/// Stock span (`FUN_A6F8` / auto-cal), single precision:
/// `(int)(−((float)a − (float)b + (float)(2·dz)) · 65536 / 2636)`.
#[inline]
fn span_of(a: f32, b: f32, dz: i32) -> i32 {
    let v = -((a - b) + (2 * dz) as f32);
    ((v * 65536.0f32) / 2636.0f32) as i32
}

/// Which heights the key pipeline sees for one object at one step.
#[derive(Debug, Clone, Copy)]
pub struct KeyInput {
    /// Press-side height `hA` (µm, max at rest).
    pub ha: i32,
    /// Release-side height `hB`.
    pub hb: i32,
    /// Filtered raw ADC value (`y`).
    pub y: i32,
}

/// What the key pipeline runs this scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPass {
    /// Engine not enabled (warm-up, baseline not captured) or calibration mode: no key work.
    None,
    /// Normal: the objects of step s are evaluated after step s.
    PerStep,
    /// First enabled scan: all 11 steps are evaluated at step 10 (stock `0x9206C`).
    AllAtEnd,
}

pub struct Engine {
    compat: Compat,
    initialized: bool,
    // ---- per slot: sample path
    /// Averages / filter output of the current scan (stock `0x8EE28`).
    raw_s: [i32; N],
    /// IIR output state (`0x8D828`).
    y: [i32; N],
    st: [i32; N],
    acc: [i32; N],
    step_init: [bool; STEP_COUNT],
    /// Crosstalk deltas `x − baseline` (`0x8EA08`).
    dlt: [i32; N],
    // ---- baseline / tracker
    base: [i32; N],
    rest0: [i32; N],
    sum: [i32; N],
    pmean: [u16; N],
    act: [u16; N],
    wsum: [u32; N],
    wn: [u16; N],
    // ---- normalisation
    dz: [i16; N],
    span: [i32; N],
    bot_eff: [u16; N],
    stored_top: [u16; N],
    stored_bot: [u16; N],
    typ: [u8; N],
    max_s: [i32; N],
    act_thr: [i16; N],
    ha: [i16; N],
    hb: [i16; N],
    hc: [i16; N],
    prev_ha: [i16; N],
    prev_hb: [i16; N],
    prev_raw: [i32; N],
    dyb: [i32; N],
    hist0: [i16; N],
    vel: [i16; N],
    // ---- auto-calibration
    autocal: bool,
    ac_state: [u8; N],
    ac_ok: [u8; N],
    ac_cnt: [u16; N],
    ac_accum: [i32; N],
    ac_fit: [u16; N],
    act_ref: [i32; N],
    swing: [i32; N],
    // ---- DA
    diag: bool,
    da_dmin: [i32; N],
    da_dmax: [i32; N],
    da_dpp: [i32; N],
    da_tmin: [u16; N],
    da_tmax: [u16; N],
    da_tpp: [u16; N],
    // ---- state machine
    warm: u32,
    n512: u32,
    captured: bool,
    enabled: bool,
    keys_started: bool,
    pass: KeyPass,
    // ---- calibration mode
    cal_on: bool,
    cal_seeded: bool,
    cal_y: [i32; N],
    cal_settle: u32,
    cal_n: u32,
    cal_snap: [[u16; N]; CAL_SNAPSHOTS],
    cal_ready: bool,
    // ---- statistics (frame tail)
    scans_per_s: u16,
    busy_permille: u16,
    uptime_s: u64,
    scans_this_s: u32,
    curves: Curves,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub const fn new() -> Self {
        Self {
            compat: Compat::FIXED,
            initialized: false,
            raw_s: [0; N],
            y: [0; N],
            st: [0; N],
            acc: [0; N],
            step_init: [false; STEP_COUNT],
            dlt: [0; N],
            base: [0; N],
            rest0: [0; N],
            sum: [0; N],
            pmean: [0; N],
            act: [0; N],
            wsum: [0; N],
            wn: [0; N],
            dz: [0; N],
            span: [1; N],
            bot_eff: [0; N],
            stored_top: [0; N],
            stored_bot: [0; N],
            typ: [0; N],
            max_s: [3420; N],
            act_thr: [3320; N],
            ha: [0; N],
            hb: [0; N],
            hc: [0; N],
            prev_ha: [0; N],
            prev_hb: [0; N],
            prev_raw: [0; N],
            dyb: [0; N],
            hist0: [0; N],
            vel: [0; N],
            autocal: false,
            ac_state: [0; N],
            ac_ok: [0; N],
            ac_cnt: [0; N],
            ac_accum: [0; N],
            ac_fit: [0; N],
            act_ref: [0; N],
            swing: [0; N],
            diag: false,
            da_dmin: [0xFFFF; N],
            da_dmax: [0; N],
            da_dpp: [0; N],
            da_tmin: [0xFFFF; N],
            da_tmax: [0; N],
            da_tpp: [0; N],
            warm: 0,
            n512: 0,
            captured: false,
            enabled: false,
            keys_started: false,
            pass: KeyPass::None,
            cal_on: false,
            cal_seeded: false,
            cal_y: [0; N],
            cal_settle: 0,
            cal_n: 0,
            cal_snap: [[0; N]; CAL_SNAPSHOTS],
            cal_ready: false,
            scans_per_s: crate::layout::SCAN_HZ as u16,
            busy_permille: 0,
            uptime_s: 0,
            scans_this_s: 0,
            curves: Curves::new(),
        }
    }

    // ------------------------------------------------------------------ configuration

    /// Select fixed (default) or stock behaviour of the vendor quirks.
    pub fn set_compat(&mut self, c: Compat) {
        self.compat = c;
    }
    pub fn compat(&self) -> Compat {
        self.compat
    }

    /// Re-derive everything that depends on the config (the engine part of stock `FUN_A6F8`):
    /// switch types, curves, `bot_eff`, span.
    pub fn configure(&mut self, g: &Global, p: &Profile) {
        let mut used = [false; TYPES];
        for s in 0..N {
            let k = SLOT_TO_OBJ[s] as usize;
            let t = type_of_code(rd16(&g.0, B1_TYPE + 2 * k));
            self.typ[s] = t;
            used[t as usize & 7] = true;
        }
        self.curves.build(&used, g);
        for s in 0..N {
            let k = SLOT_TO_OBJ[s] as usize;
            let top = rd16(&g.0, B1_TOP + 2 * k);
            let bot = rd16(&g.0, B1_BOT + 2 * k);
            let changed = top != self.stored_top[s] || bot != self.stored_bot[s];
            self.stored_top[s] = top;
            self.stored_bot[s] = bot;
            self.bot_eff[s] = if (top as i32 - bot as i32).abs() > 2999 { bot } else { top.wrapping_sub(12000) };
            self.max_s[s] = self.curves.max_travel[self.typ[s] as usize & 7];
            if self.initialized && changed && !self.compat.autocal_ref_from_boot {
                self.act_ref[s] = self.bot_eff[s] as i32;
                self.swing[s] = (self.bot_eff[s] as i32 - top as i32).abs();
            }
            self.span[s] = if self.initialized && !self.compat.autocal_ref_from_boot {
                span_of(self.act_ref[s] as f32, top as f32, self.dz[s] as i32)
            } else {
                span_of(self.bot_eff[s] as f32, top as f32, self.dz[s] as i32)
            };
        }
        for s in 0..N {
            // activity threshold: stock indexes it with slot2key of the object index
            let k = if self.compat.object_step_quirk { OBJ_TO_SLOT[s] as usize } else { SLOT_TO_OBJ[s] as usize };
            self.act_thr[s] = (self.max_s[OBJ_TO_SLOT[k] as usize] - 100) as i16;
        }
        self.autocal = p.0.get(B2_AUTO_CALIB) == Some(&1);
    }

    /// Stock `FUN_10E28` engine init with the samples read at power-up (before the scan
    /// timer starts): dead-zone signs, auto-cal reference, initial averages. Without this call
    /// the first scan's samples are used.
    pub fn init_samples(&mut self, samples: &[u16; SLOT_COUNT]) {
        self.init(samples);
    }

    fn init(&mut self, samples: &[u16; N]) {
        for s in 0..N {
            let x = samples[s] as i32;
            self.raw_s[s] = x;
            self.dz[s] = if x > 0x8000 { -DZ[s] as i16 } else { DZ[s] as i16 };
            self.act_ref[s] = self.bot_eff[s] as i32;
            self.swing[s] = (self.bot_eff[s] as i32 - self.stored_top[s] as i32).abs();
            self.span[s] = span_of(self.bot_eff[s] as f32, self.stored_top[s] as f32, self.dz[s] as i32);
            self.hc[s] = self.max_s[s] as i16;
        }
        self.initialized = true;
    }

    /// Skip the 1.5 s warm-up except its last scan (tests and the stock emulator harness).
    pub fn skip_warmup(&mut self) {
        self.warm = self.warm.max(WARMUP_SCANS - 1);
    }

    pub fn set_calibration(&mut self, on: bool) {
        self.cal_on = on;
    }

    /// DA diagnostics (`BE DA 03`): enables the min/max accumulators (words 332–497).
    pub fn set_diag(&mut self, on: bool) {
        self.diag = on;
    }

    /// Frame-tail statistics measured by the platform: scans counted in the last second and the
    /// engine busy time in per mille.
    pub fn set_stats(&mut self, scans_per_s: u16, busy_permille: u16) {
        self.scans_per_s = scans_per_s;
        self.busy_permille = busy_permille;
    }

    /// Call once per second: latches the scans counted since the previous call (word 499),
    /// the platform's busy figure (word 500) and advances the uptime (words 501–504).
    pub fn second_tick(&mut self, busy_permille: u16) {
        self.scans_per_s = self.scans_this_s.min(u16::MAX as u32) as u16;
        self.scans_this_s = 0;
        self.busy_permille = busy_permille;
        self.uptime_s += 1;
    }

    pub fn set_uptime(&mut self, s: u64) {
        self.uptime_s = s;
    }

    /// True once the baseline has been captured and keys are evaluated.
    pub fn ready(&self) -> bool {
        self.enabled
    }

    pub fn calibration(&self) -> bool {
        self.cal_on
    }

    /// Past the warm-up (stock runs the step-10 housekeeping from then on).
    pub fn warmed_up(&self) -> bool {
        self.warm >= WARMUP_SCANS
    }

    pub fn key_pass(&self) -> KeyPass {
        self.pass
    }

    /// Heights of the slot of `obj` as the key pipeline sees them when evaluating at `step`.
    #[inline]
    pub fn key_input(&self, obj: usize, step: usize) -> KeyInput {
        let s = OBJ_TO_SLOT[obj] as usize;
        let cur = self.pass == KeyPass::AllAtEnd || s / 8 <= step;
        if cur {
            KeyInput { ha: self.ha[s] as i32, hb: self.hb[s] as i32, y: self.raw_s[s] }
        } else {
            KeyInput { ha: self.prev_ha[s] as i32, hb: self.prev_hb[s] as i32, y: self.prev_raw[s] }
        }
    }

    /// Filtered raw ADC count of a key (BE FD words 0–82).
    pub fn raw(&self, key: usize) -> u16 {
        self.raw_s[OBJ_TO_SLOT[key] as usize] as u16
    }

    /// Travel of a key in µm (words 83–165).
    pub fn travel(&self, key: usize) -> u16 {
        let s = OBJ_TO_SLOT[key] as usize;
        (self.max_s[s] - self.hc[s] as i32).clamp(0, self.max_s[s]) as u16
    }

    /// Power-up baseline of a key (words 249–331).
    pub fn power_up_baseline(&self, key: usize) -> i32 {
        self.rest0[OBJ_TO_SLOT[key] as usize]
    }

    /// Full travel of object `obj` (µm).
    pub fn max_travel(&self, obj: usize) -> i32 {
        self.max_s[OBJ_TO_SLOT[obj] as usize]
    }

    /// Plain curve height `hC` of a key (µm, max at rest).
    pub fn height(&self, key: usize) -> i32 {
        self.hc[OBJ_TO_SLOT[key] as usize] as i32
    }

    /// Current span (`(top − bottom − 2·dz)·65536/2636`) of a key.
    pub fn span(&self, key: usize) -> i32 {
        self.span[OBJ_TO_SLOT[key] as usize]
    }

    /// Auto-calibration bottom reference of a key (ADC counts; starts at the stored bottom).
    pub fn autocal_ref(&self, key: usize) -> i32 {
        self.act_ref[OBJ_TO_SLOT[key] as usize]
    }

    /// Dynamic rest baseline of a key (ADC counts).
    pub fn rest_baseline(&self, key: usize) -> i32 {
        self.base[OBJ_TO_SLOT[key] as usize]
    }

    /// Per-scan velocity of a key (`hC` change, µm/scan, positive while pressing).
    pub fn velocity(&self, key: usize) -> i16 {
        self.vel[OBJ_TO_SLOT[key] as usize]
    }

    // ------------------------------------------------------------------ scan

    /// One full scan. `samples` are averaged ADC counts indexed by scan slot. `cal` = calibration
    /// mode (`BE FE 01`): the engine is bypassed and only the calibration frames run.
    pub fn scan(&mut self, samples: &[u16; SLOT_COUNT], _g: &Global, _p: &Profile, cal: bool) {
        if !self.initialized {
            self.init(samples);
        }
        self.scans_this_s = self.scans_this_s.wrapping_add(1);
        self.pass = KeyPass::None;
        if cal || self.cal_on {
            self.cal_scan(samples);
            return;
        }
        let warm_before = self.warm;
        if self.warm < WARMUP_SCANS {
            self.warm += 1;
        }
        if self.warm < WARMUP_SCANS {
            self.keys_started = false;
            return;
        }
        let enabled = self.enabled;
        if enabled {
            self.pass = if self.keys_started { KeyPass::PerStep } else { KeyPass::AllAtEnd };
            self.keys_started = true;
            self.prev_ha = self.ha;
            self.prev_hb = self.hb;
            self.prev_raw = self.raw_s;
        }
        let first_step = if warm_before >= WARMUP_SCANS { 0 } else { STEP_COUNT - 1 };
        for step in first_step..STEP_COUNT {
            self.step(step, samples, enabled);
        }
        self.bookkeeping();
    }

    fn step(&mut self, step: usize, samples: &[u16; N], enabled: bool) {
        let r = step * 8..step * 8 + 8;
        for s in r.clone() {
            self.raw_s[s] = samples[s] as i32;
        }
        if enabled {
            // crosstalk correction, in slot order (later slots see the new deltas)
            for s in r.clone() {
                let a = self.dlt[(s + N - 2) % N].wrapping_mul(XT_C1[s]);
                let b = self.dlt[(s + 6) % N].wrapping_mul(XT_C2[s]);
                let c = self.dlt[(s + N - 8) % N].wrapping_mul(XT_C3[s]);
                let comp = a.wrapping_add(b).wrapping_add(c) >> 20;
                let x = self.raw_s[s].wrapping_sub(comp);
                self.raw_s[s] = x;
                self.dlt[s] = x.wrapping_sub(self.base[s]);
            }
        }
        if !self.step_init[step] {
            self.step_init[step] = true;
            for s in r.clone() {
                self.y[s] = self.raw_s[s];
            }
        } else {
            for s in r.clone() {
                self.iir(s);
            }
        }
        for s in r.clone() {
            self.raw_s[s] = self.y[s];
        }
        if !enabled {
            return;
        }
        for s in r {
            let (b, y, dz, span) = (self.base[s], self.raw_s[s], self.dz[s] as i32, self.span[s]);
            self.dyb[s] = y.wrapping_sub(b);
            let t = self.typ[s];
            let fa = div_rv(b.wrapping_sub(y).wrapping_sub(dz).wrapping_mul(0x10000), span);
            self.ha[s] = self.curves.height(t, fa) as i16;
            let fb = div_rv(dz.wrapping_sub(self.dyb[s]).wrapping_mul(0x10000), span);
            self.hb[s] = self.curves.height(t, fb) as i16;
            let fc = div_rv(self.dyb[s].wrapping_mul(-0x10000), span);
            let hc = self.curves.height(t, fc);
            self.vel[s] = self.hist0[s].wrapping_sub(hc as i16);
            self.hist0[s] = hc as i16;
            self.hc[s] = hc as i16;
        }
    }

    /// Stock `FUN_13672`.
    #[inline]
    fn iir(&mut self, s: usize) {
        let x = self.raw_s[s];
        let yp = self.y[s];
        let d = x.wrapping_sub(yp);
        if d.unsigned_abs() > IIR_BYPASS as u32 {
            let v = d.wrapping_shl(15);
            self.st[s] = v;
            self.y[s] = (v >> 15).wrapping_add(yp);
            return;
        }
        let ai = if self.compat.shared_iir_acc { 0 } else { s };
        let g = iir_step(d).wrapping_shl(15);
        let a = (g.wrapping_sub(self.st[s]).wrapping_mul(0x5C).wrapping_add(self.acc[ai]) >> 7).clamp(-IIR_ACC_LIMIT, IIR_ACC_LIMIT);
        self.acc[ai] = a;
        let v = g.wrapping_sub(a);
        self.st[s] = v;
        self.y[s] = (v >> 15).wrapping_add(yp);
    }

    /// Step-10 bookkeeping: window, capture, activity, rest tracker, auto-cal, DA.
    fn bookkeeping(&mut self) {
        let mut n = if !self.captured {
            if self.act.iter().all(|&a| a == 0) {
                for s in 0..N {
                    self.sum[s] = self.sum[s].wrapping_add(self.raw_s[s]);
                }
                self.n512 += 1;
            }
            self.n512
        } else {
            for s in 0..N {
                self.sum[s] = self.sum[s].wrapping_add(self.raw_s[s]);
                if self.ha[s] < self.act_thr[s] {
                    let k = SLOT_TO_OBJ[s] as usize;
                    for &nb in &NEIGHBOURS[k] {
                        self.act[nb as usize] = self.act[nb as usize].saturating_add(1);
                    }
                }
            }
            self.n512 += 1;
            self.n512
        };
        let mut run_autocal = true;
        if n == WINDOW {
            self.n512 = 0;
            n = 0;
            let _ = n;
            for a in self.act.iter_mut() {
                *a = if *a >= 4 { 2 } else if *a != 0 { *a - 1 } else { 0 };
            }
            if self.captured {
                self.rest_tracker();
                self.enabled = true;
            } else if self.act.iter().all(|&a| a == 0) {
                self.capture();
                if !self.captured {
                    self.enabled = false;
                    run_autocal = false;
                }
            } else {
                self.enabled = false;
                run_autocal = false;
            }
        }
        if run_autocal && self.captured {
            if self.autocal {
                self.autocal();
            }
            self.enabled = true;
            if self.diag {
                for s in 0..N {
                    da_update(&mut self.da_dmin[s], &mut self.da_dmax[s], &mut self.da_dpp[s], self.dyb[s]);
                }
            }
        } else if !self.captured {
            self.enabled = false;
        }
    }

    fn capture(&mut self) {
        self.captured = true;
        for s in 0..N {
            let m = self.sum[s] / WINDOW as i32;
            self.base[s] = m;
            self.rest0[s] = m;
            self.pmean[s] = m as u16;
            self.sum[s] = 0;
            self.wsum[s] = 0;
            self.wn[s] = 0;
            self.vel[s] = 0;
            self.ac_cnt[s] = 0;
            self.hist0[s] = self.max_s[s] as i16;
        }
        for s in 0..N {
            if (SLOT_TO_OBJ[s] as usize) < KEY_COUNT && (self.base[s] - self.raw_s[s]).abs() > 40 {
                self.captured = false;
                return;
            }
        }
    }

    fn rest_tracker(&mut self) {
        let (hi, lo) = if self.swing[0] == 1 { (16, -40) } else { (40, -16) };
        for s in 0..N {
            let m = self.sum[s] >> 9;
            let quiet_mean = (m.wrapping_sub(self.pmean[s] as i32).wrapping_add(8) as u32) < 17;
            let dm = m.wrapping_sub(self.base[s]);
            if quiet_mean
                && (m.wrapping_sub(self.rest0[s]).wrapping_add(450) as u32) < 901
                && dm <= hi
                && lo <= dm
                && self.act[s] == 0
            {
                let wn = self.wn[s] as u32 + 1;
                let acc = ((m as u32) << (self.wn[s] as u32 & 31)).wrapping_add(self.wsum[s]);
                self.wn[s] = wn as u16;
                self.wsum[s] = acc;
                if wn & 0x10 != 0 {
                    self.wsum[s] = 0;
                    self.wn[s] = 0;
                    self.base[s] = (acc / 0xFFFF) as i32;
                }
            } else if !self.compat.held_key_baseline && quiet_mean && self.act[s] == 0 {
                // A key held at power-up was captured pressed: once it sits clearly above the
                // baseline (towards rest) for a quiet window, take this level as its rest.
                let dir = if self.stored_top[s] >= self.bot_eff[s] { 1 } else { -1 };
                if dm * dir > HELD_RECOVER_COUNTS {
                    self.base[s] = m;
                    self.rest0[s] = m;
                    self.wsum[s] = 0;
                    self.wn[s] = 0;
                    self.ac_state[s] = 0;
                }
            }
            self.pmean[s] = m as u16;
            self.sum[s] = 0;
        }
    }

    fn autocal(&mut self) {
        for s in 0..N {
            let hb = self.hb[s];
            let vel = self.vel[s];
            let state = self.ac_state[s];
            let mut exit = false;
            let mut hold = false;
            if hb < 400 {
                if state == 0 {
                    if vel > 15 {
                        self.ac_state[s] = 1;
                        self.ac_accum[s] = 0;
                        self.ac_ok[s] = 0;
                        self.ac_cnt[s] = 1;
                    }
                } else if state == 1 {
                    if vel < -15 { exit = true } else { hold = true }
                }
            } else if state == 1 {
                if vel < -15 || hb != 400 { exit = true } else { hold = true }
            }
            if exit {
                self.ac_state[s] = 0;
                if self.ac_ok[s] == 1 && self.ac_cnt[s] > 5000 {
                    self.ac_ok[s] = 0;
                    self.autocal_apply(s);
                }
            } else if hold {
                let cnt = self.ac_cnt[s].wrapping_add(1);
                self.ac_cnt[s] = cnt;
                if cnt > 2000 {
                    self.ac_accum[s] = self.ac_accum[s].wrapping_add(self.raw_s[s]);
                    if cnt == 4000 {
                        self.ac_ok[s] = 1;
                        let base = self.base[s];
                        let mean = (self.ac_accum[s] / 2000) & 0xFFFF;
                        let f10 = self.curves.f10[self.typ[s] as usize & 7] as f32;
                        let k = (f10 / 2636.0f32 + 1.0f32) * 0.5f32;
                        let fit = ((mean - base) as f32) / k + base as f32;
                        let fit = if fit <= 0.0 { 0 } else { fit as u32 as u16 };
                        self.ac_fit[s] = fit;
                        let gate = (fit as i32 - base).abs() - self.swing[s];
                        if gate.abs() > 600 {
                            self.ac_state[s] = 0;
                        }
                    }
                }
            }
        }
    }

    fn autocal_apply(&mut self, s: usize) {
        let base = self.base[s];
        let swing = (self.stored_top[s] as i32 - self.stored_bot[s] as i32).abs();
        let mut d = self.ac_fit[s] as i32 - base;
        if self.compat.autocal_inverted_clamp {
            if swing + 135 < d {
                d = swing + 135;
            }
            let lo = (-135 - swing).max(swing - 380);
            if d < lo {
                d = lo;
            }
            if 380 - swing < d {
                d = 380 - swing;
            }
        } else if self.stored_top[s] >= self.stored_bot[s] {
            d = d.clamp(-swing - 135, -swing + 380);
        } else {
            d = d.clamp(swing - 380, swing + 135);
        }
        let new = (base + d) & 0xFFFF;
        self.act_ref[s] += (new - self.act_ref[s]) / 2;
        self.ac_fit[s] = new as u16;
        self.span[s] = span_of(self.act_ref[s] as f32, base as f32, self.dz[s] as i32);
    }

    // ------------------------------------------------------------------ calibration mode

    /// Stock `FUN_E860`: raw averages through `y += (x − y) >> 4`, snapshots after 1 s.
    fn cal_scan(&mut self, samples: &[u16; N]) {
        if !self.cal_seeded {
            self.cal_seeded = true;
            for s in 0..N {
                self.cal_y[s] = samples[s] as i32;
            }
        } else {
            for s in 0..N {
                let d = samples[s] as i32 - self.cal_y[s];
                self.cal_y[s] += d >> 4;
            }
        }
        self.raw_s = self.cal_y;
        if self.cal_settle < CAL_SETTLE_SCANS {
            self.cal_settle += 1;
            return;
        }
        self.cal_n += 1;
        if self.cal_n & 7 == 0 {
            let i = (self.cal_n / 8 - 1) as usize;
            if let Some(snap) = self.cal_snap.get_mut(i) {
                for s in 0..N {
                    snap[s] = self.cal_y[s] as u16;
                }
            }
        }
        if self.cal_n as usize == 8 * CAL_SNAPSHOTS {
            self.cal_n = 0;
            self.cal_ready = true;
        }
    }

    /// The 1003-byte calibration frame, once per 6 snapshots (~167/s). No heartbeat needed.
    pub fn take_calibration_frame(&mut self, out: &mut [u8; REPORT_SIZE]) -> Option<usize> {
        if !core::mem::take(&mut self.cal_ready) {
            return None;
        }
        let snap = &self.cal_snap;
        let words = (0..CAL_WORDS).map(|i| snap[i / KEY_COUNT][OBJ_TO_SLOT[i % KEY_COUNT] as usize]);
        Some(Reply::new(out).stream(words, CAL_WORDS))
    }

    // ------------------------------------------------------------------ BE FD stream

    /// Build the 1017-byte `BE FD` frame (stock: every 320 scans while the heartbeat is alive).
    /// `diag` = DA flag (word 498); while set, the travel min/max accumulators are sampled here.
    pub fn stream_frame(&mut self, out: &mut [u8; REPORT_SIZE], diag: bool) -> usize {
        self.diag = diag;
        let mut travel_s = [0i32; N];
        for s in 0..N {
            let m = self.max_s[s];
            travel_s[s] = (m - self.hc[s] as i32).max(0).min(m);
        }
        if diag {
            for s in 0..N {
                let (mut lo, mut hi, mut pp) = (self.da_tmin[s] as i32, self.da_tmax[s] as i32, 0);
                da_update(&mut lo, &mut hi, &mut pp, travel_s[s]);
                (self.da_tmin[s], self.da_tmax[s], self.da_tpp[s]) = (lo as u16, hi as u16, pp as u16);
            }
        }
        let tail = [
            diag as u16,
            self.scans_per_s,
            self.busy_permille,
            self.uptime_s as u16,
            (self.uptime_s >> 16) as u16,
            (self.uptime_s >> 32) as u16,
            (self.uptime_s >> 48) as u16,
        ];
        let words = (0..STREAM_WORDS).map(|i| {
            let k = i % KEY_COUNT;
            let s = OBJ_TO_SLOT[k] as usize;
            match i / KEY_COUNT {
                _ if i >= 6 * KEY_COUNT => tail[i - 6 * KEY_COUNT],
                0 => self.raw_s[s] as u16,
                1 => travel_s[s] as u16,
                2 => self.base[s] as u16,
                3 => self.rest0[s] as u16,
                4 => self.da_dpp[s] as u16,
                _ => self.da_tpp[s],
            }
        });
        Reply::new(out).stream(words, STREAM_WORDS)
    }
}

/// Stock `FUN_15A84`: running min/max and their difference.
#[inline]
fn da_update(min: &mut i32, max: &mut i32, pp: &mut i32, v: i32) {
    if *max < v {
        *max = v;
    }
    if v < *min {
        *min = v;
    }
    *pp = *max - *min;
}
