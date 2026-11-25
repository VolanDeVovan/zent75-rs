import { describe, expect, it } from "vitest";
import { KEYCODE_EMPTY, KEY_MODE_NORMAL, KEY_MODE_RAPID_TRIGGER, KEY_MODE_RT_BELOW, type ProfileConfig } from "../src/protocol/keyboard";
import { KEY_DEFS } from "../src/protocol/keycodes";
import { ZentStockProtocol } from "../src/protocol/zent/protocol";
import { MockDevice } from "../src/transport/mock";
import { FACTORY_FEEL } from "../src/ui/defaults";
import {
  PICKER_CATEGORIES, categoryOfCode, changedAreas, codesOfCategory, customisedKeys, feelOf, globalFeel, globalKeys, isSocdPair, patchFeel,
  searchCodes, setFeel, setSocdPair, socdRules, validateFeel,
} from "../src/ui/model";

async function factoryConfig(): Promise<ProfileConfig> {
  const kb = new ZentStockProtocol(new MockDevice());
  await kb.connect();
  const c = await kb.readConfig();
  await kb.disconnect();
  return c;
}

const W = 32, A = 46, S = 47, D = 48;

describe("global and customised actuation", () => {
  it("takes the most common feel as global and lists the keys that differ", async () => {
    const c = await factoryConfig();
    expect(customisedKeys(c)).toEqual([]);
    expect(globalKeys(c)).toHaveLength(c.keyMode.length);
    for (const k of [W, A, S, D]) c.keyMode[k] = KEY_MODE_RAPID_TRIGGER;
    c.actuationUm[0] = 900;
    expect(globalFeel(c).mode).toBe(KEY_MODE_NORMAL);
    expect(globalFeel(c).actuationUm).toBe(1600);
    expect(customisedKeys(c)).toEqual([0, W, A, S, D]);
    expect(globalKeys(c)).not.toContain(W);
  });

  it("ignores values the mode does not use", async () => {
    const c = await factoryConfig();
    c.rtPressUm[5] = 700; // key 5 is in normal mode: rapid-trigger values do nothing
    expect(customisedKeys(c)).toEqual([]);
    c.keyMode[5] = KEY_MODE_RAPID_TRIGGER;
    expect(customisedKeys(c)).toEqual([5]);
  });

  it("an all-keys edit leaves customised keys alone; the global values bring them back", async () => {
    const c = await factoryConfig();
    c.actuationUm[W] = 500;
    c.releaseUm[W] = 300;
    for (const k of globalKeys(c)) setFeel(c, k, patchFeel(feelOf(c, k), { actuationUm: 2000 }));
    expect(c.actuationUm[W]).toBe(500);
    expect(c.actuationUm[0]).toBe(2000);
    setFeel(c, W, globalFeel(c));
    expect(customisedKeys(c)).toEqual([]);
  });

  it("the release point follows the actuation point down", () => {
    const f = patchFeel({ ...FACTORY_FEEL }, { actuationUm: 500 });
    expect(f.releaseUm).toBe(500);
    expect(patchFeel({ ...FACTORY_FEEL }, { actuationUm: 2000 }).releaseUm).toBe(800);
    expect(validateFeel(f, 3420)).toEqual([]);
  });

  it("refuses what the keyboard would not accept", () => {
    expect(validateFeel({ ...FACTORY_FEEL, actuationUm: 3420 }, 3420)).not.toEqual([]);
    expect(validateFeel({ ...FACTORY_FEEL, actuationUm: 50, releaseUm: 50 }, 3420)).not.toEqual([]);
    expect(validateFeel({ ...FACTORY_FEEL, releaseUm: 1700 }, 3420)).not.toEqual([]);
    expect(validateFeel({ ...FACTORY_FEEL, mode: KEY_MODE_RAPID_TRIGGER, rtPressUm: 5 }, 3420)).not.toEqual([]);
    expect(validateFeel({ ...FACTORY_FEEL, rtPressUm: 5 }, 3420)).toEqual([]); // not used in normal mode
    expect(validateFeel({ ...FACTORY_FEEL, mode: KEY_MODE_RAPID_TRIGGER, topDeadzoneUm: 2000, bottomDeadzoneUm: 1500 }, 3420)).not.toEqual([]);
    expect(validateFeel({ ...FACTORY_FEEL, mode: KEY_MODE_RT_BELOW, actuationUm: 3000, bottomDeadzoneUm: 500 }, 3420)).not.toEqual([]);
    expect(validateFeel({ ...FACTORY_FEEL }, 3420)).toEqual([]);
  });
});

describe("SOCD rules", () => {
  it("pairs by main-layer codes and reports pairs and one-way rules", async () => {
    const c = await factoryConfig();
    expect(socdRules(c)).toEqual({ pairs: [], oneWay: [] });
    setSocdPair(c, A, D, true);
    expect(c.socd[A]).toBe(c.keymap.main[D]);
    expect(c.socd[D]).toBe(c.keymap.main[A]);
    expect(isSocdPair(c, A, D)).toBe(true);
    c.socd[W] = c.keymap.main[S];
    expect(socdRules(c)).toEqual({ pairs: [{ a: A, b: D }], oneWay: [{ key: W, code: c.keymap.main[S] }] });
    setSocdPair(c, A, D, false);
    expect(c.socd[A]).toBe(KEYCODE_EMPTY);
    expect(socdRules(c).pairs).toEqual([]);
  });
});

describe("unsaved summary", () => {
  it("names the areas that differ", async () => {
    const a = await factoryConfig();
    const b = structuredClone(a);
    expect(changedAreas(a, b)).toEqual([]);
    b.actuationUm[3] = 1000;
    b.lighting.brightness = 3;
    b.socd[1] = 4;
    b.holdCode.fn[2] = 5;
    b.winLock = !b.winLock;
    expect(changedAreas(a, b)).toEqual(["actuation", "lighting", "SOCD", "keymap", "settings"]);
    if (b.advanced) {
      b.advanced[0].mode = 1;
      expect(changedAreas(a, b)).toContain("advanced keys");
    }
  });
});

describe("key code picker", () => {
  it("every key code is in exactly one category", () => {
    const seen = PICKER_CATEGORIES.flatMap((c) => codesOfCategory(c.id).map((d) => d.code));
    expect([...seen].sort((x, y) => x - y)).toEqual(KEY_DEFS.map((d) => d.code).sort((x, y) => x - y));
  });

  it("opens on the category of the current code, on Basic for an empty slot", () => {
    expect(categoryOfCode(201)).toBe("media");
    expect(categoryOfCode(-7)).toBe("mods");
    expect(categoryOfCode(KEYCODE_EMPTY)).toBe("basic");
    expect(categoryOfCode(12345)).toBe("basic");
  });

  it("searches labels, descriptions and code numbers, and offers raw codes", () => {
    expect(searchCodes("vol").matches.map((d) => d.label)).toEqual(["Vol+", "Vol-"]);
    expect(searchCodes("calculator").matches.map((d) => d.code)).toEqual([208]);
    expect(searchCodes("58").matches.map((d) => d.label)).toContain("F1");
    expect(searchCodes("1234")).toEqual({ matches: [], raw: 1234 });
    expect(searchCodes("40000").raw).toBeNull();
    expect(searchCodes("").matches).toEqual([]);
  });

  it("caps tap-hold codes at 1000", () => {
    expect(searchCodes("1001", KEYCODE_EMPTY).raw).toBeNull();
    expect(searchCodes("1000", KEYCODE_EMPTY).raw).toBe(1000);
    expect(searchCodes("401", KEYCODE_EMPTY).matches.map((d) => d.label)).toEqual(["Fn"]);
    for (const c of PICKER_CATEGORIES) expect(codesOfCategory(c.id, KEYCODE_EMPTY).every((d) => d.code <= KEYCODE_EMPTY)).toBe(true);
  });
});
