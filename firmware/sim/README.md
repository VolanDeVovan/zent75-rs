# zent-sim — host simulator for the firmware core

Runs `zent_core::device::Keyboard` (`firmware/core`) on Linux in real time, with a Hall-sensor
model, persistent config banks and a **virtual HID device** created through `/dev/uhid`. The
Python tools, the panel and anything else that speaks the configurator protocol can talk to it
like to the board — without flashing anything.

The simulator enumerates as **`36B0:5A11` "ZENT75 SIM"** (bus USB, `HID_PHYS=zent75-sim/vendor`).
The real board is `36B0:357E`; nothing defaults to the simulator and the flashing tools refuse it.

## Build and run

```sh
cd firmware/sim            # run cargo from here: firmware/rust-toolchain.toml selects nightly
cargo build
./target/debug/zent-sim                    # state in a temp dir, socket $XDG_RUNTIME_DIR/zent75-sim.sock
./target/debug/zent-sim --state-dir ~/.local/state/zent-sim --load-backup ../../backups/1795545170
cargo test                                  # unit + integration tests (headless, no uhid needed)
```

Startup prints machine-readable lines, e.g.

```
zent-sim: state-dir /tmp/zent75-sim-state-1234 (temporary)
zent-sim: socket /run/user/1000/zent75-sim.sock
zent-sim: uhid vendor device created (36B0:5A11 "ZENT75 SIM")
zent-sim: hidraw /dev/hidraw9
```

| Option | |
|---|---|
| `--state-dir DIR` | config banks live here (`global.bin`, `profile0..3.bin`); default: temp dir removed on exit |
| `--load-backup DIR` | seed all four profiles + bank 1 from a `tools/dump_all.py` backup before boot (overwrites the state dir) |
| `--socket PATH` | control socket |
| `--script FILE` / `--stay` | run control commands from a file, then quit (or keep running) |
| `--noise N` | ± N ADC counts of uniform noise (default 30) |
| `--slew MM_PER_S` | key travel speed (default 200 mm/s ≈ 17 ms rest → bottom; 0 = instant) |
| `--seed N` | per-key rest/bottom variation |
| `--no-uhid` | no virtual device, control socket only |
| `--hid-keyboard` | also create a keyboard/consumer/system/mouse device that **types into your desktop session**. Off by default; without it HID state changes are only logged (`zent-sim: hid mods=.. keys=..`) |
| `--sudo-chmod` | `sudo -n chmod 0666` the hidraw node when it appears (see permissions) |
| `--bootloader-ms N` | how long the device stays away after `BE FC` (default 2000) |
| `--verbose` | log every vendor request |

### What is simulated

- **Scan loop**: 8000 `scan()` per second, run in 1 ms batches against the wall clock; after each
  batch `vendor_poll` (pushed `BE C0` notices, `BE FD` stream), `service` and system actions;
  `render_leds` at 60 Hz.
- **Sensor**: per key, rest ≈ 30000 ± 300 and bottom-out ≈ 18000 ± 200 counts at 3.5 mm,
  linear in between, plus noise; fed through `layout::KEY_TO_SLOT` (spare slots read 30000).
- **Persistence**: `Storage` over the state dir (atomic writes), so saves survive restarts.
- **`BE FF` / reboot**: new `Keyboard`, boot from storage. The uhid device stays.
- **`BE FC` / bootloader**: the uhid device is destroyed, comes back after 2 s with a fresh boot
  (as if re-flashed; config kept).
- **Key held at power-up**: `hold-at-boot A S D` holds the keys and reboots, so the engine's
  warm-up sees them pressed.

## Control socket

One command per line; the reply is the output lines followed by `ok`, or one line `err <msg>`.

```
press <key> [mm]       move to mm (default full 3.5 mm) at the slew rate
release <key>|all
tap <key> [ms]         full press, released after ms (default 30) of simulated time
travel <key> <mm>
state                  mode, profile, uptime, engine_ready, hid, hidraw, one line per key that
                       is moving or reports >= 100 um travel
leds                   "leds" + 85 rrggbb values in LED-chain order
hold-at-boot <keys..>  hold keys fully, then reboot
reboot | bootloader
sleep <ms>             (scripts) wait
help | quit
```

Keys: index `0..82` or a name from `tools/rammap.py` `KEY_NAMES`, case-insensitive (`Esc`, `F1`,
`A`, `LShift`, `Space`, `FN`, `Left`, …). A bare number is an index, so the digit keys are `#1`…`#0`.

```sh
socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/zent75-sim.sock     # interactive
echo "tap A" | nc -U -q1 $XDG_RUNTIME_DIR/zent75-sim.sock  # one-shot
```

From Python, see `Sim.cmd()` in `tools/tests/test_sim_e2e.py` (send a line, read until `ok`).

Script example (`--script demo.txt`):

```
# engine warm-up takes 1.5 s
sleep 1600
tap A 50
sleep 100
press LShift
press A 2.0
sleep 50
state
release all
```

## hidraw permissions

The kernel creates the hidraw node of a uhid device as `root:root 0600` and no stock udev rule
grants it to the user: `uaccess` for hidraw is only added for hwdb-classified devices (FIDO,
3D mice, AV controllers, wallets), and the owner's rule
`KERNEL=="hidraw*", ATTRS{idVendor}=="36b0", MODE="0666"` matches a USB *ancestor* attribute,
which a uhid device (`/sys/devices/virtual/misc/uhid/…`) does not have. So, without changing the
system config, one root step per device creation is needed:

```sh
sudo chmod 0666 /dev/hidrawN        # or start the sim with --sudo-chmod (needs passwordless sudo)
```

A permanent alternative (system config, not applied): a udev rule matching the HID device name,
`SUBSYSTEM=="hidraw", KERNELS=="0003:36B0:5A11.*", MODE="0666"`.

## Pointing the tools at it

`tools/zentlib.py` stays on the real board unless one of these is set:

- `ZENT_SIM=1` — find the simulator's vendor node (`HID_ID 36B0:5A11` + usage page `0xFF60`);
  never falls back to the board.
- `ZENT_HIDRAW=/dev/hidrawN` — use that node; refused unless it is the simulator.

```sh
ZENT_SIM=1 python3 tools/dump_all.py --out /tmp/sim-backups    # --out is mandatory in sim mode
ZENT_SIM=1 python3 tools/restore.py /tmp/sim-backups/<ts>
ZENT_SIM=1 python3 tools/restore.py --selftest
```

`tools/flash_uf2.py` and `tools/rollback.sh` refuse to run when either variable is set, and their
normal discovery (`36B0:357E`) never matches the simulator. `dump_all.py` requires `--out` in
simulator mode so sim dumps never reach `backups/` (which `rollback.sh` restores from).

End-to-end test: `cd tools && uv run --with pytest python -m pytest -q tests/test_sim_e2e.py`
(skipped unless `/dev/uhid` is writable and the sim builds; uses `sudo -n chmod` if available).

## The panel

- **WebHID (browser)**: the panel picks devices by `vendorId 0x36B0` + usage page `0xFF60` /
  usage `0x61` (`WebHidTransport.granted()` / `request()`), not by product id, so the simulator
  matches the filter. Chrome on Linux enumerates hidraw nodes through udev and opens them
  directly, so it needs the node to be user-accessible (chmod above); the device is created with
  bus type USB, which Chrome's Linux backend accepts. Then: run the sim, `npm run dev`, *Connect*,
  pick "ZENT75 SIM". (Not verified in a browser here.)
- **Node scripts** (`npm run smoke`, `write-check`): `panel/scripts/hidraw-transport.ts` filters on
  `productId === 0x357E`; they would need an opt-in like the Python tools (e.g. honour
  `ZENT_SIM`/`ZENT_HIDRAW` or accept `0x5A11`) to reach the simulator.
- The panel shows `soft_version` 1.0.18 (core `VERSION`), so the ≥ 0.0.18 features are enabled.

## Layout

```
src/main.rs      CLI, startup, signal handling
src/sim.rs       Sim (Keyboard + sensor + storage + uhid), real-time loop
src/sensor.rs    travel → ADC model
src/uhid.rs      struct uhid_event encoding, device lifecycle, hidraw lookup by HID_UNIQ
src/hid.rs       report descriptors (vendor 0xFF60; optional keyboard), report encoding
src/control.rs   control socket server, script runner
src/command.rs   line protocol parser
src/keynames.rs  KEY_NAMES (checked against tools/rammap.py by a test)
src/store.rs     DirStorage
src/backup.rs    --load-backup (dump.json), src/json.rs minimal JSON reader
tests/           sensor, storage/backup, key names/commands, socket + script (spawns the binary)
```
