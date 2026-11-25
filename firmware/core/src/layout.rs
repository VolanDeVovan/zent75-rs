//! Physical layout: key indices, scan slots and LEDs (docs/hardware.md, panel/src/protocol/layout.ts).

/// Number of Hall-effect keys. Key index runs row by row: Esc = 0 … Right = 82.
pub const KEY_COUNT: usize = 83;
/// Scan slots per full scan: 11 mux steps × 8 ADC inputs.
pub const SLOT_COUNT: usize = 88;
/// Mux steps per full scan.
pub const STEP_COUNT: usize = 11;
/// ADC inputs sampled per step.
pub const INPUTS_PER_STEP: usize = 8;
/// WS2812 LEDs in the chain.
pub const LED_COUNT: usize = 85;
/// Profiles stored on the board.
pub const PROFILE_COUNT: usize = 4;
/// Full scans per second.
pub const SCAN_HZ: u32 = 8000;

/// Value meaning "no key" in [`SLOT_TO_KEY`].
pub const NO_KEY: u8 = 0xFF;

/// Scan slot (step × 8 + input position) → key index, from the stock table @ 0x80040528.
/// Entries 83..=87 in the stock table are spare slots; they map to [`NO_KEY`] here.
pub const SLOT_TO_KEY: [u8; SLOT_COUNT] = {
    const RAW: [u8; SLOT_COUNT] = [
        76, 53, 81, 27, 20, 17, 59, 49, //
        77, 52, 71, 26, 21, 16, 73, 61, //
        78, 64, 82, 25, 22, 15, 74, 62, //
        79, 55, 72, 29, 23, 19, 75, 63, //
        80, 54, 58, 28, 24, 18, 60, 50, //
        67, 39, 44, 14, 83, 3, 48, 36, //
        68, 40, 41, 13, 6, 4, 47, 35, //
        69, 38, 42, 12, 7, 0, 46, 32, //
        70, 37, 43, 11, 8, 1, 31, 33, //
        65, 51, 56, 10, 5, 2, 30, 34, //
        66, 84, 57, 9, 85, 86, 45, 87, //
    ];
    let mut out = [NO_KEY; SLOT_COUNT];
    let mut i = 0;
    while i < SLOT_COUNT {
        if (RAW[i] as usize) < KEY_COUNT {
            out[i] = RAW[i];
        }
        i += 1;
    }
    out
};

/// Key index → scan slot (inverse of [`SLOT_TO_KEY`]).
pub const KEY_TO_SLOT: [u8; KEY_COUNT] = {
    let mut out = [0u8; KEY_COUNT];
    let mut s = 0;
    while s < SLOT_COUNT {
        let k = SLOT_TO_KEY[s];
        if k != NO_KEY {
            out[k as usize] = s as u8;
        }
        s += 1;
    }
    out
};

/// Key index → first LED index. The space bar (key 76) owns LEDs 3, 4 and 5.
pub const KEY_TO_LED: [u8; KEY_COUNT] = [
    84, 83, 82, 81, 80, 79, 78, 77, 76, 75, 74, 73, 72, 71, 70, //
    55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, //
    54, 53, 52, 51, 50, 49, 48, 47, 46, 45, 44, 43, 42, 41, 40, //
    26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, //
    25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14, 13, 12, //
    0, 1, 2, 3, 6, 7, 8, 9, 10, 11, //
];

/// Key index of the space bar.
pub const KEY_SPACE: usize = 76;

/// All LEDs of a key (1 or 3 for the space bar).
pub fn leds_of_key(key: usize) -> &'static [u8] {
    const SPACE: [u8; 3] = [3, 4, 5];
    if key == KEY_SPACE {
        &SPACE
    } else {
        core::slice::from_ref(&KEY_TO_LED[key])
    }
}

/// LED index → key index (the three space LEDs map to the space bar).
pub const LED_TO_KEY: [u8; LED_COUNT] = {
    let mut out = [NO_KEY; LED_COUNT];
    let mut k = 0;
    while k < KEY_COUNT {
        out[KEY_TO_LED[k] as usize] = k as u8;
        k += 1;
    }
    out[4] = KEY_SPACE as u8;
    out[5] = KEY_SPACE as u8;
    out
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_tables_are_inverse() {
        let mut seen = [false; KEY_COUNT];
        for (s, &k) in SLOT_TO_KEY.iter().enumerate() {
            if k != NO_KEY {
                assert!(!seen[k as usize], "key {k} twice");
                seen[k as usize] = true;
                assert_eq!(KEY_TO_SLOT[k as usize] as usize, s);
            }
        }
        assert!(seen.iter().all(|&b| b));
    }

    #[test]
    fn stock_key_to_slot_table_matches() {
        // 0x80040580 in the stock image.
        const STOCK: [u8; KEY_COUNT] = [
            61, 69, 77, 45, 53, 76, 52, 60, 68, 83, 75, 67, 59, 51, 43, 21, 13, 5, 37, 29, 4, 12,
            20, 28, 36, 19, 11, 3, 35, 27, 78, 70, 63, 71, 79, 55, 47, 65, 57, 41, 49, 50, 58, 66,
            42, 86, 62, 54, 46, 7, 39, 73, 9, 1, 33, 25, 74, 82, 34, 6, 38, 15, 23, 31, 17, 72,
            80, 40, 48, 56, 64, 10, 26, 14, 22, 30, 0, 8, 16, 24, 32, 2, 18,
        ];
        assert_eq!(KEY_TO_SLOT, STOCK);
    }

    #[test]
    fn leds_cover_chain() {
        let mut seen = [false; LED_COUNT];
        for k in 0..KEY_COUNT {
            for &l in leds_of_key(k) {
                assert!(!seen[l as usize]);
                seen[l as usize] = true;
            }
        }
        assert!(seen.iter().all(|&b| b));
        assert!(LED_TO_KEY.iter().all(|&k| k != NO_KEY));
    }
}
