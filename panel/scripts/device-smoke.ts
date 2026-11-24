/**
 * Read-only smoke test of the shared codec against the real keyboard (or the mock).
 *
 *   npm run smoke                      real keyboard through /dev/hidrawN (auto-discovered)
 *   npm run smoke -- --mock            same code path against the mock device
 *   npm run smoke -- --seconds 5       length of the live-travel capture
 *   npm run smoke -- --retries 10      wait for the keyboard to come back (bootloader, replug)
 *   npm run smoke -- --device /dev/hidraw7
 *
 * Commands sent to a real keyboard: BE 04 (read RAM, only regions of the documented memory
 * map), BE 1D (serial number) and BE 00 (heartbeat). Nothing is written, saved or switched.
 */
import { keyLabel, switchType } from "../src/protocol/keycodes";
import { KEY_LAYOUT } from "../src/protocol/layout";
import type { LiveSample, ProfileConfig } from "../src/protocol/keyboard";
import type { Transport } from "../src/protocol/transport";
import { decodeDeviceFrame } from "../src/protocol/zent/frames";
import { TimeoutError, ZentStockProtocol } from "../src/protocol/zent/protocol";
import { MockDevice } from "../src/transport/mock";
import { DeviceUnavailableError, HidrawTransport, findZentHidraw, listHidraw } from "./hidraw-transport";

interface Args { mock: boolean; seconds: number; retries: number; device: string | null; verbose: boolean }

function parseArgs(argv: string[]): Args {
  const a: Args = { mock: false, seconds: 3, retries: 0, device: null, verbose: false };
  for (let i = 0; i < argv.length; i++) {
    const v = argv[i];
    if (v === "--mock") a.mock = true;
    else if (v === "--verbose") a.verbose = true;
    else if (v === "--seconds") a.seconds = Number(argv[++i]);
    else if (v === "--retries") a.retries = Number(argv[++i]);
    else if (v === "--device") a.device = argv[++i];
    else if (v === "--help" || v === "-h") {
      console.log("usage: device-smoke [--mock] [--seconds N] [--retries N] [--device /dev/hidrawN] [--verbose]");
      process.exit(0);
    } else throw new Error(`unknown argument ${v}`);
  }
  if (!(a.seconds >= 0) || !(a.retries >= 0)) throw new Error("bad numeric argument");
  return a;
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const mm = (um: number) => (um / 1000).toFixed(3);

function range(values: number[]): string {
  const lo = Math.min(...values), hi = Math.max(...values);
  return lo === hi ? `${lo}` : `${lo}..${hi}`;
}

function layerRows(codes: number[]): string[] {
  const rows = new Map<number, string[]>();
  for (const k of KEY_LAYOUT) {
    const label = keyLabel(codes[k.index]) || "·";
    (rows.get(k.y) ?? rows.set(k.y, []).get(k.y)!).push(label);
  }
  return [...rows.values()].map((r) => "    " + r.join(" "));
}

function openTransport(args: Args): Transport {
  if (args.mock) return new MockDevice({ demo: true });
  const found = args.device ? { path: args.device, name: "ZENT75-HE" } : findZentHidraw();
  if (!found) {
    const seen = listHidraw().map((c) => `${c.path}=${c.vendorId.toString(16)}:${c.productId.toString(16)}/if${c.interfaceNumber}`);
    throw new DeviceUnavailableError(
      `no hidraw node for 36b0:357e interface 02 (keyboard unplugged or in its bootloader). Present: ${seen.join(", ") || "none"}`,
    );
  }
  console.log(`device      ${found.path} (${found.name.trim() || "?"})`);
  return new HidrawTransport(found.path, found.name.trim() || "ZENT75-HE");
}

function summarise(cfg: ProfileConfig): void {
  console.log("main layer");
  for (const r of layerRows(cfg.keymap.main)) console.log(r);
  console.log("fn layer");
  for (const r of layerRows(cfg.keymap.fn)) console.log(r);
  const rt = cfg.keyMode.filter((m) => m === 1).length;
  console.log(`key_mode    normal: ${cfg.keyMode.length - rt}, rapid trigger: ${rt}, other: ${cfg.keyMode.filter((m) => m > 1).length}`);
  console.log(`actuation   press ${range(cfg.actuationUm)} µm, release ${range(cfg.releaseUm)} µm`);
  console.log(`rapid trig  press ${range(cfg.rtPressUm)} µm, release ${range(cfg.rtReleaseUm)} µm`);
  console.log(`deadzones   top ${range(cfg.topDeadzoneUm)} µm, bottom ${range(cfg.bottomDeadzoneUm)} µm`);
  const socd = cfg.socd.flatMap((c, k) => (c !== 1000 ? [`${KEY_LAYOUT[k].name}⊘${keyLabel(c)}`] : []));
  console.log(`socd        ${socd.length ? socd.join(", ") : "none"}`);
  if (cfg.advanced) {
    const combo = cfg.advanced.flatMap((a, k) => (a.mode === 1 ? [KEY_LAYOUT[k].name] : []));
    const dks = cfg.advanced.flatMap((a, k) => (a.mode === 2 ? [KEY_LAYOUT[k].name] : []));
    console.log(`advanced    combo keys: ${combo.join(", ") || "none"}; DKS keys: ${dks.join(", ") || "none"}`);
    const a0 = cfg.advanced[0];
    console.log(`            record of key 0: keys ${a0.keys.join(",")} top ${a0.dksTopUm} bottom ${a0.dksBottomUm} ticks ${a0.tapTicks}/${a0.delayTicks}/${a0.randomTicks} reserved ${a0.reserved.join(",")} keep ${a0.keepOriginal}`);
  } else console.log("advanced    not supported by this firmware");
  const l = cfg.lighting;
  console.log(
    `lighting    enabled ${l.enabled}, effect ${l.effect} (flags ${l.effectFlags}), H ${l.hue} S ${l.saturation}, brightness ${l.brightness}, speed ${l.speed}, ` +
      `static LEDs ${l.solid.filter((s) => s === 1).length}/${l.solid.length}, caps ${l.capsIndicator}, fn-layer ${l.fnLayerLighting}`,
  );
  console.log(`settings    sleep ${cfg.sleepSeconds} s, win lock ${cfg.winLock}, auto calibration ${cfg.autoCalibration}`);
}

async function run(args: Args): Promise<void> {
  const transport = openTransport(args);
  const kb = new ZentStockProtocol(transport);
  let extraWords = 0;
  let lastExtra: number[] = [];
  let variants = new Map<string, number>();
  transport.onReport((r) => {
    const f = decodeDeviceFrame(r);
    if (f?.type === "stream") {
      variants.set(f.variant, (variants.get(f.variant) ?? 0) + 1);
      if (f.variant === "travel") {
        extraWords = f.extra.length;
        lastExtra = f.extra;
      }
    }
  });
  try {
    const info = await kb.connect();
    console.log(`firmware    ${info.firmwareVersion} (numeric ${info.versionNumber})`);
    console.log(`serial      ${info.serial || "(no answer)"}`);
    console.log(`product     ${info.productName}, ${info.keyCount} keys, advanced keys: ${info.features.advancedKeys}`);
    const active = await kb.getActiveProfile();
    const names = await kb.readProfileNames();
    console.log(`profile     active ${active} of ${names.map((n, i) => `${i}:${JSON.stringify(n)}`).join(" ")}`);

    const cfg = await kb.readConfig();
    summarise(cfg);

    const cal = await kb.readCalibration();
    console.log(`calibration top ${range(cal.top)}, bottom ${range(cal.bottom)}, switch types ${[...new Set(cal.switchType)].map((t) => `${t}=${switchType(t).name}`).join("; ")}`);

    if (args.seconds > 0) {
      const samples: LiveSample[] = [];
      const t0 = Date.now();
      const stop = kb.startStream((s) => samples.push(s));
      await sleep(args.seconds * 1000);
      stop();
      const dt = (Date.now() - t0) / 1000;
      if (samples.length === 0) throw new TimeoutError("no BE FD stream frames arrived after the heartbeat");
      const maxTravel = new Array<number>(info.keyCount).fill(0);
      const minRaw = new Array<number>(info.keyCount).fill(65535);
      const maxRaw = new Array<number>(info.keyCount).fill(0);
      for (const s of samples) {
        for (let k = 0; k < info.keyCount; k++) {
          if (s.travelUm && s.travelUm[k] > maxTravel[k]) maxTravel[k] = s.travelUm[k];
          if (s.raw[k] < minRaw[k]) minRaw[k] = s.raw[k];
          if (s.raw[k] > maxRaw[k]) maxRaw[k] = s.raw[k];
        }
      }
      const last = samples[samples.length - 1];
      console.log(
        `live        ${samples.length} frames in ${dt.toFixed(1)} s (${(samples.length / dt).toFixed(0)}/s), variants ${[...variants].map(([v, n]) => `${v}:${n}`).join(" ")}, ` +
          `${extraWords} uninterpreted payload words per travel frame`,
      );
      console.log(`            raw at rest ${range(last.raw.map(Math.round))} (noise per key up to ${Math.max(...maxRaw.map((v, k) => v - minRaw[k])).toFixed(0)} counts)`);
      if (args.verbose && lastExtra.length) {
        // the part of the travel frame the vendor driver ignores, in blocks of keyCount words
        for (let b = 0; b * info.keyCount < lastExtra.length; b++) {
          const block = lastExtra.slice(b * info.keyCount, (b + 1) * info.keyCount);
          console.log(`            extra block ${b} (${block.length} words): ${block.join(" ")}`);
        }
        console.log(`            last travel frame raw: ${last.raw.join(" ")}`);
        console.log(`            last travel frame µm:  ${(last.travelUm ?? []).join(" ")}`);
      }
      const moved = maxTravel.flatMap((um, k) => (um >= 100 ? [`${KEY_LAYOUT[k].name} ${mm(um)} mm`] : []));
      console.log(`            keys that moved: ${moved.length ? moved.join(", ") : "none (nothing was pressed)"}`);
    }
    console.log(`stats       sent ${kb.stats.sent}, received ${kb.stats.received}, retries ${kb.stats.retries}, bad frames ${kb.stats.badFrames}` +
      (kb.stats.retried.length ? ` (retried: ${kb.stats.retried.join("; ")})` : ""));
    console.log("SMOKE OK");
  } finally {
    await kb.disconnect().catch(() => undefined);
  }
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  for (let attempt = 0; ; attempt++) {
    try {
      await run(args);
      process.exit(0);
    } catch (e) {
      const soft = e instanceof DeviceUnavailableError || e instanceof TimeoutError;
      if (!soft) throw e;
      console.error(`DEVICE UNAVAILABLE: ${(e as Error).message}`);
      if (attempt >= args.retries) {
        console.error("Nothing was written to the keyboard. Replug it (or wait until it leaves the bootloader) and run the smoke test again.");
        process.exit(2);
      }
      console.error(`retrying in 5 s (${attempt + 1}/${args.retries})`);
      await sleep(5000);
    }
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
