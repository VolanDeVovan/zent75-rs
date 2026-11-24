# ZENT75-HE panel

A static web app (TypeScript + Vite, no UI framework) that configures the HOVEY WORKS ZENT75-HE
hall-effect keyboard (USB `36b0:357e`) over WebHID. It replaces the vendor web driver.

> **WebHID works only in Chromium-based browsers** (Chrome, Edge, Brave, Opera), on `https://`
> or `localhost`. Firefox and Safari have no `navigator.hid`; the page says so and still lets
> you explore everything with the built-in mock device.

## What it covers

| Tab | Functions |
|---|---|
| Keymap | main and Fn layer, every key code of the firmware (HID, media, browser, mouse, board functions, Fn), raw code entry |
| Actuation | per-key mode (normal / rapid trigger), actuation and release points, RT press / release sensitivity, top and bottom dead zones, switch type |
| SOCD / DKS | SOCD ("last input wins") per key and as mutual pairs, key groups (up to 4 keys), DKS (4 slots × 4 travel phases, trigger points, timing). Groups and DKS need firmware ≥ 0.0.18 |
| Lighting | on/off, Caps indicator, Fn-layer lighting, effect, hue, saturation, brightness, speed, static colour per key |
| Calibration | manual calibration (start → press keys → finish and save, or abandon), automatic calibration switch |
| Live travel | per-key travel in mm (or raw ADC) from the `BE FD` stream, kept alive by a 1 s heartbeat |
| Device | firmware version, serial number, 4 profiles and their names, sleep timer, Win lock, save to flash, JSON export / import (one or all profiles, optional calibration), reset profile, reboot, enter bootloader (typed confirmation) |

Edits go to the keyboard's RAM immediately and are verified by reading back. They survive a
replug only after **Save to flash**. A profile switch discards unsaved edits (the panel asks).

## Run

```sh
cd panel
npm ci
npm run dev        # http://localhost:5173
npm run build      # type-check + static bundle in dist/ (relative paths, host anywhere)
npm run preview    # serve dist/
npm test           # vitest: codec, mock round trips, calibration, config files
npm run typecheck  # tsc --noEmit over src, tests and scripts
npm run smoke      # read-only check against the real keyboard from Node
npm run write-check  # profile-name write + save + restore on the real keyboard
```

### Mock device

- press **Mock device** on the start page, or open `http://localhost:5173/?mock=1`
- `?mock=1&fw=0.0.9` emulates the older firmware without key groups / DKS

The mock (`src/transport/mock.ts`) implements the `Transport` interface and behaves like the
firmware: global and per-profile memory with a separate flash copy, CRC check and ack on writes,
profile switch with the pushed notice, save, reset, reboot, bootloader, calibration mode, serial
number, version, and the `BE FD` stream (both variants) while heartbeats arrive. In the browser
it animates key presses so the live view and the calibration have something to show.

### Linux permissions

The vendor interface is the hidraw node of `36b0:357e` interface 02. Chrome and the Node scripts
need read/write access to it (udev rule, e.g. mode 0666 for vendor `36b0`). Close the vendor web
driver first: the replies go to every process that has the node open.

## Architecture

```
src/protocol/keyboard.ts      KeyboardProtocol interface + plain data model (µm, key codes)   <- the UI sees only this
src/protocol/transport.ts     Transport interface: fixed-size raw HID reports in and out
src/protocol/zent/frames.ts   BE..ED frame codec, every command, both FD stream variants      \
src/protocol/zent/memory.ts   memory map, region <-> model codec, minimal write diffing        } stock firmware protocol
src/protocol/zent/protocol.ts ZentStockProtocol implements KeyboardProtocol                   /
src/protocol/calibration.ts   calibration rules and flow, on top of KeyboardProtocol
src/protocol/keycodes.ts, layout.ts   key code table, effects, switch types, 75% layout, LED map
src/transport/webhid.ts       Transport over WebHID
src/transport/mock.ts         Transport that emulates the keyboard
scripts/hidraw-transport.ts   Transport over /dev/hidrawN (Node)
src/config-file.ts, config-io.ts   JSON format, export / import through KeyboardProtocol
src/ui/                       app shell, keyboard view, one file per tab
```

Adding a second protocol (our own firmware): implement `KeyboardProtocol` next to `zent/` and
pick it in `createProtocol()` (`src/ui/app.ts`), for example by product id or by probing. The
UI, the calibration flow, the JSON format and the tests against `KeyboardProtocol` stay as
they are. `PROTOCOL-NOTES.md` documents what was learned from the vendor driver.

Safety built into the stock protocol:

- reads and writes are refused unless they lie inside a region of the known memory map
  (`restrictToMap`); `BE 01` is implemented in the codec but never sent by the panel, because
  out-of-range flash reads have hung the firmware;
- replies are matched on the echoed address and length; writes are read back and compared;
- only what differs from what the keyboard holds is written, and in the same shapes the vendor
  driver uses: a remapped key or an SOCD entry is a 2-byte write, a key group / DKS record is a
  48-byte write, while threshold arrays, `key_mode`, `rgb`, `rgb_solid_enable`, `bind_key_mode`
  and the calibration arrays are always rewritten in one piece.

## Tests

`npm test` runs 97 tests in 6 files:

- `crc.test.ts`: CRC-16/MODBUS catalogue vector, Modbus RTU frames, residue, bitwise reference
- `frames.test.ts`: 1024-byte padding, report id 0, exact bytes of every command, both
  directions, corrupted frames, the 1017 and 1003 `BE FD` variants
- `memory.test.ts`: addresses, field codecs, the 48-byte `bind_key_setting` record, lossless
  round trip of random images, write diffing, layout and key-code tables
- `mock-roundtrip.test.ts`: every protocol operation against the mock (CRC rejection, retries,
  profile switch, save / reboot persistence, names, stream, range guard, bootloader)
- `calibration.test.ts`: acceptance rules and the full flow against the mock
- `config-file.test.ts`: export / import, all-profile restore, firmware without DKS, validation

## Smoke test against the real keyboard

`scripts/device-smoke.ts` uses the same codec and `ZentStockProtocol` as the browser, with a raw
hidraw transport (write: `0x00` report id + 1024 bytes; read: 1024/1025-byte reports). It finds
the node through `/sys/class/hidraw/*/device` (vendor/product id, `bInterfaceNumber` 02, report
descriptor usage page `0xFF60`) and sends **only** `BE 04` (regions of the documented map),
`BE 1D` and `BE 00`. Nothing is written.

```sh
npm run smoke                    # 3 s of live travel
npm run smoke -- --seconds 10    # longer capture; press keys to see them listed
npm run smoke -- --retries 12    # keep trying every 5 s (keyboard in bootloader / being replugged)
npm run smoke -- --verbose       # also dump the stream payload the vendor driver ignores
npm run smoke -- --mock          # same code path against the mock
```

Exit code 0 = OK, 2 = keyboard unavailable (missing node, `ENODEV`, `ETIMEDOUT`, `EPIPE`, no
replies), 1 = anything else.

Recorded run, 2026-11-24, firmware 0.0.18 (the profile name and the stray `writeAck` come from
`tools/restore.py --selftest`, which was running on the same hidraw node at that moment):

```
device      /dev/hidraw7 (Wob Lab ZENT  75 HE)
firmware    0.0.18 (numeric 18)
serial      0123456789abcdef0123456789abcdef
product     Wob Lab ZENT  75 HE, 83 keys, advanced keys: true
profile     active 0 of 0:"ZENT_RESTORE_TEST" 1:"game" 2:"empty" 3:"empty"
main layer
    Esc F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12 PrtSc Home
    ` ~ 1 ! 2 @ 3 # 4 $ 5 % 6 ^ 7 & 8 * 9 ( 0 ) - _ = + Bksp Del
    Tab Q W E R T Y U I O P [ { ] } \ | PgUp
    Caps A S D F G H J K L ; : ' " Enter PgDn
    LShift Z X C V B N M , < . > / ? RShift ↑ End
    LCtrl LWin LAlt Space RAlt Fn RCtrl ← ↓ →
fn layer
    5sReset · · · · · · · · · · · · · ·
    · · · · · · · · · · · · · LightSw WASD⇄Arr
    · · · · · · · · · · · · · NextFx ·
    · · · · · · · · · · · · · ·
    LShift · · Cfg2 · · · · · · · RShift Light+ ·
    LCtrl WinLock LAlt · RAlt · RCtrl FxSpd- Light- FxSpd+
key_mode    normal: 83, rapid trigger: 0, other: 0
actuation   press 1600 µm, release 800 µm
rapid trig  press 50 µm, release 30 µm
deadzones   top 0 µm, bottom 0 µm
socd        none
advanced    combo keys: none; DKS keys: none
            record of key 0: keys 1000,1000,1000,1000 top 100 bottom 3000 ticks 8/24/16 reserved 255 ×13 byte47 1
lighting    enabled true, effect 35 (flags 1), H 144 S 255, brightness 15, speed 50, static LEDs 0/85, caps true, fn-layer true
settings    sleep 180 s, win lock true, auto calibration true
calibration top 29909..31000, bottom 17000..17673, switch types 2=Gateron Magnetic Jade Pro
live        68 frames in 3.0 s (23/s), variants travel:68, 339 uninterpreted payload words per travel frame
            raw at rest 29481..30949 (noise per key up to 49 counts)
            keys that moved: none (nothing was pressed)
stats       sent 49, received 118, retries 0, bad frames 0
frames      read:48 serial:1 stream:68 writeAck:1
SMOKE OK
```

## Not verified yet

- **Most write paths on the real keyboard.** Only a profile-name RAM write and save have run
  against hardware from this code (see "Hardware write check"). Profile switch, calibration
  mode, reset, reboot and bootloader are tested against the mock only.
- **WebHID in a real Chromium against the real keyboard.** The UI was driven end to end in
  headless Chrome with the mock; the WebHID transport itself (device chooser, `sendReport`,
  `inputreport`) has not been exercised with hardware.
- The 1003-byte calibration stream variant is implemented from the vendor code and tested with
  the mock; it has not been captured from the real board.

## Hardware write check

`npm run write-check` drives the panel's own `KeyboardProtocol` stack over the Node hidraw
transport to prove edit+save+restore on the real keyboard, touching only a profile name and
putting it back (verified byte-identical). `--mock` runs the same path against the mock.
Verified 2026-11-24: names `main/game/empty/empty`, write+save+restore byte-identical.
