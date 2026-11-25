//! LED geometry of the board (stock DLM tables, QMK `g_led_config` layout).

use crate::layout::LED_COUNT;

/// No LED in this matrix cell.
pub const NO_LED: u8 = 255;
pub const MATRIX_ROWS: usize = 6;
pub const MATRIX_COLS: usize = 15;

/// Matrix cell → LED index (`0x80000`). Cells, not keys: the space bar owns three cells.
pub const MATRIX: [[u8; MATRIX_COLS]; MATRIX_ROWS] = [
    [84, 83, 82, 81, 80, 79, 78, 77, 76, 75, 74, 73, 72, 71, 70],
    [55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69],
    [54, 53, 52, 51, 50, 49, 48, 47, 46, 45, 44, 43, 42, 41, 40],
    [26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, NO_LED, 37, 38, 39],
    [NO_LED, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14, 13, 12],
    [0, 1, 2, 3, NO_LED, 4, NO_LED, 5, NO_LED, 6, 7, 8, 9, 10, 11],
];

/// Key → its matrix cells (`row·MATRIX_COLS + col`, row-major order; [`NO_LED`] = unused).
/// Three for the space bar, one for every other key.
pub const KEY_CELLS: [[u8; 3]; crate::layout::KEY_COUNT] = {
    let mut out = [[NO_LED; 3]; crate::layout::KEY_COUNT];
    let mut r = 0;
    while r < MATRIX_ROWS {
        let mut c = 0;
        while c < MATRIX_COLS {
            let led = MATRIX[r][c];
            if led != NO_LED {
                let k = crate::layout::LED_TO_KEY[led as usize] as usize;
                let mut j = 0;
                while out[k][j] != NO_LED {
                    j += 1;
                }
                out[k][j] = (r * MATRIX_COLS + c) as u8;
            }
            c += 1;
        }
        r += 1;
    }
    out
};

/// LED position (x, y) (`0x8005A`): x 0..120 left→right, y 0 (top row) .. 42 (bottom row).
pub const POINTS: [(u8, u8); LED_COUNT] = [
    (1, 42), (11, 42), (21, 42), (39, 42), (51, 42), (63, 42), (80, 42), (88, 42), (96, 42), (104, 42),
    (112, 42), (120, 42), (120, 34), (112, 34), (101, 34), (90, 34), (82, 34), (74, 34), (66, 34),
    (58, 34), (50, 34), (42, 34), (34, 34), (26, 34), (18, 34), (5, 34), (3, 26), (14, 26), (22, 26),
    (30, 26), (38, 26), (46, 26), (54, 26), (62, 26), (70, 26), (78, 26), (86, 26), (94, 26), (107, 26),
    (120, 26), (120, 18), (110, 18), (100, 18), (92, 18), (84, 18), (76, 18), (68, 18), (60, 18),
    (52, 18), (44, 18), (36, 18), (28, 18), (20, 18), (12, 18), (2, 18), (0, 10), (8, 10), (16, 10),
    (24, 10), (32, 10), (40, 10), (48, 10), (56, 10), (64, 10), (72, 10), (80, 10), (88, 10), (96, 10),
    (108, 10), (120, 10), (120, 0), (112, 0), (102, 0), (94, 0), (86, 0), (78, 0), (68, 0), (60, 0),
    (52, 0), (44, 0), (34, 0), (26, 0), (18, 0), (10, 0), (0, 0),
];

/// Effect centre (QMK `k_rgb_matrix_center`).
pub const CENTER: (u8, u8) = (60, 21);

/// QMK LED flag "keylight" — the only flag any LED on this board has (`0x80104`).
pub const FLAG_KEYLIGHT: u8 = 0x04;
pub const LED_FLAGS: [u8; LED_COUNT] = [FLAG_KEYLIGHT; LED_COUNT];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{LED_TO_KEY, KEY_TO_LED};

    #[test]
    fn matrix_covers_every_led_once() {
        let mut seen = [0; LED_COUNT];
        for row in MATRIX {
            for l in row {
                if l != NO_LED {
                    seen[l as usize] += 1;
                }
            }
        }
        assert!(seen.iter().all(|&n| n == 1));
    }

    #[test]
    fn stock_led_to_key_table() {
        // 0x80159 in the stock image
        const STOCK: [u8; LED_COUNT] = [
            73, 74, 75, 76, 76, 76, 77, 78, 79, 80, 81, 82, 72, 71, 70, 69, 68, 67, 66, 65, 64, 63, 62, 61, 60,
            59, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 44, 43, 42, 41, 40, 39, 38, 37, 36, 35,
            34, 33, 32, 31, 30, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 14, 13, 12, 11, 10, 9,
            8, 7, 6, 5, 4, 3, 2, 1, 0,
        ];
        assert_eq!(LED_TO_KEY, STOCK);
        assert_eq!(KEY_TO_LED[0], 84);
    }
}
