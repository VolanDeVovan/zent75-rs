//! Integer colour / trigonometry helpers with the stock firmware's exact rounding.
//!
//! The 8-bit trig follows FastLED's lib8tion (MIT): `sin8` by the 4-segment piecewise-linear
//! table, `sqrt16` by bisection. Division helpers reproduce RISC-V semantics (x/0 = all ones),
//! which the stock code relies on in a few corner cases (e.g. V = 0).

/// lib8tion `b_m16_interleave`: (offset, slope) pairs of the four sine segments.
const SIN_SEGMENTS: [u8; 8] = [0, 49, 49, 41, 90, 27, 117, 10];

/// Signed sine: `sin8(theta) − 128` in −127..=127 (128 steps per half wave).
pub fn sin8s(theta: u8) -> i8 {
    let mut offset = theta;
    let mut sec = theta & 0x0F;
    if theta & 0x40 != 0 {
        offset = !theta;
        sec = (!theta & 0x0F) + 1;
    }
    let s = ((offset >> 3) & 6) as usize;
    let y = (((SIN_SEGMENTS[s + 1] as u16 * sec as u16) >> 4) as u8).wrapping_add(SIN_SEGMENTS[s]);
    let y = y as i8;
    if theta & 0x80 != 0 { y.wrapping_neg() } else { y }
}

/// `sin8`: 0..255 sine, 128 at theta 0.
pub fn sin8(theta: u8) -> u8 {
    (sin8s(theta) as u8).wrapping_add(128)
}

/// `abs8` on the signed sine, 7 bits (−128 → 0, as stock masks with 0x7F).
pub fn sin_abs7(theta: u8) -> u32 {
    (sin8s(theta).unsigned_abs() & 0x7F) as u32
}

/// Breathing value used by several effects: `scale8(abs8(sin8(θ) − 128) · 2, v)`.
pub fn breath(theta: u8, v: u8) -> u8 {
    ((sin_abs7(theta) * 2 * v as u32) >> 8) as u8
}

/// lib8tion `sqrt16` (floor square root, 0..255).
pub fn sqrt16(x: u16) -> u8 {
    if x <= 1 {
        return x as u8;
    }
    let mut low: u32 = 1;
    let mut high: u32 = if x >= 7905 { 255 } else { (x as u32 >> 5) + 8 };
    loop {
        let mid = (low + high) >> 1;
        if mid * mid <= x as u32 {
            if mid == 255 {
                return 255;
            }
            low = (mid + 1) & 0xFF;
            if low > high {
                break;
            }
        } else {
            high = (mid.wrapping_sub(1)) & 0xFF;
            if high < low {
                break;
            }
        }
    }
    (low - 1) as u8
}

/// Distance between two points as the stock effects compute it (16-bit wrap of dx²+dy²).
pub fn dist(dx: i32, dy: i32) -> u8 {
    sqrt16((dx * dx + dy * dy) as u16)
}

/// 8-bit atan2 (256 units per turn) as stock: 0 = +x, 64 = +y.
pub fn atan2_8(dy: i32, dx: i32) -> u8 {
    if dy == 0 {
        return if dx < 0 { 128 } else { 0 };
    }
    let a = dy.unsigned_abs() as i32;
    let r = if dx < 0 { 96 - ((dx + a) * 32) / (a - dx) } else { 32 - ((dx - a) * 32) / (dx + a) };
    let r = r as u8;
    if dy < 0 { r.wrapping_neg() } else { r }
}

/// HSV → RGB, stock `FUN_00012964` (QMK `hsv_to_rgb` without CIE).
pub fn hsv(h: u8, s: u8, v: u8) -> [u8; 3] {
    if s == 0 {
        return [v, v, v];
    }
    let (h, s, v) = (h as u32, s as u32, v as u32);
    let region = h * 6 / 255;
    let rem = ((h * 2).wrapping_sub(region * 85) * 3) & 0xFF;
    let p = ((255 - s) * v) >> 8;
    let q = ((255 - ((s * rem) >> 8)) * v) >> 8;
    let t = ((255 - ((s * (255 - rem)) >> 8)) * v) >> 8;
    let (r, g, b) = match region {
        0 | 6 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r as u8, g as u8, b as u8]
}

/// Unsigned division as the RISC-V `divu` instruction (÷0 = all ones).
pub fn divu(a: u32, b: u32) -> u32 {
    a.checked_div(b).unwrap_or(u32::MAX)
}
/// Signed division as the RISC-V `div` instruction (truncating, ÷0 = −1, MIN/−1 = MIN).
pub fn divs(a: i32, b: i32) -> i32 {
    if b == 0 { -1 } else { a.wrapping_div(b) }
}

/// `1000 · sRGB→linear(i/255)` exactly as the firmware's float32 table (`0x8C548`).
const LIN: [u16; 256] = [
    0, 0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 6, 6, 6, 7, 8, 8, 9, 9, 10, 10, 11, 12, 12, 13, 14,
    15, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 33, 34, 35, 36, 38, 39, 40, 42,
    43, 45, 46, 48, 49, 51, 52, 54, 56, 57, 59, 61, 63, 64, 66, 68, 70, 72, 74, 76, 78, 80, 82, 84, 86, 88,
    90, 93, 95, 97, 99, 102, 104, 107, 109, 111, 114, 116, 119, 122, 124, 127, 130, 132, 135, 138, 141, 144,
    147, 149, 152, 155, 158, 162, 165, 168, 171, 174, 177, 181, 184, 187, 191, 194, 198, 201, 205, 208, 212,
    215, 219, 223, 226, 230, 234, 238, 242, 246, 250, 254, 258, 262, 266, 270, 274, 278, 283, 287, 291, 296,
    300, 304, 309, 313, 318, 323, 327, 332, 337, 341, 346, 351, 356, 361, 366, 371, 376, 381, 386, 391, 396,
    401, 407, 412, 417, 423, 428, 434, 439, 445, 450, 456, 462, 467, 473, 479, 485, 491, 496, 502, 508, 514,
    520, 527, 533, 539, 545, 552, 558, 564, 571, 577, 584, 590, 597, 603, 610, 617, 623, 630, 637, 644, 651,
    658, 665, 672, 679, 686, 693, 701, 708, 715, 723, 730, 737, 745, 752, 760, 768, 775, 783, 791, 799, 806,
    814, 822, 830, 838, 846, 854, 863, 871, 879, 887, 896, 904, 913, 921, 930, 938, 947, 955, 964, 973, 982,
    991, 1000,
];

/// Static-colour correction, stock `FUN_0000D9AA`: sRGB → CIE xy → barycentric weights on the
/// LED primaries, renormalised to keep r+g+b. All 32-bit wrapping integer maths with RISC-V
/// division, bit-exact with the firmware (docs/re-config-rgb.md §5).
pub fn color_correct(c: [u8; 3]) -> [u8; 3] {
    let sum = c[0] as i32 + c[1] as i32 + c[2] as i32;
    if sum == 0 {
        return [0; 3];
    }
    let (r, g, b) = (LIN[c[0] as usize] as i32, LIN[c[1] as usize] as i32, LIN[c[2] as usize] as i32);
    let y_ = 212 * r + 715 * g + 72 * b;
    let x_ = 412 * r + 357 * g + 180 * b;
    let s = x_ + y_ + 19 * r + 119 * g + 950 * b;
    let x = divs(x_.wrapping_mul(1000), s);
    let y = divs(y_.wrapping_mul(1000), s);
    let m = |a: i32, b: i32| a.wrapping_mul(b);
    let a = divs(m(divs(m((150 - x) * 540 + (y - 60) * 150, 255), -224100), 330000), m(y, 80));
    let bb = divs(m(divs(m((y - 330) * -490 + (640 - x) * -270, 255), -224100), 600000), m(y, 660));
    let cc = divs(m(divs(m((y - 330) * -340 + (640 - x) * 270, 255), 224100), 60000), m(y, 50));
    let mx = a.max(bb).max(cc);
    let w = |k: i32| divs(m(divs(m(y_, k), 1000), 255), mx);
    let (a2, b2, c2) = (w(a), w(bb), w(cc));
    let t = a2.wrapping_add(b2).wrapping_add(c2);
    let out = |v: i32| divs(m(sum, v), t).min(255) as u8;
    [out(a2), out(b2), out(c2)]
}

/// QMK `random8` (`0x91C06`): 16-bit LCG, returns low + high byte.
#[derive(Clone, Copy)]
pub struct Random8(pub u16);
impl Random8 {
    pub fn random8(&mut self) -> u8 {
        self.0 = self.0.wrapping_mul(2053).wrapping_add(13849);
        (self.0 as u8).wrapping_add((self.0 >> 8) as u8)
    }
}

/// newlib's small `rand()` (`FUN_80045FA6`): 32-bit LCG, 15-bit result.
#[derive(Clone, Copy)]
pub struct Rand(pub u32);
impl Rand {
    pub fn rand(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (self.0 >> 16) & 0x7FFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine() {
        assert_eq!(sin8(0), 128);
        assert_eq!(sin8(64), 255);
        assert_eq!(sin8(128), 128);
        assert_eq!(sin8(192), 1);
    }

    #[test]
    fn roots() {
        for x in 0..=u16::MAX {
            let r = sqrt16(x) as u32;
            assert!(r * r <= x as u32 && (r == 255 || (r + 1) * (r + 1) > x as u32), "{x}");
        }
    }

    #[test]
    fn cc_examples() {
        // docs/re-config-rgb.md: white → (255,161,215)
        assert_eq!(color_correct([255, 255, 255]), [255, 161, 215]);
        assert_eq!(color_correct([0, 0, 0]), [0, 0, 0]);
    }

    #[test]
    fn hsv_basics() {
        assert_eq!(hsv(0, 0, 77), [77, 77, 77]);
        assert_eq!(hsv(0, 255, 255), [255, 0, 0]);
    }
}
