/**
 * Custom switch curves (board-global records 0..3, selected by keytype_code 4095..4092): factory
 * defaults, validation as the firmware does it, and helpers for editing the curve.
 *
 * How the firmware uses a record (firmware/core/src/engine/curves.rs): point i of an n-point
 * curve is the normalised sensor position f (0 = rest … 2636 = full calibrated swing) at which the
 * key has travelled (i − 1) · 50 µm; the last point is the full travel. Heights between points are
 * interpolated linearly. A record that fails validation is ignored at run time (the key falls back
 * to the type-0 curve) and replaced by the factory record at the next boot.
 */
import {
  type CustomSwitch, type Rgb,
  CURVE_LAST, CURVE_MAX_POINTS, CURVE_STEP_UM, CUSTOM_SWITCH_CODE_BASE, CUSTOM_SWITCH_COUNT,
} from "./keyboard";

/** Factory curve of every record (firmware `CUSTOM_SWITCH_CURVE`, 71 points for 3.42 mm). */
export const DEFAULT_CURVE: readonly number[] = [
  -12, 0, 12, 25, 39, 50, 65, 78, 91, 108, 121, 136, 151, 166, 184, 200, 217, 235, 254, 271, 289, 308,
  329, 350, 371, 394, 416, 441, 463, 492, 516, 539, 565, 592, 621, 649, 681, 713, 746, 777, 818, 851,
  886, 922, 963, 1003, 1044, 1088, 1134, 1181, 1227, 1280, 1325, 1387, 1440, 1499, 1554, 1620, 1682,
  1751, 1820, 1896, 1966, 2046, 2124, 2221, 2304, 2394, 2488, 2594, 2636,
];
export const DEFAULT_TRAVEL_UM = 3420;
const DEFAULT_COLORS: readonly Rgb[] = [
  { r: 0xff, g: 0x00, b: 0xff }, { r: 0x00, g: 0xff, b: 0xff }, { r: 0x30, g: 0xff, b: 0x9f }, { r: 0x67, g: 0x26, b: 0xff },
];
/** Factory name prefix ("custom switch"); the record number 1..4 follows. */
export const DEFAULT_NAME_PREFIX = "自定义轴体";

export const TRAVEL_MIN_UM = 1500;
export const TRAVEL_MAX_UM = 4200;

/** The record the firmware writes for slot `i` when it finds an invalid one. */
export function defaultCustomSwitch(i: number): CustomSwitch {
  return {
    name: `${DEFAULT_NAME_PREFIX}${i + 1}`,
    travelUm: DEFAULT_TRAVEL_UM,
    curve: [...DEFAULT_CURVE],
    color: { ...DEFAULT_COLORS[i % DEFAULT_COLORS.length] },
    wordF8: 6000,
    byteFC: 0,
    spare: new Array(2 * (CURVE_MAX_POINTS - DEFAULT_CURVE.length)).fill(0xff),
  };
}

export const customSwitchCode = (i: number): number => CUSTOM_SWITCH_CODE_BASE - i;

/** Record index 0..3 selected by a keytype_code, or null for a built-in type. */
export function customSwitchIndex(code: number): number | null {
  const i = CUSTOM_SWITCH_CODE_BASE - code;
  return i >= 0 && i < CUSTOM_SWITCH_COUNT ? i : null;
}

/** Travel (µm) of curve point `i` of an `n`-point curve on a switch with `travelUm` full travel. */
export function pointTravelUm(i: number, n: number, travelUm: number): number {
  return i === n - 1 ? travelUm : CURVE_STEP_UM * i - CURVE_STEP_UM;
}

/** Point count that spaces the points 50 µm apart up to the full travel (71 for 3.42 mm). */
export function pointsForTravel(travelUm: number): number {
  return Math.min(CURVE_MAX_POINTS, Math.floor((travelUm - 1 + CURVE_STEP_UM) / CURVE_STEP_UM) + 2);
}

/**
 * Problems that make the firmware reject the record (stock `FUN_000148F8` + the curve loader),
 * plus one sanity rule of ours: the second-to-last point must lie above the full travel, or the
 * heights the firmware derives from the points stop decreasing.
 */
export function validateCustomSwitch(sw: CustomSwitch): string[] {
  const e: string[] = [];
  const n = sw.curve.length;
  if (!Number.isInteger(sw.travelUm) || sw.travelUm < TRAVEL_MIN_UM || sw.travelUm > TRAVEL_MAX_UM) {
    e.push(`Travel must be ${TRAVEL_MIN_UM}..${TRAVEL_MAX_UM} µm.`);
  }
  if (n < 2 || n > CURVE_MAX_POINTS) e.push(`The curve needs 2..${CURVE_MAX_POINTS} points (has ${n}).`);
  if (sw.curve.some((v) => !Number.isInteger(v) || v < -32768 || v > 32767)) e.push("Curve points must be integers (i16).");
  else {
    if (n > 0 && sw.curve[n - 1] !== CURVE_LAST) e.push(`The last point must be ${CURVE_LAST}.`);
    const drop = sw.curve.findIndex((v, i) => i > 0 && v < sw.curve[i - 1]);
    if (drop > 0) e.push(`The curve must not decrease (point ${drop}: ${sw.curve[drop]} < ${sw.curve[drop - 1]}).`);
  }
  if (n >= 2 && pointTravelUm(n - 2, n, sw.travelUm) >= sw.travelUm) {
    e.push(`Too many points for ${sw.travelUm} µm: point ${n - 2} would sit at ${pointTravelUm(n - 2, n, sw.travelUm)} µm (use at most ${pointsForTravel(sw.travelUm)}).`);
  }
  const name = new TextEncoder().encode(sw.name);
  if (name.length === 0 || name.length > 63) e.push("Name must be 1..63 bytes of UTF-8.");
  if (sw.wordF8 === 0xffff || !Number.isInteger(sw.wordF8) || sw.wordF8 < 0 || sw.wordF8 > 0xffffffff) e.push("Word 0xF8 must not be 0xFFFF.");
  if (sw.byteFC === 0xff || !Number.isInteger(sw.byteFC) || sw.byteFC < 0 || sw.byteFC > 0xff) e.push("Byte 0xFC must not be 0xFF.");
  return e;
}

/**
 * The curve reshaped to `n` points for a switch of `travelUm`: the f-vs-relative-travel shape is
 * kept (linear interpolation), the first point stays where it was and the last is CURVE_LAST.
 */
export function resampleCurve(curve: readonly number[], oldTravelUm: number, travelUm: number, n = pointsForTravel(travelUm)): number[] {
  const m = curve.length;
  if (m < 2) throw new RangeError("curve needs at least 2 points");
  const xs = curve.map((_, i) => pointTravelUm(i, m, oldTravelUm) / oldTravelUm);
  const out: number[] = [];
  for (let j = 0; j < n; j++) {
    const x = pointTravelUm(j, n, travelUm) / travelUm;
    let k = 0;
    while (k < m - 2 && xs[k + 1] < x) k++;
    const t = (x - xs[k]) / (xs[k + 1] - xs[k]);
    out.push(Math.round(curve[k] + Math.max(0, Math.min(1, t)) * (curve[k + 1] - curve[k])));
  }
  out[n - 1] = CURVE_LAST;
  for (let j = 1; j < n; j++) if (out[j] < out[j - 1]) out[j] = out[j - 1];
  return out;
}

/** `sw` with a new curve; the spare bytes behind it are cut or padded with 0xFF to fit the record. */
export function withCurve(sw: CustomSwitch, curve: number[]): CustomSwitch {
  const room = 2 * (CURVE_MAX_POINTS - curve.length);
  const spare = sw.spare.slice(0, Math.max(0, room));
  while (spare.length < room) spare.push(0xff);
  return { ...sw, curve, spare };
}

/** Parse curve text: integers separated by commas, spaces or new lines. */
export function parseCurve(text: string): number[] {
  const parts = text.split(/[\s,;]+/).filter((p) => p.length > 0);
  const out = parts.map(Number);
  const bad = out.findIndex((v) => !Number.isInteger(v));
  if (bad >= 0) throw new RangeError(`"${parts[bad]}" is not an integer`);
  return out;
}
