# Vendor raw-HID protocol

The configurator talks to interface **IF2** (vendor HID, usage page `0xFF60`, usage `0x61`,
1024-byte reports, report id 0). Commands go **out** on EP `0x03`, replies and the live stream come
**in** on EP `0x82`. The whole command parser is `FUN_ram_0000b812` (the "dispatcher"); it is
invoked from the 8 kHz tick (`FUN_ram_00007a88`) when a report has arrived
(`DAT_ram_00092024 != 0`), so a command is handled synchronously between scans.

Two sources back this document: the firmware (0.0.18 decompilation, cited as `FUN_ram_*`/
`DAT_ram_*`) and the vendor web driver + device probing (see `panel/PROTOCOL-NOTES.md`, cited as
"vendor JS"). Where they agree it is stated once; the firmware is authoritative for behaviour the
driver never exercises.

## Framing

```
BE  <cmd:1>  <len:LE16>  <payload...>  [<crc:LE16>]  ED
```

- `len` is the **whole frame length** including `BE` and `ED`. The report is zero-padded to 1024.
- `FUN_ram_0000b812` validates, in order: `byte0 == 0xBE` (`-0x42`); `len <= 0x400`;
  `byte[len-1] == 0xED` (`-0x13`, read as `(&DAT_ram_0008cd47)[len]`). A frame that fails any check
  is dropped silently (`DAT_ram_00091ff8 = 0; return`).
- The payload starts at byte 4. Request `addr` (when present) is **LE32** at bytes 4..7; request
  `n`/length is **BE16** at bytes 8..9 (`CONCAT11(byte8, byte9)` in the decompiler).
- **CRC-16/MODBUS** (poly `0xA001` reflected, init `0xFFFF`), table at `0x80040bb4`, computed by
  `FUN_ram_000157d2(buf, count)`. It covers **everything before the 2 CRC bytes**, i.e. bytes
  `0 .. len-3`; the CRC sits at `[len-3 : len-1]`, `ED` at `[len-1]`. The CRC is present on `BE 03`
  requests, on `BE 01`/`BE 04` replies, and on the `BE FD` stream. The vendor driver verifies reply
  CRCs but ignores the stream's.

## Command table

| cmd | req `len` | Name | Handler | Notes |
|---|---|---|---|---|
| `00` | 5 | Heartbeat | switch case 0 | sets `DAT_ram_00091f74 = 1` → enables the `BE FD` stream; `DAT_ram_00092078 = 0`. No reply. |
| `01` | 11 | Read "flash" | case 1 → `FUN_ram_0001598c` | Reads `addr+0x80000000` (raw DLM) via the XPI ROM API. **Driver never sends it; unsafe** (see below). Reply is shaped like `04` with cmd `01`. |
| `02` | 5 | Save to flash | case 2 | commits the RAM shadows to NOR (`FUN_ram_00015f5a`); masks the ADC IRQ around the write. No reply. |
| `03` | 13+n | Write RAM | case 3 | writes a shadow; validated + CRC-checked (below). Ack `BE 03 05 00 ED`. |
| `04` | 11 | Read RAM | case 4 | reply `BE 04 <13+n LE16> <addr LE32> <n BE16> <data n> <crc> ED`. |
| `1D` | 5 | Serial number | case 0x1d | reply `BE 1D 15 00 <16 bytes @0x91f44> ED` (len `0x15`). |
| `C0` | 6 | Switch profile | `cmd==0xC0` branch | `byte4 ∈ 0..3`; sets `DAT_ram_00091fd0 = profile`, `DAT_ram_00091fd4 = 1` (applied at end of scan by `FUN_ram_00015abe`). The board also pushes `BE C0 06 00 <n>` on an Fn switch. |
| `DA` | 6 | Diagnostic-output toggle | `(cmd+0x26)` case 0 | `byte4 ∈ {2,3}` → `DAT_ram_00091f78 = byte4-2 ∈ {0,1}`. **Firmware-only; the vendor JS never sends it** (see "The `0xDA` handler"). |
| `EE` | 5 | Reset config | case 0x14 | restores defaults: `FUN_ram_0000f128` then re-applies via `thunk_FUN_ram_0000a6f8`. No reply. |
| `FC` | 5 | Enter bootloader | case 0x22 → `FUN_ram_00015a0c` | quiesces and jumps to `0x80020000` (see `bootloader.md`). |
| `FE` | 6 | Calibration mode on/off | case 0x24 | `byte4=1` → `DAT_ram_00091f80 = 1` (board streams the 1003 variant); `byte4=0` → normal. Sets `DAT_ram_00091f7c = 1-byte4`. No reply. |
| `FF` | 5 | Reboot | case 0x25 → `FUN_ram_00015c9c` | `PPOR 0xf410001c = 6000000`, prints "Waiting for WDG reset", spins. |

`FD` is not a command — it is the device→host stream (below). There are **no other commands**: the
`cmd < 0xC0` switch has only cases `0,1,2,3,4,0x1d`; `cmd == 0xC0` is its own branch; `0xC1..0xD9`
are rejected; `cmd >= 0xDA` is `switch(cmd+0x26)` with only cases `0 (0xDA)`, `0x14 (0xEE)`,
`0x22 (0xFC)`, `0x24 (0xFE)`, `0x25 (0xFF)` — every other value hits `default` and is dropped.

## Address validation and the shadow translation (`BE 03` / `BE 04`)

`addr` is checked against two banks; `n <= 0x200`:

| Bank | Valid `addr` range | `addr + n` must be ≤ | Shadow written/read |
|---|---|---|---|
| 1 (board-global) | `0x80070000` ≤ addr | `0x80078000` | `addr + 0x7ff97084` (ILM `0x7084`) |
| 2 (active profile) | `0x80080000` ≤ addr | `0x80100000` | `addr + 0x7ff80658` (ILM `0x658`) |

- `n > 0x200` (512) is rejected. A request wholly outside both banks returns without acting.
- A **bank-1** write (`top_value`/`bot_value`/`keytype`/names) additionally runs a flash-save
  sequence and sends `BE 03 05 00 ED` straight away — bank-1 writes are expensive. It then updates
  the bank-2 live mirror (`+0x7ff8f814`, ILM `0xf814`) for addresses in the RGB range and may
  recompute brightness.
- Two **virtual addresses** in the gap are special, not storage: writing `0x80078000` sets
  `DAT_ram_00007968 = 1`; writing `0x80078055` sets it to `2`, which triggers the global
  brightness-scale pass over the LED buffer (`FUN_ram_0000bfc0`-style loop in the dispatcher).
- **`BE 01` is dangerous.** It adds `+0x80000000` (not the shadow offset), so `0x80080000` maps to
  `0x00080000` = raw DLM, and reads go through the XPI NOR ROM API (`FUN_ram_0001598c`). The vendor
  JS defines but never calls it; out-of-range values have hung the firmware. Treat as a memory peek
  only.

Reply framing for `BE 04` (and `BE 01`), assembled at `0x8c948`, shipped by `FUN_ram_000133c6`
→ `FUN_ram_000150f0(0x82, …, 0x400)`:

```
BE 04  <len=13+n : LE16>  <addr : echoed LE32>  <n : echoed BE16>  <data : n bytes>  <crc LE16>  ED
```

so `data` is at `[10 : len-3]` and **data length = len − 13** (vendor JS: `n = t[2]+256*t[3]`,
data `t[10 : n-3]`). The driver matches replies on the echoed `addr`/`n`.

## Serial number

`BE 1D` → 16 bytes copied from `DAT_ram_00091f44`. The device on hand returns
`54 d7 33 86 65 32 44 43 58 b7 ba 62 21 14 9e 98` (`backups/.../dump.json: serial_raw`); the driver
shows it as 32 hex digits. The firmware needs ≈60 ms of quiet after this reply before it answers
again (vendor observation).

## The live stream `BE FD`

Produced inside the 8 kHz tick. It is **gated on heartbeats**: the normal frame is sent only while
`DAT_ram_00091f74 != 0` (set by `BE 00`) **and** `DAT_ram_00091ffc != 0`, the latter set every 320
scans (`DAT_ram_00092018 % 0x140 == 0`) → **≈25 Hz** (8000/320), matching the observed 23–25 fps.
Each frame gets a MODBUS CRC and `ED` like any reply.

### 1017 variant (normal), `len = 0x03F9`

Built at `0x8c948` (`FUN_ram_00007a88`, the `DAT_ram_00091f74 && DAT_ram_00091ffc` block): header
`BE FD F9 03`, then **6 arrays of 83 × u16** in README key order (indexed through the scan→key table
`0x80040580`), then a 7-word tail, then CRC + `ED`:

| Words | Source (`DAT_ram_*`) | Meaning |
|---|---|---|
| `[0..82]` | `0x8ee28` | filtered raw ADC (rest ≈ 30000, bottom ≈ 18000) |
| `[83..165]` | `0x8f0e8` | **travel in µm** (0 at rest, up to the switch's max travel) |
| `[166..248]` | `0x90588` | per-key rest baseline (dynamic) |
| `[249..331]` | `0x8ef88` | baseline copy / slow-tracked rest level |
| `[332..414]` | `0x90f28` | 0 at rest (per-key, auxiliary) |
| `[415..497]` | `0x8f3a8` | 0 at rest (per-key, auxiliary) |
| tail word 498 | `0x91f78` | the `0xDA` diagnostic flag (0/1) |
| tail word 499 | `0x92020` | scan rate = **8000** Hz |
| tail word 500 | `0x91fb0/1000` | ≈680 (a scaled internal counter) |
| tail words 501..504 | `0x843e8` (8 bytes) | FD frame counter (low word), rest 0 |

This resolves the "two more per-key arrays" and the `0,8000,680,534,0,0,0` tail noted from probing:
they are the baseline/tracked levels and the housekeeping tail; the driver uses only `[0..82]`
(raw) and `[83..165]` (travel). Travel is produced from the curve-lookup result by
`0x8f0e8 = clamp(max_travel − curve_height, 0, max_travel)` (see `algorithms.md`).

### 1003 variant (calibration mode), `len = 0x03EB`

Entered by `BE FE 01`; the tick switches to `FUN_ram_0000e860` and the normal processing is
skipped. It smooths the raw scans (IIR `+= (raw-acc)>>4`), skips the first ~8000 scans, snapshots
the 88-value raw frame into one of 6 history buffers (`0x8a708 + slot*0x160`) every 8th scan, and
when 6 snapshots are collected (`DAT_ram_00092014 == 0x30`) emits header `BE FD EB 03` then
**6 × 83 u16 raw samples** (the 6 snapshots interleaved per key, via `0x80040580`), CRC over 1000
bytes, `ED`. So the 1003 frame is exactly `6 × 83` raw values, produced only in calibration mode.

## The `0xDA` handler (firmware-only)

`BE DA 06 00 <2|3> ED` sets `DAT_ram_00091f78` to 0 or 1. The value is echoed in the 1017 tail
(word 498) and, when 1, enables an extra per-key post-processing pass in the tick:
`FUN_ram_00015a84(&DAT_ram_000801b0, &DAT_ram_0008f248, &DAT_ram_0008f3a8, &DAT_ram_0008f0e8)` — it
rewrites the travel/auxiliary arrays (`0x8f0e8`, `0x8f3a8`) from a config table at `0x801b0`. It is
a diagnostic / alternate-output mode; the vendor driver never sends it, so it is firmware-only
ground not covered by the JS. Length must be 6 and `byte4 ∈ {2,3}`, else the frame is dropped.

## RAM map — cross-checked against the firmware

The README / `tools/rammap.py` map is **correct**. Notes confirmed in the dispatcher and the
algorithm code:

- `0x8007xxxx` is bank 1 (board-global, not swapped by `BE C0`); `0x8008xxxx` is bank 2 (active
  profile). Validation bounds (bank 1 ends at `0x80078000`, bank 2 at `0x80100000`, `n ≤ 0x200`)
  match the table above.
- `soft_version` (`0x80070210`) and `cur_param_num` (`0x80070218`) are firmware-owned; the restore
  tool must skip them (`rammap.READ_ONLY`), confirmed by the dispatcher never accepting app writes
  there as config.
- `auto_calib_enable` (`0x80080C71`) is read by the algorithm code as `DAT_ram_000012c9`
  (`0x658 + 0xC71`), confirming both the address and that it is a bank-2 (per-profile) byte.
- `top_value`/`bot_value` (`0x80070000`/`0x800700B0`) are `DAT_ram_00007084`/`DAT_ram_00007134`
  and feed the per-key span (`algorithms.md`).
- The 0.0.18-only regions `bind_key_setting` (`0x80080D00`, 48 B/key) and `bind_key_mode`
  (`0x80081DE0`) are inside bank 2 and are read/written through the same case-3/case-4 path.

Semantics of each region (switch types, `key_mode`, thresholds in µm, SOCD `key_cancel_code`,
DKS/`bind_key_*` layout, RGB blocks, `sleep_second_cnt`) are documented from the vendor side in
`panel/PROTOCOL-NOTES.md`; the firmware-side meaning of the actuation/DKS/SOCD fields is in
`algorithms.md`.
