//! One key object (stock: 0xB8 bytes at `0x804A8 + 0xB8·key`, docs/re-engine.md appendix)
//! and its actuation handlers (§7), key-group and DKS state machines (§9).
// Index loops mirror the stock per-slot / per-object tables on purpose.
#![allow(clippy::needless_range_loop)]

use crate::engine::compat::Guard;
use crate::engine::KeyInput;

/// Actuation handler (`+0x58`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handler {
    /// `key_mode` 0 (`0x15898`).
    Normal,
    /// `key_mode` 1, rapid trigger with top/bottom guards (`0x10B56`).
    Rt,
    /// `key_mode` 2: RT only past the normal actuation point (`0x11EC2`).
    Mode2,
    /// `key_mode` 2 with a bottom guard (`0x1085A`; Space, `]`, Enter, LShift with `return_th < 300`).
    Mode2Guard,
}

/// Advanced binding (`bind_key_mode`, handler at `+0x60`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bind {
    None,
    Group,
    Dks,
    /// Any other mode value: listed, but its handler does nothing.
    Inert,
}

#[derive(Debug, Clone, Copy)]
pub struct Obj {
    pub press_th: u16,
    pub return_th: u16,
    /// Height thresholds (µm, max at rest): RT top dead zone, bottom dead zone, normal press /
    /// release points.
    pub top: u16,
    pub bot: u16,
    pub p: u16,
    pub r: u16,
    /// Layer codes [main, Fn], tap-hold codes, modifier masks.
    pub code: [i16; 2],
    pub hold: [i16; 2],
    pub mask: [u8; 2],
    pub key_mode: u8,
    /// Tap-hold state (0 idle, 1 pending, 2 held, 3 tap) and its previous value.
    pub th: i16,
    pub th_prev: i16,
    /// 0 released, 1 pressed, 2 mode-2 "RT-released".
    pub pressed: u8,
    pub hold_cnt: u64,
    pub cancel: i16,
    pub socd_byte: u8,
    pub socd_mask: u8,
    pub armed: bool,
    pub edge_p: bool,
    pub edge_r: bool,
    pub low: u16,
    pub peak: u16,
    pub bflag: u16,
    pub bcnt: i16,
    pub tflag: u16,
    pub tcnt: i16,
    pub blen: i16,
    pub bthr: i16,
    pub tlen: i16,
    pub tthr: i16,
    pub max: i32,
    pub handler: Handler,
    pub in_plist: bool,
    pub in_slist: bool,
    // ---- key group / DKS
    pub bind: Bind,
    pub bp: u16,
    pub tp: u16,
    pub dks: u16,
    pub t_single: u16,
    pub t_delay: u16,
    pub t_rand: u16,
    pub scode: [i16; 4],
    /// `action[slot][phase]`.
    pub action: [[u8; 4]; 4],
    pub timer: [i16; 4],
    pub cnt: [i16; 4],
    pub in_dlist: [bool; 4],
    pub out: [u8; 4],
    pub raw: [u8; 4],
    pub linger: [u8; 4],
    /// DKS guard state: bottom flag, bottom counter, top flag, top counter (`+0xA8..+0xAE`).
    pub dks_guard: [i16; 4],
    /// DKS guard constants: botLen, botThr, topLen, topThr (`+0xB0..+0xB6`).
    pub dks_len_thr: [i16; 4],
}

impl Default for Obj {
    fn default() -> Self {
        Self::new()
    }
}

/// `y` inside the plausible sensor range (`y − 6000 < 0xD120`, unsigned): guards only hold then.
#[inline]
fn y_ok(y: i32) -> bool {
    (y as u32).wrapping_sub(6000) < 0xD120
}

impl Obj {
    pub const fn new() -> Self {
        Self {
            press_th: 0,
            return_th: 0,
            top: 0,
            bot: 0,
            p: 0,
            r: 0,
            code: [1000; 2],
            hold: [1000; 2],
            mask: [0; 2],
            key_mode: 0,
            th: 0,
            th_prev: 0,
            pressed: 0,
            hold_cnt: 0,
            cancel: 1000,
            socd_byte: 0,
            socd_mask: 0,
            armed: false,
            edge_p: false,
            edge_r: false,
            low: 0,
            peak: 0,
            bflag: 0,
            bcnt: 0,
            tflag: 0,
            tcnt: 0,
            blen: 0,
            bthr: 0,
            tlen: 0,
            tthr: 0,
            max: 3420,
            handler: Handler::Normal,
            in_plist: false,
            in_slist: false,
            bind: Bind::None,
            bp: 0,
            tp: 0,
            dks: 0,
            t_single: 0,
            t_delay: 0,
            t_rand: 0,
            scode: [1000; 4],
            action: [[0; 4]; 4],
            timer: [0; 4],
            cnt: [0; 4],
            in_dlist: [false; 4],
            out: [0; 4],
            raw: [0; 4],
            linger: [0; 4],
            dks_guard: [0; 4],
            dks_len_thr: [0; 4],
        }
    }

    /// The state the stock config apply clears (the top guard state survives, like stock).
    pub fn reset_state(&mut self) {
        self.pressed = 0;
        self.bflag = 0;
        self.bcnt = 0;
        self.edge_p = false;
        self.edge_r = false;
        self.blen = 0;
        self.bthr = 0;
        self.th = 0;
        self.th_prev = 0;
        self.hold_cnt = 0;
        self.in_plist = false;
        self.in_slist = false;
        self.bind = Bind::None;
    }

    pub fn set_guard(&mut self, g: &Guard, max: i32) {
        self.blen = g.bot_len;
        self.bthr = g.bot_thr;
        self.tlen = g.top_len;
        self.tthr = g.top_thr_for(max);
    }

    #[inline]
    fn press(&mut self, ha: i32) {
        self.pressed = 1;
        self.low = ha as u16;
        self.edge_p = true;
    }
    #[inline]
    fn release(&mut self, to: u8, hb: i32) {
        self.pressed = to;
        self.peak = hb as u16;
        self.edge_r = true;
    }

    /// Run the actuation handler with this step's heights.
    pub fn actuate(&mut self, i: KeyInput) {
        match self.handler {
            Handler::Normal => self.normal(i),
            Handler::Rt => self.rt(i),
            Handler::Mode2 => self.mode2(i, false),
            Handler::Mode2Guard => self.mode2(i, true),
        }
    }

    fn normal(&mut self, i: KeyInput) {
        if self.pressed == 0 {
            if i.ha <= self.p as i32 {
                self.press(i.ha);
            }
        } else if self.r as i32 <= i.hb {
            self.release(0, i.hb);
        }
    }

    fn rt(&mut self, i: KeyInput) {
        let (ha, hb) = (i.ha, i.hb);
        let (top, bot) = (self.top as i32, self.bot as i32);
        let (pt, rt) = (self.press_th as i32, self.return_th as i32);
        if self.pressed == 0 {
            if self.tcnt != 0 {
                let c = self.tcnt.wrapping_sub(1);
                self.tcnt = if (self.tthr as i32) < ha { c } else { 0 };
                if y_ok(i.y) {
                    if self.tcnt != 0 {
                        return;
                    }
                } else {
                    self.tcnt = 0;
                }
            }
            if self.tflag == 0 && self.tthr as i32 <= ha {
                self.tflag = 1;
                self.tcnt = self.tlen;
                return;
            }
            if (self.peak as i32) < hb && hb <= top {
                self.peak = hb as u16;
            }
            if (ha <= top - pt && ha <= self.peak as i32 - pt) || ha <= bot {
                self.press(ha);
                self.tflag = 0;
            }
        } else {
            if self.bcnt != 0 {
                let c = self.bcnt.wrapping_sub(1);
                self.bcnt = if hb < self.bthr as i32 { c } else { 0 };
                if y_ok(i.y) {
                    if self.bcnt != 0 {
                        return;
                    }
                } else {
                    self.bcnt = 0;
                }
            }
            if self.bflag == 0 && hb < 101 {
                self.bflag = 1;
                self.bcnt = self.blen;
                return;
            }
            if ha < self.low as i32 && bot <= ha {
                self.low = ha as u16;
            }
            if (bot + rt <= hb && self.low as i32 + rt <= hb) || top <= hb {
                self.release(0, hb);
                self.bflag = 0;
            }
        }
    }

    fn mode2(&mut self, i: KeyInput, guard: bool) {
        let (ha, hb) = (i.ha, i.hb);
        let (top, bot) = (self.top as i32, self.bot as i32);
        let (pt, rt) = (self.press_th as i32, self.return_th as i32);
        match self.pressed {
            1 => {
                if guard {
                    if ha < self.low as i32 && bot <= ha {
                        self.low = ha as u16;
                    }
                    if self.bcnt != 0 {
                        let c = self.bcnt.wrapping_sub(1);
                        self.bcnt = if hb < self.bthr as i32 { c } else { 0 };
                        if y_ok(i.y) {
                            if self.bcnt != 0 {
                                return;
                            }
                        } else {
                            self.bcnt = 0;
                        }
                    }
                    if self.r as i32 <= hb {
                        self.release(0, hb);
                        self.bflag = 0;
                        return;
                    }
                    if self.bflag == 0 && hb < 51 {
                        self.bflag = 1;
                        self.bcnt = self.blen;
                        return;
                    }
                    if bot + rt <= hb && self.low as i32 + rt <= hb {
                        self.release(2, hb);
                        self.bflag = 0;
                    }
                } else {
                    if self.r as i32 <= hb {
                        self.release(0, hb);
                        return;
                    }
                    if ha < self.low as i32 && bot <= ha {
                        self.low = ha as u16;
                    }
                    if bot + rt <= hb && self.low as i32 + rt <= hb {
                        self.release(2, hb);
                    }
                }
            }
            2 => {
                if self.r as i32 <= hb {
                    self.release(0, hb);
                    return;
                }
                if (self.peak as i32) < hb && hb <= top {
                    self.peak = hb as u16;
                }
                if ha <= top - pt && (ha <= self.peak as i32 - pt || ha <= bot) {
                    self.press(ha);
                }
            }
            0 => {
                if (self.peak as i32) < hb && hb <= top {
                    self.peak = hb as u16;
                }
                if ha <= self.p as i32 {
                    self.press(ha);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ groups / DKS

    /// Step-10 handler of a bound key (`+0x60`).
    pub fn bind_step(&mut self, i: KeyInput, seed: &mut u16) {
        match self.bind {
            Bind::Group => {
                for j in 0..4 {
                    self.out[j] = if self.scode[j] != 1000 { self.pressed } else { 0 };
                }
            }
            Bind::Dks => self.dks_step(i, seed),
            Bind::None | Bind::Inert => {}
        }
    }

    /// Stock `FUN_14148`: apply phase `ph` to slot `j`.
    fn fire(&mut self, ph: usize, seed: &mut u16) {
        for j in 0..4 {
            if self.scode[j] == 1000 {
                self.timer[j] = 0;
                continue;
            }
            match self.action[j][ph] {
                1 => self.timer[j] = -1,
                2 => {
                    let r = rnd(seed, self.t_rand as u8);
                    self.timer[j] = (r as i16).wrapping_add(self.t_delay as i16).wrapping_add(self.t_single as i16);
                    self.cnt[j] = 0;
                }
                _ => {
                    if self.timer[j] == -1 {
                        self.timer[j] = 0;
                    }
                }
            }
        }
    }

    /// Stock `FUN_E31C`.
    fn dks_step(&mut self, i: KeyInput, seed: &mut u16) {
        let (ha, hb) = (i.ha, i.hb);
        let (bp, tp) = (self.bp as i32, self.tp as i32);
        let [blen, bthr, tlen, tthr] = self.dks_len_thr;
        let g = &mut self.dks_guard;
        let mut skip = false;
        match self.dks {
            2 => {
                if g[1] != 0 {
                    let c = g[1].wrapping_sub(1);
                    g[1] = if hb < bthr as i32 { c } else { 0 };
                    if y_ok(i.y) {
                        skip = g[1] != 0;
                    } else {
                        g[1] = 0;
                    }
                }
                if !skip {
                    if g[0] == 0 && hb <= bthr as i32 {
                        g[0] = 1;
                        g[1] = blen;
                    } else if bp <= hb {
                        self.fire(2, seed);
                        self.dks = 3;
                        self.dks_guard[0] = 0;
                    }
                }
            }
            0 => {
                if g[3] != 0 {
                    let c = g[3].wrapping_sub(1);
                    g[3] = if (tthr as i32) < ha { c } else { 0 };
                    if y_ok(i.y) {
                        skip = g[3] != 0;
                    } else {
                        g[3] = 0;
                    }
                }
                if !skip {
                    if g[2] == 0 && tthr as i32 <= ha {
                        g[2] = 1;
                        g[3] = tlen;
                    } else if ha <= tp {
                        self.fire(0, seed);
                        self.dks = 1;
                        self.dks_guard[2] = 0;
                    }
                }
            }
            1 => {
                if ha <= bp {
                    self.fire(1, seed);
                    self.dks = 2;
                }
                if tp <= hb {
                    self.fire(3, seed);
                    self.dks = 0;
                }
            }
            3 => {
                if tp <= hb {
                    self.fire(3, seed);
                    self.dks = 0;
                }
                if ha <= bp {
                    self.fire(1, seed);
                    self.dks = 2;
                }
            }
            _ => {}
        }
        for j in 0..4 {
            if self.scode[j] == 1000 {
                self.raw[j] = 0;
                continue;
            }
            let t = self.timer[j];
            if t == -1 {
                self.raw[j] = 1;
            } else if t < 1 {
                self.raw[j] = 0;
            } else if self.cnt[j] < t {
                self.cnt[j] += 1;
                if (t as i32 - self.t_single as i32) < self.cnt[j] as i32 {
                    self.raw[j] = 1;
                }
            } else {
                self.raw[j] = 0;
            }
        }
        for j in 0..4 {
            if self.raw[j] == 0 {
                if self.out[j] != 0 {
                    if self.timer[j] == 0 {
                        if self.linger[j] < 8 {
                            self.linger[j] += 1;
                        } else {
                            self.out[j] = 0;
                        }
                    } else {
                        self.out[j] = 0;
                    }
                }
            } else {
                self.out[j] = 1;
                self.linger[j] = 0;
            }
        }
    }
}

/// Stock `FUN_15C06`: `s = s·0x805 + 0x3619`, returns `((s>>8)+s & 0xFF)·n >> 8`.
fn rnd(seed: &mut u16, n: u8) -> u16 {
    let s = seed.wrapping_mul(0x805).wrapping_add(0x3619);
    *seed = s;
    let s = s as u32;
    (((((s >> 8) + s) & 0xFF) * n as u32) >> 8) as u16
}
