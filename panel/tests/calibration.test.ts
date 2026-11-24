import { describe, expect, it } from "vitest";
import { CalibrationFlow, CalibrationSession } from "../src/protocol/calibration";
import { REGIONS, u16le } from "../src/protocol/zent/memory";
import { ZentStockProtocol } from "../src/protocol/zent/protocol";
import { MockDevice } from "../src/transport/mock";

const before = (type = 2) => ({
  top: new Array(83).fill(31000), bottom: new Array(83).fill(17000), switchType: new Array(83).fill(type),
});
const rest = () => new Array<number>(83).fill(30000);
const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));

describe("CalibrationSession (vendor acceptance rules)", () => {
  it("accepts a key once its raw span exceeds 7000 counts and leaves the others alone", () => {
    const s = new CalibrationSession(before());
    for (let i = 0; i < 5; i++) s.feed(rest().map((v, k) => v + (k % 3)));
    expect(s.acceptedCount).toBe(0);
    const frame = rest();
    frame[73] = 24000; // 6000 below rest: not enough
    s.feed(frame);
    expect(s.accepted[73]).toBe(false);
    frame[73] = 18200;
    s.feed(frame);
    expect(s.accepted[73]).toBe(true);
    expect(s.acceptedCount).toBe(1);
    const r = s.result();
    expect(r.top[73]).toBe(30000 + (73 % 3));
    expect(r.bottom[73]).toBe(18200);
    expect(r.top[0]).toBe(31000);
    expect(r.bottom[0]).toBe(17000);
  });

  it("switch type 3 needs a span above 8000", () => {
    const s = new CalibrationSession(before(3));
    s.feed(rest());
    const f = rest();
    f[10] = 22500;
    s.feed(f);
    expect(s.requiredSpan(10)).toBe(8000);
    expect(s.accepted[10]).toBe(false);
    f[10] = 21900;
    s.feed(f);
    expect(s.accepted[10]).toBe(true);
  });

  it("freezes the rest level once any key has moved more than 3000 counts", () => {
    const s = new CalibrationSession(before());
    s.feed(rest());
    const f = rest();
    f[5] = 26000; // key 5 moves by 4000 -> rest levels freeze from the next sample on
    s.feed(f);
    const g = rest();
    g[5] = 18000;
    g[6] = 30900; // a later upward glitch on another key must not raise its rest level
    s.feed(g);
    expect(s.max[6]).toBe(30000);
    g[6] = 20000;
    s.feed(g);
    expect(s.result().top[6]).toBe(30000);
    expect(s.result().bottom[6]).toBe(20000);
    expect(s.result().top[5]).toBe(30000);
  });

  it("handles sensors of the opposite polarity (rest above 32768): top = min, bottom = max", () => {
    const s = new CalibrationSession(before());
    const hi = new Array<number>(83).fill(40000);
    s.feed(hi);
    expect(s.restIsHigh[0]).toBe(true);
    const f = [...hi];
    f[0] = 48500;
    s.feed(f);
    expect(s.accepted[0]).toBe(true);
    expect(s.result().top[0]).toBe(40000);
    expect(s.result().bottom[0]).toBe(48500);
  });

  it("rejects frames of the wrong size", () => {
    expect(() => new CalibrationSession(before()).feed([1, 2, 3])).toThrow(RangeError);
  });
});

describe("CalibrationFlow against the mock", () => {
  async function setup() {
    const dev = new MockDevice({ streamIntervalMs: 2 });
    const kb = new ZentStockProtocol(dev, { timeoutMs: 40, saveDelayMs: 1, heartbeatMs: 20 });
    await kb.connect();
    return { dev, kb, flow: new CalibrationFlow(kb, 1) };
  }

  it("on -> collect -> off -> write top/bot -> save", async () => {
    const { dev, kb, flow } = await setup();
    const session = await flow.start();
    await tick(15);
    expect(dev.calibrationMode).toBe(true);
    expect(session.frames).toBeGreaterThan(2);
    expect(session.acceptedCount).toBe(0);
    dev.press(59, 1);
    dev.press(73, 1);
    dev.press(10, 0.3); // a light touch is not enough
    await tick(15);
    dev.press(59, 0);
    dev.press(73, 0);
    await tick(10);
    expect(session.accepted[59] && session.accepted[73]).toBe(true);
    expect(session.accepted[10]).toBe(false);

    const done = await flow.finish();
    expect(done).toEqual([59, 73]);
    expect(dev.calibrationMode).toBe(false);
    expect(dev.log.saves).toBe(1);
    const cal = await kb.readCalibration();
    for (const k of [59, 73]) {
      expect(Math.abs(cal.top[k] - dev.sensorTop[k])).toBeLessThan(20);
      expect(Math.abs(cal.bottom[k] - dev.sensorBottom[k])).toBeLessThan(20);
    }
    expect([cal.top[10], cal.bottom[10]]).toEqual([31000, 17000]);
    // persisted
    expect(u16le(dev.peekFlash(REGIONS.top_value.address + 2 * 73, 2))[0]).toBe(cal.top[73]);
    expect(flow.running).toBe(false);
  });

  it("cancel leaves the stored calibration untouched", async () => {
    const { dev, kb, flow } = await setup();
    await flow.start();
    dev.press(0, 1);
    await tick(15);
    await flow.cancel();
    await tick(5);
    expect(dev.calibrationMode).toBe(false);
    expect(dev.log.writes).toBe(0);
    expect(dev.log.saves).toBe(0);
    expect((await kb.readCalibration()).top[0]).toBe(31000);
  });

  it("finishing with no calibrated key writes nothing", async () => {
    const { dev, flow } = await setup();
    await flow.start();
    await tick(10);
    expect(await flow.finish()).toEqual([]);
    expect(dev.log.writes).toBe(0);
    expect(dev.log.saves).toBe(0);
    await expect(flow.finish()).rejects.toThrow(/not running/);
  });
});
