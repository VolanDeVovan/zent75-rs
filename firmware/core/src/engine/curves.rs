//! Switch curves (stock `FUN_E04E` + `FUN_13510`, docs/re-engine.md §5): normalised position
//! `f` (0..2636 over a full swing) → key height in µm (`max_travel` at rest, 0 at bottom-out).
// Index loops mirror the stock per-slot / per-object tables on purpose.
#![allow(clippy::needless_range_loop)]

use super::tables::{TEMPLATE_0, TEMPLATE_1, TEMPLATE_2, TEMPLATE_3, TYPE_MAX_TRAVEL};
use crate::config::Global;

/// Entries per table (stock `0xA61`).
pub const TABLE_LEN: usize = 0xA61;
/// `f` at full travel.
pub const F_FULL: i32 = 2636;
/// Switch types: 0..3 built in, 4..7 = custom records 4..1 of bank 1.
pub const TYPES: usize = 8;
/// Tables the stock firmware keeps (types in use share them in type order, at most 4).
const BUFFERS: usize = 4;
/// Bank-1 offset of custom switch record 0 (type 4), 256 bytes each.
const CUSTOM_BASE: usize = 0x390;
const CUSTOM_STRIDE: usize = 0x100;
/// Most points a custom record may have (stock validation, bank-1 repair).
const CUSTOM_MAX_POINTS: usize = 88;

pub struct Curves {
    buf: [[i16; TABLE_LEN]; BUFFERS],
    /// Buffer used by each type (`0x91E94`), `NONE` until built.
    ptr: [u8; TYPES],
    /// First template value per type (`0x91E74`).
    pub t0: [i32; TYPES],
    /// Mean table index whose height is 8..12 µm (`0x91EB4`), auto-cal only.
    pub f10: [i32; TYPES],
    /// Full travel per type (custom: from the record).
    pub max_travel: [i32; TYPES],
}

const NONE: u8 = 0xFF;

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

impl Curves {
    pub const fn new() -> Self {
        Self {
            buf: [[0; TABLE_LEN]; BUFFERS],
            ptr: [NONE; TYPES],
            t0: [-12, -11, -7, -10, -12, -12, -12, -12],
            f10: [2626, 2625, 2622, 2621, 2626, 2626, 2626, 2626],
            max_travel: [3420, 3375, 3420, 3780, 3420, 3420, 3420, 3420],
        }
    }

    /// Build the tables of the types in `used` (stock order: types 0..3, then custom 4..7;
    /// each built type takes the next buffer, the fifth and later share the last one, like stock).
    pub fn build(&mut self, used: &[bool; TYPES], g: &Global) {
        let mut next = 0usize;
        let mut tpl_buf = [0i16; CUSTOM_MAX_POINTS];
        for t in 0..TYPES {
            let (tpl, max): (&[i16], i32) = match t {
                0 => (&TEMPLATE_0, TYPE_MAX_TRAVEL[0]),
                1 => (&TEMPLATE_1, TYPE_MAX_TRAVEL[1]),
                2 => (&TEMPLATE_2, TYPE_MAX_TRAVEL[2]),
                3 => (&TEMPLATE_3, TYPE_MAX_TRAVEL[3]),
                _ => match custom_record(g, t - 4, &mut tpl_buf) {
                    Some((n, max)) => (&tpl_buf[..n], max),
                    None => (&TEMPLATE_0, TYPE_MAX_TRAVEL[0]),
                },
            };
            self.t0[t] = tpl[0] as i32;
            if t >= 4 {
                self.max_travel[t] = max;
            }
            if !used[t] {
                continue;
            }
            let b = next;
            next = (next + 1).min(BUFFERS - 1);
            build_table(tpl, max, &mut self.buf[b]);
            self.ptr[t] = b as u8;
            let (mut sum, mut n) = (0i32, 0i32);
            for (i, &v) in self.buf[b].iter().enumerate() {
                if (8..=12).contains(&v) {
                    sum += i as i32;
                    n += 1;
                }
            }
            self.f10[t] = if n == 0 { 0xA4C } else { sum / n };
        }
    }

    /// Height in µm at position `f` (stock `FUN_14D86`).
    #[inline]
    pub fn height(&self, t: u8, f: i32) -> i32 {
        let t = t as usize & 7;
        let b = match self.ptr[t] {
            NONE => return 0,
            b => &self.buf[b as usize],
        };
        let t0 = self.t0[t];
        let i = if f < t0 {
            0
        } else if f > F_FULL - 1 {
            F_FULL - t0
        } else {
            f - t0
        };
        b.get(i as usize).copied().unwrap_or(0) as i32
    }
}

/// Custom record `i` of bank 1: (point count, full travel), template copied into `out`.
fn custom_record(g: &Global, i: usize, out: &mut [i16; CUSTOM_MAX_POINTS]) -> Option<(usize, i32)> {
    let o = CUSTOM_BASE + CUSTOM_STRIDE * i;
    let b = &g.0;
    let rd32 = |off: usize| b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let max = rd32(o)? as i32;
    let n = rd32(o + 4)? as usize;
    if !(2..=CUSTOM_MAX_POINTS).contains(&n) || b.len() < o + 8 + 2 * n {
        return None;
    }
    for (k, v) in out.iter_mut().take(n).enumerate() {
        *v = i16::from_le_bytes([b[o + 8 + 2 * k], b[o + 9 + 2 * k]]);
    }
    Some((n, max))
}

/// Stock table build: heights at the template points, linear interpolation in between
/// (truncating division). The stock loop also writes one point past the template; that write
/// lands outside the table for every built-in type and is not reproduced.
fn build_table(tpl: &[i16], max: i32, out: &mut [i16; TABLE_LEN]) {
    out.fill(0);
    let n = tpl.len();
    let t0 = tpl[0] as i32;
    let x = |i: usize| if i == n - 1 { max } else { -50 + 50 * i as i32 };
    let put = |out: &mut [i16; TABLE_LEN], f: i32, v: i32| {
        if let Some(e) = out.get_mut((f - t0) as usize) {
            *e = v as i16;
        }
    };
    let get = |out: &[i16; TABLE_LEN], f: i32| out.get((f - t0) as usize).copied().unwrap_or(0) as i32;
    for i in 0..n {
        put(out, tpl[i] as i32, (x(n - 1) - x(i)) as u16 as i16 as i32);
    }
    for i in 1..n {
        let (f1, f2) = (tpl[i - 1] as i32, tpl[i] as i32);
        if f1 + 1 < f2 {
            let (h1, h2) = (get(out, f1), get(out, f2));
            for j in f1 + 1..f2 {
                put(out, j, ((j - f1) * h2 + (f2 - j) * h1) / (f2 - f1));
            }
        }
    }
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
