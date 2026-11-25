//! Factory defaults. PLACEHOLDER values until docs/re-config-rgb.md lands; the RE pass replaces
//! them with the stock tables.

use crate::config::{b1, b2, Global, Profile, KC_NONE, BIND_RECORD_SIZE};
use crate::layout::{KEY_COUNT, LED_COUNT};

/// Factory main layer (vendor driver table `j[0]`).
pub const KEYMAP: [i16; KEY_COUNT] = [
    41, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 74, //
    53, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 45, 46, 42, 76, //
    43, 20, 26, 8, 21, 23, 28, 24, 12, 18, 19, 47, 48, 49, 75, //
    57, 4, 22, 7, 9, 10, 11, 13, 14, 15, 51, 52, 40, 78, //
    -7, 29, 27, 6, 25, 5, 17, 16, 54, 55, 56, -3, 82, 77, //
    -8, -5, -6, 44, -2, 401, -4, 80, 81, 79, //
];

pub fn global(g: &mut Global) {
    g.0.fill(0);
    global_reset(g);
}

/// Bank-1 defaults applied by `BE EE` (calibration is kept? decided by RE).
pub fn global_reset(g: &mut Global) {
    for k in 0..KEY_COUNT {
        if g.top_value(k) == 0 {
            g.set_top_value(k, 31000);
            g.set_bot_value(k, 17000);
        }
        g.set_keytype(k, 2);
    }
    let _ = b1::CONFIG_NAME;
}

pub fn profile(p: &mut Profile, _index: usize) {
    p.0.fill(0);
    for k in 0..KEY_COUNT {
        p.set_key_map(k, KEYMAP[k]);
        p.set_key_map_fn(k, KC_NONE);
        p.set_press_th(k, 50);
        p.set_return_th(k, 30);
        p.set_norm_press_th(k, 1600);
        p.set_norm_return_th(k, 800);
        p.set_key_mode_raw(k, 0);
        p.set_cancel_code(k, KC_NONE);
        let r = p.bind_record_mut(k);
        r.fill(0);
        for i in 0..4 {
            r[16 + 2 * i..18 + 2 * i].copy_from_slice(&KC_NONE.to_le_bytes());
        }
        r[24..26].copy_from_slice(&100u16.to_le_bytes());
        r[26..28].copy_from_slice(&3000u16.to_le_bytes());
        r[28..30].copy_from_slice(&8u16.to_le_bytes());
        r[30..32].copy_from_slice(&24u16.to_le_bytes());
        r[32..34].copy_from_slice(&16u16.to_le_bytes());
        r[34..47].fill(0xFF);
        r[BIND_RECORD_SIZE - 1] = 1;
    }
    // Fn layer of the real board.
    for (k, v) in [(0, 319), (28, 315), (29, 320), (43, 313), (62, 308), (71, 311), (74, 316), (80, 318), (81, 312), (82, 317)] {
        p.set_key_map_fn(k, v);
    }
    for led in 0..LED_COUNT {
        p.0[b2::RGB + 3 * led..b2::RGB + 3 * led + 3].copy_from_slice(&[0, 0x80, 0xFF]);
    }
    p.0[b2::RGB_CONFIG..b2::RGB_CONFIG + 5].copy_from_slice(&[1 + 4 * 3, 0, 255, 10, 128]);
    p.0[b2::RGB_ENABLE] = 1;
    p.0[b2::CAP_ENABLE] = 1;
    p.0[b2::AUTO_CALIB_ENABLE] = 1;
}
