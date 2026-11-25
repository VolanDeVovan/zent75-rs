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
    /// Consumer-page usage currently held (0 = none).
    pub consumer: u16,
    /// Generic-desktop system-control usage (0x81 power down, 0x82 sleep, 0x83 wake), 0 = none.
    pub system: u8,
    /// Mouse buttons bit0 = left … bit4 = forward.
    pub mouse_buttons: u8,
}

/// Host LED output report bits.
pub mod host_led {
    pub const NUM_LOCK: u8 = 1 << 0;
    pub const CAPS_LOCK: u8 = 1 << 1;
    pub const SCROLL_LOCK: u8 = 1 << 2;
}
