//! HID output state produced by the key pipeline. The exact wire formats (report descriptors)
//! live with the USB code; this is the logical state.

/// Logical keyboard state: modifier byte + bitmap of HID keyboard usages 0..=255.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyboardState {
    pub modifiers: u8,
    pub keys: [u8; 32],
}

impl KeyboardState {
    pub fn press(&mut self, usage: u8) {
        self.keys[usage as usize >> 3] |= 1 << (usage & 7);
    }
    pub fn release(&mut self, usage: u8) {
        self.keys[usage as usize >> 3] &= !(1 << (usage & 7));
    }
    pub fn is_pressed(&self, usage: u8) -> bool {
        self.keys[usage as usize >> 3] & (1 << (usage & 7)) != 0
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    /// Pressed usages in ascending order.
    pub fn pressed(&self) -> impl Iterator<Item = u8> + '_ {
        (0u16..256).map(|u| u as u8).filter(|&u| self.is_pressed(u))
    }
    /// The stock 16-byte NKRO report: byte 0 = modifiers E0–E7, bytes 1..15 = bitmap of usages
    /// 0x00–0x77 (usage u → byte `u/8 + 1`, bit `u%8`).
    pub fn nkro_report(&self) -> [u8; 16] {
        let mut r = [0u8; 16];
        r[0] = self.modifiers;
        r[1..].copy_from_slice(&self.keys[..15]);
        r
    }
    /// Inverse of [`KeyboardState::nkro_report`].
    pub fn from_nkro(r: &[u8; 16]) -> Self {
        let mut s = Self { modifiers: r[0], keys: [0; 32] };
        s.keys[..15].copy_from_slice(&r[1..]);
        s
    }
    /// 6KRO boot-protocol report `[mods, 0, k1..k6]` (first six usages, ErrorRollOver on overflow).
    pub fn boot_report(&self) -> [u8; 8] {
        let mut r = [0u8; 8];
        r[0] = self.modifiers;
        let mut n = 0;
        for u in self.pressed() {
            if n == 6 {
                r[2..8].fill(0x01);
                break;
            }
            r[2 + n] = u;
            n += 1;
        }
        r
    }
}

/// Everything the host sees from the HID interfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HidState {
    pub keyboard: KeyboardState,
    /// Consumer-page usage currently held (0 = none; the lowest bit of `consumer_bits`).
    pub consumer: u16,
    /// Stock 3-byte consumer report as bits: bit n = code 201 + n (`keys::CONSUMER_USAGES`).
    pub consumer_bits: u32,
    /// Generic-desktop system-control usage (0x81 power down, 0x82 sleep, 0x83 wake), 0 = none.
    pub system: u8,
    /// Mouse buttons bit0 = left … bit4 = forward.
    pub mouse_buttons: u8,
}

impl HidState {
    /// The stock 3-byte consumer report (EP 0x83).
    pub fn consumer_report(&self) -> [u8; 3] {
        [self.consumer_bits as u8, (self.consumer_bits >> 8) as u8, (self.consumer_bits >> 16) as u8]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nkro_roundtrip() {
        let mut k = KeyboardState { modifiers: 0x22, ..Default::default() };
        k.press(4);
        k.press(0x77);
        let r = k.nkro_report();
        assert_eq!(r[0], 0x22);
        assert_eq!(r[1], 0x10);
        assert_eq!(r[15], 0x80);
        assert_eq!(KeyboardState::from_nkro(&r), k);
    }
}

/// Host LED output report bits.
pub mod host_led {
    pub const NUM_LOCK: u8 = 1 << 0;
    pub const CAPS_LOCK: u8 = 1 << 1;
    pub const SCROLL_LOCK: u8 = 1 << 2;
}
