/**
 * End-to-end check of the panel's protocol layer against the firmware simulator (firmware/sim):
 * writes tap-hold codes, modifier masks, key_mode 2 and a custom switch curve through
 * ZentStockProtocol over hidraw, then drives keys through the simulator's control socket and
 * checks what the firmware core makes of them (its HID state and engine travel).
 *
 * Only ever opens the simulator's own node: the script starts the simulator itself, takes the
 * hidraw path it prints, and refuses it unless sysfs says the node is 36B0:5A11 ("ZENT75 SIM").
 * The real keyboard (36B0:357E) is never looked up or opened.
 *
 *   cd firmware/sim && cargo build      # once
 *   cd panel && npm run sim-e2e
 *
 * Needs write access to /dev/uhid; the hidraw node is made accessible with `sudo -n chmod 0666`
 * when it is root-only (as tools/tests/test_sim_e2e.py does).
 */
import { spawn, spawnSync } from "node:child_process";
import { accessSync, constants, existsSync, mkdtempSync, rmSync } from "node:fs";
import { createConnection, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { cloneConfig, KEY_MODE_NORMAL, KEY_MODE_RT_BELOW, KEYCODE_EMPTY } from "../src/protocol/keyboard";
import { customSwitchCode, resampleCurve, withCurve } from "../src/protocol/switches";
import { ZentStockProtocol } from "../src/protocol/zent/protocol";
import { HidrawTransport, listHidraw } from "./hidraw-transport";

const SIM_VID = 0x36b0;
const SIM_PID = 0x5a11;
const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const SIM_BIN = join(ROOT, "firmware", "sim", "target", "debug", "zent-sim");
const KEY = { A: 46, S: 47, D: 48, F: 49 } as const;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
function check(ok: boolean, what: string, detail = ""): void {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${detail ? `  (${detail})` : ""}`);
  if (!ok) failures++;
}

class Control {
  private buf = "";
  private waiting: ((lines: string[]) => void) | null = null;
  private lines: string[] = [];
  constructor(private readonly sock: Socket) {
    sock.setEncoding("utf8");
    sock.on("data", (d: string) => {
      this.buf += d;
      let i;
      while ((i = this.buf.indexOf("\n")) >= 0) {
        const line = this.buf.slice(0, i);
        this.buf = this.buf.slice(i + 1);
        if (line === "ok" || line.startsWith("err")) {
          const done = this.waiting;
          const body = this.lines;
          this.waiting = null;
          this.lines = [];
          if (line.startsWith("err")) body.push(line);
          done?.(body);
        } else this.lines.push(line);
      }
    });
  }
  cmd(line: string): Promise<string[]> {
    return new Promise((resolve, reject) => {
      this.waiting = (body) => (body.at(-1)?.startsWith("err") ? reject(new Error(`${line}: ${body.at(-1)}`)) : resolve(body));
      this.sock.write(line + "\n");
    });
  }
  async state(): Promise<Record<string, string>> {
    const out: Record<string, string> = {};
    for (const l of await this.cmd("state")) {
      if (l.startsWith("key ")) out[`key${l.split(" ")[1]}`] = l;
      else {
        const sp = l.indexOf(" ");
        out[l.slice(0, sp)] = l.slice(sp + 1);
      }
    }
    return out;
  }
  async hid(): Promise<{ mods: string; keys: string }> {
    const m = /mods=(\w+) keys=([\w,-]+)/.exec((await this.state()).hid ?? "");
    return { mods: m?.[1] ?? "?", keys: m?.[2] ?? "?" };
  }
  async travelUm(key: number): Promise<number> {
    const l = (await this.state())[`key${key}`];
    return l ? Number(/travel_um=(\d+)/.exec(l)?.[1] ?? 0) : 0;
  }
}

async function main(): Promise<void> {
  if (!existsSync(SIM_BIN)) throw new Error(`${SIM_BIN} missing: run "cargo build" in firmware/sim first`);
  const dir = mkdtempSync(join(tmpdir(), "zent-panel-e2e-"));
  const sockPath = join(process.env.XDG_RUNTIME_DIR ?? dir, `zent-panel-e2e-${process.pid}.sock`);
  const sim = spawn(SIM_BIN, ["--state-dir", join(dir, "state"), "--socket", sockPath], { stdio: ["ignore", "pipe", "pipe"] });
  const log: string[] = [];
  const onLine = (d: Buffer) => log.push(...d.toString().split("\n").filter(Boolean));
  sim.stdout.on("data", onLine);
  sim.stderr.on("data", onLine);
  let kb: ZentStockProtocol | null = null;
  let sock: Socket | null = null;
  try {
    // ---- the simulator's node, and only that one
    let node = "";
    for (let i = 0; i < 100 && !node; i++) {
      node = log.map((l) => /zent-sim: hidraw (\/dev\/hidraw\d+)/.exec(l)?.[1]).find(Boolean) ?? "";
      if (sim.exitCode !== null) throw new Error(`simulator exited:\n${log.join("\n")}`);
      await sleep(100);
    }
    if (!node) throw new Error(`simulator printed no hidraw node:\n${log.join("\n")}`);
    const info = listHidraw().find((c) => c.path === node);
    if (!info || info.vendorId !== SIM_VID || info.productId !== SIM_PID) {
      throw new Error(`${node} is not the simulator (${info ? `${info.vendorId.toString(16)}:${info.productId.toString(16)}` : "not found"}); refusing`);
    }
    try {
      accessSync(node, constants.R_OK | constants.W_OK);
    } catch {
      if (spawnSync("sudo", ["-n", "chmod", "0666", node]).status !== 0) throw new Error(`${node} is root-only and sudo -n chmod failed`);
    }
    console.log(`simulator ${node} (${info.name}, ${SIM_VID.toString(16)}:${SIM_PID.toString(16)})`);

    sock = createConnection(sockPath);
    await new Promise<void>((res, rej) => sock!.once("connect", res).once("error", rej));
    const ctl = new Control(sock);
    for (let i = 0; i < 50 && (await ctl.state()).engine_ready !== "1"; i++) await sleep(100);
    check((await ctl.state()).engine_ready === "1", "engine ready");

    kb = new ZentStockProtocol(new HidrawTransport(node, "ZENT75 SIM"));
    const dev = await kb.connect();
    console.log(`firmware ${dev.firmwareVersion}`);

    // ---- tap-hold + modifier mask + key_mode 2, one writeConfig
    const before = await kb.readConfig();
    const cfg = cloneConfig(before);
    cfg.holdCode.main[KEY.A] = 5; // A: tap a, hold b
    cfg.modMask.main[KEY.S] = 0x02; // S: + LShift
    cfg.keyMode[KEY.D] = KEY_MODE_RT_BELOW;
    // engine travel in the simulator: 0.4 mm -> ~0.9, 1.0 -> ~1.7, 1.2 -> ~1.9, 2.0 -> ~2.5, 2.5 -> ~2.8 mm
    cfg.actuationUm[KEY.D] = 2000;
    cfg.releaseUm[KEY.D] = 1000;
    cfg.rtPressUm[KEY.D] = 150;
    cfg.rtReleaseUm[KEY.D] = 150;
    cfg.topDeadzoneUm[KEY.D] = 100;
    cfg.bottomDeadzoneUm[KEY.D] = 100;
    const written = await kb.writeConfig(cfg);
    check(written > 0, "writeConfig (verified by read-back)", `${written} bytes`);
    check(JSON.stringify(await kb.readConfig()) === JSON.stringify(cfg), "readConfig returns what was written");

    // tap-hold: nothing while pending, hold code after 0.5 s, tap code on a short tap
    await ctl.cmd("press A");
    await sleep(150);
    const pending = await ctl.hid();
    check(pending.keys === "-", "tap-hold: nothing sent while pending", `keys=${pending.keys}`);
    await sleep(600);
    const held = await ctl.hid();
    check(held.keys === "05", "tap-hold: hold code (b) after 0.5 s", `keys=${held.keys}`);
    await ctl.cmd("release A");
    await sleep(100);
    check((await ctl.hid()).keys === "-", "tap-hold: released");
    const mark = log.length;
    await ctl.cmd("tap A 80");
    await sleep(300);
    // keyboard reports (16-byte NKRO: byte 0 modifiers, usage u at byte (u >> 3) + 1, bit u & 7)
    const reports = log.slice(mark).map((l) => /report 81 ([0-9a-f]{32})/.exec(l)?.[1]).filter((x): x is string => !!x);
    const has = (hex: string, usage: number) => (parseInt(hex.slice(2 * ((usage >> 3) + 1), 2 * ((usage >> 3) + 2)), 16) >> (usage & 7)) & 1;
    check(reports.length === 2 && has(reports[0], 4) === 1 && reports.every((h) => !has(h, 5)) && /^0+$/.test(reports[1]),
      "tap-hold: a short tap sends the tap code (a) once, on release, then an empty report", reports.join(" | "));

    // modifier mask
    await ctl.cmd("press S");
    await sleep(100);
    const masked = await ctl.hid();
    check(masked.mods === "02" && masked.keys === "16", "modifier mask: S sends LShift+s", `mods=${masked.mods} keys=${masked.keys}`);
    await ctl.cmd("release S");
    await sleep(100);

    // key_mode 2 on D: normal press at 1.6 mm, RT below it, full reset above 0.8 mm
    const d = async (mm: number) => {
      await ctl.cmd(`travel D ${mm}`);
      await sleep(80);
      const h = await ctl.hid();
      return { on: h.keys.split(",").includes("07"), um: await ctl.travelUm(KEY.D) };
    };
    const steps: [number, boolean, string][] = [
      [1.2, false, "above the actuation point (2.0 mm): not pressed"],
      [2.5, true, "past the actuation point: pressed"],
      [2.0, false, "lifted ~0.27 mm (> RT release 0.15): RT release"],
      [2.5, true, "pressed ~0.27 mm again (> RT press 0.15): RT re-press"],
      [0.8, false, "lifted above the actuation point but below the release point (1.0 mm): RT release, no reset"],
      [1.2, true, "RT re-press above the actuation point (state 'RT-released')"],
      [0.4, false, "back above the release point: full release"],
      [1.2, false, "after the reset the same depth no longer presses"],
      [2.5, true, "actuation point again"],
    ];
    for (const [mm, want, what] of steps) {
      const r = await d(mm);
      check(r.on === want, `key_mode 2: ${what}`, `travel ${mm} mm -> engine ${r.um} µm, pressed ${r.on}`);
    }
    await ctl.cmd("release D");
    // the same moves in normal mode stay pressed on the 0.5 mm lift (contrast)
    const normal = cloneConfig(cfg);
    normal.keyMode[KEY.D] = KEY_MODE_NORMAL;
    await kb.writeConfig(normal);
    await sleep(100);
    await d(2.5);
    check((await d(2.0)).on, "key_mode 0 for contrast: the same ~0.27 mm lift keeps the key pressed");
    await ctl.cmd("release D");
    await sleep(100);

    // ---- custom switch curve: record 0 at 4.0 mm, used by F
    const customs = await kb.readCustomSwitches();
    const rec = withCurve({ ...customs[0], name: "e2e 4mm", travelUm: 4000 }, resampleCurve(customs[0].curve, customs[0].travelUm, 4000));
    const list = customs.map((c, i) => (i === 0 ? rec : c));
    check((await kb.writeCustomSwitches(list)) === 256, "custom switch: one 256-byte record written (bank 1, verified)");
    check(JSON.stringify((await kb.readCustomSwitches())[0]) === JSON.stringify(rec), "custom switch: read back equal");
    const cal = await kb.readCalibration();
    const types = { ...cal, switchType: cal.switchType.map((t, k) => (k === KEY.F ? customSwitchCode(0) : t)) };
    check((await kb.writeCalibration(types)) === 166, "switch type of F -> 4095 (only keytype_code written)");
    await ctl.cmd("press F");
    await ctl.cmd("press G");
    await sleep(150);
    const fUm = await ctl.travelUm(KEY.F);
    const gUm = await ctl.travelUm(KEY.F + 1);
    // the sim's sensor does not reach the stored factory bottom (17000), so neither key shows its
    // full travel; the ratio shows which curve each one runs on (4000 / 3420 = 1.17)
    check(fUm / gUm > 1.12 && fUm / gUm < 1.22, "custom switch: F on the 4.0 mm record travels 4000/3420 times as far as G on the built-in type",
      `F ${fUm} µm, G ${gUm} µm, ratio ${(fUm / gUm).toFixed(3)}`);
    await ctl.cmd("release all");
    // the simulator's banks live in a temp dir; still put things back
    await kb.writeCalibration(cal);
    await kb.writeCustomSwitches(customs);
    await kb.writeConfig(before);
    check(cfg.holdCode.main[KEY.A] !== KEYCODE_EMPTY && JSON.stringify(await kb.readConfig()) === JSON.stringify(before), "profile restored");
  } finally {
    await kb?.disconnect().catch(() => undefined);
    sock?.end();
    sim.kill("SIGTERM");
    await new Promise((r) => (sim.exitCode !== null ? r(null) : sim.once("exit", r)));
    rmSync(dir, { recursive: true, force: true });
  }
  console.log(failures ? `SIM E2E FAILED (${failures})` : "SIM E2E OK");
  process.exit(failures ? 1 : 0);
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
