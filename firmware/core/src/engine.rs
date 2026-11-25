//! Hall-sensor engine: samples → filtered raw → baseline → travel (µm).
//! PLACEHOLDER (linear travel) until docs/re-engine.md lands.

use crate::config::{Global, Profile};
use crate::layout::{KEY_COUNT, KEY_TO_SLOT, SLOT_COUNT};
use crate::protocol::{Reply, REPORT_SIZE};

const WARMUP_SCANS: u32 = 12000;
const MAX_TRAVEL_UM: i32 = 3400;

pub struct Engine {
    pub raw: [u16; KEY_COUNT],
    pub travel: [u16; KEY_COUNT],
    pub baseline: [u16; KEY_COUNT],
    pub tracked: [u16; KEY_COUNT],
    span: [i32; KEY_COUNT],
    scans: u32,
    ready: bool,
    calibration: bool,
}

impl Engine {
    pub const fn new() -> Self {
        Self {
            raw: [0; KEY_COUNT],
            travel: [0; KEY_COUNT],
            baseline: [0; KEY_COUNT],
            tracked: [0; KEY_COUNT],
            span: [12000; KEY_COUNT],
            scans: 0,
            ready: false,
            calibration: false,
        }
    }

    pub fn configure(&mut self, g: &Global, _p: &Profile) {
        for k in 0..KEY_COUNT {
            let s = g.top_value(k) as i32 - g.bot_value(k) as i32;
            self.span[k] = if s > 3000 { s } else { 12000 };
        }
    }

    pub fn set_calibration(&mut self, on: bool) {
        self.calibration = on;
    }

    pub fn ready(&self) -> bool {
        self.ready
    }

    pub fn scan(&mut self, samples: &[u16; SLOT_COUNT], _g: &Global, _p: &Profile, _cal: bool) {
        self.scans = self.scans.saturating_add(1);
        for k in 0..KEY_COUNT {
            let s = samples[KEY_TO_SLOT[k] as usize];
            let r = if self.raw[k] == 0 { s } else { ((self.raw[k] as u32 * 3 + s as u32) / 4) as u16 };
            self.raw[k] = r;
        }
        if !self.ready {
            if self.scans >= WARMUP_SCANS {
                self.baseline = self.raw;
                self.tracked = self.raw;
                self.ready = true;
            }
            return;
        }
        for k in 0..KEY_COUNT {
            let d = self.baseline[k] as i32 - self.raw[k] as i32;
            let t = (d * MAX_TRAVEL_UM / self.span[k]).clamp(0, MAX_TRAVEL_UM);
            self.travel[k] = t as u16;
        }
    }

    pub fn stream_frame(&self, out: &mut [u8; REPORT_SIZE], diag: bool) -> usize {
        let raw = self.raw;
        let travel = self.travel;
        let base = self.baseline;
        let tr = self.tracked;
        let words = (0..505).map(move |i| match i {
            0..=82 => raw[i],
            83..=165 => travel[i - 83],
            166..=248 => base[i - 166],
            249..=331 => tr[i - 249],
            498 => diag as u16,
            499 => 8000,
            _ => 0,
        });
        Reply::new(out).stream(words, 505)
    }
}
