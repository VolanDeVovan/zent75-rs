//! Switch curves (stock `FUN_E04E` + `FUN_13510`, docs/re-engine.md §5): normalised position
//! `f` (0..2636 over a full swing) → key height in µm (`max_travel` at rest, 0 at bottom-out).
// Index loops mirror the stock per-slot / per-object tables on purpose.
#![allow(clippy::needless_range_loop)]

use super::tables::{TEMPLATE_0, TEMPLATE_1, TEMPLATE_2, TEMPLATE_3, TYPE_MAX_TRAVEL};
use crate::config::Global;

/// Entries of the stock table (`0xA61`); heights are computed on the fly from the template with
/// the stock interpolation, which gives exactly the table entries without the 5.3 KB per type.
pub const TABLE_LEN: usize = 0xA61;
/// `f` at full travel.
pub const F_FULL: i32 = 2636;
/// Switch types: 0..3 built in, 4..7 = custom records 0..3 of bank 1 (codes 4095..4092).
pub const TYPES: usize = 8;
/// Bank-1 offset of custom switch record 0 (type 4), 256 bytes each.
const CUSTOM_BASE: usize = 0x390;
const CUSTOM_STRIDE: usize = 0x100;
/// Most points a curve may have (stock validation of the custom records).
const MAX_POINTS: usize = 88;

/// `f` positions per lookup bucket (speeds the segment search up to a few compares).
const BUCKET_SHIFT: u32 = 5;
const BUCKETS: usize = (TABLE_LEN >> BUCKET_SHIFT) + 1;

pub struct Curves {
    tpl: [[i16; MAX_POINTS]; TYPES],
    /// Height (µm) at each template point.
    hts: [[i16; MAX_POINTS]; TYPES],
    /// Last template point `≤ t0 + 32·j` per bucket j.
    bucket: [[u8; BUCKETS]; TYPES],
    n: [u8; TYPES],
    /// First template value per type (`0x91E74`).
    pub t0: [i32; TYPES],
    /// Mean table index whose height is 8..12 µm (`0x91EB4`), auto-cal only.
    pub f10: [i32; TYPES],
    /// Full travel per type (custom: from the record).
    pub max_travel: [i32; TYPES],
}

/// Stock `FUN_16146`: `keytype_code` → switch type 0..7.
pub fn type_of_code(code: u16) -> u8 {
    let c = code as i32;
    if c <= 3 {
        c.max(0) as u8
    } else if c > 0xFFB && c <= 0x1003 {
        (0x1003 - c) as u8
    } else {
        0
    }
}

impl Default for Curves {
    fn default() -> Self {
        Self::new()
    }
}

const fn padded<const N: usize>(t: &[i16; N]) -> [i16; MAX_POINTS] {
    let mut o = [0i16; MAX_POINTS];
    let mut i = 0;
    while i < N {
        o[i] = t[i];
        i += 1;
    }
    o
}

impl Curves {
    pub const fn new() -> Self {
        let t0 = padded(&TEMPLATE_0);
        Self {
            tpl: [t0, padded(&TEMPLATE_1), padded(&TEMPLATE_2), padded(&TEMPLATE_3), t0, t0, t0, t0],
            hts: [[0; MAX_POINTS]; TYPES],
            bucket: [[0; BUCKETS]; TYPES],
            n: [71, 70, 71, 78, 71, 71, 71, 71],
            t0: [-12, -11, -7, -10, -12, -12, -12, -12],
            f10: [2626, 2625, 2622, 2621, 2626, 2626, 2626, 2626],
            max_travel: [3420, 3375, 3420, 3780, 3420, 3420, 3420, 3420],
        }
    }

    /// Load the curves (stock `FUN_E04E`): built-in types plus the custom records of bank 1;
    /// `f10` is derived for the types in `used`. Stock keeps at most four tables and lets a fifth
    /// type in use overwrite the fourth; every type gets its own curve here.
    pub fn build(&mut self, used: &[bool; TYPES], g: &Global) {
        for t in 0..TYPES {
            let mut max = TYPE_MAX_TRAVEL[t.min(3)];
            match t {
                0 => self.tpl[t] = padded(&TEMPLATE_0),
                1 => self.tpl[t] = padded(&TEMPLATE_1),
                2 => self.tpl[t] = padded(&TEMPLATE_2),
                3 => self.tpl[t] = padded(&TEMPLATE_3),
                _ => {
                    let mut buf = [0i16; MAX_POINTS];
                    if let Some((n, m)) = custom_record(g, t - 4, &mut buf) {
                        self.tpl[t] = buf;
                        self.n[t] = n as u8;
                        max = m;
                    } else {
                        self.tpl[t] = padded(&TEMPLATE_0);
                        self.n[t] = TEMPLATE_0.len() as u8;
                        max = TYPE_MAX_TRAVEL[0];
                    }
                }
            }
            self.max_travel[t] = max;
            self.t0[t] = self.tpl[t][0] as i32;
            self.index(t);
            if !used[t] {
                continue;
            }
            let (mut sum, mut cnt) = (0i32, 0i32);
            for i in 0..TABLE_LEN as i32 {
                if (8..=12).contains(&self.at(t, self.t0[t] + i)) {
                    sum += i;
                    cnt += 1;
                }
            }
            self.f10[t] = if cnt == 0 { 0xA4C } else { sum / cnt };
        }
    }

    /// Point heights and the bucket index of type `t`.
    fn index(&mut self, t: usize) {
        let n = self.n[t] as usize;
        let max = self.max_travel[t];
        let x = |i: usize| if i == n - 1 { max } else { -50 + 50 * i as i32 };
        for i in 0..n {
            self.hts[t][i] = (x(n - 1) - x(i)) as u16 as i16;
        }
        let t0 = self.t0[t];
        let tpl = &self.tpl[t][..n];
        for j in 0..BUCKETS {
            let f = t0 + ((j as i32) << BUCKET_SHIFT);
            self.bucket[t][j] = tpl.partition_point(|&v| v as i32 <= f).saturating_sub(1) as u8;
        }
    }

    /// Height in µm at position `f` (stock `FUN_14D86` on the stock table).
    #[inline]
    pub fn height(&self, t: u8, f: i32) -> i32 {
        let t = t as usize & 7;
        let t0 = self.t0[t];
        let f = if f < t0 {
            t0
        } else if f > F_FULL - 1 {
            F_FULL
        } else {
            f
        };
        self.at(t, f)
    }

    /// Stock table entry at position `f` (`t0 ≤ f < t0 + TABLE_LEN`).
    #[inline]
    fn at(&self, t: usize, f: i32) -> i32 {
        let n = self.n[t] as usize;
        let tpl = &self.tpl[t];
        let hts = &self.hts[t];
        let k = ((f - self.t0[t]) >> BUCKET_SHIFT) as usize;
        let mut i = match self.bucket[t].get(k) {
            Some(&b) => b as usize,
            None => return 0,
        };
        if (tpl[i] as i32) > f {
            return 0; // before the first point (unreachable: f ≥ t0)
        }
        // last point with tpl[i] <= f (a later duplicate overwrites an earlier one in stock)
        while i + 1 < n && tpl[i + 1] as i32 <= f {
            i += 1;
        }
        let f1 = tpl[i] as i32;
        if f1 == f {
            return hts[i] as i32;
        }
        if i + 1 >= n {
            return 0; // past the last point: never written (zero-filled table)
        }
        let f2 = tpl[i + 1] as i32;
        let mut j = i + 1;
        while j + 1 < n && tpl[j + 1] as i32 == f2 {
            j += 1;
        }
        (((f - f1) * hts[j] as i32 + (f2 - f) * hts[i] as i32) / (f2 - f1)) as i16 as i32
    }
}

/// Custom record `i` of bank 1: (point count, full travel), template copied into `out`.
fn custom_record(g: &Global, i: usize, out: &mut [i16; MAX_POINTS]) -> Option<(usize, i32)> {
    let o = CUSTOM_BASE + CUSTOM_STRIDE * i;
    let b = &g.0;
    let rd32 = |off: usize| b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let max = rd32(o)? as i32;
    let n = rd32(o + 4)? as usize;
    if !(2..=MAX_POINTS).contains(&n) || b.len() < o + 8 + 2 * n {
        return None;
    }
    for (k, v) in out.iter_mut().take(n).enumerate() {
        *v = i16::from_le_bytes([b[o + 8 + 2 * k], b[o + 9 + 2 * k]]);
    }
    if out[..n].windows(2).any(|w| w[0] > w[1]) {
        return None; // stock's boot repair replaces a non-monotonic record
    }
    Some((n, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type2_samples() {
        // docs/re-engine.md §5 sample row
        let mut c = Curves::new();
        let mut used = [false; TYPES];
        used[2] = true;
        c.build(&used, &Global::zeroed());
        let pts = [(0, 0), (3, 22), (100, 433), (500, 1476), (1000, 2232), (1500, 2713), (2000, 3073), (2268, 3234), (2500, 3354), (2620, 3412), (2636, 3420)];
        for (f, travel) in pts {
            assert_eq!(3420 - c.height(2, f), travel, "f={f}");
        }
        assert_eq!(c.f10[2], 2622);
        assert_eq!(c.height(2, -100), 3470);
        // every type matches a brute-force build of the stock table
        for t in 0..4 {
            let tpl: &[i16] = [&TEMPLATE_0[..], &TEMPLATE_1[..], &TEMPLATE_2[..], &TEMPLATE_3[..]][t];
            let max = TYPE_MAX_TRAVEL[t];
            let n = tpl.len();
            let t0 = tpl[0] as i32;
            let mut out = vec![0i32; TABLE_LEN];
            let x = |i: usize| if i == n - 1 { max } else { -50 + 50 * i as i32 };
            for i in 0..n {
                out[(tpl[i] as i32 - t0) as usize] = x(n - 1) - x(i);
            }
            for i in 1..n {
                let (f1, f2) = (tpl[i - 1] as i32, tpl[i] as i32);
                for j in f1 + 1..f2 {
                    out[(j - t0) as usize] = ((j - f1) * out[(f2 - t0) as usize] + (f2 - j) * out[(f1 - t0) as usize]) / (f2 - f1);
                }
            }
            let mut c = Curves::new();
            c.build(&[true; TYPES], &Global::zeroed());
            for (i, &v) in out.iter().enumerate() {
                assert_eq!(c.at(t, t0 + i as i32), v, "type {t} f {}", t0 + i as i32);
            }
        }
        assert_eq!(c.height(2, 5000), 0);
    }

    #[test]
    fn type_codes() {
        assert_eq!(type_of_code(2), 2);
        assert_eq!(type_of_code(4095), 4);
        assert_eq!(type_of_code(4092), 7);
        assert_eq!(type_of_code(17), 0);
    }
}
