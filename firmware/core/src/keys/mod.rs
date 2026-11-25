//! Key pipeline: curve heights → per-key actuation → layers / tap-hold / SOCD / key groups /
//! DKS / function codes → HID reports.
//!
//! A port of the stock 0.0.18 `FUN_9D10` and its helpers (docs/re-engine.md §7–§11). The key
//! objects are evaluated step by step after each scan, in the same order as the stock firmware,
//! the reports are rebuilt only when something changed and "sent" (latched into [`HidReports`])
//! on change, so the host sees exactly the report sequence of the stock firmware
//! (`tests/engine_diff.rs`). Deliberate deviations are in [`crate::engine::compat`].
// Index loops mirror the stock per-slot / per-object tables on purpose.
#![allow(clippy::needless_range_loop)]

mod object;

use crate::config::{Profile, LAYERS};
use crate::engine::compat::{guard_for, Compat, GUARDS_DKS, GUARDS_RT, GUARDS_304, GUARD_DEFAULT};
use crate::engine::tables::{MOD_FLAG_CODES, OBJ_TO_SLOT, SLOT_TO_OBJ};
use crate::engine::{Engine, KeyPass};
use crate::hid::HidReports;
use crate::keycode::{self as kc, func};
use crate::layout::{SLOT_COUNT, STEP_COUNT};
use object::{Bind, Handler, Obj};

/// Key objects (83 keys + 5 spare slots, like stock).
pub const OBJ_COUNT: usize = SLOT_COUNT;
/// Profile-switch cool-down (0.9 s): no report is rebuilt meanwhile (stock `0x9214C`).
pub const SWITCH_COOLDOWN: i32 = 7200;
/// Fn + Esc held this many scans (5 s) triggers the hard reset.
pub const HARD_RESET_SCANS: u32 = 40000;
/// Tap-hold: a key held longer than this (0.5 s) sends its hold code.
pub const TAP_HOLD_SCANS: u16 = 4000;
/// More keys than this in the pressed list: empty reports until ≤ [`ROLLOVER_RESUME`].
pub const ROLLOVER_LIMIT: usize = 20;
pub const ROLLOVER_RESUME: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightCmd {
    BrightnessUp,
    BrightnessDown,
    NextEffect,
    PrevEffect,
    Toggle,
    SpeedUp,
    SpeedDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// 301: jump to the UF2 loader.
    Bootloader,
    /// 302: watchdog reboot.
    Reboot,
    /// Fn + Esc held 5 s. Stock erases both config banks including the calibration; ours
    /// keeps `top/bot` and the switch types (`Keyboard::wipe`).
    FactoryReset,
    /// 305 `INITSET`: reset the active profile to its defaults and save it (repeats while held).
    ResetProfile,
    /// 307–310: switch profile (the key layer already applied the 0.9 s cool-down).
    SwitchProfile(u8),
    /// 311–315, 317, 318.
    Lighting(LightCmd),
    /// 316 toggled WinLock (new state).
    WinLock(bool),
}

/// Small fixed-capacity event list.
#[derive(Default)]
pub struct Events {
    buf: [Option<Event>; 8],
    n: usize,
}
impl Events {
    pub fn push(&mut self, e: Event) {
        if self.buf[..self.n].contains(&Some(e)) {
            return; // level codes repeat every rebuild; once per scan is enough
        }
        if self.n < self.buf.len() {
            self.buf[self.n] = Some(e);
            self.n += 1;
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = &Event> {
        self.buf[..self.n].iter().flatten()
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
}

/// One report the stock firmware would send this scan (diagnostics / differential tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    /// 0x81 keyboard (16 bytes), 0x83 consumer (3), 0x84 mouse (3).
    pub ep: u8,
    pub data: [u8; 16],
    pub len: u8,
}

/// At most keyboard + consumer + mouse per step.
#[cfg(feature = "std")]
const SENT_CAP: usize = 3 * STEP_COUNT;

pub struct KeyProcessor {
    compat: Compat,
    dirty: bool,
    objs: [Obj; OBJ_COUNT],
    /// Objects evaluated at each step, in table order.
    step_objs: [[u8; 8]; STEP_COUNT],
    socd_step: [[u8; 8]; STEP_COUNT],
    socd_step_n: [u8; STEP_COUNT],
    bind_objs: [u8; OBJ_COUNT],
    bind_n: usize,
    /// Pressed list (`0x8E1C8`), in press order.
    plist: [u8; OBJ_COUNT],
    pn: usize,
    /// SOCD armed list (`0x8ECC8`).
    slist: [u8; OBJ_COUNT],
    sn: usize,
    /// Active key-group / DKS slots (`0x8BFC8` / `0x8BA48`).
    dlist: [(u8, u8); 4 * OBJ_COUNT],
    dn: usize,
    th_flag: bool,
    mask_flag: bool,
    socd_flag: bool,
    /// `0x91F9C`: [0] Fn layer, [1..7] LCtrl, LShift, LAlt, LWin, RCtrl, RShift, RAlt held.
    flags: [bool; 12],
    report: [u8; 16],
    snapshot: [u8; 16],
    cons: [u8; 3],
    mouse: [u8; 3],
    last_cmp: [u8; 16],
    last_cons: u32,
    last_mouse: u32,
    /// The reports as last "sent" (what the host has) and a counter of those sends.
    hid: HidReports,
    hid_gen: u32,
    lock: bool,
    lockcnt: u32,
    cooldown: i32,
    win_lock: bool,
    seed: u16,
    fn_esc: u32,
    /// Per-scan send log (host builds only: tests, simulator).
    #[cfg(feature = "std")]
    sent: [Sent; SENT_CAP],
    #[cfg(feature = "std")]
    sent_n: usize,
}

impl Default for KeyProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyProcessor {
    pub const fn new() -> Self {
        Self {
            compat: Compat::FIXED,
            dirty: true,
            objs: [Obj::new(); OBJ_COUNT],
            step_objs: [[0; 8]; STEP_COUNT],
            socd_step: [[0; 8]; STEP_COUNT],
            socd_step_n: [0; STEP_COUNT],
            bind_objs: [0; OBJ_COUNT],
            bind_n: 0,
            plist: [0; OBJ_COUNT],
            pn: 0,
            slist: [0; OBJ_COUNT],
            sn: 0,
            dlist: [(0, 0); 4 * OBJ_COUNT],
            dn: 0,
            th_flag: false,
            mask_flag: false,
            socd_flag: false,
            flags: [false; 12],
            report: [0; 16],
            snapshot: [0; 16],
            cons: [0; 3],
            mouse: [0; 3],
            last_cmp: [0xFF; 16],
            last_cons: u32::MAX,
            last_mouse: u32::MAX,
            hid: HidReports::EMPTY,
            hid_gen: 0,
            lock: false,
            lockcnt: 0,
            cooldown: 0,
            win_lock: false,
            seed: 0,
            fn_esc: 0,
            #[cfg(feature = "std")]
            sent: [Sent { ep: 0, data: [0; 16], len: 0 }; SENT_CAP],
            #[cfg(feature = "std")]
            sent_n: 0,
        }
    }

    pub fn set_compat(&mut self, c: Compat) {
        self.compat = c;
        self.dirty = true;
    }

    /// The config changed (or the profile was switched): the objects are re-derived from it
    /// at the next [`KeyProcessor::update`] (stock `FUN_A6F8`, which also resets every key state
    /// and the reports).
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// The reports the host currently has.
    pub fn hid(&self) -> &HidReports {
        &self.hid
    }
    /// Advances whenever a report in [`KeyProcessor::hid`] was replaced.
    pub fn hid_generation(&self) -> u32 {
        self.hid_gen
    }
    /// Key (object) actuated (pressed state 1; a mode-2 "RT-released" key is not pressed).
    pub fn is_pressed(&self, key: usize) -> bool {
        self.objs.get(key).is_some_and(|o| o.pressed == 1)
    }
    /// The Fn layer is active (as of the last report rebuild).
    pub fn fn_layer(&self) -> bool {
        self.flags[0]
    }
    /// Start the 0.9 s profile cool-down (stock: `BE C0` does this too).
    pub fn start_cooldown(&mut self) {
        self.cooldown = SWITCH_COOLDOWN;
    }
    /// Reports the stock firmware would have sent during the last scan (host builds only).
    #[cfg(feature = "std")]
    pub fn sent(&self) -> &[Sent] {
        &self.sent[..self.sent_n]
    }
    pub fn object(&self, k: usize) -> &Obj {
        &self.objs[k]
    }

    // ------------------------------------------------------------------ config apply

    /// Stock `FUN_A6F8`, key part.
    fn apply(&mut self, p: &Profile, e: &Engine) {
        self.dirty = false;
        self.th_flag = false;
        self.mask_flag = false;
        self.socd_flag = false;
        self.socd_step_n = [0; STEP_COUNT];
        let quirk = self.compat.object_step_quirk;
        for q in 0..OBJ_COUNT {
            let k = if quirk { OBJ_TO_SLOT[q] } else { SLOT_TO_OBJ[q] };
            self.step_objs[q / 8][q % 8] = k;
        }
        for k in 0..OBJ_COUNT {
            let max = e.max_travel(k);
            let o = &mut self.objs[k];
            o.reset_state();
            o.max = max as i16;
            let (pt, rt) = (p.press_th(k), p.return_th(k));
            if pt != 0 && rt != 0 {
                o.press_th = pt;
                o.return_th = rt;
            }
            let pdz = p.press_deadzone(k);
            o.top = (max as u16).wrapping_sub(pdz);
            if pdz == 0 && o.press_th < 20 {
                o.top = o.top.wrapping_sub(10);
            }
            o.peak = o.top;
            let rdz = p.return_deadzone(k) as i32 + 10;
            o.bot = rdz.min(max) as u16;
            o.low = o.bot;
            let (np, nr) = (p.norm_press_th(k), p.norm_return_th(k));
            if np != 0 && nr != 0 {
                o.p = (max as u16).wrapping_sub(np);
                o.r = (max as u16).wrapping_sub(nr);
            }
            if o.r < o.p {
                o.r = o.p;
            }
            for l in 0..LAYERS {
                o.code[l] = p.code(l, k);
                o.hold[l] = p.hold_code(l, k);
                o.mask[l] = p.mod_mask(l, k);
                self.th_flag |= o.hold[l] != kc::NONE;
                self.mask_flag |= o.mask[l] != 0;
            }
            let c = p.cancel_code(k);
            o.cancel = c;
            o.armed = false;
            o.socd_mask = 1 << (c as i32 & 7);
            o.socd_byte = 0;
            if c != kc::NONE {
                self.socd_flag = true;
                if (1..=119).contains(&c) {
                    o.socd_byte = (c >> 3) as u8 + 1;
                }
                let step = if quirk { SLOT_TO_OBJ[k] } else { OBJ_TO_SLOT[k] } as usize / 8;
                let n = self.socd_step_n[step] as usize;
                if n < 8 {
                    self.socd_step[step][n] = k as u8;
                    self.socd_step_n[step] += 1;
                }
            }
            let mode = p.key_mode(k);
            o.key_mode = mode;
            o.handler = match mode {
                1 => {
                    let g = if o.return_th < 300 { guard_for(&GUARDS_RT, k) } else { GUARD_DEFAULT };
                    o.set_guard(&g, max);
                    Handler::Rt
                }
                2 => {
                    if matches!(k, 76 | 43 | 57 | 59) && o.return_th < 300 {
                        Handler::Mode2Guard
                    } else {
                        Handler::Mode2
                    }
                }
                _ => Handler::Normal,
            };
        }
        // key groups / DKS
        self.bind_n = 0;
        for k in 0..OBJ_COUNT {
            let mode = p.bind_mode(k);
            if mode == 0 {
                continue;
            }
            let rec = p.bind(k);
            if mode > 2 {
                // other values bind a handler that does nothing (stock `FUN_16494`): not listed
                // (nothing observable, saves the step-10 work), but byte 47 still applies
                if rec.suppress_original() {
                    let o = &mut self.objs[k];
                    o.handler = Handler::Normal;
                    o.code = [kc::NONE; 2];
                }
                continue;
            }
            self.bind_objs[self.bind_n] = k as u8;
            self.bind_n += 1;
            let o = &mut self.objs[k];
            o.bind = if mode == 1 { Bind::Group } else { Bind::Dks };
            o.dks = 0;
            o.dks_guard = [0; 4];
            (o.t_single, o.t_delay, o.t_rand) = rec.tap_times();
            let g = guard_for(&GUARDS_DKS, k);
            o.dks_len_thr = [g.bot_len, g.bot_thr, g.top_len, g.top_thr_for(o.max as i32)];
            for i in 0..4 {
                o.timer[i] = 0;
                o.cnt[i] = 0;
                o.in_dlist[i] = false;
                o.out[i] = 0;
                o.raw[i] = 0;
                o.linger[i] = 0;
                o.scode[i] = rec.code(i);
                for ph in 0..4 {
                    o.action[i][ph] = rec.action(i, ph);
                }
            }
            o.bp = (o.max as u16).wrapping_sub(rec.bottom_point());
            o.tp = (o.max as u16).wrapping_sub(rec.top_point());
            if rec.suppress_original() {
                o.handler = Handler::Normal;
                o.code = [kc::NONE; 2];
            }
        }
        self.report = [0; 16];
        self.snapshot = [0; 16];
        self.cons = [0; 3];
        self.mouse = [0; 3];
        self.flags = [false; 12];
        self.pn = 0;
        self.sn = 0;
        self.dn = 0;
        self.last_cmp = [0xFF; 16];
        self.last_cons = u32::MAX;
        self.last_mouse = u32::MAX;
    }

    // ------------------------------------------------------------------ scan

    /// Run the key pipeline for the scan the engine just processed.
    pub fn update(&mut self, e: &Engine, p: &Profile) -> Events {
        let mut ev = Events::default();
        if self.dirty {
            self.apply(p, e);
        }
        #[cfg(feature = "std")]
        {
            self.sent_n = 0;
        }
        if e.calibration() {
            return ev; // stock: no key processing, reports frozen
        }
        let pass = e.key_pass();
        for step in 0..STEP_COUNT {
            if step == STEP_COUNT - 1 && self.cooldown > 0 {
                self.cooldown -= 1;
            }
            match pass {
                KeyPass::None => {}
                KeyPass::PerStep => self.step(step, e, &mut ev),
                KeyPass::AllAtEnd => {
                    if step == STEP_COUNT - 1 {
                        for s in 0..STEP_COUNT {
                            self.step(s, e, &mut ev);
                        }
                    }
                }
            }
        }
        if e.warmed_up() {
            // hard-wired Fn + Esc (physical key 0) held 5 s
            if self.fn_esc >= HARD_RESET_SCANS {
                self.fn_esc = 0;
                ev.push(Event::FactoryReset);
            }
            self.fn_esc = if self.flags[0] && self.objs[0].pressed == 1 { self.fn_esc + 1 } else { 0 };
        }
        ev
    }

    /// Stock `FUN_9D10(gate, step, 8)`.
    fn step(&mut self, step: usize, e: &Engine, ev: &mut Events) {
        let objs = self.step_objs[step];
        for &k in &objs {
            let inp = e.key_input(k as usize, step);
            self.objs[k as usize].actuate(inp);
        }
        if step == STEP_COUNT - 1 {
            for i in 0..self.bind_n {
                let k = self.bind_objs[i] as usize;
                let inp = e.key_input(k, step);
                let seed = &mut self.seed;
                self.objs[k].bind_step(inp, seed);
            }
        }
        if self.th_flag {
            let layer = self.flags[0] as usize;
            for &k in &objs {
                let o = &mut self.objs[k as usize];
                let h = o.hold[layer];
                if o.pressed == 1 {
                    if h != kc::NONE {
                        o.hold_cnt = o.hold_cnt.saturating_add(1);
                        o.th = if o.hold_cnt > TAP_HOLD_SCANS { 2 } else { 1 };
                    }
                } else {
                    o.th = if h != kc::NONE && o.th == 1 { 3 } else { 0 };
                    o.hold_cnt = 0;
                }
            }
        }
        if self.socd_flag {
            for i in 0..self.socd_step_n[step] as usize {
                let o = &mut self.objs[self.socd_step[step][i] as usize];
                if o.cancel == kc::NONE {
                    continue;
                }
                let b = o.socd_byte as usize;
                if !o.armed {
                    if o.edge_p && self.report[b] & o.socd_mask != 0 {
                        o.armed = true;
                    }
                } else if o.pressed != 1 || self.snapshot[b] & o.socd_mask == 0 {
                    o.armed = false;
                }
            }
        }
        // list maintenance
        let (old_pn, old_sn, old_dn) = (self.pn, self.sn, self.dn);
        let mut changed = 0u32;
        let mut n = 0;
        for i in 0..self.pn {
            let k = self.plist[i] as usize;
            let o = &mut self.objs[k];
            if o.pressed == 1 || o.th == 3 {
                self.plist[n] = k as u8;
                n += 1;
            } else {
                o.in_plist = false;
                changed += 1;
            }
        }
        self.pn = n;
        let mut n = 0;
        for i in 0..self.dn {
            let (k, j) = self.dlist[i];
            let o = &mut self.objs[k as usize];
            if o.out[j as usize] == 1 {
                self.dlist[n] = (k, j);
                n += 1;
            } else {
                o.in_dlist[j as usize] = false;
                changed += 1;
            }
        }
        self.dn = n;
        let mut n = 0;
        for i in 0..self.sn {
            let k = self.slist[i] as usize;
            let o = &mut self.objs[k];
            if o.armed {
                self.slist[n] = k as u8;
                n += 1;
            } else {
                o.in_slist = false;
                changed += 1;
            }
        }
        self.sn = n;
        for &k in &objs {
            let o = &mut self.objs[k as usize];
            if (o.pressed == 1 || o.th == 3) && !o.in_plist {
                o.in_plist = true;
                self.plist[self.pn] = k;
                self.pn += 1;
                changed += 1;
            }
            if o.th_prev != o.th {
                changed += 2;
            }
            if o.armed && !o.in_slist {
                o.in_slist = true;
                self.slist[self.sn] = k;
                self.sn += 1;
                changed += 1;
            }
            o.th_prev = o.th;
        }
        if step == STEP_COUNT - 1 {
            for i in 0..self.bind_n {
                let k = self.bind_objs[i];
                let o = &mut self.objs[k as usize];
                for j in 0..4 {
                    if o.out[j] == 1 && !o.in_dlist[j] {
                        o.in_dlist[j] = true;
                        self.dlist[self.dn] = (k, j as u8);
                        self.dn += 1;
                        changed += 1;
                    }
                }
            }
        }
        let changed = changed != 0 || self.pn != old_pn || self.sn != old_sn || self.dn != old_dn;
        if !changed {
            if self.lockcnt != 0 && self.pn <= ROLLOVER_RESUME {
                self.unlock_tick();
            }
            return;
        }
        if self.cooldown == 0 {
            self.rebuild(&objs, ev);
        }
        if self.lockcnt != 0 && self.pn <= ROLLOVER_RESUME {
            self.unlock_tick();
        }
        if self.report != self.last_cmp {
            if self.pn <= ROLLOVER_LIMIT {
                if self.lock {
                    self.report = [0; 16];
                }
            } else {
                self.lock = true;
                self.lockcnt = 44000;
                self.report = [0; 16];
            }
            self.last_cmp = self.report;
            self.hid.keyboard = self.report;
            self.send(0x81, &self.report.clone());
        }
        let cons = (self.cons[0] as u32) << 16 | (self.cons[1] as u32) << 8 | self.cons[2] as u32;
        if cons != self.last_cons {
            self.last_cons = cons;
            self.hid.consumer = self.cons;
            self.send(0x83, &self.cons.clone());
        }
        let mouse = self.mouse[0] as u32;
        if mouse != self.last_mouse {
            self.last_mouse = mouse;
            self.hid.mouse_buttons = self.mouse[0];
            self.send(0x84, &self.mouse.clone());
        }
    }

    fn unlock_tick(&mut self) {
        self.lockcnt -= 1;
        if self.lockcnt != 0 {
            self.lock = false;
        }
    }

    /// A report went out on endpoint `ep` (stock: queued for USB).
    #[cfg_attr(not(feature = "std"), allow(unused_variables))]
    fn send(&mut self, ep: u8, d: &[u8]) {
        self.hid_gen = self.hid_gen.wrapping_add(1);
        #[cfg(feature = "std")]
        if self.sent_n < SENT_CAP {
            let mut data = [0; 16];
            data[..d.len()].copy_from_slice(d);
            self.sent[self.sent_n] = Sent { ep, data, len: d.len() as u8 };
            self.sent_n += 1;
        }
    }

    /// Report rebuild (the `0x92050 != 0` branch of `FUN_9D10`).
    fn rebuild(&mut self, step_objs: &[u8; 8], ev: &mut Events) {
        self.report = [0; 16];
        self.cons = [0; 3];
        self.mouse = [0; 3];
        self.flags = [false; 12];
        if self.pn >= 1 {
            self.flags[0] = self.plist[..self.pn].iter().any(|&k| self.objs[k as usize].code[0] == kc::FN);
            let l = self.flags[0] as usize;
            for i in 0..self.pn {
                let o = &self.objs[self.plist[i] as usize];
                if o.code[0] != kc::FN {
                    let c = o.code[l];
                    for (j, &m) in MOD_FLAG_CODES.iter().enumerate().skip(1) {
                        if m == c {
                            self.flags[j] = true;
                        }
                    }
                }
            }
            if self.mask_flag {
                for i in 0..self.pn {
                    let o = &self.objs[self.plist[i] as usize];
                    if o.pressed == 1 {
                        self.report[0] |= o.mask[l];
                    }
                }
            }
            for i in 0..self.pn {
                let k = self.plist[i] as usize;
                let o = &self.objs[k];
                let code = if o.pressed == 1 {
                    match o.th {
                        0 | 3 => Some(o.code[l]),
                        2 => Some(o.hold[l]),
                        _ => None,
                    }
                } else if o.th == 3 {
                    Some(o.code[l])
                } else {
                    None
                };
                if let Some(c) = code {
                    self.emit(c, k, ev);
                }
            }
        }
        for i in 0..self.dn {
            let (k, j) = self.dlist[i];
            let o = &self.objs[k as usize];
            if o.out[j as usize] == 1 {
                let c = o.scode[j as usize];
                self.emit(c, k as usize, ev);
            }
        }
        if self.win_lock {
            self.report[0] &= !0x88;
        }
        if self.socd_flag {
            self.snapshot = self.report;
            for i in 0..self.sn {
                let o = &self.objs[self.slist[i] as usize];
                if o.armed {
                    self.report[o.socd_byte as usize] &= !o.socd_mask;
                }
            }
        }
        for &k in step_objs {
            let o = &mut self.objs[k as usize];
            o.edge_p = false;
            o.edge_r = false;
        }
    }

    /// Stock `FUN_C698(code, obj)`: put one code into the reports or run its function.
    fn emit(&mut self, code: i16, k: usize, ev: &mut Events) {
        match code {
            kc::CONSUMER_FIRST..=kc::CONSUMER_LAST => {
                let n = (code - kc::CONSUMER_FIRST) as usize;
                self.cons[n >> 3] |= 1 << (n & 7);
            }
            121..=200 => {} // stock writes below the consumer report
            kc::MOUSE_FIRST..=kc::MOUSE_LAST => self.mouse[0] |= 1 << (code - kc::MOUSE_FIRST),
            func::FIRST..=func::LAST => self.function(code, k, ev),
            kc::FN.. => {}
            ..=0 => self.report[0] |= 1 << (code & 7),
            _ => {
                // usages past 119 have no bit in the 16-byte report (225–250, 256–300 too)
                let i = (code >> 3) as usize + 1;
                if i < 16 {
                    self.report[i] |= 1 << (code & 7);
                }
            }
        }
    }

    fn function(&mut self, c: i16, k: usize, ev: &mut Events) {
        let edge = self.objs[k].edge_p;
        match c {
            func::BOOTLOADER => ev.push(Event::Bootloader),
            func::REBOOT => ev.push(Event::Reboot),
            func::RT_OFF => {
                for o in self.objs.iter_mut() {
                    o.handler = Handler::Normal;
                }
            }
            func::RT_ON => {
                // handlers from key_mode again; no `return_th` gate, ignores DKS byte 47
                for (i, o) in self.objs.iter_mut().enumerate() {
                    o.handler = match o.key_mode {
                        1 => {
                            let g = guard_for(&GUARDS_304, i);
                            o.set_guard(&g, o.max as i32);
                            Handler::Rt
                        }
                        2 => Handler::Mode2Guard,
                        _ => Handler::Normal,
                    };
                }
            }
            func::RESET_PROFILE => ev.push(Event::ResetProfile),
            func::GRAVE_ESC => {
                if !self.flags[2] && !self.flags[6] {
                    self.report[6] |= 2; // Esc
                } else {
                    if self.flags[2] {
                        self.report[0] |= 2;
                    }
                    if self.flags[6] {
                        self.report[0] |= 0x20;
                    }
                    self.report[7] |= 0x20; // ` ~
                }
            }
            func::PROFILE_FIRST..=func::PROFILE_LAST => {
                if edge && self.cooldown == 0 {
                    self.cooldown = SWITCH_COOLDOWN;
                    ev.push(Event::SwitchProfile((c - func::PROFILE_FIRST) as u8));
                }
            }
            func::WIN_LOCK if edge => {
                self.win_lock = !self.win_lock;
                ev.push(Event::WinLock(self.win_lock));
            }
            func::BRIGHTNESS_UP..=func::SPEED_DOWN if edge => {
                ev.push(Event::Lighting(match c {
                    func::BRIGHTNESS_UP => LightCmd::BrightnessUp,
                    func::BRIGHTNESS_DOWN => LightCmd::BrightnessDown,
                    func::NEXT_EFFECT => LightCmd::NextEffect,
                    func::PREV_EFFECT => LightCmd::PrevEffect,
                    func::LIGHT_TOGGLE => LightCmd::Toggle,
                    func::SPEED_UP => LightCmd::SpeedUp,
                    _ => LightCmd::SpeedDown,
                }));
            }
            func::SWAP_WASD if edge => {
                for o in self.objs.iter_mut() {
                    o.code[0] = match o.code[0] {
                        4 => 80,
                        80 => 4,
                        26 => 82,
                        82 => 26,
                        22 => 81,
                        81 => 22,
                        7 => 79,
                        79 => 7,
                        x => x,
                    };
                }
            }
            _ => {}
        }
    }
}
