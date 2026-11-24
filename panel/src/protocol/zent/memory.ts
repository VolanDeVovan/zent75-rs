/**
 * Memory map of the stock firmware and the codec between raw region images and the
 * protocol-neutral config model.
 *
 * The addresses are the ones the vendor driver calls "RAM" (BE 03 / BE 04). 0x8007xxxx is
 * board-global, 0x8008xxxx belongs to the active profile and is swapped by BE C0.
 */
import {
  type AdvancedKey, type Calibration, type Lighting, type ProfileConfig, type Rgb,
  ADV_NONE, KEYCODE_EMPTY,
} from "../keyboard";
import { KEY_COUNT, LED_COUNT } from "../layout";

export interface Region {
  name: string;
  address: number;
  length: number;
  /** element size: writes are aligned to whole elements, like the vendor driver does */
  unit: number;
  writable: boolean;
}

const region = (name: string, address: number, length: number, unit: number, writable = true): Region => ({
  name, address, length, unit, writable,
});

export const BIND_RECORD_SIZE = 48;
export const PROFILE_NAME_SIZE = 64;
export const PROFILE_COUNT = 4;

export const REGIONS = {
  // ---- board-global
  top_value: region("top_value", 0x80070000, 2 * KEY_COUNT, 2),
  bot_value: region("bot_value", 0x800700b0, 2 * KEY_COUNT, 2),
  keytype_code: region("keytype_code", 0x80070160, 2 * KEY_COUNT, 2),
  soft_version: region("soft_version", 0x80070210, 4, 4, false),
  cur_param_num: region("cur_param_num", 0x80070218, 4, 4, false),
  config_name: region("config_name", 0x80070290, PROFILE_NAME_SIZE * PROFILE_COUNT, PROFILE_NAME_SIZE),
  // ---- active profile
  key_map: region("key_map", 0x80080160, 2 * KEY_COUNT, 2),
  key_map_fn: region("key_map_fn", 0x80080210, 2 * KEY_COUNT, 2),
  press_th: region("press_th", 0x800802c0, 2 * KEY_COUNT, 2),
  return_th: region("return_th", 0x80080370, 2 * KEY_COUNT, 2),
  press_deadzone: region("press_deadzone", 0x80080420, 2 * KEY_COUNT, 2),
  return_deadzone: region("return_deadzone", 0x800804d0, 2 * KEY_COUNT, 2),
  norm_press_th: region("norm_press_th", 0x80080580, 2 * KEY_COUNT, 2),
  norm_return_th: region("norm_return_th", 0x80080630, 2 * KEY_COUNT, 2),
  key_mode: region("key_mode", 0x800806e0, KEY_COUNT, 1),
  key_cancel_code: region("key_cancel_code", 0x80080a50, 2 * KEY_COUNT, 2),
  rgb: region("rgb", 0x80080b00, 3 * LED_COUNT, 3),
  rgb_solid_enable: region("rgb_solid_enable", 0x80080c08, LED_COUNT, 1),
  rgb_config: region("rgb_config", 0x80080c60, 5, 5),
  rgb_enable: region("rgb_enable", 0x80080c68, 1, 1),
  cap_enable: region("cap_enable", 0x80080c69, 1, 1),
  rgb_fn_enable: region("rgb_fn_enable", 0x80080c6b, 1, 1),
  sleep_second_cnt: region("sleep_second_cnt", 0x80080c6c, 4, 4),
  win_lock_state: region("win_lock_state", 0x80080c70, 1, 1),
  auto_calib_enable: region("auto_calib_enable", 0x80080c71, 1, 1),
  // ---- firmware >= 0.0.18 only
  bind_key_setting: region("bind_key_setting", 0x80080d00, BIND_RECORD_SIZE * KEY_COUNT, BIND_RECORD_SIZE),
  bind_key_mode: region("bind_key_mode", 0x80081de0, KEY_COUNT, 1),
} as const satisfies Record<string, Region>;

export type RegionName = keyof typeof REGIONS;
export type Images = Partial<Record<RegionName, Uint8Array>>;

/** Regions that make up a ProfileConfig. */
export const PROFILE_REGIONS: readonly RegionName[] = [
  "key_map", "key_map_fn", "press_th", "return_th", "press_deadzone", "return_deadzone",
  "norm_press_th", "norm_return_th", "key_mode", "key_cancel_code",
  "rgb", "rgb_solid_enable", "rgb_config", "rgb_enable", "cap_enable", "rgb_fn_enable",
  "sleep_second_cnt", "win_lock_state", "auto_calib_enable",
];
export const ADVANCED_REGIONS: readonly RegionName[] = ["bind_key_mode", "bind_key_setting"];
export const CALIBRATION_REGIONS: readonly RegionName[] = ["top_value", "bot_value", "keytype_code"];

/** First firmware (numeric version) that has bind_key_setting / bind_key_mode. */
export const ADVANCED_KEYS_MIN_VERSION = 18;

/** Is [address, address+length) completely inside one known region? */
export function findRegion(address: number, length: number): Region | undefined {
  for (const r of Object.values(REGIONS) as Region[]) {
    if (address >= r.address && address + length <= r.address + r.length) return r;
  }
  return undefined;
}

// ---------------------------------------------------------------- scalar helpers

export function u16le(bytes: ArrayLike<number>): number[] {
  const out: number[] = [];
  for (let i = 0; i + 1 < bytes.length; i += 2) out.push(bytes[i] | (bytes[i + 1] << 8));
  return out;
}

export function i16le(bytes: ArrayLike<number>): number[] {
  return u16le(bytes).map((v) => (v & 0x8000 ? v - 0x10000 : v));
}

export function packU16(values: ArrayLike<number>, what = "value"): Uint8Array {
  const out = new Uint8Array(values.length * 2);
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    if (!Number.isInteger(v) || v < 0 || v > 0xffff) throw new RangeError(`${what}[${i}] = ${v} is not a u16`);
    out[2 * i] = v & 0xff;
    out[2 * i + 1] = v >> 8;
  }
  return out;
}

export function packI16(values: ArrayLike<number>, what = "value"): Uint8Array {
  const out = new Uint8Array(values.length * 2);
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    if (!Number.isInteger(v) || v < -0x8000 || v > 0x7fff) throw new RangeError(`${what}[${i}] = ${v} is not an i16`);
    out[2 * i] = v & 0xff;
    out[2 * i + 1] = (v >> 8) & 0xff;
  }
  return out;
}

function packU8(values: ArrayLike<number>, what: string): Uint8Array {
  const out = new Uint8Array(values.length);
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    if (!Number.isInteger(v) || v < 0 || v > 0xff) throw new RangeError(`${what}[${i}] = ${v} is not a byte`);
    out[i] = v;
  }
  return out;
}

function expectLength(values: ArrayLike<unknown>, n: number, what: string): void {
  if (!values || values.length !== n) throw new RangeError(`${what} must have ${n} entries, got ${values ? values.length : "none"}`);
}

// ---------------------------------------------------------------- small fields

export interface FirmwareVersion { major: number; minor: number; patch: number; text: string; number: number }

/** soft_version: [patch, minor, major, x]; the driver prints `major.minor.patch`. */
export function decodeVersion(bytes: ArrayLike<number>): FirmwareVersion {
  const patch = bytes[0], minor = bytes[1], major = bytes[2];
  return { major, minor, patch, text: `${major}.${minor}.${patch}`, number: (major << 16) + (minor << 8) + patch };
}

export function encodeVersion(major: number, minor: number, patch: number): Uint8Array {
  return Uint8Array.of(patch, minor, major, 0);
}

/** Profile names: UTF-8, zero padded to 64 bytes. An all-zero slot means "not named". */
export function decodeProfileName(bytes: Uint8Array, index: number): string {
  const nonZero = bytes.filter((b) => b !== 0);
  if (nonZero.length === 0) return `Config${index + 1}`;
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(nonZero);
  } catch {
    return `Config${index + 1}`;
  }
}

export function encodeProfileName(name: string): Uint8Array {
  const utf8 = new TextEncoder().encode(name);
  if (utf8.length === 0) throw new RangeError("profile name must not be empty");
  if (utf8.length > PROFILE_NAME_SIZE - 1) throw new RangeError(`profile name is longer than ${PROFILE_NAME_SIZE - 1} bytes`);
  const out = new Uint8Array(PROFILE_NAME_SIZE);
  out.set(utf8);
  return out;
}

// ---------------------------------------------------------------- bind_key_setting (48 bytes)

/*
 *  0..15  4 slots x 4 phases, DKS action bytes (0 none, 1 hold until next phase, 2 tap)
 * 16..23  4 x i16 key codes (combo members / DKS slot keys), 1000 = empty
 * 24..25  u16 DKS top trigger point, µm
 * 26..27  u16 DKS bottom trigger point, µm
 * 28..29  u16 tap duration   (UI value * 8)
 * 30..31  u16 delay          (UI value * 8)
 * 32..33  u16 random delay   (UI value * 8)
 * 34..46  13 bytes, written as 0xFF
 * 47      "trigger the original key" switch of the vendor UI, inverted: 0 = on, 1 = off (default)
 */
export function decodeAdvancedKey(rec: ArrayLike<number>, mode: number): AdvancedKey {
  if (rec.length !== BIND_RECORD_SIZE) throw new RangeError(`bind record must be ${BIND_RECORD_SIZE} bytes`);
  const b = Array.from(rec);
  const actions: number[][] = [];
  for (let s = 0; s < 4; s++) actions.push(b.slice(4 * s, 4 * s + 4));
  const w = u16le(b.slice(24, 34));
  return {
    mode,
    actions,
    keys: i16le(b.slice(16, 24)),
    dksTopUm: w[0],
    dksBottomUm: w[1],
    tapTicks: w[2],
    delayTicks: w[3],
    randomTicks: w[4],
    reserved: b.slice(34, 47),
    suppressOriginal: b[47],
  };
}

export function encodeAdvancedKey(key: AdvancedKey): Uint8Array {
  const out = new Uint8Array(BIND_RECORD_SIZE);
  expectLength(key.actions, 4, "actions");
  for (let s = 0; s < 4; s++) {
    expectLength(key.actions[s], 4, `actions[${s}]`);
    out.set(packU8(key.actions[s], "action"), 4 * s);
  }
  expectLength(key.keys, 4, "keys");
  out.set(packI16(key.keys, "bind key"), 16);
  out.set(packU16([key.dksTopUm, key.dksBottomUm, key.tapTicks, key.delayTicks, key.randomTicks], "dks"), 24);
  expectLength(key.reserved, 13, "reserved");
  out.set(packU8(key.reserved, "reserved"), 34);
  out[47] = packU8([key.suppressOriginal], "suppressOriginal")[0];
  return out;
}

/** An unused record as firmware 0.0.18 holds it after a config reset (read from a real board). */
export function emptyAdvancedKey(): AdvancedKey {
  return {
    mode: ADV_NONE,
    actions: [[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
    keys: [KEYCODE_EMPTY, KEYCODE_EMPTY, KEYCODE_EMPTY, KEYCODE_EMPTY],
    dksTopUm: 100,
    dksBottomUm: 3000,
    tapTicks: 8,
    delayTicks: 24,
    randomTicks: 16,
    reserved: new Array(13).fill(0xff),
    suppressOriginal: 1,
  };
}

// ---------------------------------------------------------------- profile config <-> images

function need(images: Images, name: RegionName): Uint8Array {
  const img = images[name];
  if (!img) throw new Error(`region ${name} is missing`);
  if (img.length !== REGIONS[name].length) throw new Error(`region ${name}: expected ${REGIONS[name].length} bytes, got ${img.length}`);
  return img;
}

export function decodeProfile(images: Images): ProfileConfig {
  const rgbBytes = need(images, "rgb");
  const colors: Rgb[] = [];
  for (let i = 0; i < LED_COUNT; i++) colors.push({ r: rgbBytes[3 * i], g: rgbBytes[3 * i + 1], b: rgbBytes[3 * i + 2] });
  const cfg = need(images, "rgb_config");
  const lighting: Lighting = {
    enabled: need(images, "rgb_enable")[0] === 1,
    capsIndicator: need(images, "cap_enable")[0] === 1,
    fnLayerLighting: need(images, "rgb_fn_enable")[0] === 1,
    effect: cfg[0] >> 2,
    effectFlags: cfg[0] & 3,
    hue: cfg[1],
    saturation: cfg[2],
    brightness: cfg[3],
    speed: cfg[4],
    colors,
    solid: Array.from(need(images, "rgb_solid_enable")),
  };
  const sleep = need(images, "sleep_second_cnt");

  let advanced: AdvancedKey[] | null = null;
  if (images.bind_key_mode && images.bind_key_setting) {
    const modes = need(images, "bind_key_mode");
    const recs = need(images, "bind_key_setting");
    advanced = [];
    for (let k = 0; k < KEY_COUNT; k++) {
      advanced.push(decodeAdvancedKey(recs.subarray(k * BIND_RECORD_SIZE, (k + 1) * BIND_RECORD_SIZE), modes[k]));
    }
  }

  return {
    keymap: { main: i16le(need(images, "key_map")), fn: i16le(need(images, "key_map_fn")) },
    keyMode: Array.from(need(images, "key_mode")),
    actuationUm: u16le(need(images, "norm_press_th")),
    releaseUm: u16le(need(images, "norm_return_th")),
    rtPressUm: u16le(need(images, "press_th")),
    rtReleaseUm: u16le(need(images, "return_th")),
    topDeadzoneUm: u16le(need(images, "press_deadzone")),
    bottomDeadzoneUm: u16le(need(images, "return_deadzone")),
    socd: i16le(need(images, "key_cancel_code")),
    advanced,
    lighting,
    sleepSeconds: (sleep[0] | (sleep[1] << 8) | (sleep[2] << 16) | (sleep[3] << 24)) >>> 0,
    winLock: need(images, "win_lock_state")[0] === 1,
    autoCalibration: need(images, "auto_calib_enable")[0] === 1,
  };
}

export function encodeProfile(config: ProfileConfig): Images {
  const perKey = (values: number[], what: string): number[] => {
    expectLength(values, KEY_COUNT, what);
    return values;
  };
  const l = config.lighting;
  expectLength(l.colors, LED_COUNT, "lighting.colors");
  expectLength(l.solid, LED_COUNT, "lighting.solid");
  const rgb = new Uint8Array(3 * LED_COUNT);
  l.colors.forEach((c, i) => rgb.set(packU8([c.r, c.g, c.b], `lighting.colors[${i}]`), 3 * i));
  if (!Number.isInteger(l.effect) || l.effect < 0 || l.effect > 63) throw new RangeError(`lighting.effect ${l.effect} out of range 0..63`);
  if (!Number.isInteger(l.effectFlags) || l.effectFlags < 0 || l.effectFlags > 3) throw new RangeError("lighting.effectFlags out of range");
  const s = config.sleepSeconds;
  if (!Number.isInteger(s) || s < 0 || s > 0xffffffff) throw new RangeError(`sleepSeconds ${s} out of range`);

  const images: Images = {
    key_map: packI16(perKey(config.keymap.main, "keymap.main"), "keymap.main"),
    key_map_fn: packI16(perKey(config.keymap.fn, "keymap.fn"), "keymap.fn"),
    key_mode: packU8(perKey(config.keyMode, "keyMode"), "keyMode"),
    norm_press_th: packU16(perKey(config.actuationUm, "actuationUm"), "actuationUm"),
    norm_return_th: packU16(perKey(config.releaseUm, "releaseUm"), "releaseUm"),
    press_th: packU16(perKey(config.rtPressUm, "rtPressUm"), "rtPressUm"),
    return_th: packU16(perKey(config.rtReleaseUm, "rtReleaseUm"), "rtReleaseUm"),
    press_deadzone: packU16(perKey(config.topDeadzoneUm, "topDeadzoneUm"), "topDeadzoneUm"),
    return_deadzone: packU16(perKey(config.bottomDeadzoneUm, "bottomDeadzoneUm"), "bottomDeadzoneUm"),
    key_cancel_code: packI16(perKey(config.socd, "socd"), "socd"),
    rgb,
    rgb_solid_enable: packU8(l.solid, "lighting.solid"),
    rgb_config: packU8([(l.effect << 2) | l.effectFlags, l.hue, l.saturation, l.brightness, l.speed], "rgb_config"),
    rgb_enable: Uint8Array.of(l.enabled ? 1 : 0),
    cap_enable: Uint8Array.of(l.capsIndicator ? 1 : 0),
    rgb_fn_enable: Uint8Array.of(l.fnLayerLighting ? 1 : 0),
    sleep_second_cnt: Uint8Array.of(s & 0xff, (s >>> 8) & 0xff, (s >>> 16) & 0xff, (s >>> 24) & 0xff),
    win_lock_state: Uint8Array.of(config.winLock ? 1 : 0),
    auto_calib_enable: Uint8Array.of(config.autoCalibration ? 1 : 0),
  };

  if (config.advanced) {
    expectLength(config.advanced, KEY_COUNT, "advanced");
    const modes = new Uint8Array(KEY_COUNT);
    const recs = new Uint8Array(KEY_COUNT * BIND_RECORD_SIZE);
    config.advanced.forEach((a, k) => {
      modes[k] = packU8([a.mode], "advanced.mode")[0];
      recs.set(encodeAdvancedKey(a), k * BIND_RECORD_SIZE);
    });
    images.bind_key_mode = modes;
    images.bind_key_setting = recs;
  }
  return images;
}

export function decodeCalibration(images: Images): Calibration {
  return {
    top: u16le(need(images, "top_value")),
    bottom: u16le(need(images, "bot_value")),
    switchType: i16le(need(images, "keytype_code")),
  };
}

export function encodeCalibration(cal: Calibration): Images {
  expectLength(cal.top, KEY_COUNT, "calibration.top");
  expectLength(cal.bottom, KEY_COUNT, "calibration.bottom");
  expectLength(cal.switchType, KEY_COUNT, "calibration.switchType");
  return {
    top_value: packU16(cal.top, "calibration.top"),
    bot_value: packU16(cal.bottom, "calibration.bottom"),
    keytype_code: packI16(cal.switchType, "calibration.switchType"),
  };
}

// ---------------------------------------------------------------- diffing

export interface WriteRun { address: number; data: Uint8Array; region: RegionName; offset: number }

/**
 * Minimal list of writes that turns `before` into `after`. Runs are aligned to the region's
 * element size and split so that no frame carries more than `maxChunk` bytes.
 */
export function diffRegion(name: RegionName, before: Uint8Array | undefined, after: Uint8Array, maxChunk: number): WriteRun[] {
  const r: Region = REGIONS[name];
  if (after.length !== r.length) throw new Error(`region ${name}: expected ${r.length} bytes, got ${after.length}`);
  const unit = r.unit;
  const perChunk = Math.max(unit, Math.floor(maxChunk / unit) * unit);
  const runs: WriteRun[] = [];
  let start = -1;
  const flush = (end: number) => {
    for (let o = start; o < end; o += perChunk) {
      const e = Math.min(end, o + perChunk);
      runs.push({ address: r.address + o, data: after.slice(o, e), region: name, offset: o });
    }
    start = -1;
  };
  for (let o = 0; o < r.length; o += unit) {
    let changed = !before || before.length !== after.length;
    if (!changed) {
      for (let i = o; i < o + unit; i++) {
        if (before![i] !== after[i]) { changed = true; break; }
      }
    }
    if (changed && start < 0) start = o;
    if (!changed && start >= 0) flush(o);
  }
  if (start >= 0) flush(r.length);
  return runs;
}
