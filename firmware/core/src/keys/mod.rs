//! Key pipeline: travel → per-key state → layers / SOCD / DKS / functions → HID state.
//! PLACEHOLDER (normal mode, main + Fn layer) until docs/re-engine.md lands.

use crate::config::{Global, Profile, KC_NONE};
use crate::engine::Engine;
use crate::hid::HidState;
use crate::layout::KEY_COUNT;

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
    Bootloader,
    Reboot,
    FactoryReset,
    SwitchProfile(u8),
    ProfileChanged,
    ConfigChanged,
    Lighting(LightCmd),
}

/// Small fixed-capacity event list.
#[derive(Default)]
pub struct Events {
    buf: [Option<Event>; 8],
    n: usize,
}
impl Events {
    pub fn push(&mut self, e: Event) {
        if self.n < self.buf.len() {
            self.buf[self.n] = Some(e);
            self.n += 1;
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = &Event> {
        self.buf[..self.n].iter().flatten()
    }
}

pub struct KeyProcessor {
    pressed: [bool; KEY_COUNT],
    hid: HidState,
}

impl KeyProcessor {
    pub const fn new() -> Self {
        Self { pressed: [false; KEY_COUNT], hid: HidState { keyboard: crate::hid::KeyboardState { modifiers: 0, keys: [0; 32] }, consumer: 0, system: 0, mouse_buttons: 0 } }
    }
    pub fn configure(&mut self, _p: &Profile) {}
    pub fn reset(&mut self) {
        self.pressed = [false; KEY_COUNT];
        self.hid = HidState::default();
    }
    pub fn hid(&self) -> &HidState {
        &self.hid
    }
    pub fn is_pressed(&self, key: usize) -> bool {
        self.pressed[key]
    }

    pub fn update(&mut self, e: &Engine, p: &Profile, _g: &Global) -> Events {
        let ev = Events::default();
        for k in 0..KEY_COUNT {
            let t = e.travel[k];
            if !self.pressed[k] && e.ready() && t >= p.norm_press_th(k) {
                self.pressed[k] = true;
            } else if self.pressed[k] && t <= p.norm_return_th(k) {
                self.pressed[k] = false;
            }
        }
        let fn_held = (0..KEY_COUNT).any(|k| self.pressed[k] && p.key_map(k) == 401);
        let mut h = HidState::default();
        for k in 0..KEY_COUNT {
            if !self.pressed[k] {
                continue;
            }
            let mut code = if fn_held { p.key_map_fn(k) } else { p.key_map(k) };
            if code == KC_NONE && fn_held {
                code = p.key_map(k);
            }
            match code {
                -8..=-1 => h.keyboard.modifiers |= 1 << (code + 8),
                4..=231 => h.keyboard.press(code as u8),
                _ => {}
            }
        }
        self.hid = h;
        ev
    }
}
