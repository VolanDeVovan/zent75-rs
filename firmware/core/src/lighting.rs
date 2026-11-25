//! RGB lighting. PLACEHOLDER (static colours) until docs/re-config-rgb.md lands.

use crate::config::{b2, Profile};
use crate::engine::Engine;
use crate::keys::{KeyProcessor, LightCmd};
use crate::layout::LED_COUNT;

pub struct Lighting {
    host_leds: u8,
}

impl Lighting {
    pub const fn new() -> Self {
        Self { host_leds: 0 }
    }
    pub fn configure(&mut self, _p: &Profile) {}
    pub fn on_scan(&mut self, _k: &KeyProcessor) {}
    pub fn set_host_leds(&mut self, l: u8) {
        self.host_leds = l;
    }
    pub fn command(&mut self, _c: LightCmd, _p: &mut Profile) {}
    pub fn brightness_trigger(&mut self, _addr: u32, _data: &[u8], _p: &mut Profile) {}

    pub fn render(&mut self, _now_ms: u32, p: &Profile, _k: &KeyProcessor, _e: &Engine, out: &mut [[u8; 3]; LED_COUNT]) {
        let on = p.flag(b2::RGB_ENABLE);
        let br = p.rgb_config()[3].min(30) as u16;
        for (i, o) in out.iter_mut().enumerate() {
            let c = p.led_rgb(i);
            *o = if on { c.map(|v| (v as u16 * br / 30) as u8) } else { [0; 3] };
        }
    }
}
