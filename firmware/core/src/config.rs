//! Configuration banks, kept byte-for-byte in the stock 0.0.18 layout so the panel, `tools/` and
//! the stock firmware all read the same memory (docs/re-config-rgb.md §1, docs/protocol.md).
//!
//! * Bank 1 (`0x8007_0000`, 0x790 bytes): board-global — calibration, switch types, version,
//!   active profile, profile names, custom switch curves.
//! * Bank 2 (`0x8008_0000`, 0x1E38 bytes): the active profile — keymaps, actuation, SOCD,
//!   lighting, DKS.
//!
//! The NOR images are plain copies of these arrays (no header or CRC). All per-key arrays have 88
//! entries (keys 0..82, 83..87 spare).
//!
//! **Why byte banks and not typed structs.** The byte layout *is* the interface: the vendor
//! protocol reads and writes arbitrary byte ranges of it, NOR stores it verbatim, and the stock
//! firmware must find its own banks again after a roll-back. [`Global`] and [`Profile`] therefore
//! own the raw image and expose typed accessors that read/write fields in place; the offsets
//! ([`b1`], [`b2`], [`cs`], [`bind`]) are defined here and nowhere else in the firmware. The raw
//! array (`.0`) is for the protocol, storage and tests.
//!
//! Boot-time validation and repair reproduce stock `FUN_0000DD24` (bank 1) and
//! `FUN_00011822`/`FUN_0000CD56` (profile) exactly, so banks the stock firmware wrote load
//! unchanged and garbage is repaired field by field (checked against stock output in
//! `tests/config.rs`). Deliberate deviations are marked **deviation**.

use crate::layout::{LED_COUNT, PROFILE_COUNT};

pub const BANK1_BASE: u32 = 0x8007_0000;
pub const BANK2_BASE: u32 = 0x8008_0000;
/// Live-preview area (RAM only, `BE 03` without ack).
pub const PREVIEW_BASE: u32 = 0x8007_8000;
/// Address ranges the stock dispatcher accepts for `BE 03` / `BE 04` (half-open).
pub const BANK1_LIMIT: u32 = 0x8007_8000;
pub const PREVIEW_LIMIT: u32 = 0x8008_0000;
pub const BANK2_LIMIT: u32 = 0x8010_0000;

/// Bytes of the bank-1 shadow (and of its NOR image).
pub const BANK1_SIZE: usize = 0x790;
/// Bytes of the bank-2 shadow (`0x1000 + 0xE38`, two NOR sectors).
pub const BANK2_SIZE: usize = 0x1E38;
/// Bytes of the preview area we keep: `alpha[85]` + `rgb[85×3]`. Stock keeps internal state (the
/// preview mode, its timer and the live LED frame) right behind it and lets writes clobber it;
/// we drop anything past this (**deviation**, safety).
pub const PREVIEW_SIZE: usize = LED_COUNT * 4;

/// Entries of every per-key array.
pub const KEY_SLOTS: usize = 88;
pub const NAME_SIZE: usize = 64;
pub const BIND_RECORD_SIZE: usize = 48;
pub const CUSTOM_SWITCH_SIZE: usize = 256;
pub const CUSTOM_SWITCH_COUNT: usize = 4;
/// Maximum points of a custom switch curve.
pub const CURVE_MAX: usize = 88;
/// Keymap layers of a profile: 0 = main, 1 = Fn.
pub const LAYERS: usize = 2;
/// `soft_version` the stock 0.0.18 firmware forces on every boot.
pub const STOCK_SOFT_VERSION: u32 = 0x12;

/// Field offsets inside bank 1.
pub mod b1 {
    pub const TOP_VALUE: usize = 0x000;
    pub const BOT_VALUE: usize = 0x0B0;
    pub const KEYTYPE_CODE: usize = 0x160;
    pub const SOFT_VERSION: usize = 0x210;
    /// Unknown u32; stock keeps `0xFFFFFFFF` and sets anything else to 1 (rewriting bank 1).
    pub const FLAG_214: usize = 0x214;
    pub const CUR_PARAM_NUM: usize = 0x218;
    pub const CONFIG_NAME: usize = 0x290;
    /// Four 256-byte custom switch records (`keytype_code` 0xFFF−i selects record i).
    pub const CUSTOM_SWITCH: usize = 0x390;
}

/// Field offsets inside a custom switch record.
pub mod cs {
    /// u32 full travel, µm (valid 1500..=4200).
    pub const TRAVEL: usize = 0x00;
    /// u32 number of curve points (≤ 88).
    pub const POINTS: usize = 0x04;
    /// `i16[n]` curve, non-decreasing, last point = [`super::CURVE_LAST`].
    pub const CURVE: usize = 0x08;
    /// 64-byte UTF-8 name.
    pub const NAME: usize = 0xB8;
    /// u32, default 6000 (meaning unknown).
    pub const WORD_F8: usize = 0xF8;
    /// u8, default 0 (meaning unknown).
    pub const BYTE_FC: usize = 0xFC;
    /// RGB colour (driver display colour).
    pub const COLOUR: usize = 0xFD;
}
/// Required last point of a custom switch curve.
pub const CURVE_LAST: i16 = 2636;

/// Field offsets inside bank 2.
pub mod b2 {
    /// `i16[88]` tap-hold codes of the main / Fn layer (sent after 0.5 s held), 1000 = off.
    pub const HOLD_CODE: usize = 0x000;
    pub const HOLD_CODE_FN: usize = 0x0B0;
    pub const KEY_MAP: usize = 0x160;
    pub const KEY_MAP_FN: usize = 0x210;
    pub const PRESS_TH: usize = 0x2C0;
    pub const RETURN_TH: usize = 0x370;
    pub const PRESS_DEADZONE: usize = 0x420;
    pub const RETURN_DEADZONE: usize = 0x4D0;
    pub const NORM_PRESS_TH: usize = 0x580;
    pub const NORM_RETURN_TH: usize = 0x630;
    pub const KEY_MODE: usize = 0x6E0;
    /// `u8[88]`, forced to `FF` on every profile load.
    pub const FORCED_738: usize = 0x738;
    /// `u8[88]` modifier masks of the main / Fn layer, ORed into the modifier byte while the key
    /// is pressed; default 0.
    pub const MOD_MASK: usize = 0x790;
    pub const MOD_MASK_FN: usize = 0x7E8;
    /// `u16[88]`, forced to `FFFF` on every profile load.
    pub const FORCED_840: usize = 0x840;
    /// `u16[88]` written by reset (100), no reader known.
    pub const UNK_8F0: usize = 0x8F0;
    pub const UNK_9A0: usize = 0x9A0;
    pub const KEY_CANCEL_CODE: usize = 0xA50;
    pub const RGB: usize = 0xB00;
    pub const RGB_SOLID_ENABLE: usize = 0xC08;
    /// `u8[8]`: mode (effect<<2 | enable bits), H, S, brightness, speed, LED-flag mask, C66, C67.
    pub const RGB_CONFIG: usize = 0xC60;
    pub const RGB_MODE: usize = 0xC60;
    pub const RGB_HUE: usize = 0xC61;
    pub const RGB_SAT: usize = 0xC62;
    pub const RGB_BRIGHTNESS: usize = 0xC63;
    pub const RGB_SPEED: usize = 0xC64;
    pub const RGB_FLAGS: usize = 0xC65;
    pub const RGB_ENABLE: usize = 0xC68;
    pub const CAP_ENABLE: usize = 0xC69;
    /// u8, no reader (default 1).
    pub const UNUSED_C6A: usize = 0xC6A;
    pub const RGB_FN_ENABLE: usize = 0xC6B;
    pub const SLEEP_SECOND_CNT: usize = 0xC6C;
    /// WinLock indicator enable.
    pub const WIN_LOCK_STATE: usize = 0xC70;
    pub const AUTO_CALIB_ENABLE: usize = 0xC71;
    pub const BIND_KEY_SETTING: usize = 0xD00;
    pub const BIND_KEY_MODE: usize = 0x1DE0;
    /// Writes to bank 2 that start above this and end at or below [`LIGHT_WINDOW_END`] only
    /// reload the lighting (stock); everything else re-derives keys + lighting.
    pub const LIGHT_WINDOW_START: usize = 0xA20;
    pub const LIGHT_WINDOW_END: usize = 0xC60;
}

/// Field offsets inside a 48-byte `bind_key_setting` record (key group / DKS).
pub mod bind {
    /// `u8[4][4]`: DKS action of member `i` in phase `ph` at `4·i + ph` (0 end, 1 hold, 2 tap).
    pub const ACTIONS: usize = 0;
    /// `i16[4]` member keycodes, 1000 = unused.
    pub const CODES: usize = 16;
    /// u16 top / bottom trigger points, µm of travel.
    pub const TOP_POINT: usize = 24;
    pub const BOTTOM_POINT: usize = 26;
    /// u16 tap timing in scans: pressed time, delay, random extra delay.
    pub const TAP_SINGLE: usize = 28;
    pub const TAP_DELAY: usize = 30;
    pub const TAP_RANDOM: usize = 32;
    /// u8: 1 = the key's own code is suppressed, 0 = sent as well; `FF` marks a blank record.
    pub const SUPPRESS: usize = 47;
}

/// Where a `BE 03` / `BE 04` range lands, per the stock dispatcher's range checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bank {
    Global,
    Profile,
    Preview,
}

/// Validate a `BE 03` range like the stock dispatcher. Returns the bank and the offset; the
/// range may extend past our shadow (callers clip — stock overwrote live RAM there).
pub fn locate_write(addr: u32, n: usize) -> Option<(Bank, usize)> {
    let end = addr.checked_add(n as u32)?;
    if addr < BANK1_BASE {
        None
    } else if end <= BANK1_LIMIT {
        (n <= 512).then_some((Bank::Global, (addr - BANK1_BASE) as usize))
    } else if addr < PREVIEW_BASE {
        None
    } else if addr < PREVIEW_LIMIT {
        // stock: end must stay below 0x80080000 (strictly)
        (end < PREVIEW_LIMIT).then_some((Bank::Preview, (addr - PREVIEW_BASE) as usize))
    } else {
        (end <= BANK2_LIMIT && n <= 512).then_some((Bank::Profile, (addr - BANK2_BASE) as usize))
    }
}

/// Validate a `BE 04` range like the stock dispatcher (the preview area is not readable).
pub fn locate_read(addr: u32, n: usize) -> Option<(Bank, usize)> {
    let end = addr.checked_add(n as u32)?;
    if n > 512 || addr < BANK1_BASE {
        None
    } else if end <= BANK1_LIMIT {
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
pub(crate) fn rd_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
#[inline]
pub(crate) fn wr_u16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes())
}
#[inline]
pub(crate) fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
#[inline]
pub(crate) fn wr_u32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes())
}

fn all_ffff(b: &[u8], off: usize) -> bool {
    b[off..off + 2 * KEY_SLOTS].iter().all(|&x| x == 0xFF)
}

/// Board-global configuration (bank 1).
#[derive(Clone)]
pub struct Global(pub [u8; BANK1_SIZE]);

impl Global {
    pub const fn zeroed() -> Self {
        Self([0; BANK1_SIZE])
    }
    pub const fn blank() -> Self {
        Self([0xFF; BANK1_SIZE])
    }
    u16_array!(top_value, set_top_value, b1::TOP_VALUE);
    u16_array!(bot_value, set_bot_value, b1::BOT_VALUE);
    i16_array!(keytype, set_keytype, b1::KEYTYPE_CODE);

    /// Active profile (`cur_param_num`); out-of-range values read as 0.
    pub fn active_profile(&self) -> usize {
        let p = rd_u32(&self.0, b1::CUR_PARAM_NUM) as usize;
        if p < PROFILE_COUNT { p } else { 0 }
    }
    pub fn set_active_profile(&mut self, p: usize) {
        wr_u32(&mut self.0, b1::CUR_PARAM_NUM, p as u32);
    }
    pub fn soft_version(&self) -> u32 {
        rd_u32(&self.0, b1::SOFT_VERSION)
    }
    pub fn set_soft_version(&mut self, v: u32) {
        wr_u32(&mut self.0, b1::SOFT_VERSION, v)
    }
    pub fn name(&self, profile: usize) -> &[u8; NAME_SIZE] {
        let o = b1::CONFIG_NAME + NAME_SIZE * profile;
        self.0[o..o + NAME_SIZE].try_into().unwrap()
    }
    pub fn name_mut(&mut self, profile: usize) -> &mut [u8; NAME_SIZE] {
        let o = b1::CONFIG_NAME + NAME_SIZE * profile;
        (&mut self.0[o..o + NAME_SIZE]).try_into().unwrap()
    }
    /// Custom switch record `i` (0..[`CUSTOM_SWITCH_COUNT`]).
    pub fn custom_switch(&self, i: usize) -> CustomSwitch<'_> {
        let o = b1::CUSTOM_SWITCH + CUSTOM_SWITCH_SIZE * i;
        CustomSwitch(self.0[o..o + CUSTOM_SWITCH_SIZE].try_into().unwrap())
    }
    pub fn custom_switch_mut(&mut self, i: usize) -> &mut [u8; CUSTOM_SWITCH_SIZE] {
        let o = b1::CUSTOM_SWITCH + CUSTOM_SWITCH_SIZE * i;
        (&mut self.0[o..o + CUSTOM_SWITCH_SIZE]).try_into().unwrap()
    }
    /// Full travel in µm of the switch type of `key` (driver table; custom types from their
    /// record).
    pub fn switch_travel_um(&self, key: usize) -> u32 {
        match self.keytype(key) {
            1 => 3375,
            3 => 3780,
            c @ 0xFFC..=0xFFF => self.custom_switch(0xFFF - c as usize).travel_um(),
            _ => 3420,
        }
    }

    /// `|bot − top| < 3000` for any of the 88 entries: stock discards such a bank-1 write
    /// (`FUN_00015B66`).
    pub fn calibration_implausible(&self) -> bool {
        (0..KEY_SLOTS).any(|k| (self.bot_value(k) as i32 - self.top_value(k) as i32).abs() < 3000)
    }

    /// Stock boot repair of bank 1 (`FUN_0000DD24`). Returns true if stock would rewrite the
    /// bank. Each sub-repair is conditioned on its own check, like stock:
    ///
    /// * `cur_param_num ≥ 4` → 0 (**deviation**: stock compares signed, a negative value loads
    ///   a profile from a wild address);
    /// * `soft_version ≠ version` → `version` (alone it does not ask for a rewrite: our
    ///   version differs from stock's `0x12`, **deviation**, avoids a flash write at first boot);
    /// * word `0x214 ≠ FFFFFFFF` → 1 (stock then rewrites bank 1 on *every* boot; we only
    ///   rewrite if the word was not already 1, **deviation**, flash wear);
    /// * `keytype[0] = FFFF` → all 2; `bot[0] = FFFF` → all top/bot 31000/17000 (a blank
    ///   `top[0]` alone only triggers the other repairs, as in stock);
    /// * invalid custom switch record → default record; name byte 0 = `FF` → default name.
    pub fn repair(&mut self, version: u32) -> bool {
        let cur = rd_u32(&self.0, b1::CUR_PARAM_NUM);
        let ver_ok = self.soft_version() == version;
        let w214 = rd_u32(&self.0, b1::FLAG_214);
        let kt_blank = self.keytype(0) == -1;
        let top_blank = self.top_value(0) == 0xFFFF;
        let bot_blank = self.bot_value(0) == 0xFFFF;
        let names_ok = (0..PROFILE_COUNT).all(|i| self.name(i)[0] != 0xFF);
        let cs_ok = (0..CUSTOM_SWITCH_COUNT).all(|i| self.custom_switch(i).is_valid());
        let rest_ok = (cur as usize) < PROFILE_COUNT && !kt_blank && !top_blank && !bot_blank && names_ok && cs_ok;
        if !ver_ok {
            self.set_soft_version(version);
        }
        if w214 != u32::MAX {
            wr_u32(&mut self.0, b1::FLAG_214, 1);
        }
        if rest_ok && (w214 == u32::MAX || w214 == 1) {
            // nothing but our own version word (or the already-set flag) differs: no rewrite
            return false;
        }
        if cur as usize >= PROFILE_COUNT {
            self.set_active_profile(0);
        }
        if kt_blank {
            for k in 0..KEY_SLOTS {
                self.set_keytype(k, 2);
            }
        }
        if bot_blank {
            for k in 0..KEY_SLOTS {
                self.set_top_value(k, 31000);
                self.set_bot_value(k, 17000);
            }
        }
        for i in 0..CUSTOM_SWITCH_COUNT {
            if !self.custom_switch(i).is_valid() {
                crate::defaults::custom_switch(self.custom_switch_mut(i), i);
            }
        }
        for i in 0..PROFILE_COUNT {
            if self.name(i)[0] == 0xFF {
                crate::defaults::profile_name(self.name_mut(i), i);
            }
        }
        true
    }
}

/// Typed view of one 256-byte custom switch record ([`cs`] offsets).
#[derive(Clone, Copy)]
pub struct CustomSwitch<'a>(&'a [u8; CUSTOM_SWITCH_SIZE]);

impl CustomSwitch<'_> {
    /// Full travel in µm.
    pub fn travel_um(&self) -> u32 {
        rd_u32(self.0, cs::TRAVEL)
    }
    /// Number of curve points as stored (unchecked).
    pub fn points(&self) -> usize {
        rd_u32(self.0, cs::POINTS) as usize
    }
    /// Curve point `i` (`i <` [`CURVE_MAX`]).
    pub fn point(&self, i: usize) -> i16 {
        rd_u16(self.0, cs::CURVE + 2 * i) as i16
    }

    /// Stock `FUN_000148F8`.
    pub fn is_valid(&self) -> bool {
        let r = self.0;
        if !(1500..=4200).contains(&self.travel_um()) {
            return false;
        }
        let n = self.points();
        if n > CURVE_MAX || r[cs::NAME] == 0xFF || rd_u32(r, cs::WORD_F8) == 0xFFFF || r[cs::BYTE_FC] == 0xFF {
            return false;
        }
        // stock reads point n−1 (for n = 0 the high half of the count, i.e. 0) and requires 2636
        let last = if n == 0 { rd_u16(r, cs::POINTS + 2) as i16 } else { self.point(n - 1) };
        last == CURVE_LAST && (1..n).all(|i| self.point(i - 1) <= self.point(i))
    }
}

/// One profile (bank 2).
#[derive(Clone)]
pub struct Profile(pub [u8; BANK2_SIZE]);

/// Typed view of one 48-byte key-group / DKS record ([`bind`] offsets).
#[derive(Clone, Copy)]
pub struct BindRecord<'a>(&'a [u8; BIND_RECORD_SIZE]);

impl BindRecord<'_> {
    /// DKS action of member `i` in phase `phase`.
    pub fn action(&self, i: usize, phase: usize) -> u8 {
        self.0[bind::ACTIONS + 4 * i + phase]
    }
    /// Keycode of member `i`.
    pub fn code(&self, i: usize) -> i16 {
        rd_u16(self.0, bind::CODES + 2 * i) as i16
    }
    pub fn top_point(&self) -> u16 {
        rd_u16(self.0, bind::TOP_POINT)
    }
    pub fn bottom_point(&self) -> u16 {
        rd_u16(self.0, bind::BOTTOM_POINT)
    }
    /// Tap timing in scans: (pressed time, delay, random extra delay).
    pub fn tap_times(&self) -> (u16, u16, u16) {
        (rd_u16(self.0, bind::TAP_SINGLE), rd_u16(self.0, bind::TAP_DELAY), rd_u16(self.0, bind::TAP_RANDOM))
    }
    /// The key's own code is suppressed while the binding is active (byte 47 = 1).
    pub fn suppress_original(&self) -> bool {
        self.0[bind::SUPPRESS] == 1
    }
}

/// The fixed Fn-layer shortcuts stock re-imposes on every profile load, bank-1 save and reset
/// (`FUN_00013DF8`): (key, keycode). Fn+C (key 62) is added per profile by
/// [`Profile::apply_fixed_fn`].
pub const FIXED_FN: [(usize, i16); 9] = [
    (0, 319),  // Esc: 5 s full reset
    (71, 311), // Up: brighter
    (80, 318), // Left: slower
    (81, 312), // Down: dimmer
    (82, 317), // Right: faster
    (74, 316), // LWin: WinLock
    (43, 313), // \: next effect
    (28, 315), // Backspace: light on/off
    (29, 320), // Del: WASD <-> arrows
];
/// Key index of C (Fn+C cycles profiles).
pub const KEY_C: usize = 62;

impl Profile {
    pub const fn zeroed() -> Self {
        Self([0; BANK2_SIZE])
    }
    pub const fn blank() -> Self {
        Self([0xFF; BANK2_SIZE])
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

    /// Keycode of `key` on `layer` (0 main, 1 Fn).
    #[inline]
    pub fn code(&self, layer: usize, key: usize) -> i16 {
        rd_u16(&self.0, [b2::KEY_MAP, b2::KEY_MAP_FN][layer] + 2 * key) as i16
    }
    /// Tap-hold code of `key` on `layer` ([`crate::keycode::NONE`] = off).
    #[inline]
    pub fn hold_code(&self, layer: usize, key: usize) -> i16 {
        rd_u16(&self.0, [b2::HOLD_CODE, b2::HOLD_CODE_FN][layer] + 2 * key) as i16
    }
    pub fn set_hold_code(&mut self, layer: usize, key: usize, code: i16) {
        wr_u16(&mut self.0, [b2::HOLD_CODE, b2::HOLD_CODE_FN][layer] + 2 * key, code as u16)
    }
    /// Modifier mask of `key` on `layer`.
    #[inline]
    pub fn mod_mask(&self, layer: usize, key: usize) -> u8 {
        self.0[[b2::MOD_MASK, b2::MOD_MASK_FN][layer] + key]
    }
    pub fn set_mod_mask(&mut self, layer: usize, key: usize, mask: u8) {
        self.0[[b2::MOD_MASK, b2::MOD_MASK_FN][layer] + key] = mask;
    }
    /// `key_mode`: 0 normal, 1 rapid trigger, 2 rapid trigger below the normal press point.
    #[inline]
    pub fn key_mode(&self, key: usize) -> u8 {
        self.0[b2::KEY_MODE + key]
    }
    pub fn set_key_mode(&mut self, key: usize, v: u8) {
        self.0[b2::KEY_MODE + key] = v;
    }
    /// `bind_key_mode`: 0 none, 1 key group, 2 DKS.
    #[inline]
    pub fn bind_mode(&self, key: usize) -> u8 {
        self.0[b2::BIND_KEY_MODE + key]
    }
    pub fn bind_record(&self, key: usize) -> &[u8; BIND_RECORD_SIZE] {
        let o = b2::BIND_KEY_SETTING + BIND_RECORD_SIZE * key;
        self.0[o..o + BIND_RECORD_SIZE].try_into().unwrap()
    }
    /// Typed view of the key-group / DKS record of `key`.
    pub fn bind(&self, key: usize) -> BindRecord<'_> {
        BindRecord(self.bind_record(key))
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
    /// `rgb_config[0..5]`: mode, H, S, brightness, speed.
    pub fn rgb_config(&self) -> [u8; 5] {
        self.0[b2::RGB_CONFIG..b2::RGB_CONFIG + 5].try_into().unwrap()
    }
    /// Effect id (bits 7..2 of `rgb_config[0]`).
    pub fn effect(&self) -> u8 {
        self.0[b2::RGB_MODE] >> 2
    }
    /// Set the effect id, keeping the two low bits.
    pub fn set_effect(&mut self, e: u8) {
        self.0[b2::RGB_MODE] = (e << 2) | (self.0[b2::RGB_MODE] & 3);
    }
    /// LED-flag mask of the effects (`rgb_config[5]`).
    pub fn rgb_flags(&self) -> u8 {
        self.0[b2::RGB_FLAGS]
    }
    /// Stored brightness step (0..=30).
    pub fn set_brightness(&mut self, b: u8) {
        self.0[b2::RGB_BRIGHTNESS] = b;
    }
    /// Master lighting switch (`rgb_enable`).
    pub fn rgb_enabled(&self) -> bool {
        self.0[b2::RGB_ENABLE] != 0
    }
    /// Keycode 315: stock computes `1 − rgb_enable`.
    pub fn toggle_rgb_enabled(&mut self) {
        self.0[b2::RGB_ENABLE] = 1u8.wrapping_sub(self.0[b2::RGB_ENABLE]);
    }
    /// Caps Lock indicator enabled (`cap_enable`).
    pub fn caps_indicator(&self) -> bool {
        self.0[b2::CAP_ENABLE] != 0
    }
    /// Fn-layer highlight enabled (`rgb_fn_enable`).
    pub fn fn_highlight(&self) -> bool {
        self.0[b2::RGB_FN_ENABLE] != 0
    }
    /// WinLock indicator enabled (`win_lock_state` = 1 exactly, like stock).
    pub fn win_lock_indicator(&self) -> bool {
        self.0[b2::WIN_LOCK_STATE] == 1
    }
    /// Auto-calibration enabled (`auto_calib_enable` = 1 exactly, like stock).
    pub fn auto_calibration(&self) -> bool {
        self.0[b2::AUTO_CALIB_ENABLE] == 1
    }
    /// Stock lighting reload clamps the brightness to 30 and writes it back; returns the
    /// run-time value V = 6 × brightness (≤ 180).
    pub fn clamp_brightness(&mut self) -> u8 {
        if self.0[b2::RGB_BRIGHTNESS] > 30 {
            self.0[b2::RGB_BRIGHTNESS] = 30;
        }
        self.0[b2::RGB_BRIGHTNESS] * 6
    }
    pub fn sleep_seconds(&self) -> u32 {
        rd_u32(&self.0, b2::SLEEP_SECOND_CNT)
    }

    /// Re-impose the fixed Fn shortcuts; Fn+C goes to the next profile (`index` = the active
    /// profile, as stock reads `cur_param_num`).
    pub fn apply_fixed_fn(&mut self, index: usize) {
        for (k, c) in FIXED_FN {
            self.set_key_map_fn(k, c);
        }
        let c = match index {
            0 => 308,
            1 => 309,
            2 => 310,
            3 => 307,
            _ => return,
        };
        self.set_key_map_fn(KEY_C, c);
    }

    /// What stock does right after reading a profile from NOR (`FUN_000146AE`): force the
    /// two unknown arrays and re-impose the fixed Fn shortcuts.
    pub fn on_load(&mut self, index: usize) {
        self.0[b2::FORCED_738..b2::FORCED_738 + KEY_SLOTS].fill(0xFF);
        self.0[b2::FORCED_840..b2::FORCED_840 + 2 * KEY_SLOTS].fill(0xFF);
        self.apply_fixed_fn(index);
    }

    fn thresholds_bad(&self) -> bool {
        [b2::PRESS_TH, b2::RETURN_TH, b2::NORM_PRESS_TH, b2::NORM_RETURN_TH, b2::PRESS_DEADZONE, b2::RETURN_DEADZONE]
            .iter()
            .any(|&o| rd_u16(&self.0, o) > 5000)
            || self.0[b2::KEY_MODE] > 10
    }
    fn bind_record_valid(&self, k: usize) -> bool {
        let r = self.bind_record(k);
        // stock compares the trigger points signed
        self.bind_mode(k) <= 2
            && rd_u16(r, bind::BOTTOM_POINT) as i16 <= 5000
            && rd_u16(r, bind::TOP_POINT) as i16 <= 5000
            && r[bind::SUPPRESS] != 0xFF
    }
    fn binds_all_invalid(&self) -> bool {
        (0..KEY_SLOTS).all(|k| !self.bind_record_valid(k))
    }

    /// Stock validation `FUN_00011822` (run after [`Profile::on_load`]).
    pub fn is_valid(&self) -> bool {
        !(all_ffff(&self.0, b2::KEY_MAP)
            || all_ffff(&self.0, b2::KEY_MAP_FN)
            // signed compare, as stock: a blank (`FFFF` = −1) hold code passes
            || self.hold_code(0, 0) > 1000
            || self.thresholds_bad()
            || self.sleep_seconds() > 3600
            // stock tests only the low half here; the repair below tests the whole word
            || rd_u16(&self.0, b2::RGB_CONFIG) == 0xFFFF
            || all_ffff(&self.0, b2::KEY_CANCEL_CODE)
            || self.binds_all_invalid()
            || self.0[b2::WIN_LOCK_STATE] > 10
            || self.0[b2::AUTO_CALIB_ENABLE] > 10)
    }

    /// Stock field-wise repair `FUN_0000CD56`: every block is reset to its default only if its
    /// own check fails; for profiles 1–3 the effect id and `rgb_solid_enable` are always reset.
    pub fn repair(&mut self, index: usize) {
        use crate::defaults as d;
        if rd_u32(&self.0, b2::RGB_CONFIG) == u32::MAX {
            d::reset_lighting(self);
        }
        if all_ffff(&self.0, b2::KEY_MAP) || all_ffff(&self.0, b2::KEY_MAP_FN) {
            d::reset_keymaps(self);
        }
        if self.thresholds_bad() {
            d::reset_actuation(self, index);
        }
        if all_ffff(&self.0, b2::KEY_CANCEL_CODE) {
            d::reset_cancel(self);
        }
        if all_ffff(&self.0, b2::HOLD_CODE) {
            d::reset_tap_hold(self);
        }
        if self.binds_all_invalid() {
            d::reset_binds(self);
        }
        if self.sleep_seconds() > 3600 {
            wr_u32(&mut self.0, b2::SLEEP_SECOND_CNT, 0);
        }
        if self.0[b2::WIN_LOCK_STATE] > 10 {
            self.0[b2::WIN_LOCK_STATE] = 1;
        }
        if self.0[b2::AUTO_CALIB_ENABLE] > 10 {
            self.0[b2::AUTO_CALIB_ENABLE] = 1;
        }
        self.apply_fixed_fn(index);
        if let Some(&e) = d::PROFILE_EFFECT.get(index).filter(|_| index != 0) {
            self.0[b2::RGB_SOLID_ENABLE..b2::RGB_SOLID_ENABLE + LED_COUNT].fill(0);
            self.set_effect(e);
        }
    }

    /// Load-time processing of an image read from NOR (blank NOR = all `FF`): force, validate,
    /// repair. Returns true if the profile was repaired and must be written back.
    pub fn load_fixup(&mut self, index: usize) -> bool {
        self.on_load(index);
        if self.is_valid() {
            return false;
        }
        self.repair(index);
        true
    }
}

const _: () = assert!(b2::BIND_KEY_MODE + KEY_SLOTS <= BANK2_SIZE);
const _: () = assert!(b2::BIND_KEY_SETTING + BIND_RECORD_SIZE * KEY_SLOTS <= 0x1D80);
const _: () = assert!(b2::RGB_SOLID_ENABLE + LED_COUNT <= b2::RGB_CONFIG);
const _: () = assert!(b1::CUSTOM_SWITCH + CUSTOM_SWITCH_SIZE * CUSTOM_SWITCH_COUNT == BANK1_SIZE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_ranges() {
        assert_eq!(locate_write(0x8007_0000, 166), Some((Bank::Global, 0)));
        assert_eq!(locate_write(0x8008_0160, 2), Some((Bank::Profile, 0x160)));
        assert_eq!(locate_write(0x8007_7FFF, 2), None); // crosses into the preview area
        assert_eq!(locate_write(0x8007_7F00, 0x100), Some((Bank::Global, 0x7F00)));
        assert_eq!(locate_write(0x8007_7F00, 0x101), None);
        assert_eq!(locate_write(0x8007_8000, 85), Some((Bank::Preview, 0)));
        assert_eq!(locate_write(0x8007_FF00, 0x100), None);
        assert_eq!(locate_write(0x8006_FFFF, 1), None);
        assert_eq!(locate_write(0x800F_FFFF, 1), Some((Bank::Profile, 0x7FFFF)));
        assert_eq!(locate_write(0x8008_0000, 513), None);
        assert_eq!(locate_write(0xFFFF_FFFF, 2), None);
        assert_eq!(locate_read(0x8007_8000, 4), None);
        assert_eq!(locate_read(0x8007_0790, 4), Some((Bank::Global, 0x790)));
    }

    #[test]
    fn layered_access() {
        let mut p = Profile::zeroed();
        p.set_key_map_fn(3, 319);
        p.set_hold_code(1, 4, -7);
        p.set_mod_mask(0, 5, 0x22);
        assert_eq!((p.code(0, 3), p.code(1, 3)), (0, 319));
        assert_eq!(rd_u16(&p.0, b2::HOLD_CODE_FN + 8) as i16, -7);
        assert_eq!(p.0[b2::MOD_MASK + 5], 0x22);
        assert_eq!((p.hold_code(1, 4), p.mod_mask(0, 5)), (-7, 0x22));
    }

    #[test]
    fn typed_access() {
        let mut p = Profile::zeroed();
        p.set_key_map(76, 44);
        p.set_cancel_code(3, -8);
        assert_eq!(p.0[b2::KEY_MAP + 152], 44);
        assert_eq!(p.cancel_code(3), -8);
        p.0[b2::RGB_MODE] = 0x5D;
        assert_eq!(p.effect(), 23);
        p.set_effect(13);
        assert_eq!(p.0[b2::RGB_MODE], 0x35);
    }
}
