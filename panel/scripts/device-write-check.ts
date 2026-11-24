/**
 * Harmless WRITE round-trip against the real keyboard, driven entirely through the panel's own
 * KeyboardProtocol stack (same codec the WebHID UI uses) over the Node hidraw transport.
 *
 * It proves the panel can edit + save + restore on real hardware (GOAL "Done when" for Track B)
 * without disturbing the user's config: it only touches a profile name, then puts it back.
 *
 *   npm run write-check                 real keyboard (auto-discovered)
 *   npm run write-check -- --mock       same code path against the mock
 *   npm run write-check -- --slot 1     which profile-name slot to poke (default 1 = "game")
 *
 * Steps: connect -> read all names -> write a probe into one slot -> save (BE 02) -> read back and
 * assert it took -> write the original name back -> save -> assert byte-identical. Exits non-zero
 * and tries to restore if anything mismatches.
 */
import { ZentStockProtocol } from "../src/protocol/zent/protocol";
import { MockDevice } from "../src/transport/mock";
import { DeviceUnavailableError, HidrawTransport, findZentHidraw } from "./hidraw-transport";
import type { Transport } from "../src/protocol/transport";

interface Args { mock: boolean; slot: number; device: string | null }

function parseArgs(argv: string[]): Args {
  const a: Args = { mock: false, slot: 1, device: null };
  for (let i = 0; i < argv.length; i++) {
    const v = argv[i];
    if (v === "--mock") a.mock = true;
    else if (v === "--slot") a.slot = Number(argv[++i]);
    else if (v === "--device") a.device = argv[++i];
    else if (v === "--help" || v === "-h") {
      console.log("usage: device-write-check [--mock] [--slot 0..3] [--device /dev/hidrawN]");
      process.exit(0);
    } else throw new Error(`unknown argument ${v}`);
  }
  if (!(a.slot >= 0 && a.slot <= 3)) throw new Error("--slot must be 0..3");
  return a;
}

function openTransport(args: Args): Transport {
  if (args.mock) return new MockDevice({ demo: true });
  const found = args.device ? { path: args.device, name: "ZENT75-HE" } : findZentHidraw();
  if (!found) throw new DeviceUnavailableError("no hidraw node for 36b0:357e interface 02 (unplugged or in bootloader)");
  console.log(`device   ${found.path} (${found.name.trim() || "?"})`);
  return new HidrawTransport(found.path, found.name.trim() || "ZENT75-HE");
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const proto = new ZentStockProtocol(openTransport(args));
  const info = await proto.connect();
  console.log(`firmware ${info.firmwareVersion}  serial ${info.serial}`);

  const names = await proto.readProfileNames();
  console.log(`names    ${JSON.stringify(names)}`);
  const slot = args.slot;
  const original = names[slot];
  const probe = "PANEL_WR_CHK";

  let rc = 0;
  try {
    await proto.writeProfileName(slot, probe);
    await proto.save();
    const after = await proto.readProfileNames();
    if (after[slot] !== probe) throw new Error(`write/save did not take: slot ${slot} = ${after[slot]}`);
    console.log(`write    slot ${slot} "${original}" -> "${probe}" + save: read back OK`);
  } finally {
    await proto.writeProfileName(slot, original);
    await proto.save();
  }
  const restored = await proto.readProfileNames();
  const ok = restored[slot] === original && restored.join("\u0000") === names.join("\u0000");
  console.log(`restore  slot ${slot} -> "${original}" + save: byte-identical ${ok}`);
  if (!ok) rc = 1;

  await proto.disconnect();
  console.log(rc === 0 ? "WRITE-CHECK OK" : "WRITE-CHECK FAILED");
  process.exit(rc);
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(2);
});
