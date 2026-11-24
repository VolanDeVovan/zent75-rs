import { describe, expect, it } from "vitest";
import { ConfigFileError, FILE_FORMAT, parseConfigFile, validateProfileConfig } from "../src/config-file";
import { exportConfig, importConfig } from "../src/config-io";
import { ADV_COMBO, cloneConfig } from "../src/protocol/keyboard";
import { ZentStockProtocol } from "../src/protocol/zent/protocol";
import { MockDevice } from "../src/transport/mock";

const FAST = { timeoutMs: 40, saveDelayMs: 1, resetDelayMs: 1, profilePollMs: 3, heartbeatMs: 20 };

async function setup(version: [number, number, number] = [0, 0, 18]) {
  const dev = new MockDevice({ switchDelayMs: 1, version });
  const kb = new ZentStockProtocol(dev, FAST);
  await kb.connect();
  return { dev, kb };
}

describe("config file", () => {
  it("exports the active profile and parses it back unchanged", async () => {
    const { kb } = await setup();
    await kb.writeProfileName(0, "Daily");
    const file = await exportConfig(kb, { allProfiles: false, includeCalibration: true });
    expect(file.format).toBe(FILE_FORMAT);
    expect(file.device).toMatchObject({ firmwareVersion: "0.0.18", keyCount: 83, ledCount: 85, protocol: "zent-stock" });
    expect(file.profiles.map((p) => [p.index, p.name])).toEqual([[0, "Daily"]]);
    expect(file.calibration?.top.length).toBe(83);
    const parsed = parseConfigFile(JSON.stringify(file), 83, 85);
    expect(parsed).toEqual(file);
  });

  it("exports all four profiles and returns to the active one", async () => {
    const { dev, kb } = await setup();
    await kb.setActiveProfile(1);
    const cfg = await kb.readConfig();
    cfg.keymap.main[0] = 53;
    await kb.writeConfig(cfg);
    await kb.save();
    const file = await exportConfig(kb, { allProfiles: true, includeCalibration: false });
    expect(file.activeProfile).toBe(1);
    expect(file.profiles.map((p) => p.index)).toEqual([0, 1, 2, 3]);
    expect(file.profiles[1].config.keymap.main[0]).toBe(53);
    expect(file.profiles[0].config.keymap.main[0]).toBe(41);
    expect(file.calibration).toBeUndefined();
    expect(dev.activeProfile).toBe(1);
  });

  it("imports into the active profile (RAM only unless asked to save)", async () => {
    const a = await setup();
    const cfg = await a.kb.readConfig();
    cfg.keymap.main[45] = -8;
    cfg.lighting.effect = 30;
    cfg.advanced![3] = { ...cfg.advanced![3], mode: ADV_COMBO, keys: [-8, 6, 1000, 1000] };
    await a.kb.writeConfig(cfg);
    await a.kb.writeProfileName(0, "Src");
    const text = JSON.stringify(await exportConfig(a.kb, { allProfiles: false, includeCalibration: true }));

    const b = await setup();
    await b.kb.setActiveProfile(2);
    const res = await importConfig(b.kb, parseConfigFile(text, 83, 85), {
      mode: "active", restoreNames: true, restoreCalibration: false, save: false,
    });
    expect(res.profiles).toEqual([2]);
    expect(res.saved).toBe(false);
    expect(res.bytesWritten).toBe(2 + 5 + 48 + 83);
    expect(await b.kb.readConfig()).toEqual(cfg);
    expect((await b.kb.readProfileNames())[2]).toBe("Src");
    expect(b.dev.log.saves).toBe(0);
  });

  it("imports all profiles, saving each, and restores calibration on request", async () => {
    const a = await setup();
    for (const i of [0, 3]) {
      await a.kb.setActiveProfile(i);
      const c = await a.kb.readConfig();
      c.keymap.main[1] = 100 + i;
      await a.kb.writeConfig(c);
      await a.kb.save();
    }
    const cal = await a.kb.readCalibration();
    cal.top[7] = 29999;
    await a.kb.writeCalibration(cal);
    await a.kb.setActiveProfile(0);
    const file = await exportConfig(a.kb, { allProfiles: true, includeCalibration: true });

    const b = await setup();
    const res = await importConfig(b.kb, file, { mode: "all", restoreNames: false, restoreCalibration: true, save: true });
    expect(res.profiles).toEqual([0, 1, 2, 3]);
    expect(res.saved).toBe(true);
    expect(b.dev.activeProfile).toBe(0);
    await b.kb.reboot();
    const kb2 = new ZentStockProtocol(b.dev, FAST);
    await kb2.connect();
    expect((await kb2.readConfig()).keymap.main[1]).toBe(100);
    expect((await kb2.readCalibration()).top[7]).toBe(29999);
    await kb2.setActiveProfile(3);
    expect((await kb2.readConfig()).keymap.main[1]).toBe(103);
  });

  it("skips key groups / DKS when importing into firmware without them, and keeps them when the file has none", async () => {
    const a = await setup();
    const file = await exportConfig(a.kb, { allProfiles: false, includeCalibration: false });
    const old = await setup([0, 0, 9]);
    const res = await importConfig(old.kb, file, { mode: "active", restoreNames: false, restoreCalibration: true, save: false });
    expect(res.notes.join(" ")).toMatch(/does not support/);
    expect(res.notes.join(" ")).toMatch(/no calibration/);

    const oldFile = await exportConfig(old.kb, { allProfiles: false, includeCalibration: false });
    expect(oldFile.profiles[0].config.advanced).toBeNull();
    const cfg = await a.kb.readConfig();
    cfg.advanced![1] = { ...cfg.advanced![1], mode: ADV_COMBO, keys: [4, 5, 1000, 1000] };
    await a.kb.writeConfig(cfg);
    await importConfig(a.kb, oldFile, { mode: "active", restoreNames: false, restoreCalibration: false, save: false });
    expect((await a.kb.readConfig()).advanced![1].mode).toBe(ADV_COMBO);
  });

  it("rejects broken files with the path of the problem", async () => {
    const { kb } = await setup();
    const file = await exportConfig(kb, { allProfiles: false, includeCalibration: false });
    const mutate = (fn: (f: any) => void) => {
      const copy = cloneConfig(file) as any;
      fn(copy);
      return JSON.stringify(copy);
    };
    const bad = (text: string, pattern: RegExp) => {
      expect(() => parseConfigFile(text, 83, 85)).toThrow(ConfigFileError);
      expect(() => parseConfigFile(text, 83, 85)).toThrow(pattern);
    };
    bad("{", /not valid JSON/);
    bad("[]", /expected an object/);
    bad(mutate((f) => (f.format = "rgb_config")), /format/);
    bad(mutate((f) => (f.formatVersion = 2)), /unsupported version/);
    bad(mutate((f) => (f.device.keyCount = 61)), /61-key board/);
    bad(mutate((f) => (f.profiles = [])), /at least one profile/);
    bad(mutate((f) => f.profiles.push(f.profiles[0])), /appears twice/);
    bad(mutate((f) => f.profiles[0].config.keymap.main.pop()), /profiles\[0\]\.config\.keymap\.main: expected 83 entries, got 82/);
    bad(mutate((f) => (f.profiles[0].config.actuationUm[5] = 1.5)), /actuationUm\[5\]/);
    bad(mutate((f) => (f.profiles[0].config.socd[2] = "A")), /socd\[2\]/);
    bad(mutate((f) => (f.profiles[0].config.lighting.colors[3].g = 256)), /colors\[3\]\.g/);
    bad(mutate((f) => (f.profiles[0].config.winLock = 1)), /winLock/);
    bad(mutate((f) => (f.profiles[0].config.advanced[0].keys = [1, 2])), /advanced\[0\]\.keys/);
    bad(mutate((f) => delete f.profiles[0].config.lighting), /lighting/);
    bad(mutate((f) => (f.calibration = { top: [], bottom: [], switchType: [] })), /calibration\.top/);
    bad(mutate((f) => (f.activeProfile = 9)), /activeProfile/);
    expect(() => validateProfileConfig(null, 83, 85)).toThrow(ConfigFileError);
  });
});
