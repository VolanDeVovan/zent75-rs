# Protocol notes from the vendor web driver

Source: `https://hoveyworks-driver.com/js/app.1c24d6d2.js` (Vue 2 bundle, driver version 0.0.7,
2025-02-14), read after pretty-printing. Names in `code` are the bundle's own identifiers and
serve as evidence. Items marked **(device)** were additionally observed on the real keyboard
(firmware 0.0.18) with read-only commands. Items marked **(unverified)** are inferences.

Everything here is implemented in `src/protocol/` and covered by `tests/`.

## Transport

- WebHID filter (`connectToDevice`): `vendorId 14000 (0x36B0)`, `usagePage 65376 (0xFF60)`,
  `usage 97 (0x61)`; the product id must be `13694 (0x357E)` (`autoConnectDevice`).
- Output: `device.sendReport(0, <1024 bytes>)` (`sendHIDData`, `HidPushSendQueue` pads with zeros).
- Input: `inputreport` events, handled by `handleHIDData`.
- Only one request is in flight at a time (`sent_state` flag). A read waits 5 × 10 ms for the
  reply (`HidCmdReadRam`), a write 15 × 10 ms for the ack (`HidCmdSetRam`). Between logical steps
  the driver sleeps 100 ms (`w()`).
- After every write the driver reads the region back and compares; on mismatch it repeats the
  write up to 3 times (`submitKeyMode`, `submitNormalKeyValue`, ...).

## Frames

`BE <cmd> <len LE16> … ED`, `len` counts the whole frame.

| Cmd | Function in the bundle | Request | Reply |
|---|---|---|---|
| `00` | `sendHeartReport` | `BE 00 05 00 ED` | none; keeps the `FD` stream alive |
| `01` | `HidCmdReadFlash` | `BE 01 0B 00 <addr LE32> <n BE16> ED` | like `04` with cmd `01`. **Defined but never called** by the driver. Do not use: out-of-range reads have hung the firmware. |
| `02` | `HidCmdSave` | `BE 02 05 00 ED` | none; the driver just waits 1100 ms |
| `03` | `HidCmdSetRam` | `BE 03 <13+n LE16> <addr LE32> <n BE16> <data> <crc LE16> ED` | `BE 03 05 00 ED` |
| `04` | `HidCmdReadRam` | `BE 04 0B 00 <addr LE32> <n BE16> ED` | `BE 04 <13+n LE16> <addr LE32> <n BE16> <data> <crc LE16> ED` |
| `1D` | `HidCmdSerialNumber` | `BE 1D 05 00 ED` | `BE 1D 15 00 <16 bytes> ED`; shown as 32 hex digits |
| `C0` | `HidCmdSwitchConfig` | `BE C0 06 00 <0..3> ED` | the board pushes `BE C0 06 00 <n>` whenever the profile changes |
| `EE` | `HidCmdResetConfig` | `BE EE 05 00 ED` | none; the driver waits 300 ms and re-reads everything |
| `FC` | `HidCmdDownload` | `BE FC 05 00 ED` | device re-enumerates as a UF2 drive named `Zent_DLD` |
| `FD` | `ReceiveKeyData` | — | live stream, see below |
| `FE` | `HidCmdCalib` | `BE FE 06 00 <1/0> ED` | none |
| `FF` | `HidCmdReset` | `BE FF 05 00 ED` | reboot |

There are no other commands in the bundle.

- CRC: `Yt()` with the table `Xt` is CRC-16/MODBUS (init `0xFFFF`, reflected poly `0xA001`),
  computed over everything before the CRC field (`Zt(a, s + 10)` for a write).
- Read reply parsing (`ReceiveData`): `n = t[2] + 256*t[3]`, CRC over `n-3` bytes compared with
  `t[n-3] | t[n-2] << 8`, `t[n-1] == 0xED`, data = `t[10 .. n-3)`, so data length is `n - 13`.
  The driver ignores bytes 4..9; **(device)** they echo the address and the length of the request,
  which is what our implementation matches replies on.
- Regions the driver only ever writes in one piece: `press_th`, `return_th`, `press_deadzone`,
  `return_deadzone`, `norm_press_th`, `norm_return_th`, `key_mode`, `keytype_code`, `rgb`,
  `rgb_solid_enable`, `bind_key_mode`, `top_value`, `bot_value`. The panel does the same.
- Largest single transfers the driver itself performs: read 256 bytes (`config_name`), write
  255 bytes (`rgb`). Writes of arbitrary sub-ranges are normal: a single key is remapped with a
  2-byte write at `key_map + 2*index` (`handleChangeKey`).
- **(device)** A request sent immediately after the `1D` reply went unanswered 3 times out of 3;
  with a 60 ms pause it never did. The panel pauses after reading the serial number.
- **(device)** hidraw hands every input report to every process that has the node open, so
  replies to another tool's requests are visible too. Matching on the echoed address/length and
  verifying writes by read-back makes that harmless.

## Version, profiles, names

- `soft_version` (4 bytes): text is `t[2].t[1].t[0]`, numeric `(t[2]<<16)+(t[1]<<8)+t[0]`
  (`getAllData`). Features are gated on `version >= 18` (`equipmentVersion`): `bind_key_mode`
  is only read and key groups/DKS only offered from 0.0.18 on.
- `cur_param_num[0]` is the active profile. Switching (`getConfigData`): send `C0`, then poll
  `cur_param_num` (every 300 ms, up to 11 times) until it shows the requested profile, then
  re-read everything. The driver asks the user to save first: a switch drops unsaved RAM edits
  (hint 16).
- `config_name1..4`: 64 bytes each, UTF-8, zero padded (`handleFinishName`); the UI limits names
  to 10 characters and refuses empty ones. An all-zero slot is displayed as `ConfigN`.
- The key codes 307..310 switch profiles from the keyboard ("1 s cool-down" per the tooltip).

## Key codes (`O` table, signed 16-bit)

| Range | Meaning |
|---|---|
| -8..-1 | LCtrl, LShift, LAlt, LWin, RCtrl, RShift, RAlt, RWin |
| 0..3 | HID reserved / error codes |
| 4..119 | HID keyboard usages as-is (A=4 … F24=115, Execute, Help, Menu=118, Select=119) |
| 201..207 | Vol+, Vol-, Mute, Play/Pause, Stop, Prev, Next |
| 208..210 | Calculator, E-mail, Explorer |
| 211..217 | Browser: Search, Home, Back, Stop, Forward, Refresh, Bookmarks |
| 218..225 | Control panel, Media player, Brightness+, Brightness-, Launchpad, Mission Control, Assistant, VK_PERIOD |
| 251..255 | Mouse left, right, middle, 4 (back), 5 (forward) — new in 0.0.18 |
| 301..306 | BOOTLOADER, REBOOT, DISABLE_EX (RT off), ENABLE_EX (RT on), INITSET (reset config), ~ESC (Esc, `~` with Shift) |
| 307..310 | Config1..4 |
| 311..318 | Light+, Light-, next effect, previous effect, light on/off, WinLock (316), effect speed+, speed- |
| 319, 320 | 5 s hold = factory reset; swap WASD with the arrows |
| 401..404 | FN, FN2, FN3, FN4 (the picker only offers FN) |
| 1000 | empty |

Full list with labels: `src/protocol/keycodes.ts`.

- Default main layer: table `j[0]`; default Fn layer: all 1000 (`j[1]`).
- The driver refuses to remap the Fn layer of keys `[0, 28, 29, 43, 62, 71, 74, 80, 81, 82]`
  (Esc, Backspace, Del, `\`, C, ↑, LWin, ←, ↓, →; "fixed keys cannot be remapped",
  `handleChangeKey`). They carry the built-in shortcuts (hints 61–65: Fn+C profile cycle,
  Fn+Backspace light on/off, Fn+↑/↓ brightness, Fn+`\` next effect, Fn+→/← effect speed).
  **(device)** Fn layer of the real board: Esc=319, Bksp=315, Del=320, `\`=313, C=308 (Config2),
  ↑=311, LWin=316, ←=318, ↓=312, →=317, plus the modifiers mapped to themselves.
- Right-click on a key writes 1000 (empty).

## Layout

83 keys, indices row by row (`getKeyClass`, `getKeyClass_int`, CSS `keyBig1..6`):

| Row | Indices | Widths (u) |
|---|---|---|
| 0 | 0..14 | Esc, F1–F12, PrtSc, Home; 0.25u gaps after keys 0, 4, 8, 12 |
| 1 | 15..29 | 13 × 1, Backspace 2 (28), Del 1 |
| 2 | 30..44 | Tab 1.5 (30), 12 × 1, `\` 1.5 (43), PgUp |
| 3 | 45..58 | Caps 1.75 (45), 11 × 1, Enter 2.25 (57), PgDn |
| 4 | 59..72 | LShift 2.25 (59), 10 × 1, RShift 1.75 (70), ↑, End |
| 5 | 73..82 | LCtrl, LWin, LAlt 1.25 each (73–75), Space 6.25 (76), RAlt, Fn, RCtrl, ←, ↓, → |

Quick-select groups (`handleFastSelect`): WASD = `[32, 46, 47, 48]`, digits = 16..25, letters.

## Actuation (`key_mode`, thresholds)

- `key_mode[i]` (u8): **0 = normal** ("普通模式"), **1 = rapid trigger** ("快速触发"). The
  driver's initial array is all 1. No other value is ever written.
- All thresholds are u16 in **micrometres** (`1e3 * mm` in `handleSliderChange`).

| Region | UI name | Used in | UI range |
|---|---|---|---|
| `norm_press_th` | Actuation Point | normal | 0.1 … 4.0 mm, step 0.01 |
| `norm_return_th` | Release Point | normal | ≤ actuation; kept 0.1 below unless "set individually" |
| `press_th` | Actuation Threshold (RT press sensitivity) | RT | 0.1 … 1.0 mm (0.01 with "allow < 0.1 mm"), step 0.01 or 0.005 |
| `return_th` | Release Threshold (RT release sensitivity) | RT | same |
| `press_deadzone` | Starting Dead Zone (top) | RT | switch minimum … 1.0 mm |
| `return_deadzone` | Ending Dead Zone (bottom) | RT | 0.055 … 1.0 mm |

- When the mode is "normal" the driver writes only `norm_*`; in RT mode only `press_th`,
  `return_th` and the dead zones. `confirm()` always writes all seven regions in full.
- Validation (`journeyIsCorrect`): `norm_press_th` and `press_th` must be below the switch
  travel: type 0 < 3420, type 1 < 3375, type 2 < 3420, type 3 < 3780 µm.
- **(device)** current values: all 83 keys normal, actuation 1600, release 800, `press_th` 50,
  `return_th` 30, both dead zones 0.

### Switch types (`keytype_code`, i16 per key, board-global)

| Value | Vendor name | Travel | Min. top dead zone |
|---|---|---|---|
| 0 | 万磁王轴 / TTC KOM | 3.42 mm | 0.01 |
| 1 | 万磁王轴(新) / TTC KOM (new) | 3.375 mm | 0.01 |
| 2 | 磁玉轴pro / Gateron Jade Pro (firmware default since 0.0.8) | 3.42 mm | 0.015 |
| 3 | not in the UI list, but handled (`journeyIsCorrect`, calibration) | 3.78 mm | 0.03 |

At most 4 different types may be in use (`new Set(t).size > 4`). The driver warns that the
board must be recalibrated after a change (hints 47–49).

## SOCD (`key_cancel_code`)

- i16 per **physical key**; value = the **key code** that gets cancelled while this key is
  held; 1000 = off (`handleChangeKey` with `RearCoverStatus === "0"`, default `ht` all 1000).
- Hint 13: "pressing it cancels a key that is currently held; does not distinguish the main
  and the Fn layer". A classic A/D pair needs two entries: key A → code of D, key D → code of A.
- Hint 56: function codes cannot be SOCD targets. Right-click writes 1000.

## Key groups and DKS (`bind_key_mode`, `bind_key_setting`), firmware ≥ 0.0.18

`bind_key_mode[i]` (u8): **0 = none, 1 = key group ("组合键"), 2 = DKS**
(`handleGroupKeysConfirm`, `handleDKSKeysConfirm`, `handleRightClick`).

`bind_key_setting`: 48 bytes per key at `0x80080D00 + 48*i`. Layout from `DKSConfirm`
(`h = [...t, ...e, ...s, ...i, ...a, ...n, ...r, ...o]; h.push(l)`) and `getAdvancedKeyClick`:

| Offset | Size | Field |
|---|---|---|
| 0 | 16 | DKS actions: 4 slots × 4 phases (`triggerMode1..4`), one byte each |
| 16 | 8 | 4 × i16 key codes: group members or DKS slot keys; 1000 = empty |
| 24 | 2 | DKS top trigger point, µm (`topJourney`, UI 0.1 … 3.2 mm, step 0.1) |
| 26 | 2 | DKS bottom trigger point, µm (`bottomJourney`, UI top+0.1 … 3.3 mm) |
| 28 | 2 | `singleTime * 8` (UI 1..50, "single press time") |
| 30 | 2 | `delayTime * 8` (UI 0..10) |
| 32 | 2 | `randomTime * 8` (UI 0..10, "random delay") |
| 34 | 13 | written as `0xFF` |
| 47 | 1 | `DKSOriginCover ? 0 : 1`; UI label "Trigger the original key" |

- Action byte: **0 = nothing, 2 = tap at this phase, 1 = pressed and held on to the next
  phase** (`DKSMousedown` sets 2, dragging the bar across phases sets 1 on every covered phase;
  the last phase can only be 0 or 2). Phases, by the UI's four columns: press past the top
  point, press past the bottom point, release above the bottom point, release above the top
  point **(unverified wording; the order follows the travel)**.
- The timing unit is labelled "Tick" in Chinese and "ms" in English; the factor 8 matches an
  8 kHz scan, so the stored value is in scan ticks **(unverified)**.
- Byte 47: the switch labelled "Trigger the original key" (zh "当前设置是否触发原本按键值")
  stores 0 when on, 1 when off (default). The variable name says "cover", the label says
  "trigger": which of the two the firmware does needs confirmation from the firmware side.
- A key group writes only bytes 16..23 and byte 47 (`handleGroupKeysConfirm`); DKS writes all
  48. Clearing (right-click) sets the mode to 0 and bytes 16..23 to four × 1000.
- Order of writes: first the whole `bind_key_mode` array, 100 ms pause, then the record.
- **(device)** an unused record on 0.0.18 reads: actions 0, keys 1000 ×4, top 100, bottom 3000,
  times 8 / 24 / 16, 13 × `FF`, byte 47 = 1.

## Lighting

- `rgb`: 255 bytes = 85 LEDs × (R, G, B). `rgb_solid_enable`: 85 bytes, **1 = this LED shows its
  static colour, 0 = it follows the dynamic effect** (`handleLightingSettingReturn`,
  `tabActive` "1" static / "0" dynamic).
- Key → LED map `J`: the chain snakes from the bottom-left corner upwards; the space bar
  (key 76) owns LEDs 3, 4, 5. Table in `src/protocol/layout.ts` (`KEY_TO_LED`).
- `rgb_config` (5 bytes): `[1 + 4*effect, H, S, brightness, speed]`
  (`handleLightingSettingReturn`, `getKeyDynamicStateData`: `effect = (t[0]-1)/4`). The low two
  bits are always written as 1 **(meaning unverified)**.
- Brightness slider 0..30; above 10 the driver warns about USB power (hint 12). H, S, speed 0..255.
- Effects (`W`, value = `effect`): 1 ice-blue solid, 3 rainbow gradient, 5 ice-blue breathing,
  12 three-colour breathing, 13 RGB scroll left-right, 14 RGB scroll up-down, 16 RGB converge to
  centre, 18 RGB rotate around centre, 21 RGB marquee, 23 rainbow breathing, 24 starlight,
  29 ripples, 30 piano RGB, 32 single-colour bloom, 33 RGB bloom, 35 ice-blue single key reactive,
  39 reactive cross, 43 reactive whole-board RGB, 50 single-colour sparkle. The gaps suggest more
  effect ids exist in the firmware than the UI lists.
- Byte block at `0x80080C68`, read as 10 bytes (`getRgbSwitchData`):

  | Offset | Address | Field |
  |---|---|---|
  | 0 | `C68` | `rgb_enable` (1 = on) |
  | 1 | `C69` | `cap_enable` (Caps Lock indicator) |
  | 2 | `C6A` | not used by the driver |
  | 3 | `C6B` | `rgb_fn_enable` ("separate Fn layer lighting") |
  | 4..7 | `C6C` | `sleep_second_cnt`, u32 LE seconds; the driver reads only the low 16 bits |
  | 8 | `C70` | `win_lock_state` (the UI calls it "WinLock indicator") |
  | 9 | `C71` | `auto_calib_enable` |

  Sleep options: 0 (never), 60, 120, 180, 300, 600, 900, 1200, 1500, 1800 s.
- `C65..C67` are not named anywhere; the driver's export reads 17 bytes from `C60` and so
  carries them along.
- Vendor "lighting share" file (`handleExportToJson`): `rgb_matrix_settings` = `rgb` followed
  directly by `rgb_solid_enable` (340 bytes), `rgb_config_settings` = 17 bytes from `C60`.
  **Vendor bug:** on import (`sendLightConfig`) the 340 bytes are written in one block at
  `rgb` (`0x80080B00`), but `rgb_solid_enable` lives at `0x80080C08`, 9 bytes further than
  `0x80080BFF`. The static/dynamic flags therefore land 9 bytes too low. Our JSON format is
  unrelated to that file.

## Calibration

- `top_value` = ADC at rest, `bot_value` = ADC bottomed out (u16 per key, board-global).
- Flow (`handleStartCal`, `handleKeyData`, `handleFinishSubmit`):
  1. read `top_value` and `bot_value` into `maxCopy` / `minCopy`;
  2. `BE FE 01`; the heartbeat is running because the calibration tab is open (`tabNum === 3`);
  3. the board now streams the **1003-byte** `FD` variant; each frame is averaged to one value
     per key and only every second frame is used;
  4. first frame: polarity per key, `KeyDirection = raw > 32768 ? "0" : "1"`. On this board the
     rest value is ≈ 30000, i.e. direction "1";
  5. per frame and key: track `max` and `min`. As soon as **any** key shows `max - min > 3000`
     a global `flag` freezes the rest-side extreme of every key (`max` for direction "1"), so
     the rest level is whatever was seen before the first key moved. "Do not press any key for
     the first few seconds" (hint 19);
  6. a key is accepted when `max - min >` **7000** (switch types 0–2) or **8000** (type 3),
     checked every 100th frame;
  7. Finish: `BE FE 00`, 100 ms, then write `top_value` and `bot_value` in full. Accepted keys
     get `top = max, bot = min` (direction "1"; swapped for direction "0"); all other keys keep
     the values read in step 1. Then `BE 02` (`sendFinish` → `autoCalibSave`);
  8. Abandon: `BE FE 00` only.
- `auto_calib_enable`: toggled with a 1-byte write and saved immediately. Hint 17: automatic
  calibration keeps refining during use but does not replace the manual one.
- While calibration runs the driver blocks "Save" (`isAgreeSave`).

## Live stream (`BE FD`)

- Sent only while heartbeats arrive. The driver sends `BE 00` every 1000 ms and **only while
  the calibration tab is open** (`watch.tabNum`); that tab doubles as its key tester.
- Length **1017** (normal): payload `t[4 .. 1014)` = 505 u16 LE. The driver uses the first
  83 (raw ADC, `KeyValues`) and the next 83 (travel, shown as `0.001 * value` mm, `KeyHeights`).
- Length **1003** (calibration mode): 6 × 83 u16 raw scans, exactly filling the payload.
- **(device)** observed on 0.0.18:
  - about 23–25 frames per second;
  - rest values 29500 … 30950, noise up to ≈ 50 counts; travel idles at 0 with occasional 8 or
    15 µm steps (one count ≈ 7.5 µm);
  - words 166..248 and 249..331 of the 1017 payload hold two more per-key arrays that sit within
    a few counts of the raw values (they look like filtered / tracked levels); words 332..497
    were all zero with no key pressed; the last 7 words read `0, 8000, 680, 534, 0, 0, 0`
    (8000 fits the scan rate in Hz). The driver ignores all of these.

## Things the driver does that are easy to miss

- `handleVisibilityChange`: when the tab becomes visible again the driver re-reads the active
  profile and, if it changed on the keyboard, the whole configuration.
- The firmware page offers `0.0.9.uf2` ("stable") and `0.0.18.uf2` ("latest") from
  `./firmware/zent75/master/`; `updatePrompt` nags when the firmware is not 0.0.18.
- Firmware changelog embedded in the bundle: 0.0.18 mouse key codes, key groups and DKS;
  0.0.17 auto-calibration tuning; 0.0.16 switch curve tuning and fixes; 0.0.10 logic tweaks and a
  sleep bug fix; 0.0.8 default switch changed to Jade Pro.
