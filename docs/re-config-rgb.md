# Stock 0.0.18: configuration storage, dispatcher, profiles, lighting

Reverse-engineered from the 0.0.18 app (static analysis of the unpacked ILM image + unicorn emulation,
`re-work/config/`). `FUN_ram_X` = ILM function address, `0x9xxxx` = DLM globals, "shadow" = the RAM
copy of a config bank. Emulated results are marked **(emu)**; everything else is from the code.
A diff-test harness that renders any effect lives in `re-work/config/rgb_harness.py` (§7).

Time units used below:

- **scan** = one full 11-step hall scan, 8000/s.
- **tick** = the firmware's "ms" counter `0x91FCC` (`FUN_ram_00016418`), incremented every 64 ADC
  steps (`FUN_ram_00015250`, 11 steps per scan) → **1375 Hz, 0.727 ms**. Every "ms" constant in the
  lighting code and USB waits is in ticks, so effects run 1.375× faster than the same QMK constants.
- `delay_ms(n)` (`FUN_ram_80045108`) is a real busy-wait on `mcycle`.

## 1. Config banks

| Bank | NOR | RAM shadow (ILM) | Size saved | Swapped by `C0` |
|---|---|---|---|---|
| 1, board-global | `0x80070000` (sector `0x80070000..0FFF`) | `0x7084` | `0x790` | no |
| 2, profile *p* | `0x80080000 + p·0x2000` (two sectors) | `0x0658` | `0x1000 + 0xE38` = `0x1E38` | yes |
| live-preview area | none (RAM only) | `0x7814` | — | — |

The NOR images are byte-for-byte copies of the shadows (no header, magic, version or CRC). Flash I/O goes
through the ROM `xpi_nor` API: read (`FUN_ram_0001598c`) and erase+program (`FUN_ram_00014a44`,
erase(addr,len) then program(src,addr,len)). The profile NOR address is
`(cur_param_num + 0x40) · 0x2000` (`0x9209C`). Profiles 0..3 therefore occupy
`0x80080000..0x80087FFF`; `0x80088000..` is unused.

### Bank 1 layout (`0x80070000`, 88-entry arrays indexed by key 0..82, 83..87 spare)

| Offset | Type | Field | Default (blank NOR) |
|---|---|---|---|
| `000` | u16[88] | `top_value` | 31000 |
| `0B0` | u16[88] | `bot_value` | 17000 |
| `160` | u16[88] | `keytype_code` | 2 (Jade Pro) |
| `210` | u32 | `soft_version` | `0x00000012` (forced every boot) |
| `214` | u32 | unknown flag | `0xFFFFFFFF`; if anything else boot sets it to 1 and rewrites bank 1 |
| `218` | u32 | `cur_param_num` | 0 (≥4 or `0xFFFFFFFF` → 0) |
| `21C..28F` | — | unused | `0xFF` |
| `290 + 64·i` | 64 B UTF-8 | `config_name1..4` | `游戏模式`, `办公模式`, `默认配置3`, `默认配置4` (zero padded) |
| `390 + 256·i` | 256 B | custom switch *i* (4 records, below) | |
| `790..` | — | not part of the shadow (erased with the sector) | |

Custom switch record *i* (not used by the web driver; `keytype_code` 0xFFF−i selects it, decoded as
type `0x1003 − code` = 4..7 by `FUN_ram_0000E04E`):

| Off | Field | Default |
|---|---|---|
| `00` | u32 full travel, µm | 3420 |
| `04` | u32 number of curve points *n* (≤ 88) | 71 |
| `08` | i16[n] curve, non-decreasing, last = 2636 | see below |
| `96..B7` | `0xFF` | |
| `B8` | 64 B UTF-8 name | `自定义轴体<i+1>` ("custom switch N") |
| `F8` | u32 | 6000 |
| `FC` | u8 | 0 |
| `FD..FF` | RGB colour | `FF00FF`, `00FFFF`, `30FF9F`, `6726FF` |

Default curve (from `0x80041DFC`): `-12 0 12 25 39 50 65 78 91 108 121 136 151 166 184 200 217 235
254 271 289 308 329 350 371 394 416 441 463 492 516 539 565 592 621 649 681 713 746 777 818 851 886
922 963 1003 1044 1088 1134 1181 1227 1280 1325 1387 1440 1499 1554 1620 1682 1751 1820 1896 1966
2046 2124 2221 2304 2394 2488 2594 2636`. A record is valid (`FUN_ram_000148F8`) when travel ∈
1500..4200, n ≤ 88, name byte 0 ≠ `FF`, word `F8` ≠ `0xFFFF`, byte `FC` ≠ `FF`, last point = 2636
and the curve is non-decreasing.

### Bank 2 layout (`0x80080000`, one profile, `0x1E38` bytes)

| Offset | Type | Field | Default |
|---|---|---|---|
| `000` | i16[88] | tap-hold code, main layer (key struct +0x10; sent after 0.5 s held) | 1000 |
| `0B0` | i16[88] | tap-hold code, Fn layer (key struct +0x12) | 1000 |
| `160` / `210` | i16[88] | `key_map` / `key_map_fn` | §2 |
| `2C0` / `370` | u16[88] | `press_th` / `return_th` (RT) | 50 / 30 |
| `420` / `4D0` | u16[88] | `press_deadzone` / `return_deadzone` | 55 / 0 |
| `580` / `630` | u16[88] | `norm_press_th` / `norm_return_th` | 1600 / 800 |
| `6E0` | u8[88] | `key_mode` (0 normal, 1 RT, 2 a third handler `0x1085A`) | 0; profile 0: W,A,S,D (32,46,47,48) = 1 |
| `738` | u8[88] | unknown, forced to `FF` on every profile load (`FUN_ram_000146AE`) | `FF` |
| `790` / `7E8` | u8[88] | modifier masks of the main / Fn layer, ORed into report byte 0 while the key is pressed (key struct +0x14/+0x15) | 0 |
| `840` | u16[88] | unknown, forced to `FFFF` on every load | `FFFF` |
| `8F0` / `9A0` | u16[88] | unknown (written by reset, no reader found) | 100 / 100 |
| `A50` | i16[88] | `key_cancel_code` (SOCD) | 1000 |
| `B00` | u8[85×3] | `rgb` static colours R,G,B | `FF` (white) |
| `BFF..C07` | — | gap (the vendor import bug lands here) | `FF` |
| `C08` | u8[85] | `rgb_solid_enable` | 0 |
| `C5D..C5F` | — | gap | `FF` |
| `C60` | u8[8] | `rgb_config` = mode, H, S, brightness, speed, **led_flags**, C66, C67 | `5D 90 FF 0A 32 FF FF FF` |
| `C68` | u8 | `rgb_enable` | 1 |
| `C69` | u8 | `cap_enable` | 1 |
| `C6A` | u8 | unused (no reader) | 1 |
| `C6B` | u8 | `rgb_fn_enable` | 1 |
| `C6C` | u32 | `sleep_second_cnt` (≤ 3600) | 0 |
| `C70` | u8 | `win_lock_state` = **WinLock indicator enable** (1) | 1 |
| `C71` | u8 | `auto_calib_enable` | 1 |
| `C72..CFF` | — | unused | `FF` |
| `D00 + 48·k` | 48 B ×88 | `bind_key_setting` | actions 0, keys 1000×4, top 100, bottom 3000, 8/24/16, 13×`FF`, byte 47 = 1 |
| `1D80..1DDF` | — | unused | `FF` |
| `1DE0` | u8[88] | `bind_key_mode` | 0 |

`rgb_config` bytes 6–7 (`C66`, `C67`) are copied into RAM but never read. Byte 5 (`C65`) is the
QMK-style LED flag mask (§5). Profiles 1–3 differ from profile 0 only in `rgb_config[0]`
(1 → `0x35` effect 13, 2 → `0x65` effect 25, 3 → `0xC9` effect 50), in Fn+C (`key_map_fn[62]` =
309/310/307) and in `key_mode` (all 0) **(emu)**.

### Boot (`FUN_ram_0000DD24`, from `FUN_ram_00010E28`)

1. Read bank 1. Repair it if `cur_param_num` ∉ 0..3, `soft_version ≠ 0x12`, word `214 ≠ FFFFFFFF`,
   `keytype[0] = FFFF` (→ all 2), `top[0] = FFFF`/`bot[0] = FFFF` (→ all 31000/17000), any name byte 0 =
   `FF` (→ default name), or any custom switch record invalid (→ default record). After a repair
   bank 1 is rewritten.
2. Load profile `cur_param_num` (`FUN_ram_000146AE`): read both sectors, force `738..78F` and
   `840..8EF` to `FF`, apply the fixed Fn shortcuts (`FUN_ram_00013DF8`, §2).
3. Validate (`FUN_ram_00011822`); invalid if `key_map` or `key_map_fn` is all `FFFF`, tap-hold code[0] (signed) >
   1000, any of `press_th[0]`, `return_th[0]`, `norm_*[0]`, deadzones[0] > 5000, `key_mode[0]` > 10,
   `sleep` > 3600, `rgb_config` (u32) = `FFFFFFFF`, `key_cancel_code` all `FFFF`, any bind record
   invalid (`FUN_ram_00014804`), `C70` > 10 or `C71` > 10. Invalid → UART "flash last data invalid",
   field-wise repair `FUN_ram_0000CD56` (each block above is reset to its default only if its own
   check fails; for profiles 1–3 the effect id and `rgb_solid_enable` are always reset) and the profile
   is written back.
4. `FUN_ram_0000A6F8` derives the run-time key structs, `FUN_ram_000149D8` the run-time lighting
   state (§5), sleep timeout = `sleep_second_cnt`.

Blank NOR therefore produces exactly the defaults above **(emu: `re-work/config/t_defaults.py`)**.
Unvisited profiles stay blank in NOR until first selected; they are then created with their defaults.

Holding **C** (key 62) at plug-in (its swing > 3× the mean and > 2000 counts) enters an on-board
calibration routine `FUN_ram_0000BFC0` (outside this document).

## 2. Factory defaults

`BE EE` and keycode 305 (`INITSET`) call `FUN_ram_0000F128`: every bank-2 field of the **active**
profile gets the default value from the table above (rgb_config `5D 90 FF 0A 32`, so profiles 1–3 also
get effect 23 here; key_mode WASD = RT only if the active profile is 0). Bank 1 (calibration, switch
types, names, custom switches, active profile) is untouched. Only the active profile is saved.

Keycode 319 held 5 s (40000 scans, counter `0x920A0`) erases NOR `0x80070000..0x80087FFF` (bank 1
and all four profiles), blinks all LEDs (60,60,60) three times (50 ms on/off) and resets via the
watchdog: the next boot is a first boot.

Default `key_map` (index = key, from table `0x80040EF8`, modifiers negative, 401 = Fn):

```
 0- 14: 41 58 59 60 61 62 63 64 65 66 67 68 69 70 74
15- 29: 53 30 31 32 33 34 35 36 37 38 39 45 46 42 76
30- 44: 43 20 26 8 21 23 28 24 12 18 19 47 48 49 75
45- 58: 57 4 22 7 9 10 11 13 14 15 51 52 40 78
59- 72: -7 29 27 6 25 5 17 16 54 55 56 -3 82 77
73- 82: -8 -5 -6 44 -2 401 -4 80 81 79          83-87: 1000
```

Default `key_map_fn`: 1000 except LShift/RShift/LCtrl/LAlt/RAlt/RCtrl mapped to themselves
(59 = −7, 70 = −3, 73 = −8, 75 = −6, 77 = −2, 79 = −4) and the fixed shortcuts below.

**Fixed Fn shortcuts** — `FUN_ram_00013DF8` rewrites these on every profile load, bank-1 save and
reset, which is why the driver refuses to remap them: Esc→319 (5 s reset), Bksp→315 (light on/off),
Del→320 (WASD↔arrows), `\`→313 (next effect), C→308+p (next profile: p0→308, p1→309, p2→310,
p3→307), ↑→311 (brighter), LWin→316 (WinLock), ←→318 (slower), ↓→312 (dimmer), →→317 (faster).

## 3. Dispatcher `FUN_ram_0000B812`

Called from the per-step tick `FUN_ram_00007A88` when a vendor OUT report arrived (`0x92024`); one
frame per call, handled inline in the tick (scanning for that step waits). The frame is copied to
`0x8CD48`. Acceptance: transfer length exactly 1024, byte 0 = `BE`, `len` (LE16) ≤ 1024, byte
`len−1` = `ED`. Every accepted frame resets the idle counter `0x91FF8` (sleep, §5). `n` = BE16 at
bytes 8–9, `addr` = LE32 at 4–7. "USB masked" below = PLIC enable word 1 bit 19 (source 51, USB0)
cleared; "scan stopped" = `FUN_ram_00015F90` (flag `0x920E4`: the ADC ISR stops PWM0/PWM1 at the end
of the current scan, then `delay_ms(40)`); "scan restarted" = `FUN_ram_000143EA`;
"wait-USB(t)" = busy-wait up to t ticks for the IN endpoint to be free (`FUN_ram_00013BDA`).

| cmd | Checks | Action, reply |
|---|---|---|
| `00` | len = 5 | stream on (`0x91F74 = 1`), heartbeat counter `0x92078 = 0`. Counter +1 per scan; at 16000 (2 s) stream off and `DA` flag cleared. No reply |
| `01` | len not checked | if `0x80080000 ≤ addr`, `addr+n ≤ 0x80100000`, `n ≤ 512`: ROM NOR read of `addr` into a 0x130-byte stack buffer. **Always** replies `BE 01 <n+13> …` copying `n` bytes from that buffer → stack garbage, and for n > ~1011 a buffer overrun (why `01` hangs). Never send |
| `02` | len = 5 | wait-USB(50), USB masked, scan stopped; write bank 1 (`0x790`) then the active profile (`0x1000`+`0xE38`), `delay_ms(100)`; Fn shortcuts re-applied; scan restarted; USB unmasked; 10-tick wait; wait-USB(50). No reply. Bank 1 save also persists `cur_param_num` |
| `03` | `n = len − 13`, CRC-16/MODBUS over `len−3` bytes = LE16 at `len−3`, `addr ≥ 0x80070000` | three ranges, below |
| `04` | len not checked | bank 1 if `addr ≥ 0x80070000`, `addr+n ≤ 0x80078000`, n ≤ 512; bank 2 if `addr ≥ 0x80080000`, `addr+n ≤ 0x80100000`, n ≤ 512. Reply `BE 04 <n+13> <addr> <n> data crc ED` queued (`0x92074 = n+13`); `FUN_ram_000133C6` adds the CRC and sends it later in the same tick (wait-USB(50) before and after). Out-of-range requests still reply with n bytes of stack garbage (overrun for large n) |
| `1D` | len = 5 | wait-USB(50); reply `BE 1D 15 00 <UID 16> ED` sent immediately; wait-USB(50); 10 ticks |
| `C0` | len = 6, byte 4 ≤ 3 | `0x91FD0 = p`, pending = 1, cooldown `0x9214C = 7200` scans. Executed at the next scan end (§4). The cooldown is not checked here |
| `DA` | len = 6, byte 4 ∈ {2,3} | `0x91F78 = byte4 − 2` |
| `EE` | len = 5 | wait-USB(50), USB masked, scan stopped, defaults (§2), save active profile, re-derive (`A6F8`), scan restarted, USB unmasked, 30 ticks, wait-USB(50). No reply |
| `FC` | len = 5 | USB masked, scan stopped, jump to `0x80020000` |
| `FE` | len = 6 | `0x91F80 = byte4` (any value), `0x91F7C = 1 − byte4`. While `0x91F80 ≠ 0` the tick skips key processing and runs the calibration stream (`FUN_ram_0000E860`) |
| `FF` | len = 5 | USB masked, scan stopped, PPOR `0xF410001C = 6000000` (watchdog), UART "Waiting for WDG reset", `delay_ms(1000)` |

**`BE 03` write ranges.** Data are copied into the shadow first, then:

- **Bank 1** (`0x80070000 ≤ addr`, `addr+n ≤ 0x80078000`, n ≤ 512): wait-USB(50), USB masked, scan
  stopped. If any key has `|bot − top| < 3000` (`FUN_ram_00015B66`) the write is **discarded**
  (bank 1 re-read from NOR); else bank 1 is saved (erase + program `0x790`) and re-derived (`A6F8`).
  Scan restarted, USB unmasked, 10 ticks, then the ack `BE 03 05 00 ED` (after the flash write).
  Note the shadow is only `0x790` bytes: `0x80070790..` aliases the preview area and then live ILM
  code (`0x80070A04` = `FUN_ram_00007A88`).
- **Bank 2** (`addr ≥ 0x80080000`, `addr+n ≤ 0x80100000`, n ≤ 512): RAM only, ack sent at once.
  `addr ≤ 0x80080A1F` or `addr+n > 0x80080C60` → full re-derive `A6F8` (keys + lighting + sleep);
  otherwise (writes inside `key_cancel_code`/`rgb`/`rgb_solid_enable`) only the lighting reload
  `FUN_ram_000149D8`. The shadow ends at `0x80081E37`; higher addresses overwrite live ILM state
  (`0x80081E38` = ILM `0x2490`, the per-key curve buffers) and beyond `0x8009F9A8` fault. Not saved
  until `BE 02`.
- **Preview area** (`0x80078000 < addr+n ≤ 0x80080000`, start ≥ `0x80078000`): RAM at ILM `0x7814`,
  no ack, no save. Start exactly `0x80078000` → preview mode 1; start exactly `0x80078055` → mode 2
  and the 85 RGB triples at `0x80078055` are rewritten in place as `cc(rgb) · V / 255` (colour
  correction §5, current brightness V). The mode is cleared after 3 s without a new trigger write
  (`0x796C` counts seconds). Layout: `0x80078000` alpha[85], `0x80078055` rgb[85×3], ILM `0x7968` mode.
  Mode 1: each LED = (static·α + effect·(255−α))/255; mode 2: each LED = preview colour (replaces the
  effect, static colours and blending; Caps/WinLock/Fn overlays still apply).

Which changes re-derive: any bank-1 save, any bank-2 write except the lighting window above, `EE`,
profile load. `A6F8` always ends with the lighting reload, so speed/brightness/effect set by keys
(RAM only) are overwritten by any later config write.

## 4. Profiles

- **Switch request** — `BE C0 p` (no cooldown check) or keycodes 307–310 / Fn+C (`FUN_ram_0000C698`,
  key-down edge only, ignored while cooldown `0x9214C` ≠ 0). Both set `0x91FD0 = p`, pending `0x91FD4
  = 1` and cooldown = 7200 scans (0.9 s, decremented per scan).
- **End of scan** (ADC ISR `FUN_ram_000002A0`): pending 1 → PWM0/PWM1 stopped (`FUN_ram_00015ABE` —
  this function only halts the scan, it is not the switch itself), pending = 2.
- **Next tick** (`FUN_ram_0001170E`): pending = 0; wait for the IN endpoint; USB masked; if `p ≠
  cur_param_num`: `cur_param_num = p`, load profile p from NOR (`FUN_ram_0001385C`: read, validate,
  repair+write if invalid, `A6F8`). Unsaved RAM edits of the old profile are lost. `cur_param_num` is
  **not** written to NOR here (only by the next `BE 02` or bank-1 write), so a reboot returns to the
  last saved profile. Scan restarted, USB unmasked, wait-USB(50).
- **Notification** — if USB is configured and idle: `BE C0 06 00 <p> ED` (zero-padded to 1024) on
  EP `0x82`, also when `p` equals the current profile.
- **Feedback** — 1000 ticks (0.73 s) of all-LED blinking, 200 ticks on / 200 off (3 on-phases),
  overriding everything: p0 (60,60,60), p1 (60,0,0), p2 (0,60,0), p3 (60,30,0) (tables
  `0x80040440/30/20` = R/G/B).

## 5. Lighting engine

The engine is a port of **QMK `rgb_matrix`** (lib8tion maths, `random8`, `effect_runner_*`, hit
tracker, 17-LED render chunks) with this board's geometry. It runs in the main loop
(`FUN_ram_00008BE2`, called forever from `FUN_ram_80045256`), not in the tick.

### Run-time state (reloaded by `FUN_ram_000149D8` on every config change)

`0x843F0` = `[mode, H, S, V]`, `0x843F4` = `[speed, flags, C66, C67]` copied from `rgb_config`;
`V = 6 × brightness` where brightness > 30 is clamped to 30 (and written back), so **V ≤ 180**
(the only "power limit"). `rgb_solid_enable` → `0x91D14`; static colours → `0x9152C[led] =
cc(rgb[led]) × V / 255` (`FUN_ram_00013B42`).

- `rgb_config[0]`: bits 7..2 = effect id (0–63), bits 1..0 = QMK's `enable` field: stored and compared
  (a change re-inits the effect) but never used to switch lighting off. The driver writes 1.
- `flags` (`rgb_config[5]`): QMK `params->flags`; an effect only draws LED i if
  `led_flags[i] & flags`. Every LED has flag `0x04` (keylight), so `0xFF`/`0x04` = all LEDs, a value
  without bit 2 = effect draws nothing. Changing it clears all LEDs once.

### Frame pipeline (state `0x91C62`)

1. Every main-loop pass: per-key analog depth (`FUN_ram_00010550`, 0..255 from remaining height,
   100 µm dead band at both ends) and a 6×15 pressed bitmap; each newly **pressed** key → hit tracker
   (`FUN_ram_0000F518`, 8 entries of x, y, LED, u16 age; ages advance by elapsed ticks) and, for
   effect 32, the typing-heatmap increment. Releases are not tracked.
2. State 0: `g_rgb_timer = now` (`0x92080`, 16-bit used by effects), chunk = 0.
3. State 1: the effect renders LEDs `17·chunk … 17·chunk+16`; 5 passes per frame. After each pass
   the overlays below are applied.
4. State 2: flush (`FUN_ram_000153E2`): if `rgb_enable = 0` **or** USB suspended (`0x9200C = 0`) all
   LEDs are set to 0; then the WS2812 transfer (blocking, §6). State 3.
5. State 3: wait until `now − g_rgb_timer ≥ 8` ticks (5.8 ms) → state 0. Frame period =
   max(5.8 ms, render + 2.7 ms transfer) ≈ 170 fps.

Effect 0 renders black once (on init) and then stops flushing; ids 51–63 render nothing (LEDs keep
their previous content).

**Overlays** (after each render pass, in this order):

1. On-board calibration indicator (`0x91FE0 ≠ 0`, only set by the C-at-boot routine): keys not yet
   accepted off, accepted keys blink red (255,0,0) at 1000/2000 ticks; nothing else below.
2. **Fn layer** (`rgb_fn_enable` and Fn held, `0x91F9C`): every key LED with `key_map_fn ≠ 1000` →
   (V,V,V) white, all others off (static colours ignored).
   Otherwise: preview mode 1/2 (§3) or, normally, **static override**: LEDs with
   `rgb_solid_enable = 1` show `cc(rgb)·V/255` instead of the effect.
3. **Caps Lock** (`cap_enable` and host LED bit 1, `0x9162B`): LEDs of keys whose *main-layer*
   keycode is CapsLock (57) → (255,0,0) — full red, not brightness-scaled.
4. **WinLock** (WinLock active `0x804A4`, toggled by keycode 316, RAM only, and Fn not held and
   `C70 = 1`): LEDs of keys mapped to LWin/RWin (−5/−1) → (255,0,0).
5. **Sleep**: if `0 < sleep_second_cnt < idle_seconds` all LEDs 0. `idle_seconds` (`0x91FF8`) +1 per
   second, reset every scan while any key is pressed and by every accepted vendor HID frame. Only
   the LEDs sleep; keys keep working and the first press wakes them.

Then, overriding all of it, the profile-switch blink (§4). Caps and sleep also apply on top of the
Fn layer; WinLock does not (it requires Fn released).

**USB suspend** (`FUN_ram_00007A88`): if PORTSC1 line state bit 11 (J/idle) stays set or the device is
unconfigured for 16000 scans (2 s), `0x9200C = 0` → LEDs dark at the next flush; when the bus resumes
`0x9200C = 1`. While suspended, a key press with remote wake-up enabled pulses PORTSC1.FPR
(resume) and resends the keyboard report.

**Keyboard lighting keys** (`FUN_ram_0000C698`, all on key-down only, RAM only — persisted only by a
later `BE 02`): 311/312 V ±12 (clamp 0..180; `rgb_config[3] = V/6`), 313/314 next/previous effect
(cycle 1, 3, 4 … 50, wrapping; id 2 is skipped; `rgb_config[0]` low bits kept), 315 toggles
`rgb_enable`, 317/318 speed ±17 (clamp 0..255; **runtime only**, not copied into `rgb_config[4]`, so
lost at the next config reload), 316 WinLock. There is no visual feedback for these.

### Colour maths

- **HSV → RGB** (`FUN_ram_00012964`) = QMK `hsv_to_rgb` without CIE: `s = 0` → (v,v,v); else
  `region = h·6/255`, `rem = (h·2 − region·85)·3 & 0xFF`, `p = v(255−s)>>8`,
  `q = v(255 − (s·rem>>8))>>8`, `t = v(255 − (s(255−rem)>>8))>>8`; region 0/6 (v,t,p), 1 (q,v,p),
  2 (p,v,t), 3 (p,q,v), 4 (t,p,v), 5 (v,p,q). Effects output straight HSV; no gamma.
- **Colour correction `cc`** (`FUN_ram_0000D9AA`, only for static colours and preview mode 2) — sRGB →
  CIE xy → barycentric weights on the sRGB primaries, renormalised to keep `r+g+b`. All integer, C
  truncating division (÷0 → −1), 32-bit wrap:
  `L[i] = trunc(1000·lin(i/255))` with `lin(x) = x/12.92` (x ≤ 0.04045) else `((x+0.055)/1.055)^2.4`
  (float32 maths). For input (r,g,b), `sum = r+g+b` (0 → black), `R,G,B = L[r],L[g],L[b]`,
  `Y = 212R+715G+72B`, `X = 412R+357G+180B`, `S = X+Y+19R+119G+950B`, `x = 1000X/S`, `y = 1000Y/S`,
  `a = (((150−x)·540 + (y−60)·150)·255 / −224100)·330000 / (80y)`,
  `b = (((y−330)·−490 + (640−x)·−270)·255 / −224100)·600000 / (660y)`,
  `c = (((y−330)·−340 + (640−x)·270)·255 / 224100)·60000 / (50y)`, `m = max(a,b,c)`,
  `a' = (Y·a/1000)·255/m` (same for b', c'), `t = a'+b'+c'`, out = `sum·(a',b',c')/t` each clamped to
  255 if ≥ 256 (signed compare, then low byte). E.g. white (255,255,255) → (255,161,215); at default
  brightness V = 60 → (60,37,50). Verified against the firmware on 3005 random colours **(emu,
  `t_cc.py`)**.

### Geometry (DLM, QMK `g_led_config`)

LED index = WS2812 chain position. Matrix (6 rows × 15 columns, `0x80000`, 255 = none):

```
row0: 84 83 82 81 80 79 78 77 76 75 74 73 72 71 70
row1: 55 56 57 58 59 60 61 62 63 64 65 66 67 68 69
row2: 54 53 52 51 50 49 48 47 46 45 44 43 42 41 40
row3: 26 27 28 29 30 31 32 33 34 35 36  - 37 38 39
row4:  - 25 24 23 22 21 20 19 18 17 16 15 14 13 12
row5:  0  1  2  3  -  4  -  5  -  6  7  8  9 10 11
```

LED → key (`0x80159`): `73 74 75 76 76 76 77 78 79 80 81 82 72 71 70 69 68 67 66 65 64 63 62 61 60
59 45 46 47 48 49 50 51 52 53 54 55 56 57 58 44 43 42 41 40 39 38 37 36 35 34 33 32 31 30 15 16 17
18 19 20 21 22 23 24 25 26 27 28 29 14 13 12 11 10 9 8 7 6 5 4 3 2 1 0`.

LED points (x,y) (`0x8005A`), LED 0..84; centre (60,21), y = 0 top row, 42 bottom row:

```
 0-11 y=42: x 1 11 21 39 51 63 80 88 96 104 112 120
12-25 y=34: x 120 112 101 90 82 74 66 58 50 42 34 26 18 5
26-39 y=26: x 3 14 22 30 38 46 54 62 70 78 86 94 107 120
40-54 y=18: x 120 110 100 92 84 76 68 60 52 44 36 28 20 12 2
55-69 y=10: x 0 8 16 24 32 40 48 56 64 72 80 88 96 108 120
70-84 y=0:  x 120 112 102 94 86 78 68 60 52 44 34 26 18 10 0
```

LED flags (`0x80104`): all `0x04`. QMK constants are otherwise unchanged (e.g. BAND uses
`scale8(x,228)+28`), so on this 0..120 × 0..42 grid some effects look compressed vs QMK boards.

### Effect ids (`FUN_ram_00008BE2` switch, ids 0–50)

Inputs: H, S, V, speed (`sp`), `t = g_rgb_timer` (ticks). Runner shorthands (QMK):
**i** = `effect_runner_i`, time `((sp>>2)+1)·t >> 8`; **dxdy** = runner_dx_dy, time `(sp>>1)·t >> 8`,
dx/dy from centre; **dist** = runner_dx_dy_dist (same time, `sqrt16`); **sc** = runner_sin_cos_i,
`cos8/sin8((sp>>2)·t>>8)`; **reactive** = runner_reactive_splash over the hit tracker, tick scaled by
`min(sp+1,255)`. UI = name in the vendor driver (only those ids are offered).

| id | QMK animation | Notes / UI |
|---|---|---|
| 0 | NONE | black |
| 1 | SOLID_COLOR | (H,S,V). UI "ice-blue solid" |
| 2 | ALPHAS_MODS | LEDs with flag 1 get hue H+sp; no LED has it → solid |
| 3 | GRADIENT_UP_DOWN | `h = H + (sp>>2)·(y>>4)`. UI rainbow gradient |
| 4 | GRADIENT_LEFT_RIGHT | `h = H + ((sp>>2)·x >> 5)` |
| 5 | BREATHING | `v = scale8(abs8(sin8((sp>>3)·t>>8) −128)·2, V)`. UI ice-blue breathing |
| 6 / 7 | BAND_SAT / BAND_VAL | i |
| 8 / 9 | BAND_PINWHEEL_SAT / _VAL | dxdy |
| 10 / 11 | BAND_SPIRAL_SAT / _VAL | dist |
| 12 | CYCLE_ALL | i: `h = time`. UI "three-colour breathing" |
| 13 / 14 | CYCLE_LEFT_RIGHT / UP_DOWN | i: `h = x − time` / `y − time` (H ignored). UI scroll L-R / U-D |
| 15 | RAINBOW_MOVING_CHEVRON | i: `h = H + abs8(y−21) + x − time` |
| 16 | CYCLE_OUT_IN | dist: `h = 3·dist/2 + time`. UI converge to centre |
| 17 | CYCLE_OUT_IN_DUAL | dxdy: `dx = 30 − abs8(dx)`, `h = 3·sqrt16(dx²+dy²) + time` |
| 18 | CYCLE_PINWHEEL | dxdy: `h = H ± atan2_8 − time`. UI rotate around centre |
| 19 | CYCLE_SPIRAL | dist |
| 20 / 21 / 22 | DUAL_BEACON / RAINBOW_BEACON / RAINBOW_PINWHEELS | sc. 21 = UI marquee |
| 23 | *vendor* | time `((sp/10)+1)·t >> 8`; `h = 3(x−y) + time` for y > 21, `3(x−y) − time` above; S,V; rows y ≥ 22 output (B,G,R) swapped. UI "rainbow breathing" |
| 24 | RAINDROPS | when `((min(sp+16,255)·t)>>8) % 10 = 0`: random LED `random8()·85>>8` gets `H + ((H+180)%360−H)/4 · (random8()&3)`. UI starlight |
| 25 | JELLYBEAN_RAINDROPS | random hue, sat `random8()/2+127`, when `…%5 = 0` |
| 26 | HUE_BREATHING | `h = H + ((abs8(sin8((sp>>3)t>>8)−128) & 0x7F)·6 >> 6)` (QMK uses `scale8(…, 12)`) |
| 27 / 28 | HUE_PENDULUM / HUE_WAVE | i |
| 29 | PIXEL_RAIN | every `500/(min(sp+16,255)>>4)` ticks a random LED: random hue, sat 127–255, V, or off if `random8()&2`. UI "ripples" |
| 30 | PIXEL_FLOW | interval `3000/(…>>4)`; shifts an 85-LED colour line by one |
| 31 | PIXEL_FRACTAL | interval `3000/(…>>4)`; 6×15 grid, mirrored halves, colour `hsv_to_rgb(843F0>>8)` |
| 32 | TYPING_HEATMAP | press adds 32 to the key cell, neighbours `min(40−dist,16)`; decay −1 every 25 ticks; `h = 170 − qsub8(v,85)`, `v = scale8(qadd8(v,170)·3, V)`, S. UI single-colour bloom |
| 33 | *vendor analog heatmap* | frame buffer rebuilt every pass from key depth d: own cell += d, others within dist 60 += `(d/60)·(60−dist)`; same colouring as 32, no decay. UI RGB bloom |
| 34 | DIGITAL_RAIN | drop chance `rand() < 0x555`, step every 28 frames; green (boost, V, boost) |
| 35 | SOLID_REACTIVE_SIMPLE | per LED: newest hit on it, `tick = min(sp+1,255)·age >> 8` (age capped at `0xFFFF/(sp+1)`), `v = scale8(255−tick, V)`. UI single-key reactive |
| 36 | SOLID_REACTIVE | `h = H + qsub8(130, tick)` |
| 37 / 38 | SOLID_REACTIVE_WIDE / MULTIWIDE | reactive, `v += 255 − min(tick + 5·dist, 255)` |
| 39 / 40 | SOLID_REACTIVE_CROSS / MULTICROSS | reactive. UI reactive cross (39) |
| 41 / 42 | SOLID_REACTIVE_NEXUS / MULTINEXUS | reactive, `h = H + dy/4`, cut at dist 72 / ±8 bands |
| 43 / 44 | SPLASH / MULTISPLASH | reactive, `h += tick − dist`. UI whole-board RGB (43) |
| 45 / 46 | SOLID_SPLASH / SOLID_MULTISPLASH | reactive |
| 47 | STARLIGHT | when `((min(sp+5,255)·t)>>8) % 5 = 0`: LED `rand()%85` set to `(H,S, breathing v)` |
| 48 / 49 | STARLIGHT_DUAL_HUE / _SAT | same trigger |
| 50 | RIVERFLOW | `v = scale8(abs8(sin8(((t + 315·i)·(sp>>3))>>8) −128)·2, V)`. UI single-colour sparkle |

"Single" variants (37, 39, 41, 43, 45) start from the newest hit, "multi" from all 8. `random8` is
QMK's (`seed = seed·2053 + 13849`, returns low+high byte, `0x91C06`); `rand()` is newlib's
(`FUN_ram_80045FA6`). Exact per-LED output for any config: run the harness (§7) — it executes the
firmware's own code, including its PRNG state.

## 6. WS2812 output

`FUN_ram_000139FC` expands the 85×(R,G,B) array at ILM `0x7970` into the DMA buffer `0x6090` in
**G, R, B order**, MSB first, one 16-bit SPI word per bit: `1` → `0xFFC0` (10/16 high), `0` → `0xE000`
(3/16 high) at 12 MHz (1.33 µs/bit; table `0x5090` = 256 × 8 words, linear — no gamma). 85 × 48 bytes
+ 2 zero bytes = 4082 bytes (2041 words), sent by HDMA to SPI1 (`FUN_ram_00013D78`, busy-waits for
DMA completion ≈ 2.7 ms). Inter-frame gap ≥ 3 ms (state-3 wait) provides the latch. No current
limiter beyond V ≤ 180; overlays (Caps/WinLock/calibration) use 255 and the static path can reach
255 per channel after `cc`.

## 7. Emulator harness (`re-work/config/`, git-ignored)

- `emu.py` — unicorn RV32 model: boots the app from reset with an emulated NOR (ROM `xpi_nor`
  read/erase/program implemented, custom CSRs stubbed, busy-waits and USB sends stubbed) up to the
  main loop; `Emu.call(fn, *args)`; `backup_flash()` builds NOR from a `tools/dump_all.py` backup on top
  of the firmware's own defaults; `default_images()` returns the factory bank-1/profile images.
- `rgb_harness.py EFFECT [--frames N --h --s --bright --speed --flags --t0 --step --press K@F-G
  --height --static LED=RRGGBB --caps --fn --off --config real --raw]` — renders frames by calling
  `FUN_ram_00008BE2` with the firmware's scheduler and prints per-LED R,G,B at each flush. Ticks are
  simulated (`--step 8` = firmware frame pacing). Usage is documented at the top of the file.
- `t_defaults.py` (dumps factory images, `defaults.pkl`), `t_cc.py` (colour-correction model check),
  `decomp.c` (Ghidra headless decompilation, `fn.sh <addr>` prints one function).

## 8. Open questions

- Semantics of bank-2 layers A/B (`000`, `0B0`) with companions `790`/`7E8`, of `738`, `840`,
  `8F0`, `9A0`, of bank-1 word `214` and of `C6A`/`C66`/`C67` (no readers found for the last three).
- `key_mode = 2` handler `0x1085A` (not offered by the driver).
- Tick rate assumes one `FUN_ram_00015250` call per ADC step (11 per 8 kHz scan); confirm with a
  stopwatch on hardware (e.g. the 0.73 s profile blink) before matching effect speeds.
- The ROM erase length (`0x790`, `0xE38`) is not sector-aligned; the ROM presumably erases whole 4 KB
  sectors — confirm before relying on bytes past the shadow surviving a save.
- `BE 01` behaviour on hardware (hangs) is consistent with the stack-buffer overrun but the in-range
  hang is not explained.
