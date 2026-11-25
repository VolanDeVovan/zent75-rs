# ZENT75-HE panel

A static web app (TypeScript + Vite, no UI framework) that configures the HOVEY WORKS ZENT75-HE
hall-effect keyboard (USB `36b0:357e`) over WebHID. It replaces the vendor web driver.

> **WebHID works only in Chromium-based browsers** (Chrome, Edge, Brave, Opera), on `https://`
> or `localhost`. Firefox and Safari have no `navigator.hid`; the page says so and still lets
> you explore everything with the built-in mock device.

## What it covers

Pages are ordered by how often they are needed; the design and its reasons are in
[`docs/ux.md`](docs/ux.md).

| Page | Functions |
|---|---|
| Top bar | the keyboard's profiles (switch; menu on the active one: rename, export, reset), save state, **Undo**, **Discard**, **Save to keyboard** |
| Home | typing feel for all keys (Normal / Rapid trigger, actuation point or sensitivity), lighting on / brightness / effect, SOCD A⇄D and W⇄S, Win lock, calibration status |
| Actuation | global values with per-key override: mode (normal / rapid trigger / **RT below the actuation point**, `key_mode` 2), actuation and release points, RT press / release sensitivity, top and bottom dead zones, a travel gauge; reset per value, per selection, for all keys |
| Lighting | on/off, effect, brightness, speed, hue, saturation, static colour per key, Caps indicator, Fn-layer lighting, reset |
| SOCD | "last input wins": switches for the usual pairs, pair any two keys by clicking them, one-way rule with any key code, list of active rules |
| Keymap | main and Fn layer; key codes by search (name, description, raw number) or category; per key and layer a **tap-hold** code and a **modifier mask**; factory default / revert / clear per key, reset of the main layer |
| Advanced keys | key groups (up to 4 keys) and DKS (4 slots × 4 travel phases, trigger points, timing). Need firmware ≥ 0.0.18; on older firmware the page says so |
| Sensors | live travel in mm or raw ADC (`BE FD` stream, 1 s heartbeat); manual calibration (start → press keys → finish and save, or abandon) and the automatic calibration switch; switch type per key incl. the four **custom switches**; editor for their curves |
| Device | sleep timer, Win lock, JSON export / import (one or all profiles, optional calibration), firmware version, serial number, re-read; **Danger zone**: reset profile, reboot, enter bootloader (typed confirmation) |

### Saving, undoing, resetting

Edits go to the keyboard's RAM immediately (verified by reading back), so they can be felt at
once; they survive a replug only after **Save to keyboard** (`Ctrl+S`). The top bar always shows
`Saved` or `Unsaved: actuation, lighting, …`. **Undo** (`Ctrl+Z`, 50 steps) takes back one edit,
**Discard** puts back the profile as last saved. A profile switch drops unsaved edits; the panel
offers to save, discard or stay. Sliders have a reset-to-factory button, pages have a reset for
their part of the profile, and "Reset profile" (profile menu, Danger zone) resets the whole
profile through the firmware and saves, keeping the old profile for Undo.

Profile names, switch types, calibration and custom switch curves are board-global and stored by
the firmware on every write: they are not part of the unsaved state and Undo does not cover them.

### Tap-hold, modifier masks, key mode 2, custom switches

Firmware features the vendor driver never exposed (field layout: `firmware/core/src/config.rs`,
behaviour: `firmware/core/src/keys/`, `docs/algorithms.md`):

- **Tap-hold** (`hold_code` / `hold_code_fn`, 1000 = off): a key with a hold code sends nothing
  while pressed; held longer than 0.5 s it sends the hold code until released, released earlier it
  sends its keymap code for one report (the tap arrives on release). The 0.5 s are fixed in the
  firmware. Codes above 1000 are refused: the firmware treats a profile whose first hold code is
  > 1000 as corrupt.
- **Modifier mask** (`mod_mask` / `mod_mask_fn`): modifier bits ORed into the report while the
  key is pressed (WinLock still clears the Win bits).
- **`key_mode` 2**: normal press at the actuation point, rapid trigger below it; an RT release
  keeps the key "RT-released" until it re-presses by RT or comes back above the release point. Uses
  all six threshold arrays.
- **Custom switches** (`custom_switch`, bank 1, four 256-byte records, keytype codes 4095..4092):
  name, full travel (1500..4200 µm), colour and a curve of up to 88 points; point i is the
  normalised sensor position f (0..2636) at (i − 1) × 50 µm of travel, the last point is the full
  travel and must be 2636, points must not decrease. The panel validates like the firmware
  (`src/protocol/switches.ts`) and refuses invalid records (the firmware would ignore them and
  replace them with the factory record at the next boot). **Every bank-1 write is saved to flash by
  the firmware at once** (and discarded if any key's stored calibration span is < 3000), so the
  editor asks before writing, writes only changed records, and never touches `top/bot`.

## Run

```sh
cd panel
npm ci
npm run dev        # http://localhost:5173
npm run build      # type-check + static bundle in dist/ (relative paths, host anywhere)
npm run preview    # serve dist/
npm test           # vitest: codec, mock round trips, calibration, config files, UI state
npm run typecheck  # tsc --noEmit over src, tests and scripts
npm run smoke      # read-only check against the real keyboard from Node
npm run write-check  # profile-name write + save + restore on the real keyboard
```

### Mock device

- press **Try the demo keyboard** on the start page, or open `http://localhost:5173/?mock=1`
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
src/protocol/switches.ts      custom switch curves: factory record, firmware validation, resampling
src/transport/webhid.ts       Transport over WebHID
src/transport/mock.ts         Transport that emulates the keyboard
scripts/hidraw-transport.ts   Transport over /dev/hidrawN (Node)
src/config-file.ts, config-io.ts   JSON format, export / import through KeyboardProtocol
src/ui/store.ts               state and device operations: save state, undo, discard, reset (no DOM)
src/ui/model.ts, defaults.ts  derived values (global actuation, SOCD pairs, picker search), factory values
src/ui/app.ts                 shell: top bar, navigation, confirmations
src/ui/components.ts, keyboard-view.ts, dom.ts   controls, key picker, travel gauge, keyboard drawing
src/ui/pages/                 one file per page
```

Adding a second protocol (our own firmware): implement `KeyboardProtocol` next to `zent/` and
pick it in `createProtocol()` (`src/ui/store.ts`), for example by product id or by probing. The
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
  and the calibration arrays are always rewritten in one piece. Tap-hold codes and modifier masks
  (not used by the vendor driver) are written per element; custom switch records one 256-byte
  record at a time.

## Tests

`npm test` runs 141 tests in 9 files:

- `crc.test.ts`: CRC-16/MODBUS catalogue vector, Modbus RTU frames, residue, bitwise reference
- `frames.test.ts`: 1024-byte padding, report id 0, exact bytes of every command, both
  directions, corrupted frames, the 1017 and 1003 `BE FD` variants
- `memory.test.ts`: addresses, field codecs, the 48-byte `bind_key_setting` record, lossless
  round trip of random images, write diffing, layout and key-code tables
- `mock-roundtrip.test.ts`: every protocol operation against the mock (CRC rejection, retries,
  profile switch, save / reboot persistence, names, stream, range guard, bootloader)
- `calibration.test.ts`: acceptance rules and the full flow against the mock
- `config-file.test.ts`: export / import, all-profile restore, firmware without DKS, validation
- `tap-hold-switches.test.ts`: tap-hold / modifier-mask codec and mock round trip, custom switch
  record codec (byte-exact factory record, lossless), firmware validation rules, resampling,
  record-wise bank-1 writes, discard on implausible calibration, config-file round trip
- `ui-model.test.ts`: global / customised actuation, validation, SOCD pairs, unsaved summary,
  key picker categories and search, the tap-hold cap of 1000
- `ui-store.test.ts`: the UI's state against the mock: unsaved state, save, undo, discard, queued
  edits, profile switch and reset, board-global writes, refused tap-hold codes and custom switch
  records, firmware without advanced keys, calibration finish

## Simulator end-to-end check

`npm run sim-e2e` (`scripts/sim-e2e.ts`) starts the firmware simulator (`firmware/sim`, build it
first), opens **only** the hidraw node it prints and checks in sysfs that it is `36B0:5A11`
before using it; the real keyboard is never looked up. Through `ZentStockProtocol` it writes a
tap-hold code, a modifier mask, `key_mode` 2 and a custom switch record, then drives keys through
the simulator's control socket and checks the firmware core's HID output and engine travel: no
report while a tap-hold is pending, the hold code after 0.5 s, a one-scan tap report on a short
tap, LShift+s from a masked S, the full `key_mode` 2 sequence (press at the actuation point, RT
release / re-press below and above it, reset above the release point), and a key on the 4.0 mm
custom record travelling 4000/3420 as far as one on the built-in type. Needs rw on `/dev/uhid`
(and `sudo -n chmod` for the hidraw node when it is root-only).

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
  a browser with the mock; the WebHID transport itself (device chooser, `sendReport`,
  `inputreport`) has not been exercised with hardware.
- The 1003-byte calibration stream variant is implemented from the vendor code and tested with
  the mock; it has not been captured from the real board.

## Hardware write check

`npm run write-check` drives the panel's own `KeyboardProtocol` stack over the Node hidraw
transport to prove edit+save+restore on the real keyboard, touching only a profile name and
putting it back (verified byte-identical). `--mock` runs the same path against the mock.
Verified 2026-11-24: names `main/game/empty/empty`, write+save+restore byte-identical.
