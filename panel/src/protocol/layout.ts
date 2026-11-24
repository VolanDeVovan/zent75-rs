/**
 * Physical layout of the 83-key ZENT75-HE (75 %), in key-index order (row by row).
 * Positions are in key units (1u = one alpha key). Widths come from the vendor driver's
 * CSS classes (keyBig1..6 = 2u, 1.5u, 1.75u, 2.25u, 1.25u, 6.25u).
 */
export interface KeyPos {
  index: number;
  x: number;
  y: number;
  w: number;
  /** legend of the factory keymap, used as a stable name for the physical key */
  name: string;
}

export const KEY_COUNT = 83;
export const LED_COUNT = 85;

const ROWS: { names: string[]; widths?: Record<number, number>; gapAfter?: Record<number, number> }[] = [
  {
    names: ["Esc", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "PrtSc", "Home"],
    gapAfter: { 0: 0.25, 4: 0.25, 8: 0.25, 12: 0.25 },
  },
  { names: ["`", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "-", "=", "Bksp", "Del"], widths: { 13: 2 } },
  { names: ["Tab", "Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P", "[", "]", "\\", "PgUp"], widths: { 0: 1.5, 13: 1.5 } },
  { names: ["Caps", "A", "S", "D", "F", "G", "H", "J", "K", "L", ";", "'", "Enter", "PgDn"], widths: { 0: 1.75, 12: 2.25 } },
  { names: ["LShift", "Z", "X", "C", "V", "B", "N", "M", ",", ".", "/", "RShift", "↑", "End"], widths: { 0: 2.25, 11: 1.75 } },
  {
    names: ["LCtrl", "LWin", "LAlt", "Space", "RAlt", "Fn", "RCtrl", "←", "↓", "→"],
    widths: { 0: 1.25, 1: 1.25, 2: 1.25, 3: 6.25 },
  },
];

export const KEY_LAYOUT: readonly KeyPos[] = (() => {
  const out: KeyPos[] = [];
  let index = 0;
  ROWS.forEach((row, r) => {
    let x = 0;
    row.names.forEach((name, i) => {
      const w = row.widths?.[i] ?? 1;
      // the function row sits a quarter unit above the rest
      out.push({ index: index++, x, y: r === 0 ? 0 : r + 0.25, w, name });
      x += w + (row.gapAfter?.[i] ?? 0);
    });
  });
  return out;
})();

export const LAYOUT_WIDTH = 16;
export const LAYOUT_HEIGHT = 6.25;

/**
 * Key index -> LED index into the `rgb` / `rgb_solid_enable` arrays. The LED chain snakes
 * from the bottom-left corner upwards; the space bar (key 76) has three LEDs.
 */
export const KEY_TO_LED: readonly (number | readonly number[])[] = [
  84, 83, 82, 81, 80, 79, 78, 77, 76, 75, 74, 73, 72, 71, 70,
  55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69,
  54, 53, 52, 51, 50, 49, 48, 47, 46, 45, 44, 43, 42, 41, 40,
  26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39,
  25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14, 13, 12,
  0, 1, 2, [3, 4, 5], 6, 7, 8, 9, 10, 11,
];

export function ledsOfKey(key: number): readonly number[] {
  const v = KEY_TO_LED[key];
  return typeof v === "number" ? [v] : v;
}

/** The LED whose colour represents the key in the UI (middle LED of the space bar). */
export function primaryLedOfKey(key: number): number {
  const leds = ledsOfKey(key);
  return leds[leds.length >> 1];
}

/** Factory main-layer keymap (the vendor driver's default table). */
export const DEFAULT_KEYMAP: readonly number[] = [
  41, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 74,
  53, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 45, 46, 42, 76,
  43, 20, 26, 8, 21, 23, 28, 24, 12, 18, 19, 47, 48, 49, 75,
  57, 4, 22, 7, 9, 10, 11, 13, 14, 15, 51, 52, 40, 78,
  -7, 29, 27, 6, 25, 5, 17, 16, 54, 55, 56, -3, 82, 77,
  -8, -5, -6, 44, -2, 401, -4, 80, 81, 79,
];

/**
 * Keys whose Fn-layer binding the vendor driver refuses to change ("fixed keys cannot be
 * remapped"): they carry the built-in Fn shortcuts (profile switch, lighting, ...).
 * We warn instead of blocking.
 */
export const FN_FIXED_KEYS: readonly number[] = [0, 28, 29, 43, 62, 71, 74, 80, 81, 82];

/** Quick-selection groups of the vendor driver. */
export const QUICK_SELECT = {
  wasd: [32, 46, 47, 48],
  digits: [16, 17, 18, 19, 20, 21, 22, 23, 24, 25],
  letters: [31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 46, 47, 48, 49, 50, 51, 52, 53, 54, 60, 61, 62, 63, 64, 65, 66],
} as const;
