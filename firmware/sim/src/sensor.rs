//! Hall-sensor model: physical key travel (mm) → averaged ADC counts per scan slot.
//!
//! Each key has its own rest level (≈ 30000 ± 300) and bottom-out level (≈ 18000 ± 200),
//! derived deterministically from the key index and a seed. Counts fall linearly with travel
//! from rest (0 mm) to bottom ([`FULL_TRAVEL_MM`]), plus uniform noise of ±`noise` counts.
//! Travel moves toward a scripted target at a finite slew rate so presses take a few ms,
//! like a finger (0 = instant).

use zent_core::layout::{KEY_COUNT, KEY_TO_SLOT, SCAN_HZ, SLOT_COUNT};

/// Total travel of the switch (rest → bottom-out).
pub const FULL_TRAVEL_MM: f32 = 3.5;
pub const REST_NOMINAL: f32 = 30000.0;
pub const BOTTOM_NOMINAL: f32 = 18000.0;
pub const REST_SPREAD: i32 = 300;
pub const BOTTOM_SPREAD: i32 = 200;
/// Level reported for the five spare scan slots (no magnet).
pub const SPARE_SLOT: u16 = 30000;

pub struct SensorModel {
    rest: [f32; KEY_COUNT],
    bottom: [f32; KEY_COUNT],
    noise: u16,
    rng: u64,
    travel: [f32; KEY_COUNT],
    target: [f32; KEY_COUNT],
    /// mm per scan; `f32::INFINITY` = instant.
    slew: f32,
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn spread(h: u64, half: i32) -> f32 {
    ((h % (2 * half as u64 + 1)) as i32 - half) as f32
}

impl SensorModel {
    /// `noise`: ± counts of uniform noise per sample. `slew_mm_s`: travel speed, 0 = instant.
    pub fn new(seed: u64, noise: u16, slew_mm_s: f32) -> Self {
        let mut rest = [0.0; KEY_COUNT];
        let mut bottom = [0.0; KEY_COUNT];
        for k in 0..KEY_COUNT {
            let h = splitmix(seed ^ (k as u64).wrapping_mul(0x1000_0001));
            rest[k] = REST_NOMINAL + spread(h, REST_SPREAD);
            bottom[k] = BOTTOM_NOMINAL + spread(h >> 32, BOTTOM_SPREAD);
        }
        let slew = if slew_mm_s > 0.0 {
            slew_mm_s / SCAN_HZ as f32
        } else {
            f32::INFINITY
        };
        Self {
            rest,
            bottom,
            noise,
            rng: splitmix(seed ^ 0xA5A5),
            travel: [0.0; KEY_COUNT],
            target: [0.0; KEY_COUNT],
            slew,
        }
    }

    pub fn rest(&self, key: usize) -> f32 {
        self.rest[key]
    }
    pub fn bottom(&self, key: usize) -> f32 {
        self.bottom[key]
    }

    /// Noise-free ADC level at `mm` of travel.
    pub fn level(&self, key: usize, mm: f32) -> u16 {
        let t = (mm / FULL_TRAVEL_MM).clamp(0.0, 1.0);
        (self.rest[key] - (self.rest[key] - self.bottom[key]) * t).round() as u16
    }

    /// Current physical travel.
    pub fn travel(&self, key: usize) -> f32 {
        self.travel[key]
    }
    pub fn target(&self, key: usize) -> f32 {
        self.target[key]
    }

    /// Move `key` toward `mm` at the slew rate.
    pub fn set_target(&mut self, key: usize, mm: f32) {
        self.target[key] = mm.clamp(0.0, FULL_TRAVEL_MM);
    }

    /// Put `key` at `mm` immediately.
    pub fn set_now(&mut self, key: usize, mm: f32) {
        let mm = mm.clamp(0.0, FULL_TRAVEL_MM);
        self.target[key] = mm;
        self.travel[key] = mm;
    }

    pub fn release_all(&mut self) {
        self.target = [0.0; KEY_COUNT];
    }

    /// Advance the travel of every key by one scan period.
    pub fn step(&mut self) {
        for k in 0..KEY_COUNT {
            let d = self.target[k] - self.travel[k];
            if d != 0.0 {
                self.travel[k] = if d.abs() <= self.slew {
                    self.target[k]
                } else {
                    self.travel[k] + self.slew.copysign(d)
                };
            }
        }
    }

    fn noise_sample(&mut self) -> f32 {
        if self.noise == 0 {
            return 0.0;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        spread(self.rng, self.noise as i32)
    }

    /// One full scan: 88 averaged samples indexed by scan slot.
    pub fn samples(&mut self) -> [u16; SLOT_COUNT] {
        let mut s = [SPARE_SLOT; SLOT_COUNT];
        for k in 0..KEY_COUNT {
            let v = self.level(k, self.travel[k]) as f32 + self.noise_sample();
            s[KEY_TO_SLOT[k] as usize] = v.clamp(0.0, 65535.0) as u16;
        }
        s
    }
}
