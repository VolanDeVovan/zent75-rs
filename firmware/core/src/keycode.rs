//! Keymap codes of the config banks (docs/protocol.md): signed 16-bit values.
//!
//! | Range | Meaning |
//! |---|---|
//! | −8 … −1 | modifiers LCtrl … RWin (bit `code & 7` of the modifier byte) |
//! | 1 … 119 | HID keyboard usages |
//! | 201 … 224 | consumer keys (bit `code − 201` of the consumer report) |
//! | 251 … 255 | mouse buttons 1 … 5 |
//! | 301 … 320 | board functions |
//! | 401 | Fn layer (402–404 are inert) |
//! | 1000 | empty |

/// Empty keymap entry / "off" for tap-hold, SOCD and key-group members.
pub const NONE: i16 = 1000;
/// Fn layer key.
pub const FN: i16 = 401;

pub const LWIN: i16 = -5;
pub const RWIN: i16 = -1;
/// HID usage of Caps Lock.
pub const CAPS_LOCK: i16 = 57;

/// Last HID keyboard usage with a bit in the 16-byte NKRO report.
pub const KEY_LAST: i16 = 119;
pub const CONSUMER_FIRST: i16 = 201;
pub const CONSUMER_LAST: i16 = 224;
pub const MOUSE_FIRST: i16 = 251;
pub const MOUSE_LAST: i16 = 255;

/// Board functions (stock `FUN_C698`).
pub mod func {
    pub const FIRST: i16 = 301;
    pub const LAST: i16 = 400;
    /// Jump to the UF2 loader.
    pub const BOOTLOADER: i16 = 301;
    pub const REBOOT: i16 = 302;
    /// Rapid trigger off / on for every key (RAM only).
    pub const RT_OFF: i16 = 303;
    pub const RT_ON: i16 = 304;
    /// Reset the active profile to its defaults (`INITSET`).
    pub const RESET_PROFILE: i16 = 305;
    /// Esc, or `` ` `` / `~` while a Shift is held.
    pub const GRAVE_ESC: i16 = 306;
    /// 307–310: switch to profile 0–3.
    pub const PROFILE_FIRST: i16 = 307;
    pub const PROFILE_LAST: i16 = 310;
    pub const BRIGHTNESS_UP: i16 = 311;
    pub const BRIGHTNESS_DOWN: i16 = 312;
    pub const NEXT_EFFECT: i16 = 313;
    pub const PREV_EFFECT: i16 = 314;
    pub const LIGHT_TOGGLE: i16 = 315;
    pub const WIN_LOCK: i16 = 316;
    pub const SPEED_UP: i16 = 317;
    pub const SPEED_DOWN: i16 = 318;
    /// Sits on Fn + Esc; does nothing by itself (the 5 s reset is hard-wired to the keys).
    pub const HARD_RESET_MARK: i16 = 319;
    /// Swap WASD and the arrow keys (RAM only).
    pub const SWAP_WASD: i16 = 320;
}
