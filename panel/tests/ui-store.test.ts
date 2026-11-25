import { describe, expect, it } from "vitest";
import { REGIONS } from "../src/protocol/zent/memory";
import { ZentStockProtocol } from "../src/protocol/zent/protocol";
import { MockDevice } from "../src/transport/mock";
import { Store } from "../src/ui/store";

async function connected(version: [number, number, number] = [0, 0, 18]) {
  const mock = new MockDevice({ version, switchDelayMs: 0 });
  const store = new Store((t) => new ZentStockProtocol(t, { saveDelayMs: 0, resetDelayMs: 0 }));
  const toasts: string[] = [];
  store.onToast((kind, text) => toasts.push(`${kind}: ${text}`));
  await store.attach(mock);
  expect(store.connected).toBe(true);
  return { mock, store, toasts };
}

const u16 = (b: Uint8Array, i: number) => b[2 * i] | (b[2 * i + 1] << 8);
const flashActuation = (mock: MockDevice, key: number) => u16(mock.peekFlash(REGIONS.norm_press_th.address, REGIONS.norm_press_th.length), key);
const ramActuation = (mock: MockDevice, key: number) => u16(mock.peek(REGIONS.norm_press_th.address, REGIONS.norm_press_th.length), key);

describe("Store: save state", () => {
  it("starts saved; an edit goes to RAM at once and is listed as unsaved", async () => {
    const { mock, store } = await connected();
    expect(store.dirty).toBe(false);
    expect(store.canUndo).toBe(false);
    expect(store.profileCount).toBe(store.info!.profileCount);
    await store.edit("actuation", (c) => { c.actuationUm[3] = 1200; });
    expect(ramActuation(mock, 3)).toBe(1200);
    expect(flashActuation(mock, 3)).toBe(1600);
    expect(store.dirty).toBe(true);
    expect(store.unsavedAreas).toEqual(["actuation"]);
    expect(store.undoLabel).toBe("actuation");
  });

  it("save persists and clears the unsaved state", async () => {
    const { mock, store } = await connected();
    await store.edit("actuation", (c) => { c.actuationUm[3] = 1200; });
    await store.save();
    expect(flashActuation(mock, 3)).toBe(1200);
    expect(store.dirty).toBe(false);
    expect(mock.log.saves).toBe(1);
  });

  it("an edit that changes nothing writes nothing and leaves no undo step", async () => {
    const { mock, store } = await connected();
    const writes = mock.log.writes;
    await store.edit("actuation", (c) => { c.actuationUm[3] = 1600; });
    expect(mock.log.writes).toBe(writes);
    expect(store.canUndo).toBe(false);
  });

  it("quick successive edits are all applied", async () => {
    const { mock, store } = await connected();
    const a = store.edit("actuation", (c) => { c.actuationUm[1] = 1000; });
    const b = store.edit("actuation", (c) => { c.actuationUm[2] = 1100; });
    const l = store.edit("brightness", (c) => { c.lighting.brightness = 4; });
    await Promise.all([a, b, l]);
    expect(ramActuation(mock, 1)).toBe(1000);
    expect(ramActuation(mock, 2)).toBe(1100);
    expect(store.config!.lighting.brightness).toBe(4);
    expect(store.unsavedAreas).toEqual(["actuation", "lighting"]);
  });
});

describe("Store: undo and discard", () => {
  it("undo steps back one edit at a time, down to the saved state", async () => {
    const { mock, store } = await connected();
    await store.edit("actuation", (c) => { c.actuationUm[3] = 1200; });
    await store.edit("brightness", (c) => { c.lighting.brightness = 4; });
    await store.undo();
    expect(store.config!.lighting.brightness).toBe(10);
    expect(store.unsavedAreas).toEqual(["actuation"]);
    await store.undo();
    expect(ramActuation(mock, 3)).toBe(1600);
    expect(store.dirty).toBe(false);
    expect(store.canUndo).toBe(false);
    await store.undo(); // nothing left: no error
    expect(store.dirty).toBe(false);
  });

  it("undo after a save makes the profile unsaved again", async () => {
    const { mock, store } = await connected();
    await store.edit("actuation", (c) => { c.actuationUm[3] = 1200; });
    await store.save();
    await store.undo();
    expect(ramActuation(mock, 3)).toBe(1600);
    expect(flashActuation(mock, 3)).toBe(1200);
    expect(store.dirty).toBe(true);
  });

  it("discard puts the saved profile back and can itself be undone", async () => {
    const { mock, store } = await connected();
    await store.edit("actuation", (c) => { c.actuationUm[3] = 1200; });
    await store.edit("SOCD", (c) => { c.socd[46] = 7; });
    await store.discard();
    expect(ramActuation(mock, 3)).toBe(1600);
    expect(store.dirty).toBe(false);
    await store.undo();
    expect(ramActuation(mock, 3)).toBe(1200);
    expect(store.config!.socd[46]).toBe(7);
    expect(store.unsavedAreas).toEqual(["actuation", "SOCD"]);
  });
});

describe("Store: profiles", () => {
  it("a profile switch loads the other profile as saved and forgets the undo history", async () => {
    const { mock, store } = await connected();
    await store.edit("actuation", (c) => { c.actuationUm[3] = 1200; });
    await store.switchProfile(1);
    expect(mock.activeProfile).toBe(1);
    expect(store.activeProfile).toBe(1);
    expect(store.dirty).toBe(false);
    expect(store.canUndo).toBe(false);
    expect(store.config!.actuationUm[3]).toBe(1600);
  });

  it("follows a profile switch made on the keyboard", async () => {
    const { mock, store } = await connected();
    mock.userSwitchesProfile(2);
    await new Promise((r) => setTimeout(r, 50));
    await store.run("wait", async () => undefined);
    expect(store.activeProfile).toBe(2);
    expect(store.dirty).toBe(false);
  });

  it("reset profile stores factory values at once and keeps the old profile for undo", async () => {
    const { mock, store } = await connected();
    await store.edit("actuation", (c) => { c.actuationUm[3] = 1200; });
    await store.save();
    await store.resetProfile();
    expect(store.config!.actuationUm[3]).toBe(1600);
    expect(flashActuation(mock, 3)).toBe(1600);
    expect(store.dirty).toBe(false);
    await store.undo();
    expect(ramActuation(mock, 3)).toBe(1200);
    expect(store.dirty).toBe(true);
    await store.save();
    expect(flashActuation(mock, 3)).toBe(1200);
  });

  it("renaming is stored by the keyboard at once and is not an unsaved change", async () => {
    const { store } = await connected();
    await store.renameProfile(1, "game");
    expect(store.profileNames[1]).toBe("game");
    expect(store.dirty).toBe(false);
  });
});

describe("Store: board-global data and firmware gating", () => {
  it("switch types are written at once, outside the unsaved state", async () => {
    const { mock, store } = await connected();
    const saves = mock.log.globalSaves;
    await store.editCalibration("switch type", (c) => { c.switchType[0] = 0; });
    expect(store.calibration!.switchType[0]).toBe(0);
    expect(mock.log.globalSaves).toBeGreaterThan(saves);
    expect(store.dirty).toBe(false);
  });

  it("an invalid custom switch record is refused and the keyboard's records are kept", async () => {
    const { store, toasts } = await connected();
    const before = structuredClone(store.customSwitches);
    await store.editCustomSwitches("custom switch", (list) => { list[0].travelUm = 99; });
    expect(store.customSwitches).toEqual(before);
    expect(toasts.some((t) => t.startsWith("error:"))).toBe(true);
  });

  it("tap-hold codes above 1000 are refused and RAM is left as it was", async () => {
    const { store, toasts } = await connected();
    await store.edit("tap-hold", (c) => { c.holdCode.main[0] = 1001; });
    expect(store.config!.holdCode.main[0]).toBe(1000);
    expect(store.dirty).toBe(false);
    expect(store.canUndo).toBe(false);
    expect(toasts.some((t) => t.startsWith("error:"))).toBe(true);
  });

  it("older firmware has no advanced keys", async () => {
    const { store } = await connected([0, 0, 9]);
    expect(store.info!.features.advancedKeys).toBe(false);
    expect(store.config!.advanced).toBeNull();
    await store.edit("lighting", (c) => { c.lighting.enabled = false; });
    expect(store.unsavedAreas).toEqual(["lighting"]);
  });

  it("calibration finish saves, which also stores unsaved profile edits", async () => {
    const { mock, store } = await connected();
    await store.edit("actuation", (c) => { c.actuationUm[3] = 1200; });
    await store.startCalibration(undefined);
    expect(mock.calibrationMode).toBe(true);
    mock.emitStreamFrame();
    mock.press(5, 1);
    for (let i = 0; i < 5; i++) mock.emitStreamFrame();
    await new Promise((r) => setTimeout(r, 20));
    const keys = await store.finishCalibration();
    expect(keys).toEqual([5]);
    expect(mock.calibrationMode).toBe(false);
    expect(flashActuation(mock, 3)).toBe(1200);
    expect(store.dirty).toBe(false);
  });

  it("disconnect clears the state", async () => {
    const { store } = await connected();
    await store.disconnect();
    expect(store.connected).toBe(false);
    expect(store.dirty).toBe(false);
  });
});
