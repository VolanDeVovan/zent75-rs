/**
 * JSON import / export of a full configuration. Protocol-neutral: it stores the model from
 * protocol/keyboard.ts, not raw memory, so a file can be moved to another firmware later.
 */
import type { AdvancedKey, Calibration, DeviceInfo, ProfileConfig } from "./protocol/keyboard";

export const FILE_FORMAT = "zent75-he-config";
export const FILE_FORMAT_VERSION = 1;

export interface ProfileExport {
  index: number;
  name: string;
  config: ProfileConfig;
}

export interface ConfigFile {
  format: typeof FILE_FORMAT;
  formatVersion: number;
  exportedAt: string;
  device: Pick<DeviceInfo, "protocol" | "productName" | "firmwareVersion" | "serial" | "keyCount" | "ledCount">;
  activeProfile: number;
  profiles: ProfileExport[];
  /** board-global; only restored when the user asks for it */
  calibration?: Calibration;
}

export class ConfigFileError extends Error {}

function fail(path: string, message: string): never {
  throw new ConfigFileError(`${path}: ${message}`);
}

function intArray(value: unknown, path: string, length: number, min: number, max: number): number[] {
  if (!Array.isArray(value)) fail(path, "expected an array");
  if (value.length !== length) fail(path, `expected ${length} entries, got ${value.length}`);
  value.forEach((v, i) => {
    if (!Number.isInteger(v) || v < min || v > max) fail(`${path}[${i}]`, `expected an integer ${min}..${max}, got ${JSON.stringify(v)}`);
  });
  return value as number[];
}

function int(value: unknown, path: string, min: number, max: number): number {
  if (!Number.isInteger(value) || (value as number) < min || (value as number) > max) {
    fail(path, `expected an integer ${min}..${max}, got ${JSON.stringify(value)}`);
  }
  return value as number;
}

function bool(value: unknown, path: string): boolean {
  if (typeof value !== "boolean") fail(path, "expected true or false");
  return value;
}

function obj(value: unknown, path: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) fail(path, "expected an object");
  return value as Record<string, unknown>;
}

function validateAdvanced(value: unknown, path: string): AdvancedKey {
  const a = obj(value, path);
  if (!Array.isArray(a.actions) || a.actions.length !== 4) fail(`${path}.actions`, "expected 4 slots");
  return {
    mode: int(a.mode, `${path}.mode`, 0, 255),
    actions: (a.actions as unknown[]).map((s, i) => intArray(s, `${path}.actions[${i}]`, 4, 0, 255)),
    keys: intArray(a.keys, `${path}.keys`, 4, -32768, 32767),
    dksTopUm: int(a.dksTopUm, `${path}.dksTopUm`, 0, 65535),
    dksBottomUm: int(a.dksBottomUm, `${path}.dksBottomUm`, 0, 65535),
    tapTicks: int(a.tapTicks, `${path}.tapTicks`, 0, 65535),
    delayTicks: int(a.delayTicks, `${path}.delayTicks`, 0, 65535),
    randomTicks: int(a.randomTicks, `${path}.randomTicks`, 0, 65535),
    reserved: intArray(a.reserved, `${path}.reserved`, 13, 0, 255),
    suppressOriginal: int(a.suppressOriginal, `${path}.suppressOriginal`, 0, 255),
  };
}

/** Check and normalise an untrusted profile config. Throws ConfigFileError with the offending path. */
export function validateProfileConfig(value: unknown, keyCount: number, ledCount: number, path = "config"): ProfileConfig {
  const c = obj(value, path);
  const keymap = obj(c.keymap, `${path}.keymap`);
  const l = obj(c.lighting, `${path}.lighting`);
  const perKeyU16 = (name: string) => intArray(c[name], `${path}.${name}`, keyCount, 0, 65535);
  if (!Array.isArray(l.colors) || l.colors.length !== ledCount) fail(`${path}.lighting.colors`, `expected ${ledCount} entries`);
  let advanced: AdvancedKey[] | null = null;
  if (c.advanced !== null && c.advanced !== undefined) {
    if (!Array.isArray(c.advanced) || c.advanced.length !== keyCount) fail(`${path}.advanced`, `expected ${keyCount} entries or null`);
    advanced = (c.advanced as unknown[]).map((a, i) => validateAdvanced(a, `${path}.advanced[${i}]`));
  }
  return {
    keymap: {
      main: intArray(keymap.main, `${path}.keymap.main`, keyCount, -32768, 32767),
      fn: intArray(keymap.fn, `${path}.keymap.fn`, keyCount, -32768, 32767),
    },
    keyMode: intArray(c.keyMode, `${path}.keyMode`, keyCount, 0, 255),
    actuationUm: perKeyU16("actuationUm"),
    releaseUm: perKeyU16("releaseUm"),
    rtPressUm: perKeyU16("rtPressUm"),
    rtReleaseUm: perKeyU16("rtReleaseUm"),
    topDeadzoneUm: perKeyU16("topDeadzoneUm"),
    bottomDeadzoneUm: perKeyU16("bottomDeadzoneUm"),
    socd: intArray(c.socd, `${path}.socd`, keyCount, -32768, 32767),
    advanced,
    lighting: {
      enabled: bool(l.enabled, `${path}.lighting.enabled`),
      capsIndicator: bool(l.capsIndicator, `${path}.lighting.capsIndicator`),
      fnLayerLighting: bool(l.fnLayerLighting, `${path}.lighting.fnLayerLighting`),
      effect: int(l.effect, `${path}.lighting.effect`, 0, 63),
      effectFlags: int(l.effectFlags, `${path}.lighting.effectFlags`, 0, 3),
      hue: int(l.hue, `${path}.lighting.hue`, 0, 255),
      saturation: int(l.saturation, `${path}.lighting.saturation`, 0, 255),
      brightness: int(l.brightness, `${path}.lighting.brightness`, 0, 255),
      speed: int(l.speed, `${path}.lighting.speed`, 0, 255),
      colors: (l.colors as unknown[]).map((v, i) => {
        const p = `${path}.lighting.colors[${i}]`;
        const o = obj(v, p);
        return { r: int(o.r, `${p}.r`, 0, 255), g: int(o.g, `${p}.g`, 0, 255), b: int(o.b, `${p}.b`, 0, 255) };
      }),
      solid: intArray(l.solid, `${path}.lighting.solid`, ledCount, 0, 255),
    },
    sleepSeconds: int(c.sleepSeconds, `${path}.sleepSeconds`, 0, 0xffffffff),
    winLock: bool(c.winLock, `${path}.winLock`),
    autoCalibration: bool(c.autoCalibration, `${path}.autoCalibration`),
  };
}

export function validateCalibration(value: unknown, keyCount: number, path = "calibration"): Calibration {
  const c = obj(value, path);
  return {
    top: intArray(c.top, `${path}.top`, keyCount, 0, 65535),
    bottom: intArray(c.bottom, `${path}.bottom`, keyCount, 0, 65535),
    switchType: intArray(c.switchType, `${path}.switchType`, keyCount, -32768, 32767),
  };
}

export function buildConfigFile(
  info: DeviceInfo, activeProfile: number, profiles: ProfileExport[], calibration?: Calibration, now: Date = new Date(),
): ConfigFile {
  return {
    format: FILE_FORMAT,
    formatVersion: FILE_FORMAT_VERSION,
    exportedAt: now.toISOString(),
    device: {
      protocol: info.protocol, productName: info.productName, firmwareVersion: info.firmwareVersion,
      serial: info.serial, keyCount: info.keyCount, ledCount: info.ledCount,
    },
    activeProfile,
    profiles,
    ...(calibration ? { calibration } : {}),
  };
}

/** Parse and validate an exported file. `keyCount` / `ledCount` are those of the connected board. */
export function parseConfigFile(text: string, keyCount: number, ledCount: number): ConfigFile {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch (e) {
    throw new ConfigFileError(`not valid JSON: ${(e as Error).message}`);
  }
  const f = obj(raw, "file");
  if (f.format !== FILE_FORMAT) fail("format", `expected "${FILE_FORMAT}"`);
  if (f.formatVersion !== FILE_FORMAT_VERSION) fail("formatVersion", `unsupported version ${JSON.stringify(f.formatVersion)}`);
  const device = obj(f.device, "device");
  if (device.keyCount !== keyCount) fail("device.keyCount", `file is for a ${JSON.stringify(device.keyCount)}-key board, this one has ${keyCount}`);
  if (!Array.isArray(f.profiles) || f.profiles.length === 0) fail("profiles", "expected at least one profile");
  const seen = new Set<number>();
  const profiles = (f.profiles as unknown[]).map((p, i) => {
    const o = obj(p, `profiles[${i}]`);
    const index = int(o.index, `profiles[${i}].index`, 0, 3);
    if (seen.has(index)) fail(`profiles[${i}].index`, `profile ${index} appears twice`);
    seen.add(index);
    if (typeof o.name !== "string") fail(`profiles[${i}].name`, "expected a string");
    return { index, name: o.name, config: validateProfileConfig(o.config, keyCount, ledCount, `profiles[${i}].config`) };
  });
  return {
    format: FILE_FORMAT,
    formatVersion: FILE_FORMAT_VERSION,
    exportedAt: typeof f.exportedAt === "string" ? f.exportedAt : "",
    device: {
      protocol: String(device.protocol ?? ""), productName: String(device.productName ?? ""),
      firmwareVersion: String(device.firmwareVersion ?? ""), serial: String(device.serial ?? ""),
      keyCount, ledCount,
    },
    activeProfile: int(f.activeProfile, "activeProfile", 0, 3),
    profiles,
    ...(f.calibration !== undefined ? { calibration: validateCalibration(f.calibration, keyCount) } : {}),
  };
}
