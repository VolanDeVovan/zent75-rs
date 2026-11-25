import { describe, expect, it } from "vitest";
import { ConfigFileError, parseConfigFile } from "../src/config-file";
import { exportConfig, importConfig } from "../src/config-io";
import { switchType } from "../src/protocol/keycodes";
import {
  CURVE_LAST, KEYCODE_EMPTY, KEY_MODE_RT_BELOW, cloneConfig, type CustomSwitch,
} from "../src/protocol/keyboard";
import {
  DEFAULT_CURVE, customSwitchCode, customSwitchIndex, defaultCustomSwitch, parseCurve, pointTravelUm, pointsForTravel,
  resampleCurve, validateCustomSwitch, withCurve,
} from "../src/protocol/switches";
import {
  CUSTOM_SWITCH_SIZE, REGIONS, decodeCustomSwitch, decodeProfile, diffRegion, encodeCustomSwitch, encodeCustomSwitches,
  encodeProfile, i16le,
} from "../src/protocol/zent/memory";
import { ZentStockProtocol } from "../src/protocol/zent/protocol";
import { MockDevice } from "../src/transport/mock";

const FAST = { timeoutMs: 40, saveDelayMs: 1, resetDelayMs: 1, profilePollMs: 3, heartbeatMs: 20 };

async function setup(version: [number, number, number] = [0, 0, 18]) {
  const dev = new MockDevice({ switchDelayMs: 1, version });
  const kb = new ZentStockProtocol(dev, FAST);
  await kb.connect();
  return { dev, kb };
}

const u32 = (b: Uint8Array, o: number) => (b[o] | (b[o + 1] << 8) | (b[o + 2] << 16) | (b[o + 3] << 24)) >>> 0;

describe("custom switch record codec", () => {
  it("encodes the factory record exactly like the firmware's defaults::custom_switch", () => {
    const r = encodeCustomSwitch(defaultCustomSwitch(2));
    expect(r.length).toBe(CUSTOM_SWITCH_SIZE);
    expect(u32(r, 0)).toBe(3420);
    expect(u32(r, 4)).toBe(71);
    expect(i16le(r.subarray(8, 8 + 142))).toEqual(DEFAULT_CURVE);
    expect([...r.subarray(0x96, 0xb8)].every((b) => b === 0xff)).toBe(true);
    const name = new TextEncoder().encode("自定义轴体3");
    expect([...r.subarray(0xb8, 0xb8 + name.length)]).toEqual([...name]);
    expect([...r.subarray(0xb8 + name.length, 0xf8)].every((b) => b === 0)).toBe(true);
    expect(u32(r, 0xf8)).toBe(6000);
    expect(r[0xfc]).toBe(0);
    expect([...r.subarray(0xfd)]).toEqual([0x30, 0xff, 0x9f]);
  });

  it("decodes and re-encodes records losslessly, including the unused curve slots", () => {
    for (let seed = 1; seed < 20; seed++) {
      const sw = defaultCustomSwitch(seed % 4);
      const n = 2 + (seed * 7) % 87;
      sw.curve = Array.from({ length: n }, (_, i) => (i === n - 1 ? CURVE_LAST : -20 + i * 3));
      sw.spare = Array.from({ length: 176 - 2 * n }, (_, i) => (i * seed) & 0xff);
      sw.name = `sw ${seed} ✓`;
      sw.color = { r: seed, g: 2 * seed, b: 255 - seed };
      const bytes = encodeCustomSwitch(sw);
      expect(decodeCustomSwitch(bytes)).toEqual(sw);
      expect(encodeCustomSwitch(decodeCustomSwitch(bytes))).toEqual(bytes);
    }
  });

  it("refuses records that do not fit", () => {
    const sw = defaultCustomSwitch(0);
    expect(() => encodeCustomSwitch({ ...sw, name: "" })).toThrow(/name/);
    expect(() => encodeCustomSwitch({ ...sw, name: "x".repeat(64) })).toThrow(/name/);
    expect(() => encodeCustomSwitch({ ...sw, curve: new Array(89).fill(0) })).toThrow(/88/);
    expect(() => encodeCustomSwitch({ ...sw, travelUm: -1 })).toThrow(/travelUm/);
  });

  it("is written record by record", () => {
    const list = [0, 1, 2, 3].map(defaultCustomSwitch);
    const before = encodeCustomSwitches(list).custom_switch!;
    list[2].travelUm = 4000;
    const runs = diffRegion("custom_switch", before, encodeCustomSwitches(list).custom_switch!, 255);
    expect(runs.map((r) => [r.address, r.data.length])).toEqual([[0x80070390 + 2 * 256, 256]]);
  });
});

describe("custom switch rules", () => {
  it("accepts the factory record and names each violated firmware rule", () => {
    expect(validateCustomSwitch(defaultCustomSwitch(0))).toEqual([]);
    const bad = (patch: Partial<CustomSwitch>, re: RegExp) =>
      expect(validateCustomSwitch({ ...defaultCustomSwitch(0), ...patch }).join(" ")).toMatch(re);
    bad({ travelUm: 1499 }, /Travel/);
    bad({ travelUm: 4201 }, /Travel/);
    bad({ curve: [CURVE_LAST] }, /2\.\.88 points/);
    bad({ curve: [0, 10, 2600] }, /last point must be 2636/);
    bad({ curve: [0, 20, 10, CURVE_LAST] }, /must not decrease \(point 2/);
    bad({ travelUm: 1500, curve: [...DEFAULT_CURVE] }, /Too many points/);
    bad({ name: "" }, /Name/);
    bad({ wordF8: 0xffff }, /0xF8/);
    bad({ byteFC: 0xff }, /0xFC/);
  });

  it("places curve points 50 µm apart, the last one at full travel", () => {
    expect(pointsForTravel(3420)).toBe(71);
    expect(pointsForTravel(3400)).toBe(70);
    expect(pointsForTravel(4200)).toBe(86);
    expect(pointTravelUm(0, 71, 3420)).toBe(-50);
    expect(pointTravelUm(1, 71, 3420)).toBe(0);
    expect(pointTravelUm(69, 71, 3420)).toBe(3400);
    expect(pointTravelUm(70, 71, 3420)).toBe(3420);
  });

  it("resamples a curve to another travel and keeps it valid", () => {
    expect(resampleCurve(DEFAULT_CURVE, 3420, 3420)).toEqual(DEFAULT_CURVE);
    for (const t of [1500, 2000, 3000, 3780, 4200]) {
      const curve = resampleCurve(DEFAULT_CURVE, 3420, t);
      expect(curve.length).toBe(pointsForTravel(t));
      expect(validateCustomSwitch({ ...defaultCustomSwitch(0), travelUm: t, curve })).toEqual([]);
      expect(curve[0]).toBeGreaterThanOrEqual(-12);
      expect(curve[0]).toBeLessThanOrEqual(0);
    }
  });

  it("parses curve text and maps type codes", () => {
    expect(parseCurve("-12, 0 12\n25;2636")).toEqual([-12, 0, 12, 25, 2636]);
    expect(() => parseCurve("1 2 x")).toThrow(/"x"/);
    expect([0, 1, 2, 3].map(customSwitchCode)).toEqual([4095, 4094, 4093, 4092]);
    expect(customSwitchIndex(4093)).toBe(2);
    expect(customSwitchIndex(2)).toBeNull();
    const customs = [0, 1, 2, 3].map(defaultCustomSwitch);
    customs[1].travelUm = 4000;
    expect(switchType(4094, customs)).toMatchObject({ travelUm: 4000, custom: true });
    expect(switchType(4094).travelUm).toBe(3420);
    expect(switchType(2, customs).travelUm).toBe(3420);
  });
});

describe("custom switches against the mock", () => {
  it("reads the factory records and writes only the changed one, saved at once", async () => {
    const { dev, kb } = await setup();
    const list = await kb.readCustomSwitches();
    expect(list).toEqual([0, 1, 2, 3].map(defaultCustomSwitch));
    list[1] = withCurve({ ...list[1], name: "Lekker", travelUm: 4000 }, resampleCurve(list[1].curve, 3420, 4000));
    expect(list[1].spare.length).toBe(176 - 2 * 82);
    const writes = dev.log.writes;
    expect(await kb.writeCustomSwitches(list)).toBe(256);
    expect(dev.log.writes - writes).toBe(2); // 255 + 1 bytes
    expect(dev.log.saves).toBe(0);
    expect(dev.customSwitches()[1]).toEqual(list[1]);
    const flash = dev.peekFlash(REGIONS.custom_switch.address + 256, 256);
    expect(decodeCustomSwitch(flash)).toEqual(list[1]);
    expect(await kb.writeCustomSwitches(list)).toBe(0);
    dev.replug();
    const kb2 = new ZentStockProtocol(dev, FAST);
    await kb2.connect();
    expect((await kb2.readCustomSwitches())[1]).toEqual(list[1]);
  });

  it("refuses invalid records before sending anything", async () => {
    const { dev, kb } = await setup();
    const list = await kb.readCustomSwitches();
    list[3] = { ...list[3], curve: [0, 1, 2] };
    const writes = dev.log.writes;
    await expect(kb.writeCustomSwitches(list)).rejects.toThrow(/custom switch 4: .*2636/);
    expect(dev.log.writes).toBe(writes);
  });

  it("fails verification when the firmware discards the write (implausible calibration)", async () => {
    const { dev, kb } = await setup();
    // calibration that the firmware would reject: span < 3000 on key 5 (written raw, bypassing the panel)
    const cal = await kb.readCalibration();
    expect(cal.top[5] - cal.bottom[5]).toBeGreaterThan(3000);
    const g = (dev as unknown as { global: Uint8Array }).global;
    g[REGIONS.bot_value.address - 0x80070000 + 10] = 0x10;
    g[REGIONS.bot_value.address - 0x80070000 + 11] = 0x79; // 0x7910 = 30992
    const list = await kb.readCustomSwitches();
    list[0] = { ...list[0], name: "nope" };
    await expect(kb.writeCustomSwitches(list)).rejects.toThrow(/verify failed/);
    expect(dev.customSwitches()[0].name).toBe("自定义轴体1");
  });

  it("travel of a custom type comes from its record in the mock stream", async () => {
    const { dev, kb } = await setup();
    const list = await kb.readCustomSwitches();
    list[0] = withCurve({ ...list[0], travelUm: 4000 }, resampleCurve(list[0].curve, 3420, 4000));
    await kb.writeCustomSwitches(list);
    const cal = await kb.readCalibration();
    cal.switchType[10] = customSwitchCode(0);
    await kb.writeCalibration(cal);
    const samples: number[] = [];
    const stop = kb.startStream((s) => s.travelUm && samples.push(s.travelUm[10]));
    dev.press(10, 1);
    for (let i = 0; i < 20 && samples.length === 0; i++) {
      dev.emitStreamFrame();
      await new Promise((r) => setTimeout(r, 1));
    }
    stop();
    expect(samples.at(-1)!).toBeGreaterThan(3500);
  });
});

describe("tap-hold and modifier masks", () => {
  it("are decoded from their own regions and refused above 1000", () => {
    const img = encodeProfile(decodeProfile(Object.fromEntries(
      Object.entries(REGIONS).map(([n, r]) => [n, new Uint8Array(r.length)]),
    )));
    const cfg = decodeProfile(img);
    cfg.holdCode.fn[4] = -7;
    cfg.modMask.main[5] = 0x22;
    const out = encodeProfile(cfg);
    expect(Array.from(out.hold_code_fn!.subarray(8, 10))).toEqual([0xf9, 0xff]);
    expect(out.mod_mask![5]).toBe(0x22);
    expect(decodeProfile(out)).toEqual(cfg);
    cfg.holdCode.main[0] = 1001;
    expect(() => encodeProfile(cfg)).toThrow(/holdCode\.main\[0\] = 1001/);
    cfg.holdCode.main[0] = KEYCODE_EMPTY;
    cfg.modMask.fn[1] = 256;
    expect(() => encodeProfile(cfg)).toThrow(/modMask\.fn/);
  });

  it("round-trip through the mock with minimal writes; reset clears them", async () => {
    const { dev, kb } = await setup();
    const cfg = await kb.readConfig();
    expect(cfg.holdCode.main.every((c) => c === KEYCODE_EMPTY)).toBe(true);
    expect(cfg.modMask.fn.every((m) => m === 0)).toBe(true);
    const next = cloneConfig(cfg);
    next.holdCode.main[46] = -8; // A: hold = LCtrl
    next.holdCode.fn[47] = 4; // S on Fn: hold = a
    next.modMask.main[48] = 0x02; // D: + LShift
    next.modMask.fn[49] = 0x81;
    next.keyMode[50] = KEY_MODE_RT_BELOW;
    expect(await kb.writeConfig(next)).toBe(2 + 2 + 1 + 1 + 83);
    expect(i16le(dev.peek(REGIONS.hold_code.address + 92, 2))).toEqual([-8]);
    expect(Array.from(dev.peek(REGIONS.mod_mask_fn.address + 49, 1))).toEqual([0x81]);
    expect(await kb.readConfig()).toEqual(next);
    await kb.save();
    dev.replug();
    const kb2 = new ZentStockProtocol(dev, FAST);
    await kb2.connect();
    expect(await kb2.readConfig()).toEqual(next);
    await kb2.resetConfig();
    const reset = await kb2.readConfig();
    expect(reset.holdCode.main[46]).toBe(KEYCODE_EMPTY);
    expect(reset.modMask.main[48]).toBe(0);
  });
});

describe("config file", () => {
  it("carries tap-hold, masks and the custom switches (with the calibration)", async () => {
    const a = await setup();
    const cfg = await a.kb.readConfig();
    cfg.holdCode.main[46] = -8;
    cfg.modMask.fn[3] = 0x10;
    await a.kb.writeConfig(cfg);
    const list = await a.kb.readCustomSwitches();
    list[2] = { ...list[2], name: "Mine", color: { r: 1, g: 2, b: 3 } };
    await a.kb.writeCustomSwitches(list);
    const file = await exportConfig(a.kb, { allProfiles: false, includeCalibration: true });
    expect(file.customSwitches).toEqual(list);
    const text = JSON.stringify(file);
    expect(parseConfigFile(text, 83, 85)).toEqual(file);
    expect((await exportConfig(a.kb, { allProfiles: false, includeCalibration: false })).customSwitches).toBeUndefined();

    const b = await setup();
    await importConfig(b.kb, parseConfigFile(text, 83, 85), { mode: "active", restoreNames: false, restoreCalibration: false, save: false });
    expect(await b.kb.readConfig()).toEqual(cfg);
    expect((await b.kb.readCustomSwitches())[2].name).toBe("自定义轴体3");
    const res = await importConfig(b.kb, parseConfigFile(text, 83, 85), { mode: "active", restoreNames: false, restoreCalibration: true, save: false });
    expect(res.bytesWritten).toBe(256);
    expect((await b.kb.readCustomSwitches())[2]).toEqual(list[2]);
  });

  it("reads files without tap-hold as 'off' and rejects bad values", async () => {
    const { kb } = await setup();
    const file = await exportConfig(kb, { allProfiles: false, includeCalibration: true });
    const old = cloneConfig(file) as any;
    delete old.profiles[0].config.holdCode;
    delete old.profiles[0].config.modMask;
    const parsed = parseConfigFile(JSON.stringify(old), 83, 85);
    expect(parsed.profiles[0].config.holdCode.fn).toEqual(new Array(83).fill(KEYCODE_EMPTY));
    expect(parsed.profiles[0].config.modMask.main).toEqual(new Array(83).fill(0));
    const bad = (fn: (f: any) => void, re: RegExp) => {
      const copy = cloneConfig(file) as any;
      fn(copy);
      expect(() => parseConfigFile(JSON.stringify(copy), 83, 85)).toThrow(ConfigFileError);
      expect(() => parseConfigFile(JSON.stringify(copy), 83, 85)).toThrow(re);
    };
    bad((f) => (f.profiles[0].config.holdCode.main[3] = 1001), /holdCode\.main\[3\]/);
    bad((f) => (f.profiles[0].config.modMask.fn[3] = 256), /modMask\.fn\[3\]/);
    bad((f) => f.customSwitches.pop(), /customSwitches: expected 4/);
    bad((f) => (f.customSwitches[1].curve[70] = 2000), /customSwitches\[1\]: .*decrease|2636/);
  });
});
