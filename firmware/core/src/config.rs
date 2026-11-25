//! Configuration banks, kept byte-for-byte in the stock layout so the panel, `tools/` and the
//! stock firmware all read the same memory (docs/protocol.md "RAM map").
//!
//! * Bank 1 (`0x8007_0000`): board-global — calibration, switch types, version, active profile,
//!   profile names.
//! * Bank 2 (`0x8008_0000`): the active profile — keymaps, actuation, SOCD, lighting, DKS.
//!
//! The banks are plain byte arrays; typed accessors below read/write fields in place. Field
//! offsets are relative to the bank base.

use crate::layout::{KEY_COUNT, LED_COUNT, PROFILE_COUNT};

pub const BANK1_BASE: u32 = 0x8007_0000;
pub const BANK2_BASE: u32 = 0x8008_0000;
/// Address ranges the stock dispatcher accepts for `BE 03` / `BE 04` (half-open).
pub const BANK1_LIMIT: u32 = 0x8007_8000;
pub const BANK2_LIMIT: u32 = 0x8010_0000;

/// Bytes of bank 1 we keep (covers every known field; the rest reads as zero).
pub const BANK1_SIZE: usize = 0x400;
/// Bytes of bank 2 we keep (bind_key_mode ends at 0x1E33).
pub const BANK2_SIZE: usize = 0x2000;

/// Trigger addresses inside bank 1's accepted range (writes there are commands, not storage).
pub const TRIGGER_BRIGHTNESS_A: u32 = 0x8007_8000;
pub const TRIGGER_BRIGHTNESS_B: u32 = 0x8007_8055;

pub const NAME_SIZE: usize = 64;
pub const BIND_RECORD_SIZE: usize = 48;
/// Keymap value meaning "empty".
pub const KC_NONE: i16 = 1000;

/// Field offsets inside bank 1.
pub mod b1 {
    pub const TOP_VALUE: usize = 0x000;
    pub const BOT_VALUE: usize = 0x0B0;
    pub const KEYTYPE_CODE: usize = 0x160;
    pub const SOFT_VERSION: usize = 0x210;
    pub const CUR_PARAM_NUM: usize = 0x218;
    pub const CONFIG_NAME: usize = 0x290;
}

/// Field offsets inside bank 2.
pub mod b2 {
    pub const KEY_MAP: usize = 0x160;
    pub const KEY_MAP_FN: usize = 0x210;
    pub const PRESS_TH: usize = 0x2C0;
    pub const RETURN_TH: usize = 0x370;
    pub const PRESS_DEADZONE: usize = 0x420;
    pub const RETURN_DEADZONE: usize = 0x4D0;
    pub const NORM_PRESS_TH: usize = 0x580;
    pub const NORM_RETURN_TH: usize = 0x630;
    pub const KEY_MODE: usize = 0x6E0;
    pub const KEY_CANCEL_CODE: usize = 0xA50;
    pub const RGB: usize = 0xB00;
    pub const RGB_SOLID_ENABLE: usize = 0xC08;
    pub const RGB_CONFIG: usize = 0xC60;
    pub const RGB_ENABLE: usize = 0xC68;
    pub const CAP_ENABLE: usize = 0xC69;
    pub const RGB_FN_ENABLE: usize = 0xC6B;
    pub const SLEEP_SECOND_CNT: usize = 0xC6C;
    pub const WIN_LOCK_STATE: usize = 0xC70;
    pub const AUTO_CALIB_ENABLE: usize = 0xC71;
    pub const BIND_KEY_SETTING: usize = 0xD00;
    pub const BIND_KEY_MODE: usize = 0x1DE0;
}

/// Which bank an address range falls into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bank {
    Global,
    Profile,
}

/// Validate a `BE 03`/`BE 04` range like the stock dispatcher. Returns the bank and the offset.
pub fn locate(addr: u32, n: usize) -> Option<(Bank, usize)> {
    let end = addr.checked_add(n as u32)?;
    if addr >= BANK1_BASE && end <= BANK1_LIMIT {
        Some((Bank::Global, (addr - BANK1_BASE) as usize))
    } else if addr >= BANK2_BASE && end <= BANK2_LIMIT {
        Some((Bank::Profile, (addr - BANK2_BASE) as usize))
    } else {
        None
    }
}

macro_rules! u16_array {
    ($get:ident, $set:ident, $off:expr) => {
        #[inline]
        pub fn $get(&self, key: usize) -> u16 {
            rd_u16(&self.0, $off + 2 * key)
        }
        #[inline]
        pub fn $set(&mut self, key: usize, v: u16) {
            wr_u16(&mut self.0, $off + 2 * key, v)
        }
    };
}

macro_rules! i16_array {
    ($get:ident, $set:ident, $off:expr) => {
        #[inline]
        pub fn $get(&self, key: usize) -> i16 {
            rd_u16(&self.0, $off + 2 * key) as i16
        }
        #[inline]
        pub fn $set(&mut self, key: usize, v: i16) {
            wr_u16(&mut self.0, $off + 2 * key, v as u16)
        }
    };
}

#[inline]
fn rd_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
#[inline]
fn wr_u16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes())
}

/// Board-global configuration (bank 1).
#[derive(Clone)]
pub struct Global(pub [u8; BANK1_SIZE]);

impl Global {
    pub const fn zeroed() -> Self {
        Self([0; BANK1_SIZE])
    }
    u16_array!(top_value, set_top_value, b1::TOP_VALUE);
    u16_array!(bot_value, set_bot_value, b1::BOT_VALUE);
    i16_array!(keytype, set_keytype, b1::KEYTYPE_CODE);

    pub fn active_profile(&self) -> usize {
        (self.0[b1::CUR_PARAM_NUM] as usize).min(PROFILE_COUNT - 1)
    }
    pub fn set_active_profile(&mut self, p: usize) {
        self.0[b1::CUR_PARAM_NUM..b1::CUR_PARAM_NUM + 4].copy_from_slice(&(p as u32).to_le_bytes());
    }
    /// `soft_version`: bytes `[patch, minor, major, 0]`.
    pub fn set_version(&mut self, major: u8, minor: u8, patch: u8) {
        self.0[b1::SOFT_VERSION..b1::SOFT_VERSION + 4].copy_from_slice(&[patch, minor, major, 0]);
    }
    pub fn name(&self, profile: usize) -> &[u8; NAME_SIZE] {
        let o = b1::CONFIG_NAME + NAME_SIZE * profile;
        self.0[o..o + NAME_SIZE].try_into().unwrap()
    }
}

/// One profile (bank 2).
#[derive(Clone)]
pub struct Profile(pub [u8; BANK2_SIZE]);

/// Per-key actuation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyMode {
    Normal,
    RapidTrigger,
}

/// Per-key advanced binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindMode {
    None,
    Group,
    Dks,
}

impl Profile {
    pub const fn zeroed() -> Self {
        Self([0; BANK2_SIZE])
    }
    i16_array!(key_map, set_key_map, b2::KEY_MAP);
    i16_array!(key_map_fn, set_key_map_fn, b2::KEY_MAP_FN);
    u16_array!(press_th, set_press_th, b2::PRESS_TH);
    u16_array!(return_th, set_return_th, b2::RETURN_TH);
    u16_array!(press_deadzone, set_press_deadzone, b2::PRESS_DEADZONE);
    u16_array!(return_deadzone, set_return_deadzone, b2::RETURN_DEADZONE);
    u16_array!(norm_press_th, set_norm_press_th, b2::NORM_PRESS_TH);
    u16_array!(norm_return_th, set_norm_return_th, b2::NORM_RETURN_TH);
    i16_array!(cancel_code, set_cancel_code, b2::KEY_CANCEL_CODE);

    pub fn key_mode(&self, key: usize) -> KeyMode {
        if self.0[b2::KEY_MODE + key] == 1 { KeyMode::RapidTrigger } else { KeyMode::Normal }
    }
    pub fn set_key_mode_raw(&mut self, key: usize, v: u8) {
        self.0[b2::KEY_MODE + key] = v;
    }
    pub fn bind_mode(&self, key: usize) -> BindMode {
        match self.0[b2::BIND_KEY_MODE + key] {
            1 => BindMode::Group,
            2 => BindMode::Dks,
            _ => BindMode::None,
        }
    }
    pub fn bind_record(&self, key: usize) -> &[u8; BIND_RECORD_SIZE] {
        let o = b2::BIND_KEY_SETTING + BIND_RECORD_SIZE * key;
        self.0[o..o + BIND_RECORD_SIZE].try_into().unwrap()
    }
    pub fn bind_record_mut(&mut self, key: usize) -> &mut [u8; BIND_RECORD_SIZE] {
        let o = b2::BIND_KEY_SETTING + BIND_RECORD_SIZE * key;
        (&mut self.0[o..o + BIND_RECORD_SIZE]).try_into().unwrap()
    }
    pub fn led_rgb(&self, led: usize) -> [u8; 3] {
        let o = b2::RGB + 3 * led;
        [self.0[o], self.0[o + 1], self.0[o + 2]]
    }
    pub fn led_solid(&self, led: usize) -> bool {
        self.0[b2::RGB_SOLID_ENABLE + led] != 0
    }
    pub fn rgb_config(&self) -> [u8; 5] {
        self.0[b2::RGB_CONFIG..b2::RGB_CONFIG + 5].try_into().unwrap()
    }
    pub fn flag(&self, off: usize) -> bool {
        self.0[off] != 0
    }
    pub fn sleep_seconds(&self) -> u32 {
        u32::from_le_bytes(self.0[b2::SLEEP_SECOND_CNT..b2::SLEEP_SECOND_CNT + 4].try_into().unwrap())
    }
}

const _: () = assert!(b2::BIND_KEY_MODE + KEY_COUNT <= BANK2_SIZE);
const _: () = assert!(b2::RGB_SOLID_ENABLE + LED_COUNT <= b2::RGB_CONFIG);
const _: () = assert!(b1::CONFIG_NAME + NAME_SIZE * PROFILE_COUNT <= BANK1_SIZE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_ranges() {
        assert_eq!(locate(0x8007_0000, 166), Some((Bank::Global, 0)));
        assert_eq!(locate(0x8008_0160, 2), Some((Bank::Profile, 0x160)));
        assert_eq!(locate(0x8007_7FFF, 2), None);
        assert_eq!(locate(0x8006_FFFF, 1), None);
        assert_eq!(locate(0x800F_FFFF, 1), Some((Bank::Profile, 0x7FFFF)));
        assert_eq!(locate(0xFFFF_FFFF, 2), None);
    }

    #[test]
    fn typed_access() {
        let mut p = Profile::zeroed();
        p.set_key_map(76, 44);
        p.set_cancel_code(3, -8);
        assert_eq!(p.0[b2::KEY_MAP + 152], 44);
        assert_eq!(p.cancel_code(3), -8);
    }
}
