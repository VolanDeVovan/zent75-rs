import { describe, expect, it } from "vitest";
import { ADV_COMBO, ADV_DKS, KEYCODE_EMPTY, KEY_MODE_RAPID_TRIGGER, cloneConfig, type LiveSample } from "../src/protocol/keyboard";
import { crc16Modbus } from "../src/protocol/crc";
import { decodeDeviceFrame, encodeReadRam, encodeWriteRam, padReport } from "../src/protocol/zent/frames";
import { REGIONS, i16le } from "../src/protocol/zent/memory";
import { ProtocolError, TimeoutError, ZentStockProtocol } from "../src/protocol/zent/protocol";
import { MockDevice } from "../src/transport/mock";

const FAST = { timeoutMs: 40, saveDelayMs: 1, resetDelayMs: 1, profilePollMs: 5, heartbeatMs: 20 };

async function setup(mockOptions: ConstructorParameters<typeof MockDevice>[0] = {}, protoOptions = {}) {
  const dev = new MockDevice({ streamIntervalMs: 2, switchDelayMs: 2, ...mockOptions });
  const kb = new ZentStockProtocol(dev, { ...FAST, ...protoOptions });
  const info = await kb.connect();
  return { dev, kb, info };
}

const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));

describe("mock device, raw frames", () => {
  it("answers a read with an echo of address and length", async () => {
    const dev = new MockDevice();
    await dev.open();
    const got: Uint8Array[] = [];
    dev.onReport((r) => got.push(r));
    await dev.send(encodeReadRam(REGIONS.soft_version.address, 4));
    await tick();
    expect(got.length).toBe(1);
    expect(decodeDeviceFrame(got[0])).toEqual({
      type: "read", source: "ram", address: 0x80070210, length: 4, data: Uint8Array.of(18, 0, 0, 0),
    });
  });

  it("acks a write only when the CRC is right", async () => {
    const dev = new MockDevice();
    await dev.open();
    const got: Uint8Array[] = [];
    dev.onReport((r) => got.push(r));
    const good = encodeWriteRam(REGIONS.key_map.address, [57, 0]);
    const bad = good.slice();
    bad[10] = 58; // data changed, CRC not
    await dev.send(bad);
    await tick();
    expect(got.length).toBe(0);
    expect(dev.log.crcErrors).toBe(1);
    expect(i16le(dev.peek(REGIONS.key_map.address, 2))).toEqual([41]);
    await dev.send(good);
    await tick();
    expect(got.map((r) => decodeDeviceFrame(r))).toEqual([{ type: "writeAck" }]);
    expect(i16le(dev.peek(REGIONS.key_map.address, 2))).toEqual([57]);
  });

  it("requires full 1024-byte reports and stays silent on out-of-range reads", async () => {
    const dev = new MockDevice();
    await dev.open();
    await expect(dev.send(new Uint8Array(64))).rejects.toThrow(/1024/);
    const got: Uint8Array[] = [];
    dev.onReport((r) => got.push(r));
    await dev.send(encodeReadRam(0x80040000, 16));
    await dev.send(padReport([0xbe, 0x99, 0x05, 0x00, 0xed]));
    await tick();
    expect(got.length).toBe(0);
    expect(dev.log.unknown).toBe(1);
  });

  it("the write CRC covers header, address, length and data", () => {
    const r = encodeWriteRam(REGIONS.rgb_enable.address, [1]);
    expect(r[11] | (r[12] << 8)).toBe(crc16Modbus(r.subarray(0, 11)));
  });
});

describe("ZentStockProtocol against the mock", () => {
  it("connect reads firmware version and serial number", async () => {
    const { info } = await setup();
    expect(info).toMatchObject({
      protocol: "zent-stock", firmwareVersion: "0.0.18", versionNumber: 18, keyCount: 83, ledCount: 85, profileCount: 4,
      serial: "a0a1a2a3a4a5a6a7a8a9aaabacadaeaf", features: { advancedKeys: true },
    });
  });

  it("reads the full config of the active profile", async () => {
    const { kb } = await setup();
    const cfg = await kb.readConfig();
    expect(cfg.keymap.main[0]).toBe(41);
    expect(cfg.keymap.main[78]).toBe(401);
    expect(cfg.keymap.fn[0]).toBe(319);
    expect(cfg.actuationUm.every((v) => v === 1600)).toBe(true);
    expect(cfg.socd.every((v) => v === KEYCODE_EMPTY)).toBe(true);
    expect(cfg.advanced?.length).toBe(83);
    expect(cfg.lighting.effect).toBe(3);
    expect(cfg.sleepSeconds).toBe(600);
  });

  it("writes only what changed, verifies it, and reads it back identically", async () => {
    const { dev, kb } = await setup();
    const cfg = await kb.readConfig();
    const next = cloneConfig(cfg);
    next.keymap.main[45] = -8; // Caps -> LCtrl
    next.keymap.fn[16] = 58; // Fn+1 -> F1
    next.keyMode[32] = KEY_MODE_RAPID_TRIGGER;
    next.rtPressUm[32] = 150;
    next.rtReleaseUm[32] = 200;
    next.topDeadzoneUm[32] = 50;
    next.bottomDeadzoneUm[32] = 60;
    next.actuationUm[76] = 2000;
    next.releaseUm[76] = 1800;
    next.socd[46] = 7; // A cancels D
    next.socd[48] = 4; // D cancels A
    next.lighting.effect = 21;
    next.lighting.brightness = 6;
    next.lighting.colors[84] = { r: 255, g: 0, b: 16 };
    next.lighting.solid[84] = 1;
    next.lighting.enabled = false;
    next.sleepSeconds = 1800;
    next.winLock = true;
    next.autoCalibration = false;
    const before = dev.log.bytesWritten;
    const written = await kb.writeConfig(next);
    // 2+2 keymap, 1 mode, 4x2 rt, 2x2 normal, 2x2 socd, 5 rgb_config, 3 colour, 1 solid, 1 enable, 4 sleep, 1+1 flags
    expect(written).toBe(2 + 2 + 1 + 8 + 4 + 4 + 5 + 3 + 1 + 1 + 4 + 1 + 1);
    expect(dev.log.bytesWritten - before).toBe(written);
    expect(i16le(dev.peek(REGIONS.key_map.address + 90, 2))).toEqual([-8]);
    expect(Array.from(dev.peek(REGIONS.rgb_config.address, 5))).toEqual([1 + 4 * 21, 188, 255, 6, 50]);
    expect(Array.from(dev.peek(REGIONS.sleep_second_cnt.address, 4))).toEqual([0x08, 0x07, 0, 0]);
    // a second protocol instance sees exactly the same config
    const kb2 = new ZentStockProtocol(dev, FAST);
    await kb2.connect();
    expect(await kb2.readConfig()).toEqual(next);
    // nothing left to write
    expect(await kb.writeConfig(next)).toBe(0);
  });

  it("round-trips combo and DKS bindings (48-byte records + mode byte)", async () => {
    const { dev, kb } = await setup();
    const cfg = await kb.readConfig();
    const next = cloneConfig(cfg);
    next.advanced![57] = { ...next.advanced![57], mode: ADV_COMBO, keys: [-8, -7, 41, KEYCODE_EMPTY], keepOriginal: 0 };
    next.advanced![46] = {
      ...next.advanced![46], mode: ADV_DKS, actions: [[2, 0, 0, 0], [0, 1, 1, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
      keys: [4, -7, KEYCODE_EMPTY, KEYCODE_EMPTY], dksTopUm: 800, dksBottomUm: 3200, tapTicks: 16, delayTicks: 8, randomTicks: 0,
    };
    expect(await kb.writeConfig(next)).toBe(48 + 48 + 2);
    expect(Array.from(dev.peek(REGIONS.bind_key_mode.address + 46, 1))).toEqual([2]);
    expect(Array.from(dev.peek(REGIONS.bind_key_setting.address + 48 * 57 + 16, 8))).toEqual([0xf8, 0xff, 0xf9, 0xff, 41, 0, 0xe8, 0x03]);
    expect(dev.peek(REGIONS.bind_key_setting.address + 48 * 57 + 47, 1)[0]).toBe(0);
    expect(await kb.readConfig()).toEqual(next);
  });

  it("firmware below 0.0.18 has no advanced keys and the bind regions are never touched", async () => {
    const { kb, info, dev } = await setup({ version: [0, 0, 9] });
    expect(info.firmwareVersion).toBe("0.0.9");
    expect(info.features.advancedKeys).toBe(false);
    const cfg = await kb.readConfig();
    expect(cfg.advanced).toBeNull();
    cfg.keymap.main[1] = 59;
    expect(await kb.writeConfig(cfg)).toBe(2);
    await expect(kb.readRam(REGIONS.bind_key_mode.address, 83)).rejects.toThrow(TimeoutError);
    expect(dev.log.writes).toBe(1);
  });

  it("save copies RAM to flash; reboot without save loses the edit", async () => {
    const { dev, kb } = await setup();
    const cfg = await kb.readConfig();
    cfg.keymap.main[0] = 53;
    await kb.writeConfig(cfg);
    expect(i16le(dev.peekFlash(REGIONS.key_map.address, 2))).toEqual([41]);
    await kb.save();
    expect(dev.log.saves).toBe(1);
    expect(i16le(dev.peekFlash(REGIONS.key_map.address, 2))).toEqual([53]);
    cfg.keymap.main[0] = 4;
    await kb.writeConfig(cfg);
    let gone: unknown = null;
    kb.on("disconnect", (r) => (gone = r));
    await kb.reboot();
    await tick();
    expect(gone).toBeNull(); // we disconnected ourselves
    const kb2 = new ZentStockProtocol(dev, FAST);
    await kb2.connect();
    expect((await kb2.readConfig()).keymap.main[0]).toBe(53);
  });

  it("switches profiles, waits for the board, and keeps profiles separate", async () => {
    const { dev, kb } = await setup();
    const notices: number[] = [];
    kb.on("profile", (p) => notices.push(p));
    expect(await kb.getActiveProfile()).toBe(0);
    const cfg0 = await kb.readConfig();
    cfg0.keymap.main[1] = 104;
    await kb.writeConfig(cfg0);
    await kb.save();
    await kb.setActiveProfile(2);
    expect(await kb.getActiveProfile()).toBe(2);
    expect(dev.activeProfile).toBe(2);
    expect(notices).toEqual([2]);
    const cfg2 = await kb.readConfig();
    expect(cfg2.keymap.main[1]).toBe(58);
    cfg2.keymap.main[1] = 105;
    await kb.writeConfig(cfg2); // the diff is against profile 2, not the stale profile 0 cache
    expect(i16le(dev.peek(REGIONS.key_map.address + 2, 2))).toEqual([105]);
    await kb.setActiveProfile(0);
    expect((await kb.readConfig()).keymap.main[1]).toBe(104);
    await expect(kb.setActiveProfile(4)).rejects.toThrow(RangeError);
  });

  it("reports a profile switch made on the keyboard itself", async () => {
    const { dev, kb } = await setup();
    const notices: number[] = [];
    kb.on("profile", (p) => notices.push(p));
    dev.userSwitchesProfile(3);
    await tick();
    expect(notices).toEqual([3]);
    expect(await kb.getActiveProfile()).toBe(3);
  });

  it("reads and writes profile names", async () => {
    const { dev, kb } = await setup();
    expect(await kb.readProfileNames()).toEqual(["Config1", "Config2", "Config3", "Config4"]);
    await kb.writeProfileName(1, "FPS");
    await kb.writeProfileName(3, "Работа");
    expect(await kb.readProfileNames()).toEqual(["Config1", "FPS", "Config3", "Работа"]);
    expect(Array.from(dev.peek(0x80070290 + 64, 4))).toEqual([0x46, 0x50, 0x53, 0]);
    await kb.writeProfileName(1, "A"); // shorter name must not leave old bytes behind
    expect((await kb.readProfileNames())[1]).toBe("A");
    await expect(kb.writeProfileName(4, "x")).rejects.toThrow(RangeError);
    await expect(kb.writeProfileName(0, "")).rejects.toThrow(RangeError);
  });

  it("reset config restores the factory profile in RAM", async () => {
    const { kb } = await setup();
    const factory = await kb.readConfig();
    const cfg = cloneConfig(factory);
    cfg.keymap.main[5] = 4;
    cfg.lighting.brightness = 1;
    await kb.writeConfig(cfg);
    await kb.resetConfig();
    expect(await kb.readConfig()).toEqual(factory);
  });

  it("calibration data: read, write, verify", async () => {
    const { dev, kb } = await setup();
    const cal = await kb.readCalibration();
    expect(cal.top[0]).toBe(31000);
    expect(cal.bottom[0]).toBe(17000);
    cal.top[73] = 30123;
    cal.bottom[73] = 18456;
    cal.switchType[73] = 1;
    expect(await kb.writeCalibration(cal)).toBe(6);
    expect(Array.from(dev.peek(REGIONS.top_value.address + 146, 2))).toEqual([30123 & 0xff, 30123 >> 8]);
    expect(await kb.readCalibration()).toEqual(cal);
  });

  it("streams travel frames while heartbeats run, raw frames in calibration mode", async () => {
    const { dev, kb } = await setup();
    const samples: LiveSample[] = [];
    dev.press(73, 0.5);
    const stop = kb.startStream((s) => samples.push(s));
    await tick(40);
    expect(dev.log.heartbeats).toBeGreaterThanOrEqual(2);
    expect(samples.length).toBeGreaterThan(3);
    const s = samples[samples.length - 1];
    expect(s.kind).toBe("travel");
    expect(s.raw.length).toBe(83);
    expect(s.travelUm![73]).toBeGreaterThan(1000);
    expect(s.travelUm![73]).toBeLessThan(2400);
    expect(s.travelUm![0]).toBe(0);

    await kb.setCalibrationMode(true);
    await tick(20);
    expect(dev.calibrationMode).toBe(true);
    const r = samples[samples.length - 1];
    expect(r.kind).toBe("raw");
    expect(r.travelUm).toBeNull();
    expect(r.raw[73]).toBeLessThan(26000);
    await kb.setCalibrationMode(false);
    await tick(10);
    expect(samples[samples.length - 1].kind).toBe("travel");

    stop();
    await tick(10);
    const n = samples.length;
    const beats = dev.log.heartbeats;
    await tick(60);
    expect(samples.length).toBe(n);
    expect(dev.log.heartbeats).toBe(beats);
  });

  it("memory requests still work while the stream is flowing", async () => {
    const { kb } = await setup();
    const stop = kb.startStream(() => undefined);
    await tick(10);
    const cfg = await kb.readConfig();
    cfg.keymap.main[2] = 60;
    await kb.writeConfig(cfg);
    expect((await kb.readConfig()).keymap.main[2]).toBe(60);
    expect(kb.stats.streamFrames).toBeGreaterThan(0);
    stop();
  });

  it("retries after a lost reply and after a corrupted one", async () => {
    const { dev, kb } = await setup();
    dev.dropReplies = 1;
    expect(await kb.getActiveProfile()).toBe(0);
    expect(kb.stats.retries).toBe(1);
    dev.corruptReplies = 2;
    expect((await kb.readProfileNames()).length).toBe(4);
    expect(kb.stats.retries).toBe(3);
    expect(kb.stats.badFrames).toBe(2);
    dev.dropReplies = 3;
    await expect(kb.getActiveProfile()).rejects.toThrow(TimeoutError);
    expect(await kb.getActiveProfile()).toBe(0);
  });

  it("refuses reads and writes outside the known memory map", async () => {
    const { dev, kb } = await setup();
    const sent = dev.log.requests;
    await expect(kb.readRam(0x80040000, 16)).rejects.toThrow(ProtocolError);
    await expect(kb.readFlash(0x80000000, 16)).rejects.toThrow(/outside the known memory map/);
    await expect(kb.readRam(REGIONS.key_map.address, 167)).rejects.toThrow(ProtocolError);
    await expect(kb.writeRam(REGIONS.soft_version.address, Uint8Array.of(1, 0, 0, 0))).rejects.toThrow(/read-only/);
    await expect(kb.writeRam(0x80080c6a, Uint8Array.of(1))).rejects.toThrow(ProtocolError);
    await tick();
    expect(dev.log.requests).toBe(sent);
  });

  it("bootloader makes the device disappear until it is replugged", async () => {
    const { dev, kb } = await setup();
    await kb.enterBootloader();
    await tick();
    expect(dev.state).toBe("bootloader");
    const kb2 = new ZentStockProtocol(dev, FAST);
    await expect(kb2.connect()).rejects.toThrow(/bootloader/);
    dev.replug();
    expect((await kb2.connect()).firmwareVersion).toBe("0.0.18");
  });

  it("emits disconnect when the device goes away on its own", async () => {
    const { dev, kb } = await setup();
    let reason: unknown = null;
    kb.on("disconnect", (r) => (reason = r));
    // someone else reboots the board
    const other = new ZentStockProtocol(dev, FAST);
    await other.connect();
    await other.reboot();
    await tick();
    expect(reason).toBe("reboot");
    await expect(kb.getActiveProfile()).rejects.toThrow(/not connected/);
  });
});
