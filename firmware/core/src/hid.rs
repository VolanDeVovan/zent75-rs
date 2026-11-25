//! The HID side of the keyboard as the host sees it: the stock report formats, their report
//! descriptors and the current report contents ([`HidReports`]).
//!
//! | Interface | Report |
//! |---|---|
//! | keyboard | 16 bytes: modifier byte (usages `E0..E7`), then a bitmap of usages `00..77` |
//! | consumer | 3 bytes: bitmap, bit n = [`CONSUMER_USAGES`]`[n]` (keycode 201 + n) |
//! | mouse | 3 bytes: buttons 1..5, X, Y (the firmware only ever sends buttons) |
//!
//! The key pipeline writes these bytes exactly as the stock firmware does; the transports (USB
//! on the board, uhid in the simulator) only move them.

/// Length of the NKRO keyboard report.
pub const KEYBOARD_REPORT_LEN: usize = 16;
/// Highest keyboard usage that has a bit in the NKRO report.
pub const KEYBOARD_USAGE_MAX: u8 = 0x77;

/// Current contents of the three input reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HidReports {
    pub keyboard: [u8; KEYBOARD_REPORT_LEN],
    pub consumer: [u8; 3],
    /// Mouse buttons, bit 0 = left … bit 4 = forward.
    pub mouse_buttons: u8,
}

impl Default for HidReports {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl HidReports {
    /// Nothing pressed.
    pub const EMPTY: Self = Self { keyboard: [0; KEYBOARD_REPORT_LEN], consumer: [0; 3], mouse_buttons: 0 };

    /// Modifier byte (bit n = usage `E0 + n`).
    pub fn modifiers(&self) -> u8 {
        self.keyboard[0]
    }

    /// Keyboard usage `usage` (`00..=77`) is down.
    pub fn is_pressed(&self, usage: u8) -> bool {
        usage <= KEYBOARD_USAGE_MAX && self.keyboard[1 + (usage as usize >> 3)] & (1 << (usage & 7)) != 0
    }

    /// Pressed keyboard usages in ascending order (modifiers not included).
    pub fn pressed(&self) -> impl Iterator<Item = u8> + '_ {
        (0..=KEYBOARD_USAGE_MAX).filter(|&u| self.is_pressed(u))
    }

    /// 8-byte boot-protocol report `[mods, 0, k1..k6]`; more than six keys → ErrorRollOver.
    pub fn boot_report(&self) -> [u8; 8] {
        let mut r = [0u8; 8];
        r[0] = self.modifiers();
        for (n, u) in self.pressed().enumerate() {
            if n == 6 {
                r[2..8].fill(0x01);
                break;
            }
            r[2 + n] = u;
        }
        r
    }

    /// The 3-byte mouse report (no movement).
    pub fn mouse_report(&self) -> [u8; 3] {
        [self.mouse_buttons & 0x1F, 0, 0]
    }

    /// Consumer usage of the lowest set bit of the consumer report (0 = none), for hosts that
    /// take one usage at a time.
    pub fn consumer_usage(&self) -> u16 {
        let bits = u32::from_le_bytes([self.consumer[0], self.consumer[1], self.consumer[2], 0]);
        match bits.trailing_zeros() as usize {
            n if n < CONSUMER_USAGES.len() => CONSUMER_USAGES[n],
            _ => 0,
        }
    }
}

/// Consumer-page usage of each bit of the consumer report, LSB of byte 0 first.
pub const CONSUMER_USAGES: [u16; 24] = [
    0x0E9, 0x0EA, 0x0E2, 0x0CD, 0x0B7, 0x0B6, 0x0B5, 0x192, 0x18A, 0x194, 0x221, 0x223, 0x224, 0x226, 0x225,
    0x227, 0x22A, 0x19F, 0x183, 0x06F, 0x070, 0x2A0, 0x29F, 0x1CB,
];

/// Host LED output report bits.
pub mod host_led {
    pub const NUM_LOCK: u8 = 1 << 0;
    pub const CAPS_LOCK: u8 = 1 << 1;
    pub const SCROLL_LOCK: u8 = 1 << 2;
}

/// The stock report descriptors (byte-identical to the vendor device).
pub mod descriptor {
    use super::CONSUMER_USAGES;

    /// Keyboard (61 B): modifiers + bitmap of usages `00..77`, 5-bit LED output report.
    pub const KEYBOARD: [u8; 61] = [
        0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x75, 0x01, 0x95, 0x08, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15,
        0x00, 0x25, 0x01, 0x81, 0x02, 0x95, 0x78, 0x75, 0x01, 0x15, 0x00, 0x25, 0x01, 0x05, 0x07, 0x19, 0x00,
        0x29, 0x77, 0x81, 0x02, 0x15, 0x00, 0x25, 0x01, 0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01, 0x29,
        0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x03, 0xC0,
    ];

    /// Consumer (80 B), built from [`CONSUMER_USAGES`].
    pub const CONSUMER: [u8; 80] = {
        let mut d = [0u8; 80];
        let head = [0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01];
        let tail = [0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x18, 0x81, 0x02, 0xC0];
        let mut n = 0;
        let mut i = 0;
        while i < head.len() {
            d[n] = head[i];
            n += 1;
            i += 1;
        }
        let mut u = 0;
        while u < CONSUMER_USAGES.len() {
            let v = CONSUMER_USAGES[u];
            if v <= 0xFF {
                d[n] = 0x09;
                d[n + 1] = v as u8;
                n += 2;
            } else {
                d[n] = 0x0A;
                d[n + 1] = v as u8;
                d[n + 2] = (v >> 8) as u8;
                n += 3;
            }
            u += 1;
        }
        i = 0;
        while i < tail.len() {
            d[n] = tail[i];
            n += 1;
            i += 1;
        }
        assert!(n == 80);
        d
    };

    /// Vendor (36 B): 1024-byte input + output, usage page `FF60` / usage `61`.
    pub const VENDOR: [u8; 36] = [
        0x06, 0x60, 0xFF, 0x09, 0x61, 0xA1, 0x01, 0x05, 0x62, 0x19, 0x00, 0x2A, 0xFF, 0x00, 0x96, 0x00, 0x04,
        0x75, 0x08, 0x81, 0x02, 0x05, 0x63, 0x19, 0x00, 0x2A, 0xFF, 0x00, 0x96, 0x00, 0x04, 0x75, 0x08, 0x91,
        0x02, 0xC0,
    ];

    /// Mouse (50 B): 5 buttons, X, Y (relative i8), no wheel.
    pub const MOUSE: [u8; 50] = [
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x05, 0x15,
        0x00, 0x25, 0x01, 0x95, 0x05, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x03, 0x81, 0x03, 0x05, 0x01,
        0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x06, 0xC0, 0xC0,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_bitmap() {
        let mut h = HidReports::EMPTY;
        h.keyboard[0] = 0x22;
        h.keyboard[1] = 0x10; // usage 4 (A)
        h.keyboard[15] = 0x80; // usage 0x77
        assert_eq!(h.modifiers(), 0x22);
        assert!(h.is_pressed(4) && h.is_pressed(0x77) && !h.is_pressed(5) && !h.is_pressed(0xE1));
        assert_eq!(h.pressed().collect::<Vec<_>>(), [4, 0x77]);
        assert_eq!(h.boot_report(), [0x22, 0, 4, 0x77, 0, 0, 0, 0]);
    }

    #[test]
    fn boot_report_rolls_over_past_six_keys() {
        let mut h = HidReports::EMPTY;
        h.keyboard[1] = 0xF0;
        h.keyboard[2] = 0x07; // usages 4..=10
        assert_eq!(h.boot_report(), [0, 0, 1, 1, 1, 1, 1, 1]);
    }

    #[test]
    fn consumer_usage_is_the_lowest_bit() {
        let mut h = HidReports::EMPTY;
        assert_eq!(h.consumer_usage(), 0);
        h.consumer = [0x00, 0x01, 0x80];
        assert_eq!(h.consumer_usage(), 0x18A);
        h.consumer = [0x01, 0x01, 0x00];
        assert_eq!(h.consumer_usage(), 0xE9);
    }
}
