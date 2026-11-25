//! RGB lighting: an independent implementation of the stock 0.0.18 lighting engine
//! (docs/re-config-rgb.md §5), frame-exact against the stock firmware's own output
//! (`tests/lighting.rs`, fixtures rendered by the firmware in the emulator).
//!
//! # Time base
//! Stock counts *ticks* of 1/1375 s (64 ADC steps); every speed constant of the effects is in
//! ticks. [`render`](Lighting::render) takes milliseconds and converts with [`TICK_HZ`] — the
//! one constant to retune once the 0.73 s profile blink (1000 ticks) has been timed on the
//! board. [`render_at_tick`](Lighting::render_at_tick) drives the engine in ticks directly.
//!
//! # Frame pipeline (stock `FUN_00008BE2`)
//! One call = one frame: age the key-hit tracker, register new key presses, snapshot the
//! hits, then render the effect in passes of 17 LEDs (or one pass for some effects) and apply
//! the overlays after *every* pass, in stock order:
//!
//! 1. Fn layer (Fn held + `rgb_fn_enable`) — else live preview mode 1/2 or static colours,
//! 2. Caps Lock red, 3. WinLock red (Fn not held, `C70` = 1), 4. sleep blanking,
//! 5. the profile-switch blink, overriding everything.
//!
//! Stock has one more overlay on top, the indicator of its board-side calibration routine
//! (hold C at power-up); that routine is not part of this firmware.
//!
//! Effects draw into a persistent LED buffer that the overlays also write (effects that only
//! touch some LEDs therefore keep overlay colours, as on stock). The frame is flushed — with
//! everything blanked if `rgb_enable` = 0 or USB is suspended — unless the effect is 0 (NONE),
//! which stock stops flushing after its first black frame.

// Index loops mirror the stock tables (LED / matrix cell numbers) on purpose.
#![allow(clippy::needless_range_loop)]

mod effects;
pub mod geometry;
pub mod math;

use crate::config::{Global, Profile, PREVIEW_SIZE};
use crate::engine::Engine;
use crate::hid::host_led;
use crate::keycode as kc;
use crate::keys::{KeyProcessor, LightCmd};
use crate::layout::{KEY_COUNT, LED_COUNT, LED_TO_KEY, SCAN_HZ};
use geometry::{MATRIX, MATRIX_COLS, MATRIX_ROWS, NO_LED, POINTS};
use math::{color_correct, Rand, Random8};

/// Stock lighting tick rate (Hz). Assumed from the code (one tick per 64 ADC steps at
/// 11 × 8000 steps/s); measure the profile blink (1000 ticks) on hardware to confirm.
pub const TICK_HZ: u32 = 1375;
/// Minimum ticks between frame starts (stock state-3 wait, 5.8 ms).
pub const FRAME_TICKS: u32 = 8;
/// Profile-switch blink: total length and half period in ticks.
pub const BLINK_TICKS: i32 = 1000;
pub const BLINK_HALF_TICKS: i32 = 200;
/// Live-preview mode expiry without a new preview write (stock: 3 s).
pub const PREVIEW_TIMEOUT_TICKS: u32 = 3 * TICK_HZ;
/// Blink colour per profile (stock tables `0x80040440/30/20`).
pub const PROFILE_BLINK: [[u8; 3]; 4] = [[60, 60, 60], [60, 0, 0], [0, 60, 0], [60, 30, 0]];

pub type Rgb = [u8; 3];

/// Hit tracker (QMK `last_hit_t`): newest key presses with their position and age in ticks.
#[derive(Clone, Copy, Default)]
struct Hits {
    count: u8,
    x: [u8; 8],
    y: [u8; 8],
    led: [u8; 8],
    tick: [u16; 8],
}

/// Run-time lighting parameters (stock `0x843F0..0x843F7`), reloaded from the profile on every
/// config change; the keyboard lighting keys modify them (brightness/effect also in the profile).
#[derive(Clone, Copy)]
struct Params {
    mode: u8,
    h: u8,
    s: u8,
    v: u8,
    speed: u8,
    flags: u8,
}

/// Outcome of one render pass.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// Render the next 17 LEDs in another pass.
    More,
    /// Frame complete: flush.
    Flush,
    /// Frame complete, nothing to send (effect 0 after its first frame).
    Hold,
}

pub struct Lighting {
    p: Params,
    /// Static colours, colour-corrected and brightness-scaled (`0x9152C`).
    statics: [Rgb; LED_COUNT],
    solid: [bool; LED_COUNT],
    sleep_seconds: u32,
    /// The LED frame buffer effects and overlays draw into (`0x7970`).
    leds: [Rgb; LED_COUNT],
    /// Last frame handed to the LEDs.
    sent: [Rgb; LED_COUNT],
    // frame scheduler
    tick: u32,
    ms_last: Option<u32>,
    ms_acc: u32,
    frame_tick: Option<u32>,
    last_pass_tick: Option<u32>,
    stored_effect: u8,
    stored_enable: u8,
    stored_flags: u8,
    iter: u8,
    // effect state
    hits: Hits,
    hits_frame: Hits,
    rnd8: Random8,
    rnd: Rand,
    /// 6×15 per-cell buffer of the heatmaps / digital rain (`0x91C08`).
    cells: [u8; MATRIX_ROWS * MATRIX_COLS],
    /// Per-cell key depth 0..255 for effect 33.
    depth: [u8; MATRIX_ROWS * MATRIX_COLS],
    pixel_line: [Rgb; LED_COUNT],
    fractal: [[bool; 7]; MATRIX_ROWS],
    wait_rain: u32,
    wait_flow: u32,
    wait_fractal: u32,
    heat_last: u16,
    heat_decay: bool,
    rain_drop: u8,
    rain_decay: u8,
    // inputs
    host_leds: u8,
    usb_suspended: bool,
    pressed_cells: [bool; MATRIX_ROWS * MATRIX_COLS],
    new_press: [bool; MATRIX_ROWS * MATRIX_COLS],
    fn_held: bool,
    idle_scans: u32,
    winlock: bool,
    wipe_blink: Option<bool>,
    blink: Option<(u8, Option<u32>)>,
    // live preview (bank address 0x80078000: alpha[85], rgb[85×3])
    preview: [u8; PREVIEW_SIZE],
    preview_mode: u8,
    preview_age: u32,
}

impl Default for Lighting {
    fn default() -> Self {
        Self::new()
    }
}

impl Lighting {
    pub const fn new() -> Self {
        Self {
            p: Params { mode: 0, h: 0, s: 0, v: 0, speed: 0, flags: 0 },
            statics: [[0; 3]; LED_COUNT],
            solid: [false; LED_COUNT],
            sleep_seconds: 0,
            leds: [[0; 3]; LED_COUNT],
            sent: [[0; 3]; LED_COUNT],
            tick: 0,
            ms_last: None,
            ms_acc: 0,
            frame_tick: None,
            last_pass_tick: None,
            // stock boots with these at 0xFF: the first frame always initialises the effect
            stored_effect: 0xFF,
            stored_enable: 0xFF,
            stored_flags: 0xFF,
            iter: 0,
            hits: Hits { count: 0, x: [0; 8], y: [0; 8], led: [0; 8], tick: [0; 8] },
            hits_frame: Hits { count: 0, x: [0; 8], y: [0; 8], led: [0; 8], tick: [0; 8] },
            rnd8: Random8(1337),
            rnd: Rand(1),
            cells: [0; MATRIX_ROWS * MATRIX_COLS],
            depth: [0; MATRIX_ROWS * MATRIX_COLS],
            pixel_line: [[0; 3]; LED_COUNT],
            fractal: [[false; 7]; MATRIX_ROWS],
            wait_rain: 0,
            wait_flow: 0,
            wait_fractal: 0,
            heat_last: 0,
            heat_decay: false,
            rain_drop: 0,
            rain_decay: 0,
            host_leds: 0,
            usb_suspended: false,
            pressed_cells: [false; MATRIX_ROWS * MATRIX_COLS],
            new_press: [false; MATRIX_ROWS * MATRIX_COLS],
            fn_held: false,
            idle_scans: 0,
            winlock: false,
            wipe_blink: None,
            blink: None,
            preview: [0; PREVIEW_SIZE],
            preview_mode: 0,
            preview_age: 0,
        }
    }

    // ------------------------------------------------------------------ configuration

    /// Reload from the profile (stock `FUN_000149D8`): brightness clamped to 30 (written back),
    /// V = 6 × brightness, static colours colour-corrected and scaled. Drops runtime-only
    /// speed changes made with the keys.
    pub fn configure(&mut self, p: &mut Profile) {
        let v = p.clamp_brightness();
        let c = p.rgb_config();
        self.p = Params { mode: c[0], h: c[1], s: c[2], v, speed: c[4], flags: p.rgb_flags() };
        for i in 0..LED_COUNT {
            self.solid[i] = p.led_solid(i);
        }
        self.sleep_seconds = p.sleep_seconds();
        self.refresh_statics(p);
    }

    fn refresh_statics(&mut self, p: &Profile) {
        let v = self.p.v as u32;
        for i in 0..LED_COUNT {
            let c = color_correct(p.led_rgb(i));
            self.statics[i] = c.map(|x| (x as u32 * v / 255) as u8);
        }
    }

    pub fn set_host_leds(&mut self, l: u8) {
        self.host_leds = l;
    }
    pub fn set_usb_suspended(&mut self, s: bool) {
        self.usb_suspended = s;
    }
    /// WinLock state (toggled by keycode 316 in the key layer, RAM only) for the indicator.
    pub fn set_winlock(&mut self, on: bool) {
        self.winlock = on;
    }
    pub fn winlock(&self) -> bool {
        self.winlock
    }
    /// Full-wipe feedback: `Some(true)` all LEDs (60,60,60), `Some(false)` all dark, `None`
    /// normal lighting.
    pub fn set_wipe_blink(&mut self, phase: Option<bool>) {
        self.wipe_blink = phase;
    }
    /// Start the profile-switch blink (timed from the next frame).
    pub fn start_profile_blink(&mut self, p: u8) {
        self.blink = Some((p.min(3), None));
    }
    /// A vendor frame arrived: resets the sleep timer like a key press.
    pub fn note_activity(&mut self) {
        self.idle_scans = 0;
    }
    pub fn idle_seconds(&self) -> u32 {
        self.idle_scans / SCAN_HZ
    }

    /// Called every scan: key presses for the reactive effects, the Fn layer as the key pipeline
    /// resolved it (stock flag `0x91F9C`), idle timer.
    pub fn on_scan(&mut self, k: &KeyProcessor) {
        let mut any = false;
        for key in 0..KEY_COUNT {
            let down = k.is_pressed(key);
            self.press_key(key, down);
            any |= down;
        }
        self.fn_held = k.fn_layer();
        self.idle_scans = if any { 0 } else { self.idle_scans.saturating_add(1) };
    }

    /// Feed one key's state; a new press is registered with the reactive effects at the next
    /// frame (all matrix cells of the key, i.e. three for the space bar, like stock).
    pub fn press_key(&mut self, key: usize, down: bool) {
        // precomputed cells: this runs for every key on every scan
        let Some(cells) = geometry::KEY_CELLS.get(key) else { return };
        for &i in cells {
            if i == NO_LED {
                break;
            }
            let i = i as usize;
            if down && !self.pressed_cells[i] {
                self.new_press[i] = true;
            }
            self.pressed_cells[i] = down;
        }
    }

    /// Fn-layer state for callers that do not run [`Lighting::on_scan`] (tests).
    pub fn set_fn_held(&mut self, on: bool) {
        self.fn_held = on;
    }

    // ------------------------------------------------------------------ keyboard commands

    /// Lighting keycodes 311–318 (stock `FUN_0000C698`): brightness and effect also update the
    /// profile in RAM (persisted by `BE 02`), speed is runtime only.
    pub fn command(&mut self, c: LightCmd, p: &mut Profile) {
        match c {
            LightCmd::BrightnessUp => {
                self.p.v = self.p.v.min(168) + 12;
                p.set_brightness(self.p.v / 6);
                self.refresh_statics(p);
            }
            LightCmd::BrightnessDown => {
                self.p.v = self.p.v.max(12) - 12;
                p.set_brightness(self.p.v / 6);
                self.refresh_statics(p);
            }
            LightCmd::NextEffect => {
                let mut e = ((self.p.mode >> 2) + 1) & 0x3F;
                if e == 2 {
                    e = 3;
                }
                if e == 51 {
                    e = 1;
                }
                self.set_mode(e, p);
            }
            LightCmd::PrevEffect => {
                let mut e = (self.p.mode >> 2).wrapping_add(0x3F) & 0x3F;
                if e == 2 {
                    e = 1;
                }
                if e == 0 {
                    e = 50;
                }
                self.set_mode(e, p);
            }
            LightCmd::Toggle => p.toggle_rgb_enabled(),
            LightCmd::SpeedUp => self.p.speed = self.p.speed.min(238) + 17,
            LightCmd::SpeedDown => self.p.speed = self.p.speed.max(17) - 17,
        }
    }

    fn set_mode(&mut self, e: u8, p: &mut Profile) {
        self.p.mode = (self.p.mode & 3) | (e << 2);
        p.set_effect(e);
    }

    /// Current run-time effect id, H, S, V, speed.
    pub fn params(&self) -> (u8, u8, u8, u8, u8) {
        (self.p.mode >> 2, self.p.h, self.p.s, self.p.v, self.p.speed)
    }

    // ------------------------------------------------------------------ live preview

    /// `BE 03` into the preview area at byte offset `off` (0 = `alpha[85]`, 0x55 = `rgb[85×3]`).
    /// Starting exactly at 0 selects blend mode 1, exactly at 0x55 replace mode 2; mode 2
    /// colours are colour-corrected and scaled by the current V in place (on every write, as
    /// stock). Anything past the 340-byte area is dropped.
    pub fn preview_write(&mut self, off: usize, data: &[u8]) {
        if off == 0 {
            self.preview_mode = 1;
        } else if off == LED_COUNT {
            self.preview_mode = 2;
        }
        self.preview_age = 0;
        let end = (off + data.len()).min(self.preview.len());
        if off < end {
            self.preview[off..end].copy_from_slice(&data[..end - off]);
        }
        if self.preview_mode == 2 {
            let v = self.p.v as u32;
            for i in 0..LED_COUNT {
                let o = LED_COUNT + 3 * i;
                let c = color_correct([self.preview[o], self.preview[o + 1], self.preview[o + 2]]);
                for j in 0..3 {
                    self.preview[o + j] = (c[j] as u32 * v / 255) as u8;
                }
            }
        }
    }

    pub fn preview_mode(&self) -> u8 {
        self.preview_mode
    }

    // ------------------------------------------------------------------ rendering

    /// Render one frame at `now_ms` into `out`. Call at least every 5.8 ms for stock pacing;
    /// calls closer than [`FRAME_TICKS`] re-send the previous frame.
    pub fn render(&mut self, now_ms: u32, p: &Profile, g: &Global, e: &Engine, out: &mut [Rgb; LED_COUNT]) {
        match self.ms_last {
            None => self.tick = ((now_ms as u64 * TICK_HZ as u64) / 1000) as u32,
            Some(last) => {
                self.ms_acc += now_ms.wrapping_sub(last).min(1 << 20) * TICK_HZ;
                self.tick = self.tick.wrapping_add(self.ms_acc / 1000);
                self.ms_acc %= 1000;
            }
        }
        self.ms_last = Some(now_ms);
        if let Some(f) = self.frame_tick
            && self.tick.wrapping_sub(f) < FRAME_TICKS
        {
            *out = self.sent;
            self.blank_if_off(p, out);
            return;
        }
        let t = self.tick;
        self.update_depth(g, e);
        self.frame(t, p);
        *out = self.sent;
        self.blank_if_off(p, out);
    }

    /// One frame at stock tick `tick` (tests and fixed-rate callers). Key presses are taken
    /// from [`Lighting::on_scan`]; effect 33's depths from [`Lighting::set_depth`].
    pub fn render_at_tick(&mut self, tick: u32, p: &Profile, out: &mut [Rgb; LED_COUNT]) {
        self.tick = tick;
        self.frame(tick, p);
        *out = self.sent;
        self.blank_if_off(p, out);
    }

    /// Effect-33 depth (0..255) of a key, for tests / callers without an [`Engine`].
    pub fn set_depth(&mut self, key: usize, d: u8) {
        for (r, row) in MATRIX.iter().enumerate() {
            for (c, &led) in row.iter().enumerate() {
                if led != NO_LED && LED_TO_KEY[led as usize] as usize == key {
                    self.depth[r * MATRIX_COLS + c] = d;
                }
            }
        }
    }

    /// Key depth like stock `FUN_00010550`: 0..255 over the switch travel with a 100 µm dead
    /// band at both ends.
    fn update_depth(&mut self, g: &Global, e: &Engine) {
        for (r, row) in MATRIX.iter().enumerate() {
            for (c, &led) in row.iter().enumerate() {
                if led == NO_LED {
                    continue;
                }
                let key = LED_TO_KEY[led as usize] as usize;
                let full = g.switch_travel_um(key) as i32;
                let mut h = full - e.travel(key) as i32;
                if full - 99 <= h {
                    h = full - 100;
                }
                if h < 100 {
                    h = 100;
                }
                self.depth[r * MATRIX_COLS + c] = math::divs((full - 100 - h) * 255, full - 200) as u8;
            }
        }
    }

    fn blank_if_off(&self, p: &Profile, out: &mut [Rgb; LED_COUNT]) {
        // stock blanks at the flush; applied to re-sent frames too (effect 0 keeps its last
        // frame otherwise — deviation, so "off"/suspend always darkens the board)
        if !p.rgb_enabled() || self.usb_suspended {
            *out = [[0; 3]; LED_COUNT];
        }
    }

    fn set(&mut self, led: usize, c: Rgb) {
        self.leds[led] = c;
    }
    fn set_all(&mut self, c: Rgb) {
        self.leds = [c; LED_COUNT];
    }

    fn frame(&mut self, t: u32, p: &Profile) {
        // main-loop pass: age the hits by the elapsed ticks (stock: before new presses)
        let delta = match self.last_pass_tick {
            Some(l) => t.wrapping_sub(l),
            None => t,
        };
        self.last_pass_tick = Some(t);
        self.age_hits(delta);
        self.register_presses();
        if self.p.mode >> 2 == 33 {
            self.build_analog_heatmap();
        }
        // preview timer
        if self.preview_mode != 0 {
            self.preview_age = self.preview_age.saturating_add(delta);
            if self.preview_age > PREVIEW_TIMEOUT_TICKS {
                self.preview_mode = 0;
            }
        }
        // state 0
        self.frame_tick = Some(t);
        self.hits_frame = self.hits;
        self.iter = 0;
        // state 1: passes
        let result = loop {
            let init = (self.p.mode >> 2) != self.stored_effect || (self.p.mode & 3) != self.stored_enable;
            if self.stored_flags != self.p.flags {
                self.stored_flags = self.p.flags;
                self.set_all([0; 3]);
            }
            let r = self.effect_pass(t, init);
            self.iter = self.iter.wrapping_add(1);
            self.overlays(t, p);
            if r != Pass::More {
                break r;
            }
            if self.iter > 16 {
                break Pass::Flush; // cannot happen (≤ 5 passes); bound the loop anyway
            }
        };
        // state 2: flush
        if result == Pass::Flush {
            self.stored_effect = self.p.mode >> 2;
            self.stored_enable = self.p.mode & 3;
            if !p.rgb_enabled() || self.usb_suspended {
                self.set_all([0; 3]);
            }
            self.sent = self.leds;
        }
    }

    fn age_hits(&mut self, delta: u32) {
        let n = self.hits.count as usize;
        for j in 0..n.min(8) {
            let tk = self.hits.tick[j] as u32;
            if 0xFFFFu32.wrapping_sub(delta) < tk {
                // stock (QMK) drops the *count*, not this entry
                self.hits.count = self.hits.count.wrapping_sub(1);
                continue;
            }
            self.hits.tick[j] = tk.wrapping_add(delta) as u16;
        }
    }

    fn register_presses(&mut self) {
        let effect = self.p.mode >> 2;
        for r in 0..MATRIX_ROWS {
            for c in 0..MATRIX_COLS {
                let i = r * MATRIX_COLS + c;
                if !core::mem::take(&mut self.new_press[i]) {
                    continue;
                }
                let led = MATRIX[r][c];
                if led != NO_LED {
                    let h = &mut self.hits;
                    if h.count as usize + 1 > 8 {
                        h.x.copy_within(1.., 0);
                        h.y.copy_within(1.., 0);
                        h.led.copy_within(1.., 0);
                        h.tick.copy_within(1.., 0);
                        h.count = 7;
                    }
                    let n = h.count as usize;
                    let (x, y) = POINTS[led as usize];
                    h.x[n] = x;
                    h.y[n] = y;
                    h.led[n] = led;
                    h.tick[n] = 0;
                    h.count += 1;
                }
                if effect == 32 {
                    self.heatmap_hit(r, c);
                }
            }
        }
    }

    /// Typing heatmap increment (stock `FUN_000106CC`).
    fn heatmap_hit(&mut self, r: usize, c: usize) {
        let led = MATRIX[r][c];
        if led == NO_LED {
            return;
        }
        let (px, py) = POINTS[led as usize];
        for (rr, row) in MATRIX.iter().enumerate() {
            for (cc, &l) in row.iter().enumerate() {
                if l == NO_LED {
                    continue;
                }
                let i = rr * MATRIX_COLS + cc;
                if rr == r && cc == c {
                    self.cells[i] = self.cells[i].saturating_add(32);
                    continue;
                }
                let (x, y) = POINTS[l as usize];
                let sq = ((px as i32 - x as i32).pow(2) + (py as i32 - y as i32).pow(2)) as u16;
                let add = if sq < 2 {
                    16
                } else {
                    let d = math::sqrt16(sq);
                    if d > 40 {
                        continue;
                    }
                    (40 - d).min(16)
                };
                self.cells[i] = self.cells[i].saturating_add(add);
            }
        }
    }

    /// Effect 33: rebuild the per-cell buffer from key depths (stock `FUN_000103B6`).
    fn build_analog_heatmap(&mut self) {
        self.cells = [0; MATRIX_ROWS * MATRIX_COLS];
        for r in 0..MATRIX_ROWS {
            for c in 0..MATRIX_COLS {
                let d = self.depth[r * MATRIX_COLS + c];
                let led = MATRIX[r][c];
                if d == 0 || led == NO_LED {
                    continue;
                }
                let (px, py) = POINTS[led as usize];
                for (rr, row) in MATRIX.iter().enumerate() {
                    for (cc, &l) in row.iter().enumerate() {
                        if l == NO_LED {
                            continue;
                        }
                        let i = rr * MATRIX_COLS + cc;
                        if rr == r && cc == c {
                            self.cells[i] = self.cells[i].saturating_add(d);
                            continue;
                        }
                        let (x, y) = POINTS[l as usize];
                        let dist = math::dist(px as i32 - x as i32, py as i32 - y as i32);
                        if dist > 60 {
                            continue;
                        }
                        let add = ((d / 60) as u32 * (60 - dist) as u32) as u8;
                        self.cells[i] = self.cells[i].saturating_add(add);
                    }
                }
            }
        }
    }

    fn overlays(&mut self, t: u32, p: &Profile) {
        let fn_layer = p.fn_highlight() && self.fn_held;
        if fn_layer {
            let v = self.p.v;
            for led in 0..LED_COUNT {
                let key = LED_TO_KEY[led] as usize;
                self.leds[led] = if p.key_map_fn(key) == kc::NONE { [0; 3] } else { [v, v, v] };
            }
        } else if self.preview_mode == 1 {
            for led in 0..LED_COUNT {
                let a = self.preview[led] as u32;
                let s = self.statics[led];
                let e = self.leds[led];
                self.leds[led] = core::array::from_fn(|j| ((s[j] as u32 * a + e[j] as u32 * (255 - a)) / 255) as u8);
            }
        } else if self.preview_mode == 2 {
            for led in 0..LED_COUNT {
                let o = LED_COUNT + 3 * led;
                self.leds[led] = [self.preview[o], self.preview[o + 1], self.preview[o + 2]];
            }
        } else {
            for led in 0..LED_COUNT {
                if self.solid[led] {
                    self.leds[led] = self.statics[led];
                }
            }
        }
        if p.caps_indicator() && self.host_leds & host_led::CAPS_LOCK != 0 {
            for led in 0..LED_COUNT {
                if p.key_map(LED_TO_KEY[led] as usize) == kc::CAPS_LOCK {
                    self.leds[led] = [255, 0, 0];
                }
            }
        }
        if self.winlock && !self.fn_held && p.win_lock_indicator() {
            for led in 0..LED_COUNT {
                let c = p.key_map(LED_TO_KEY[led] as usize);
                if c == kc::LWIN || c == kc::RWIN {
                    self.leds[led] = [255, 0, 0];
                }
            }
        }
        let sleep = self.sleep_seconds as i32;
        if sleep > 0 && (sleep as u32) < self.idle_seconds() {
            self.set_all([0; 3]);
        }
        if let Some((prof, start)) = self.blink {
            let start = start.unwrap_or(t);
            self.blink = Some((prof, Some(start)));
            let el = t.wrapping_sub(start) as i32;
            if (el / BLINK_HALF_TICKS) & 1 == 0 {
                self.set_all(PROFILE_BLINK[prof as usize]);
            } else {
                self.set_all([0; 3]);
            }
            if el > BLINK_TICKS {
                self.blink = None;
            }
        }
        if let Some(on) = self.wipe_blink {
            self.set_all(if on { [60, 60, 60] } else { [0; 3] });
        }
    }
}
