//! Signed division by a run-time invariant divisor as a multiply-high (Hacker's Delight §10-1),
//! bit-exact with the RISC-V `div` instruction for every numerator: truncating, `x / 0 = −1`,
//! `MIN / −1 = MIN`. The divisor of the per-scan normalisation (the span) changes only on a
//! (re)configuration or an auto-cal step, while `div` costs tens of cycles on the D45.

/// A precomputed divisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recip {
    /// Magic multiplier.
    m: i32,
    /// Correction added after the high multiply: `+n` (d > 0, m < 0), `−n` (d < 0, m > 0), 0.
    add: i8,
    shift: u8,
    kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Kind {
    Magic,
    /// `d = 1`.
    One,
    /// `d = −1` (`MIN / −1 = MIN`).
    MinusOne,
    /// `d = 0` (always −1).
    Zero,
}

impl Recip {
    pub const fn new(d: i32) -> Self {
        let kind = match d {
            0 => Kind::Zero,
            1 => Kind::One,
            -1 => Kind::MinusOne,
            _ => Kind::Magic,
        };
        if !matches!(kind, Kind::Magic) {
            return Self { m: 0, add: 0, shift: 0, kind };
        }
        // Hacker's Delight, figure 10-1 (magic).
        const TWO31: u32 = 0x8000_0000;
        let ad = d.unsigned_abs();
        let t = TWO31 + ((d as u32) >> 31);
        let anc = t - 1 - t % ad;
        let mut p = 31u32;
        let mut q1 = TWO31 / anc;
        let mut r1 = TWO31 - q1 * anc;
        let mut q2 = TWO31 / ad;
        let mut r2 = TWO31 - q2 * ad;
        loop {
            p += 1;
            q1 = q1.wrapping_mul(2);
            r1 = r1.wrapping_mul(2);
            if r1 >= anc {
                q1 = q1.wrapping_add(1);
                r1 = r1.wrapping_sub(anc);
            }
            q2 = q2.wrapping_mul(2);
            r2 = r2.wrapping_mul(2);
            if r2 >= ad {
                q2 = q2.wrapping_add(1);
                r2 = r2.wrapping_sub(ad);
            }
            let delta = ad - r2;
            if !(q1 < delta || (q1 == delta && r1 == 0)) {
                break;
            }
        }
        let mut m = q2.wrapping_add(1) as i32;
        if d < 0 {
            m = m.wrapping_neg();
        }
        let add = if d > 0 && m < 0 {
            1
        } else if d < 0 && m > 0 {
            -1
        } else {
            0
        };
        Self { m, add, shift: (p - 32) as u8, kind }
    }

    /// `n / d` exactly as RISC-V `div` computes it.
    #[inline(always)]
    pub fn div(&self, n: i32) -> i32 {
        match self.kind {
            Kind::Magic => {
                let mut q = ((self.m as i64 * n as i64) >> 32) as i32;
                q = q.wrapping_add(n.wrapping_mul(self.add as i32));
                q >>= self.shift;
                q.wrapping_add(((q as u32) >> 31) as i32)
            }
            Kind::One => n,
            Kind::MinusOne => n.wrapping_neg(),
            Kind::Zero => -1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rv(a: i32, b: i32) -> i32 {
        if b == 0 { -1 } else { a.wrapping_div(b) }
    }

    #[test]
    fn size() {
        assert_eq!(core::mem::size_of::<Recip>(), 8);
    }

    #[test]
    fn matches_div() {
        let mut x = 0x1234_5678u32;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as i32
        };
        let mut ds: Vec<i32> = (-3000..=3000).collect();
        ds.extend([i32::MIN, i32::MIN + 1, i32::MAX, i32::MAX - 1, 1 << 30, -(1 << 30), 65536, 75000, -75000, 7, 641]);
        for _ in 0..3000 {
            ds.push(rnd());
            ds.push(rnd() >> (rnd() as u32 & 31));
        }
        let edges = [0, 1, -1, 2, -2, i32::MIN, i32::MIN + 1, i32::MAX, i32::MAX - 1, 65536, -65536];
        for &d in &ds {
            let r = Recip::new(d);
            for &n in &edges {
                assert_eq!(r.div(n), rv(n, d), "{n} / {d}");
                assert_eq!(r.div(n.wrapping_add(d)), rv(n.wrapping_add(d), d), "{n}+d / {d}");
                assert_eq!(r.div(d.wrapping_mul(3).wrapping_sub(1)), rv(d.wrapping_mul(3).wrapping_sub(1), d), "3d-1 / {d}");
            }
            for _ in 0..200 {
                let n = rnd();
                assert_eq!(r.div(n), rv(n, d), "{n} / {d}");
                let n = n >> 12;
                assert_eq!(r.div(n), rv(n, d), "{n} / {d}");
            }
        }
    }
}
