import { describe, expect, it } from "vitest";
import { ADV_DKS, KEYCODE_EMPTY } from "../src/protocol/keyboard";
import { KEY_COUNT, KEY_LAYOUT, KEY_TO_LED, LAYOUT_WIDTH, LED_COUNT, DEFAULT_KEYMAP } from "../src/protocol/layout";
import { KEY_DEFS, keyLabel, EFFECTS } from "../src/protocol/keycodes";
import {
  BIND_RECORD_SIZE, PROFILE_REGIONS, REGIONS, type Images, type RegionName,
  decodeAdvancedKey, decodeCalibration, decodeProfile, decodeProfileName, decodeVersion, diffRegion, emptyAdvancedKey,
  encodeAdvancedKey, encodeCalibration, encodeProfile, encodeProfileName, encodeVersion, findRegion, i16le, packI16, packU16, u16le,
} from "../src/protocol/zent/memory";

function randomImages(seed: number, withAdvanced = true): Images {
  let x = seed;
  const next = () => {
    x = (x * 1664525 + 1013904223) >>> 0;
    return x >>> 24;
  };
  const images: Images = {};
  const names: RegionName[] = [...PROFILE_REGIONS, ...(withAdvanced ? (["bind_key_mode", "bind_key_setting"] as RegionName[]) : [])];
  for (const n of names) {
    const img = new Uint8Array(REGIONS[n].length);
    for (let i = 0; i < img.length; i++) img[i] = next();
    images[n] = img;
  }
  // booleans are stored as exactly 0 / 1
  for (const n of ["rgb_enable", "cap_enable", "rgb_fn_enable", "win_lock_state", "auto_calib_enable"] as RegionName[]) images[n]![0] &= 1;
  return images;
}

describe("memory map", () => {
  it("matches the addresses of the vendor driver's addressList", () => {
    const a = (n: RegionName) => [REGIONS[n].address, REGIONS[n].length];
    expect(a("top_value")).toEqual([0x80070000, 166]);
    expect(a("bot_value")).toEqual([0x800700b0, 166]);
    expect(a("keytype_code")).toEqual([0x80070160, 166]);
    expect(a("soft_version")).toEqual([0x80070210, 4]);
    expect(a("cur_param_num")).toEqual([0x80070218, 4]);
    expect(a("config_name")).toEqual([0x80070290, 256]);
    expect(a("key_map")).toEqual([0x80080160, 166]);
    expect(a("key_map_fn")).toEqual([0x80080210, 166]);
    expect(a("press_th")).toEqual([0x800802c0, 166]);
    expect(a("return_th")).toEqual([0x80080370, 166]);
    expect(a("press_deadzone")).toEqual([0x80080420, 166]);
    expect(a("return_deadzone")).toEqual([0x800804d0, 166]);
    expect(a("norm_press_th")).toEqual([0x80080580, 166]);
    expect(a("norm_return_th")).toEqual([0x80080630, 166]);
    expect(a("key_mode")).toEqual([0x800806e0, 83]);
    expect(a("key_cancel_code")).toEqual([0x80080a50, 166]);
    expect(a("rgb")).toEqual([0x80080b00, 255]);
    expect(a("rgb_solid_enable")).toEqual([0x80080c08, 85]);
    expect(a("rgb_config")).toEqual([0x80080c60, 5]);
    expect(a("rgb_enable")).toEqual([0x80080c68, 1]);
    expect(a("cap_enable")).toEqual([0x80080c69, 1]);
    expect(a("rgb_fn_enable")).toEqual([0x80080c6b, 1]);
    expect(a("sleep_second_cnt")).toEqual([0x80080c6c, 4]);
    expect(a("win_lock_state")).toEqual([0x80080c70, 1]);
    expect(a("auto_calib_enable")).toEqual([0x80080c71, 1]);
    expect(a("bind_key_setting")).toEqual([0x80080d00, 48 * 83]);
    expect(a("bind_key_mode")).toEqual([0x80081de0, 83]);
  });

  it("regions do not overlap", () => {
    const rs = Object.values(REGIONS).sort((x, y) => x.address - y.address);
    for (let i = 1; i < rs.length; i++) expect(rs[i - 1].address + rs[i - 1].length).toBeLessThanOrEqual(rs[i].address);
  });

  it("findRegion only accepts ranges fully inside a region", () => {
    expect(findRegion(0x80080160, 166)?.name).toBe("key_map");
    expect(findRegion(0x80080160 + 2 * 59, 2)?.name).toBe("key_map");
    expect(findRegion(0x80080160, 167)).toBeUndefined();
    expect(findRegion(0x80080c6a, 1)).toBeUndefined(); // the unnamed byte between cap_enable and rgb_fn_enable
    expect(findRegion(0x80040000, 4)).toBeUndefined();
    expect(findRegion(0x80080d00 + 48 * 82, 48)?.name).toBe("bind_key_setting");
    expect(findRegion(0x80080d00 + 48 * 82, 49)).toBeUndefined();
  });
});

describe("scalars", () => {
  it("u16 / i16 little endian", () => {
    expect(u16le([0x18, 0x79, 0xff, 0xff])).toEqual([31000, 65535]);
    expect(i16le([0xf8, 0xff, 0xe8, 0x03, 0x91, 0x01])).toEqual([-8, 1000, 401]);
    expect(Array.from(packI16([-8, 1000, 401]))).toEqual([0xf8, 0xff, 0xe8, 0x03, 0x91, 0x01]);
    expect(Array.from(packU16([31000, 17000]))).toEqual([0x18, 0x79, 0x68, 0x42]);
    expect(() => packU16([65536])).toThrow(RangeError);
    expect(() => packU16([1.5])).toThrow(RangeError);
    expect(() => packI16([40000])).toThrow(RangeError);
  });

  it("firmware version is stored patch, minor, major", () => {
    expect(decodeVersion([18, 0, 0, 0])).toMatchObject({ text: "0.0.18", number: 18, major: 0, minor: 0, patch: 18 });
    expect(decodeVersion([9, 0, 0, 0]).text).toBe("0.0.9");
    expect(decodeVersion([3, 2, 1, 0])).toMatchObject({ text: "1.2.3", number: 0x010203 });
    expect(Array.from(encodeVersion(0, 0, 18))).toEqual([18, 0, 0, 0]);
  });

  it("profile names are zero-padded UTF-8 in 64 bytes", () => {
    const b = encodeProfileName("Игры ⌨");
    expect(b.length).toBe(64);
    expect(decodeProfileName(b, 0)).toBe("Игры ⌨");
    expect(decodeProfileName(new Uint8Array(64), 2)).toBe("Config3");
    expect(decodeProfileName(Uint8Array.of(0xff, 0xfe, 0, 0), 1)).toBe("Config2"); // invalid UTF-8
    expect(() => encodeProfileName("")).toThrow(RangeError);
    expect(() => encodeProfileName("x".repeat(64))).toThrow(RangeError);
  });
});

describe("bind_key_setting record (48 bytes)", () => {
  it("encodes a DKS record exactly like the vendor driver's DKSConfirm", () => {
    // slot 0: W tapped on press-top, slot 1: LShift held from press-bottom to release-bottom
    const key = {
      ...emptyAdvancedKey(),
      mode: ADV_DKS,
      actions: [[2, 0, 0, 0], [0, 1, 1, 0], [0, 0, 0, 0], [0, 0, 0, 2]],
      keys: [26, -7, KEYCODE_EMPTY, 44],
      dksTopUm: 1000, dksBottomUm: 3000,
      tapTicks: 8 * 5, delayTicks: 8 * 2, randomTicks: 8 * 1,
      suppressOriginal: 1,
    };
    const b = Array.from(encodeAdvancedKey(key));
    expect(b.length).toBe(BIND_RECORD_SIZE);
    expect(b.slice(0, 16)).toEqual([2, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 2]);
    expect(b.slice(16, 24)).toEqual([26, 0, 0xf9, 0xff, 0xe8, 0x03, 44, 0]);
    expect(b.slice(24, 28)).toEqual([0xe8, 0x03, 0xb8, 0x0b]);
    expect(b.slice(28, 34)).toEqual([40, 0, 16, 0, 8, 0]);
    expect(b.slice(34, 47)).toEqual(new Array(13).fill(0xff));
    expect(b[47]).toBe(1);
    expect(decodeAdvancedKey(Uint8Array.from(b), ADV_DKS)).toEqual(key);
  });

  it("decodes any 48 bytes losslessly", () => {
    const rec = Uint8Array.from({ length: 48 }, (_, i) => (i * 37 + 11) & 0xff);
    expect(Array.from(encodeAdvancedKey(decodeAdvancedKey(rec, 1)))).toEqual(Array.from(rec));
    expect(() => decodeAdvancedKey(new Uint8Array(47), 0)).toThrow(RangeError);
  });
});

describe("profile config <-> region images", () => {
  it("decode(encode(x)) and encode(decode(bytes)) are identities", () => {
    for (const seed of [1, 2, 3]) {
      const images = randomImages(seed);
      const cfg = decodeProfile(images);
      const again = encodeProfile(cfg);
      for (const n of Object.keys(images) as RegionName[]) expect(Array.from(again[n]!), n).toEqual(Array.from(images[n]!));
      expect(decodeProfile(again)).toEqual(cfg);
    }
  });

  it("maps fields to the right regions and units", () => {
    const images = randomImages(7);
    images.key_map = packI16(DEFAULT_KEYMAP);
    images.norm_press_th = packU16(new Array(83).fill(1600));
    images.press_th = packU16(new Array(83).fill(300));
    images.press_deadzone = packU16(new Array(83).fill(100));
    images.return_deadzone = packU16(new Array(83).fill(150));
    images.rgb_config = Uint8Array.of(1 + 4 * 13, 188, 255, 10, 50);
    images.sleep_second_cnt = Uint8Array.of(0x08, 0x07, 0, 0);
    images.win_lock_state = Uint8Array.of(1);
    images.auto_calib_enable = Uint8Array.of(0);
    const c = decodeProfile(images);
    expect(c.keymap.main[0]).toBe(41);
    expect(c.keymap.main[59]).toBe(-7);
    expect(c.keymap.main[78]).toBe(401);
    expect(c.actuationUm[10]).toBe(1600);
    expect(c.rtPressUm[10]).toBe(300);
    expect(c.topDeadzoneUm[0]).toBe(100);
    expect(c.bottomDeadzoneUm[0]).toBe(150);
    expect(c.lighting).toMatchObject({ effect: 13, effectFlags: 1, hue: 188, saturation: 255, brightness: 10, speed: 50 });
    expect(c.sleepSeconds).toBe(1800);
    expect(c.winLock).toBe(true);
    expect(c.autoCalibration).toBe(false);
    expect(c.lighting.colors.length).toBe(LED_COUNT);
    expect(c.lighting.colors[1]).toEqual({ r: images.rgb![3], g: images.rgb![4], b: images.rgb![5] });
  });

  it("leaves advanced null when the firmware has no bind regions", () => {
    const cfg = decodeProfile(randomImages(4, false));
    expect(cfg.advanced).toBeNull();
    const out = encodeProfile(cfg);
    expect(out.bind_key_mode).toBeUndefined();
    expect(out.bind_key_setting).toBeUndefined();
  });

  it("refuses malformed configs", () => {
    const cfg = decodeProfile(randomImages(5));
    expect(() => encodeProfile({ ...cfg, keyMode: cfg.keyMode.slice(1) })).toThrow(/keyMode/);
    expect(() => encodeProfile({ ...cfg, actuationUm: cfg.actuationUm.map(() => 70000) })).toThrow(/actuationUm/);
    expect(() => encodeProfile({ ...cfg, lighting: { ...cfg.lighting, effect: 64 } })).toThrow(/effect/);
    expect(() => decodeProfile({})).toThrow(/missing/);
  });

  it("calibration images", () => {
    const cal = { top: new Array(83).fill(31000), bottom: new Array(83).fill(17000), switchType: new Array(83).fill(2) };
    const img = encodeCalibration(cal);
    expect(Array.from(img.top_value!.subarray(0, 2))).toEqual([0x18, 0x79]);
    expect(decodeCalibration(img)).toEqual(cal);
  });
});

describe("diffRegion", () => {
  it("returns nothing for identical images", () => {
    const a = packI16(DEFAULT_KEYMAP);
    expect(diffRegion("key_map", a, a.slice(), 240)).toEqual([]);
  });

  it("writes single changed elements at their own address, like the vendor driver", () => {
    const before = packI16(DEFAULT_KEYMAP);
    const after = before.slice();
    after.set(packI16([57]), 2 * 73); // LCtrl -> Caps
    after.set(packI16([-8]), 2 * 45); // Caps -> LCtrl
    const runs = diffRegion("key_map", before, after, 240);
    expect(runs.map((r) => [r.address, Array.from(r.data)])).toEqual([
      [0x80080160 + 90, [0xf8, 0xff]],
      [0x80080160 + 146, [57, 0]],
    ]);
  });

  it("aligns to whole elements even if only one byte differs", () => {
    const before = new Uint8Array(REGIONS.bind_key_setting.length);
    const after = before.slice();
    after[48 * 3 + 47] = 1;
    const runs = diffRegion("bind_key_setting", before, after, 240);
    expect(runs.length).toBe(1);
    expect(runs[0].address).toBe(0x80080d00 + 48 * 3);
    expect(runs[0].data.length).toBe(48);
    expect(diffRegion("rgb_config", Uint8Array.of(5, 1, 2, 3, 4), Uint8Array.of(5, 1, 2, 3, 9), 240)[0].data.length).toBe(5);
  });

  it("rewrites regions the vendor driver always writes in one piece", () => {
    const rgbB = new Uint8Array(255), rgbA = new Uint8Array(255);
    rgbA[7] = 9;
    const runs = diffRegion("rgb", rgbB, rgbA, 255);
    expect(runs.length).toBe(1);
    expect(runs[0]).toMatchObject({ address: 0x80080b00, offset: 0 });
    expect(runs[0].data.length).toBe(255);
    expect(diffRegion("rgb", rgbA, rgbA.slice(), 255)).toEqual([]);
    const modeB = new Uint8Array(83), modeA = new Uint8Array(83);
    modeA[82] = 1;
    expect(diffRegion("key_mode", modeB, modeA, 255).map((r) => [r.address, r.data.length])).toEqual([[0x800806e0, 83]]);
    const thB = new Uint8Array(166), thA = new Uint8Array(166);
    thA[0] = 1;
    expect(diffRegion("norm_press_th", thB, thA, 255)[0].data.length).toBe(166);
    for (const n of ["top_value", "bot_value", "keytype_code", "press_th", "return_th", "press_deadzone", "return_deadzone",
      "norm_return_th", "rgb_solid_enable", "bind_key_mode"] as const) expect(REGIONS[n].whole, n).toBe(true);
    for (const n of ["key_map", "key_map_fn", "key_cancel_code", "bind_key_setting", "config_name"] as const) expect(REGIONS[n].whole, n).toBe(false);
  });

  it("splits long runs into chunks of whole elements and writes everything without a baseline", () => {
    const after = Uint8Array.from({ length: REGIONS.bind_key_setting.length }, (_, i) => i & 0xff);
    const runs = diffRegion("bind_key_setting", undefined, after, 250);
    expect(runs.every((r) => r.data.length % 48 === 0 && r.data.length <= 240)).toBe(true);
    const total = new Uint8Array(after.length);
    for (const r of runs) total.set(r.data, r.offset);
    expect(Array.from(total)).toEqual(Array.from(after));
  });
});

describe("layout and key code tables", () => {
  it("has 83 keys in 6 rows, each row 16u wide", () => {
    expect(KEY_LAYOUT.length).toBe(KEY_COUNT);
    const rows = new Map<number, number>();
    for (const k of KEY_LAYOUT) rows.set(k.y, Math.max(rows.get(k.y) ?? 0, k.x + k.w));
    expect(rows.size).toBe(6);
    for (const w of rows.values()) expect(w).toBeCloseTo(LAYOUT_WIDTH);
  });

  it("key indices match README anchors", () => {
    const name = (i: number) => KEY_LAYOUT[i].name;
    expect([name(0), name(59), name(73), name(79), name(82)]).toEqual(["Esc", "LShift", "LCtrl", "RCtrl", "→"]);
    expect([name(28), name(45), name(57), name(76)]).toEqual(["Bksp", "Caps", "Enter", "Space"]);
    expect(KEY_LAYOUT[76].w).toBe(6.25);
    expect(DEFAULT_KEYMAP.length).toBe(83);
    expect(keyLabel(DEFAULT_KEYMAP[59])).toBe("LShift");
    expect(keyLabel(DEFAULT_KEYMAP[82])).toBe("→");
  });

  it("LED map covers all 85 LEDs exactly once; the space bar has three", () => {
    const leds = KEY_TO_LED.flatMap((v) => (typeof v === "number" ? [v] : [...v]));
    expect(KEY_TO_LED.length).toBe(83);
    expect([...leds].sort((a, b) => a - b)).toEqual(Array.from({ length: 85 }, (_, i) => i));
    expect(KEY_TO_LED[76]).toEqual([3, 4, 5]);
  });

  it("key codes are unique and cover the documented ranges", () => {
    const codes = KEY_DEFS.map((d) => d.code);
    expect(new Set(codes).size).toBe(codes.length);
    for (let c = -8; c <= 119; c++) expect(codes, `code ${c}`).toContain(c);
    for (let c = 201; c <= 225; c++) expect(codes).toContain(c);
    for (let c = 251; c <= 255; c++) expect(codes).toContain(c);
    for (let c = 301; c <= 320; c++) expect(codes).toContain(c);
    for (let c = 401; c <= 404; c++) expect(codes).toContain(c);
    expect(codes).toContain(1000);
    expect(keyLabel(319)).toBe("5sReset");
    expect(keyLabel(777)).toBe("#777");
    expect(EFFECTS.map((e) => e.id)).toEqual([1, 3, 5, 12, 13, 14, 16, 18, 21, 23, 24, 29, 30, 32, 33, 35, 39, 43, 50]);
  });
});
