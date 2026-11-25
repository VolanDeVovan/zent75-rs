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
| `000` | i16[88] | extra keycode layer A (copied to key struct +0x10) | 1000 |
| `0B0` | i16[88] | extra keycode layer B (key struct +0x12) | 1000 |
| `160` / `210` | i16[88] | `key_map` / `key_map_fn` | §2 |
| `2C0` / `370` | u16[88] | `press_th` / `return_th` (RT) | 50 / 30 |
| `420` / `4D0` | u16[88] | `press_deadzone` / `return_deadzone` | 55 / 0 |
| `580` / `630` | u16[88] | `norm_press_th` / `norm_return_th` | 1600 / 800 |
| `6E0` | u8[88] | `key_mode` (0 normal, 1 RT, 2 a third handler `0x1085A`) | 0; profile 0: W,A,S,D (32,46,47,48) = 1 |
| `738` | u8[88] | unknown, forced to `FF` on every profile load (`FUN_ram_000146AE`) | `FF` |
| `790` / `7E8` | u8[88] | companions of layers A / B (key struct +0x14/+0x15) | 0 |
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
3. Validate (`FUN_ram_00011822`); invalid if `key_map` or `key_map_fn` is all `FFFF`, layer A[0] >
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
