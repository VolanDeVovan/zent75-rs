//! The 51 effect ids of the stock firmware (0..=50; 51..=63 draw nothing). Behaviour, maths and
//! rounding follow the stock output (docs/re-config-rgb.md §5 table), implemented here from
//! those descriptions; names are QMK's for reference.
//!
//! Each call renders one *pass*: LEDs `17·iter .. 17·iter+16` for the chunked effects, or the
//! whole board / a single random LED for the others, exactly as many passes per frame as stock
//! takes (it matters for the random-driven effects and for which overlay colours they keep).

use super::geometry::{CENTER, LED_FLAGS, MATRIX, MATRIX_COLS, NO_LED, POINTS};
use super::math::{atan2_8, breath, dist, divs, divu, hsv, sin8, sin8s, sin_abs7};
use super::{Lighting, Pass};
use crate::layout::LED_COUNT;

const CHUNK: u32 = 17;

/// Pixel-rain / flow / fractal interval divisor: `min(speed + 16, 255) >> 4` (≥ 1).
fn pixel_div(speed: u8) -> u32 {
    (speed as u32 + 16).min(255) >> 4
}

impl Lighting {
    /// LED range of the current pass and whether another pass follows.
    fn chunk(&self) -> (usize, usize, bool) {
        let start = (self.iter as u32 * CHUNK) & 0xFF;
        let end_raw = (start + CHUNK) & 0xFF;
        (start as usize, end_raw.min(LED_COUNT as u32) as usize, end_raw < LED_COUNT as u32)
    }

    fn flag_ok(&self, led: usize) -> bool {
        LED_FLAGS[led] & self.p.flags != 0
    }

    /// Per-LED effect over the current chunk: `f(led) -> (h, s, v)`.
    fn per_led(&mut self, mut f: impl FnMut(&Self, usize) -> (u8, u8, u8)) -> Pass {
        let (start, end, more) = self.chunk();
        for led in start..end {
            if self.flag_ok(led) {
                let (h, s, v) = f(self, led);
                self.set(led, hsv(h, s, v));
            }
        }
        if more { Pass::More } else { Pass::Flush }
    }

    pub(super) fn effect_pass(&mut self, timer: u32, init: bool) -> Pass {
        let p = self.p;
        let (h0, s0, v0, sp) = (p.h, p.s, p.v, p.speed);
        let t = timer as u16 as u32;
        let px = |led: usize| POINTS[led].0;
        let py = |led: usize| POINTS[led].1;
        let dx = |led: usize| POINTS[led].0 as i32 - CENTER.0 as i32;
        let dy = |led: usize| POINTS[led].1 as i32 - CENTER.1 as i32;
        // runner time bases
        let t_i = ((((sp >> 2) as u32 + 1) * t) >> 8) as u8;
        let t_dxdy = (((sp >> 1) as u32 * t) >> 8) as u8;
        let t_sc = (((sp >> 2) as u32 * t) >> 8) as u8;
        let t_breath = ((t * (sp >> 3) as u32) >> 8) as u8;
        match p.mode >> 2 {
            0 => {
                if init {
                    self.set_all([0; 3]);
                    Pass::Flush
                } else {
                    Pass::Hold
                }
            }
            // solid colour
            1 => self.per_led(|_, _| (h0, s0, v0)),
            // "alphas mods": LEDs with flag 1 (none on this board) get H + speed
            2 => self.per_led(|_, led| (if LED_FLAGS[led] & 1 != 0 { h0.wrapping_add(sp) } else { h0 }, s0, v0)),
            // gradient up/down, left/right
            3 => self.per_led(|_, led| (h0.wrapping_add((sp >> 2).wrapping_mul(py(led) >> 4)), s0, v0)),
            4 => self.per_led(|_, led| (h0.wrapping_add(((px(led) as u32 * (sp >> 2) as u32) >> 5) as u8), s0, v0)),
            // breathing
            5 => {
                let v = breath(t_breath, v0);
                self.per_led(|_, _| (h0, s0, v))
            }
            // bands (runner i)
            6 | 7 => {
                let sat = p.mode >> 2 == 6;
                self.per_led(|_, led| {
                    let d = (((px(led) as i32 * 228) >> 8) + 28 - t_i as i32).abs() * 8;
                    if sat {
                        let s = (s0 as i32 - d).max(0) as u32;
                        (h0, ((s * s0 as u32) >> 8) as u8, v0)
                    } else {
                        let v = (255 - d).max(0) as u32;
                        (h0, s0, ((v * v0 as u32) >> 8) as u8)
                    }
                })
            }
            // pinwheel bands (runner dx/dy)
            8 | 9 => {
                let sat = p.mode >> 2 == 8;
                self.per_led(|_, led| {
                    let a = atan2_8(dy(led), dx(led));
                    let base = if sat { s0 } else { v0 };
                    let x = base.wrapping_sub(t_dxdy).wrapping_sub(a.wrapping_mul(3));
                    let x = ((x as u32 * base as u32) >> 8) as u8;
                    if sat { (h0, x, v0) } else { (h0, s0, x) }
                })
            }
            // spiral bands (runner dist)
            10 | 11 => {
                let sat = p.mode >> 2 == 10;
                self.per_led(|_, led| {
                    let a = atan2_8(dy(led), dx(led));
                    let d = dist(dx(led), dy(led));
                    let base = if sat { s0 } else { v0 };
                    let x = d.wrapping_sub(t_dxdy).wrapping_add(base).wrapping_sub(a);
                    let x = ((x as u32 * base as u32) >> 8) as u8;
                    if sat { (h0, x, v0) } else { (h0, s0, x) }
                })
            }
            // hue cycles
            12 => self.per_led(|_, _| (t_i, s0, v0)),
            13 => self.per_led(|_, led| (px(led).wrapping_sub(t_i), s0, v0)),
            14 => self.per_led(|_, led| (py(led).wrapping_sub(t_i), s0, v0)),
            15 => self.per_led(|_, led| {
                let ay = (py(led) as i8).wrapping_sub(CENTER.1 as i8).unsigned_abs();
                (h0.wrapping_add(px(led)).wrapping_sub(t_i).wrapping_add(ay), s0, v0)
            }),
            16 => self.per_led(|_, led| {
                let d = dist(dx(led), dy(led)) as u32;
                (t_dxdy.wrapping_add(((d * 3) >> 1) as u8), s0, v0)
            }),
            17 => self.per_led(|_, led| {
                let x = 30 - (dx(led) as i8).unsigned_abs() as i32;
                (t_dxdy.wrapping_add(dist(x, dy(led)).wrapping_mul(3)), s0, v0)
            }),
            18 => self.per_led(|_, led| (t_dxdy.wrapping_add(atan2_8(dy(led), dx(led))), s0, v0)),
            19 => self.per_led(|_, led| {
                let d = dist(dx(led), dy(led));
                (d.wrapping_sub(t_dxdy).wrapping_sub(atan2_8(dy(led), dx(led))), s0, v0)
            }),
            // beacons (runner sin/cos)
            20..=22 => {
                let cos = sin8s(t_sc.wrapping_add(64)) as i32;
                let sin = sin8s(t_sc) as i32;
                let id = p.mode >> 2;
                self.per_led(|_, led| {
                    let off = match id {
                        20 => (dy(led) * sin + dx(led) * cos) / 128,
                        21 => (dy(led) * sin + dx(led) * cos) / 64,
                        _ => {
                            let ax = (dx(led) as i8).unsigned_abs() as i32;
                            ((56 - ax) * cos + dy(led) * sin) * 3 / 128
                        }
                    };
                    (h0.wrapping_add(off as u8), s0, v0)
                })
            }
            // vendor "rainbow breathing": diagonal rainbow, lower rows scroll the other way
            // and are sent with R and B swapped
            23 => {
                let tt = ((((sp / 10) as u32 + 1) * t) >> 8) as u8;
                let (start, end, more) = self.chunk();
                for led in start..end {
                    if !self.flag_ok(led) {
                        continue;
                    }
                    let c = px(led).wrapping_sub(py(led)).wrapping_mul(3);
                    if py(led) < 22 {
                        self.set(led, hsv(c.wrapping_sub(tt), s0, v0));
                    } else {
                        let [r, g, b] = hsv(c.wrapping_add(tt), s0, v0);
                        self.set(led, [b, g, r]);
                    }
                }
                if more { Pass::More } else { Pass::Flush }
            }
            24 | 25 => self.raindrops(p.mode >> 2 == 25, t, init),
            // hue breathing / pendulum / wave
            26 => {
                let h = h0.wrapping_add(((sin_abs7(t_breath) * 6) >> 6) as u8);
                self.per_led(|_, _| (h, s0, v0))
            }
            27 => self.per_led(|_, led| {
                let a = sin8(t_i).wrapping_add(px(led)).wrapping_sub(128) as i8;
                (h0.wrapping_add((((a.unsigned_abs() as u32 & 0x7F) * 6) >> 6) as u8), s0, v0)
            }),
            28 => self.per_led(|_, led| {
                let d = (px(led).wrapping_sub(t_i) as i8).unsigned_abs();
                (h0.wrapping_add(((d as u32 * 3) >> 5) as u8), s0, v0)
            }),
            29 => self.pixel_rain(timer),
            30 => self.pixel_flow(timer, init),
            31 => self.pixel_fractal(timer, init),
            32 | 33 => self.heatmap(p.mode >> 2 == 32, init),
            34 => self.digital_rain(init),
            35 | 36 => self.reactive_simple(p.mode >> 2 == 36),
            37..=46 => self.reactive_splash(p.mode >> 2),
            47..=49 => self.starlight(p.mode >> 2, t, init),
            // "riverflow": per-LED phase-shifted breathing
            50 => {
                let (start, end, more) = self.chunk();
                for led in start..end {
                    let th = ((((t + 315 * led as u32) & 0xFFFF) * (sp >> 3) as u32) >> 8) as u8;
                    if self.flag_ok(led) {
                        self.set(led, hsv(h0, s0, breath(th, v0)));
                    }
                }
                if more { Pass::More } else { Pass::Flush }
            }
            _ => Pass::Flush,
        }
    }

    fn raindrop_colour(&mut self, jelly: bool) -> [u8; 3] {
        let (h0, s0, v0) = (self.p.h, self.p.s, self.p.v);
        if jelly {
            let h = self.rnd8.random8();
            let s = (self.rnd8.random8() >> 1) + 127;
            hsv(h, s, v0)
        } else {
            let r = (self.rnd8.random8() & 3) as i32;
            let delta = (h0 as i32 + 180) % 360 - h0 as i32;
            hsv(h0.wrapping_add(((delta / 4) * r) as u8), s0, v0)
        }
    }

    /// 24 raindrops / 25 jelly-bean raindrops: every pass may colour one random LED.
    fn raindrops(&mut self, jelly: bool, t: u32, init: bool) -> Pass {
        let (start, end, more) = self.chunk();
        if !init {
            let n = (((self.p.speed as u32 + 16).min(255) * t) >> 8) & 0xFFFF;
            if n.is_multiple_of(if jelly { 5 } else { 10 }) {
                let led = (self.rnd8.random8() as usize * LED_COUNT) >> 8;
                if self.flag_ok(led) {
                    let c = self.raindrop_colour(jelly);
                    self.set(led, c);
                }
            }
        } else {
            for led in start..end {
                if self.flag_ok(led) {
                    let c = self.raindrop_colour(jelly);
                    self.set(led, c);
                }
            }
        }
        if more { Pass::More } else { Pass::Flush }
    }

    /// Random colour of the pixel effects: off if `random8 & 2`, else random hue,
    /// saturation 127..254 at V.
    fn pixel_colour(&mut self) -> [u8; 3] {
        if self.rnd8.random8() & 2 == 0 {
            let h = self.rnd8.random8();
            let s = (self.rnd8.random8() >> 1) + 127;
            hsv(h, s, self.p.v)
        } else {
            [0; 3]
        }
    }

    /// 29: one random LED per interval.
    fn pixel_rain(&mut self, timer: u32) -> Pass {
        let more = (((self.iter as u32 + 1) * CHUNK) & 0xFF) < LED_COUNT as u32;
        if self.wait_rain < timer {
            let led = (self.rnd8.random8() as usize * LED_COUNT) >> 8;
            if self.flag_ok(led) {
                let c = self.pixel_colour();
                self.set(led, c);
                self.wait_rain = timer.wrapping_add(500 / pixel_div(self.p.speed));
            }
        }
        if more { Pass::More } else { Pass::Flush }
    }

    /// 30: an 85-LED colour line shifted by one LED per interval.
    fn pixel_flow(&mut self, timer: u32, init: bool) -> Pass {
        if self.wait_flow <= timer {
            if init {
                self.set_all([0; 3]);
                for i in 0..LED_COUNT {
                    self.pixel_line[i] = self.pixel_colour();
                }
            }
            let (start, end, more) = self.chunk();
            for led in start..end {
                if self.flag_ok(led) {
                    self.set(led, self.pixel_line[led]);
                }
            }
            if more {
                return Pass::More;
            }
            self.pixel_line.copy_within(1.., 0);
            self.pixel_line[LED_COUNT - 1] = self.pixel_colour();
            self.wait_flow = timer.wrapping_add(3000 / pixel_div(self.p.speed));
        }
        Pass::Flush
    }

    /// 31: mirrored per-row bit pattern growing outwards from the middle.
    fn pixel_fractal(&mut self, timer: u32, init: bool) -> Pass {
        if init {
            self.set_all([0; 3]);
        }
        let more = (((self.iter as u32 + 1) * CHUNK) & 0xFF) < LED_COUNT as u32;
        if self.wait_fractal < timer {
            let col = hsv(self.p.h, self.p.s, self.p.v);
            for r in 0..MATRIX.len() {
                for j in 0..7 {
                    let c = if self.fractal[r][j] { col } else { [0; 3] };
                    let (a, b) = if j < 6 { (j, MATRIX_COLS - 1 - j) } else { (6, 8) };
                    for l in [MATRIX[r][a], MATRIX[r][b]] {
                        if l != NO_LED {
                            self.set(l as usize, c);
                        }
                    }
                    if j < 6 {
                        self.fractal[r][j] = self.fractal[r][j + 1];
                    }
                }
                self.fractal[r][6] = self.rnd8.random8() & 3 == 0;
            }
            self.wait_fractal = timer.wrapping_add(3000 / pixel_div(self.p.speed));
        }
        if more { Pass::More } else { Pass::Flush }
    }

    /// 32 typing heatmap (decays) / 33 analog heatmap (rebuilt from key depth every frame).
    fn heatmap(&mut self, typing: bool, init: bool) -> Pass {
        if init {
            self.set_all([0; 3]);
            self.cells = [0; MATRIX.len() * MATRIX_COLS];
        }
        if self.iter == 0 {
            if typing {
                let now = self.tick as u16;
                self.heat_decay = now.wrapping_sub(self.heat_last) > 24;
                if self.heat_decay {
                    self.heat_last = now;
                }
            } else {
                self.heat_decay = false;
            }
        }
        let (start, end, more) = self.chunk();
        let (s0, v0) = (self.p.s, self.p.v);
        for (r, row) in MATRIX.iter().enumerate() {
            for (c, &l) in row.iter().enumerate() {
                let led = l as usize;
                if !(start..end).contains(&led) || !self.flag_ok(led) {
                    continue;
                }
                let i = r * MATRIX_COLS + c;
                let b = self.cells[i];
                let v = ((((b.saturating_add(170) as u32) * 3 + 2) & 0xFF) * v0 as u32) >> 8;
                let h = 170u8.wrapping_sub(b.saturating_sub(85));
                self.set(led, hsv(h, s0, v as u8));
                if self.heat_decay {
                    self.cells[i] = b.saturating_sub(1);
                }
            }
        }
        if more { Pass::More } else { Pass::Flush }
    }

    /// 34: green drops falling down the matrix columns (whole board in one pass).
    fn digital_rain(&mut self, init: bool) -> Pass {
        let v = self.p.v;
        let vv = v as u32;
        let pg = (vv * 3) >> 2;
        if init {
            self.set_all([0; 3]);
            self.cells = [0; MATRIX.len() * MATRIX_COLS];
            self.rain_drop = 0;
        }
        self.rain_decay = self.rain_decay.wrapping_add(1);
        let decay_now = self.rain_decay as u32 == divu(255, vv);
        for c in 0..MATRIX_COLS {
            for r in 0..MATRIX.len() {
                let i = r * MATRIX_COLS + c;
                if r == 0 && self.rain_drop == 0 && self.rnd.rand() < 0x555 {
                    self.cells[c] = v;
                } else {
                    let b = self.cells[i];
                    if b != 0 && b < v && decay_now {
                        self.cells[i] = b - 1;
                    }
                }
                let led = MATRIX[r][c];
                if led != NO_LED {
                    let x = self.cells[i] as u32;
                    let col = if x > pg {
                        let boost = divs(((x - pg) * pg) as i32, vv as i32 - pg as i32) as u8;
                        [boost, v, boost]
                    } else {
                        [0, divu(vv * x, pg) as u8, 0]
                    };
                    self.set(led as usize, col);
                }
            }
        }
        if decay_now {
            self.rain_decay = 0;
        }
        self.rain_drop = self.rain_drop.wrapping_add(1);
        if self.rain_drop > 28 {
            self.rain_drop = 0;
            for r in (1..MATRIX.len()).rev() {
                for c in 0..MATRIX_COLS {
                    let last = 5 * MATRIX_COLS + c;
                    if r == 5 && self.cells[last] == v {
                        self.cells[last] = v.wrapping_sub(1);
                    }
                    let above = (r - 1) * MATRIX_COLS + c;
                    if self.cells[above] >= v {
                        self.cells[above] = v.wrapping_sub(1);
                        self.cells[r * MATRIX_COLS + c] = v;
                    }
                }
            }
        }
        Pass::Flush
    }

    fn react_speed(&self) -> u32 {
        (self.p.speed as u32 + 1).min(255)
    }

    /// 35 / 36: each LED fades from its own newest press.
    fn reactive_simple(&mut self, hue: bool) -> Pass {
        let (start, end, more) = self.chunk();
        let sc = self.react_speed();
        let max_tick = 0xFFFF / sc;
        let h = self.hits_frame;
        for led in start..end {
            if !self.flag_ok(led) {
                continue;
            }
            let mut tick = max_tick;
            for j in (0..h.count.min(8) as usize).rev() {
                if h.led[j] as usize == led && (h.tick[j] as u32) < max_tick {
                    tick = h.tick[j] as u32;
                    break;
                }
            }
            let off = ((sc * tick) >> 8) & 0xFFFF;
            let c = if hue {
                hsv(self.p.h.wrapping_add(130u8.saturating_sub(off as u8)), self.p.s, self.p.v)
            } else {
                hsv(self.p.h, self.p.s, (((!off & 0xFF) * self.p.v as u32) >> 8) as u8)
            };
            self.set(led, c);
        }
        if more { Pass::More } else { Pass::Flush }
    }

    /// 37–46: splash family over the hit tracker; odd ids use only the newest hit.
    fn reactive_splash(&mut self, id: u8) -> Pass {
        let (start, end, more) = self.chunk();
        let sc = self.react_speed();
        let h = self.hits_frame;
        let count = h.count as usize;
        let first = if id % 2 == 1 { count.saturating_sub(1) } else { 0 };
        let (h0, s0, v0) = (self.p.h, self.p.s, self.p.v);
        for led in start..end {
            if !self.flag_ok(led) {
                continue;
            }
            let (x, y) = POINTS[led];
            let (mut hh, mut v) = (h0, 0u8);
            for j in first..count.min(8) {
                let dx = (x as i32 - h.x[j] as i32) as i16 as i32;
                let dy = (y as i32 - h.y[j] as i32) as i16 as i32;
                let d = dist(dx, dy) as u32;
                let tick = (sc * h.tick[j] as u32) >> 8 & 0xFFFF;
                let clamp = |e: u32| if e & 0xFFFF > 255 { 255 } else { e };
                let add = |v: u8, e: u32| ((!e & 0xFF) + v as u32).min(255) as u8;
                match id {
                    37 | 38 => v = add(v, clamp(tick + d * 5)),
                    39 | 40 => {
                        let ax = ((dx.unsigned_abs() as i32) << 4).min(255);
                        let ay = ((dy.unsigned_abs() as i32) << 4).min(255);
                        v = add(v, clamp(((d + tick) & 0xFFFF) + ax.min(ay) as u32));
                    }
                    41 | 42 => {
                        let e = if d < 73 { clamp(tick.wrapping_sub(d)) & 0xFFFF } else { 255 };
                        let near = ((dx + 8) as u16) < 17 || ((dy + 8) as u32 & 0xFFFF) < 17;
                        v = (v as u32 + if near { !e & 0xFF } else { 0 }).min(255) as u8;
                        hh = h0.wrapping_add((dy / 4) as u8);
                    }
                    43 | 44 => {
                        let e = clamp(tick.wrapping_sub(d));
                        v = add(v, e);
                        hh = hh.wrapping_add(e as u8);
                    }
                    _ => v = add(v, clamp(tick.wrapping_sub(d))),
                }
            }
            let vout = ((v0 as u32 * v as u32) >> 8) as u8;
            self.set(led, hsv(hh, s0, vout));
        }
        if more { Pass::More } else { Pass::Flush }
    }

    fn star_colour(&mut self, id: u8, t: u32) -> [u8; 3] {
        let th = ((t * (self.p.speed >> 3) as u32) >> 8) as u8;
        let v = breath(th, self.p.v);
        let (h0, s0) = (self.p.h, self.p.s);
        match id {
            47 => hsv(h0, s0, v),
            48 => {
                let r = (self.rnd.rand() % 61) as u8;
                hsv(h0, s0.wrapping_sub(30).wrapping_add(r), v)
            }
            _ => {
                let r = (self.rnd.rand() % 61) as u8;
                hsv(h0.wrapping_sub(30).wrapping_add(r), s0, v)
            }
        }
    }

    /// 47–49: random LEDs pick up the current breathing value (one per frame).
    fn starlight(&mut self, id: u8, t: u32, init: bool) -> Pass {
        if !init {
            let n = ((self.p.speed as u32 + 5).min(255) * t) >> 8;
            if n.is_multiple_of(5) {
                let led = (self.rnd.rand() % LED_COUNT as u32) as usize;
                let c = self.star_colour(id, t);
                self.set(led, c);
            }
            return Pass::Flush;
        }
        let (start, end, more) = self.chunk();
        for led in start..end {
            let c = self.star_colour(id, t);
            self.set(led, c);
        }
        if more { Pass::More } else { Pass::Flush }
    }
}
