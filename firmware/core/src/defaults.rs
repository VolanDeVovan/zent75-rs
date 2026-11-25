//! Factory defaults of the stock 0.0.18 firmware (docs/re-config-rgb.md §1–2).
//!
//! The factory images are what stock writes on blank NOR: a blank (`FF`) bank put through the
//! boot repair. [`global`] / [`profile`] build them the same way, so bytes stock never
//! initialises (gaps, `C66`/`C67`, bind-record bytes 34..46, …) stay `FF` exactly as on a
//! freshly flashed board. `tests/config.rs` compares them with the images stock produces.

use crate::config::{self, b2, cs, wr_u16, wr_u32, Global, Profile, CUSTOM_SWITCH_SIZE, KC_NONE, KEY_SLOTS, NAME_SIZE};
use crate::layout::{LED_COUNT, PROFILE_COUNT};

/// Stock default main layer (table `0x80040EF8`); 401 = Fn, negative = modifiers.
pub const KEYMAP: [i16; KEY_SLOTS] = [
    41, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 74, //
    53, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 45, 46, 42, 76, //
    43, 20, 26, 8, 21, 23, 28, 24, 12, 18, 19, 47, 48, 49, 75, //
    57, 4, 22, 7, 9, 10, 11, 13, 14, 15, 51, 52, 40, 78, //
    -7, 29, 27, 6, 25, 5, 17, 16, 54, 55, 56, -3, 82, 77, //
    -8, -5, -6, 44, -2, 401, -4, 80, 81, 79, //
    KC_NONE, KC_NONE, KC_NONE, KC_NONE, KC_NONE,
];

/// Stock default Fn layer before the fixed shortcuts are imposed: empty except the modifiers,
/// which map to themselves.
pub const KEYMAP_FN: [i16; KEY_SLOTS] = {
    let mut t = [KC_NONE; KEY_SLOTS];
    t[59] = -7;
    t[70] = -3;
    t[73] = -8;
    t[75] = -6;
    t[77] = -2;
    t[79] = -4;
    t
};

/// Effect id each profile gets when its image is created or repaired (profile 0 keeps the
/// default `rgb_config[0]` = 0x5D, effect 23).
pub const PROFILE_EFFECT: [u8; PROFILE_COUNT] = [23, 13, 25, 50];
/// Default `rgb_config` bytes 0..5 and the following enables (`C68..C6B`).
pub const RGB_CONFIG: [u8; 6] = [0x5D, 0x90, 0xFF, 0x0A, 0x32, 0xFF];
/// Keys that are rapid-trigger in profile 0: W, A, S, D.
pub const WASD: [usize; 4] = [32, 46, 47, 48];

/// Default profile names (zero padded to 64 bytes).
pub const PROFILE_NAMES: [&str; PROFILE_COUNT] = ["游戏模式", "办公模式", "默认配置3", "默认配置4"];
/// Custom switch record name prefix; the record number 1..4 follows.
pub const CUSTOM_SWITCH_NAME: &str = "自定义轴体";
/// Custom switch record colours (driver display).
pub const CUSTOM_SWITCH_COLOUR: [[u8; 3]; 4] = [[0xFF, 0x00, 0xFF], [0x00, 0xFF, 0xFF], [0x30, 0xFF, 0x9F], [0x67, 0x26, 0xFF]];
/// Default custom switch curve (`0x80041DFC`), µm per step.
pub const CUSTOM_SWITCH_CURVE: [i16; 71] = [
    -12, 0, 12, 25, 39, 50, 65, 78, 91, 108, 121, 136, 151, 166, 184, 200, 217, 235, 254, 271, 289, 308,
    329, 350, 371, 394, 416, 441, 463, 492, 516, 539, 565, 592, 621, 649, 681, 713, 746, 777, 818, 851,
    886, 922, 963, 1003, 1044, 1088, 1134, 1181, 1227, 1280, 1325, 1387, 1440, 1499, 1554, 1620, 1682,
    1751, 1820, 1896, 1966, 2046, 2124, 2221, 2304, 2394, 2488, 2594, 2636,
];

/// Factory bank 1 (what stock writes on blank NOR). `soft_version` is stock's `0x12`; the
/// device overrides it with its own version.
pub fn global(g: &mut Global) {
    *g = Global::blank();
    g.repair(config::STOCK_SOFT_VERSION);
}

/// Factory image of profile `index` (what stock writes when it first selects a blank profile).
pub fn profile(p: &mut Profile, index: usize) {
    *p = Profile::blank();
    p.load_fixup(index);
}

pub fn profile_name(n: &mut [u8; NAME_SIZE], i: usize) {
    n.fill(0);
    let s = PROFILE_NAMES[i].as_bytes();
    n[..s.len()].copy_from_slice(s);
}

/// Default custom switch record `i` (stock leaves its gap at `0x96..0xB7` as `FF`).
pub fn custom_switch(r: &mut [u8; CUSTOM_SWITCH_SIZE], i: usize) {
    wr_u32(r, cs::TRAVEL, 3420);
    wr_u32(r, cs::POINTS, CUSTOM_SWITCH_CURVE.len() as u32);
    r[0x96..0xB8].fill(0xFF);
    for (j, &v) in CUSTOM_SWITCH_CURVE.iter().enumerate() {
        wr_u16(r, cs::CURVE + 2 * j, v as u16);
    }
    let name = &mut r[cs::NAME..cs::NAME + NAME_SIZE];
    name.fill(0);
    let s = CUSTOM_SWITCH_NAME.as_bytes();
    name[..s.len()].copy_from_slice(s);
    name[s.len()] = b'1' + i as u8;
    wr_u32(r, cs::WORD_F8, 6000);
    r[cs::BYTE_FC] = 0;
    r[cs::COLOUR..cs::COLOUR + 3].copy_from_slice(&CUSTOM_SWITCH_COLOUR[i]);
}

/// Lighting block: white static colours (byte `BFF` untouched), no static LEDs,
/// `rgb_config` 5D 90 FF 0A 32 FF (`C66`/`C67` untouched), `C68..C6B` = 1.
pub fn reset_lighting(p: &mut Profile) {
    p.0[b2::RGB..b2::RGB + 3 * LED_COUNT].fill(0xFF);
    p.0[b2::RGB_SOLID_ENABLE..b2::RGB_SOLID_ENABLE + LED_COUNT].fill(0);
    p.0[b2::RGB_CONFIG..b2::RGB_CONFIG + 6].copy_from_slice(&RGB_CONFIG);
    p.0[b2::RGB_ENABLE..b2::RGB_ENABLE + 4].fill(1);
}

pub fn reset_keymaps(p: &mut Profile) {
    for k in 0..KEY_SLOTS {
        p.set_key_map(k, KEYMAP[k]);
        p.set_key_map_fn(k, KEYMAP_FN[k]);
    }
}

/// Thresholds, dead zones, the two unknown 100-arrays and `key_mode` (WASD rapid trigger if
/// `index` = 0).
pub fn reset_actuation(p: &mut Profile, index: usize) {
    for k in 0..KEY_SLOTS {
        p.set_press_th(k, 50);
        p.set_return_th(k, 30);
        p.set_norm_press_th(k, 1600);
        p.set_norm_return_th(k, 800);
        p.set_press_deadzone(k, 55);
        p.set_return_deadzone(k, 0);
        wr_u16(&mut p.0, b2::UNK_8F0 + 2 * k, 100);
        wr_u16(&mut p.0, b2::UNK_9A0 + 2 * k, 100);
        p.set_key_mode_raw(k, 0);
    }
    if index == 0 {
        for k in WASD {
            p.set_key_mode_raw(k, 1);
        }
    }
}

pub fn reset_cancel(p: &mut Profile) {
    for k in 0..KEY_SLOTS {
        p.set_cancel_code(k, KC_NONE);
    }
}

/// Extra layers A/B = 1000, their companion bytes 0.
pub fn reset_layers(p: &mut Profile) {
    p.0[b2::LAYER_A_EXT..b2::LAYER_A_EXT + KEY_SLOTS].fill(0);
    p.0[b2::LAYER_B_EXT..b2::LAYER_B_EXT + KEY_SLOTS].fill(0);
    for k in 0..KEY_SLOTS {
        p.set_layer_a(k, KC_NONE);
        p.set_layer_b(k, KC_NONE);
    }
}

/// Bind modes 0, every record: actions 0, keys 1000×4, top 100, bottom 3000, 8/24/16, byte 47 = 1
/// (bytes 34..46 untouched).
pub fn reset_binds(p: &mut Profile) {
    p.0[b2::BIND_KEY_MODE..b2::BIND_KEY_MODE + KEY_SLOTS].fill(0);
    for k in 0..KEY_SLOTS {
        let r = p.bind_record_mut(k);
        r[..16].fill(0);
        for i in 0..4 {
            wr_u16(r, 16 + 2 * i, KC_NONE as u16);
        }
        for (o, v) in [(24, 100), (26, 3000), (28, 8), (30, 24), (32, 16)] {
            wr_u16(r, o, v);
        }
        r[47] = 1;
    }
}

/// `BE EE` / keycode 305 (`FUN_0000F128`): every bank-2 field of the active profile back to
/// its default. Profiles 1–3 also get effect 23 here (stock); WASD is rapid trigger only if
/// `index` = 0. Bank 1 is untouched.
pub fn reset_profile(p: &mut Profile, index: usize) {
    reset_lighting(p);
    reset_keymaps(p);
    reset_layers(p);
    reset_actuation(p, index);
    reset_cancel(p);
    reset_binds(p);
    p.0[b2::WIN_LOCK_STATE] = 1;
    p.0[b2::AUTO_CALIB_ENABLE] = 1;
    wr_u32(&mut p.0, b2::SLEEP_SECOND_CNT, 0);
    p.apply_fixed_fn(index);
}

