# Configurator protocol (stock firmware)

Vendor raw HID on **interface 2** (usage page `0xFF60`, usage `0x61`), 1024-byte reports, report
id 0. Commands go out on EP `0x03`, replies and the live stream come in on EP `0x82`. The dispatcher
is `FUN_ram_0000B812`, run from the 8 kHz tick between scans. Our firmware should speak the same
protocol first so the panel (`panel/src/protocol/`) and `tools/zentlib.py` drive both unchanged.
Vendor-side semantics of every region (switch types, thresholds, DKS record, lighting ids,
calibration flow, driver timings): `panel/PROTOCOL-NOTES.md`.

## Framing

```
BE <cmd> <len:LE16> <payload…> [<crc:LE16>] ED        zero-padded to 1024 bytes
```

- `len` = whole frame length including `BE` and `ED`, ≤ 1024. Frames with a bad `BE`/`ED`/`len` are
  dropped silently.
- Request `addr` is **LE32** at bytes 4–7, request `n` is **BE16** at bytes 8–9.
- CRC-16/MODBUS (poly `0xA001` reflected, init `0xFFFF`) over bytes `0 .. len−3`; present on `BE 03`
  requests, `BE 04` replies and `BE FD` frames.

## Commands

| cmd | len | Name | Behaviour |
|---|---|---|---|
| `00` | 5 | Heartbeat | enables the `BE FD` stream while heartbeats keep coming (~1/s). No reply. |
| `01` | 11 | Read flash | **Do not use** — hangs stock 0.0.18 even in range. The driver never sends it. |
| `02` | 5 | Save | RAM shadows → NOR config banks. No reply. |
| `03` | 13+n | Write RAM | `addr`, `n`, data, CRC. Ack `BE 03 05 00 ED`. |
| `04` | 11 | Read RAM | reply `BE 04 <13+n> <addr> <n> <data> <crc> ED`; data at `[10 : len−3]`. |
| `1D` | 5 | Serial | reply `BE 1D 15 00 <16-byte UID> ED`. Allow ~60 ms quiet afterwards. |
| `C0` | 6 | Switch profile | byte 4 = 0–3. The board also pushes `BE C0 06 00 <n> ED` on an Fn switch. |
| `DA` | 6 | Diagnostic output | byte 4 ∈ {2,3} → DA flag 0/1: enables the min/max accumulators of words 332–497 (travel and actuation are **not** affected). Cleared with the stream when heartbeats stop. Driver never sends it. |
| `EE` | 5 | Reset config | factory defaults for the **active profile** only (bank 1 — calibration, types, names — untouched), saved. No reply. |
| `FC` | 5 | Enter bootloader | jump to `0x80020000` (`boot-recovery.md`). |
| `FE` | 6 | Calibration mode | byte 4 = 1 on / 0 off. While on, the engine is bypassed, **no keys are sent**, and 1003-byte frames are pushed without heartbeats (below). |
| `FF` | 5 | Reboot | watchdog reset. |

Every other command byte is ignored.

## Address validation (`BE 03` / `BE 04`)

| Bank | `addr` ≥ | `addr + n` ≤ | RAM shadow |
|---|---|---|---|
| 1, board-global | `0x80070000` | `0x80078000` | ILM `0x7084 + (addr − 0x80070000)` |
| 2, active profile | `0x80080000` | `0x80100000` | ILM `0x0658 + (addr − 0x80080000)` |

- `n ≤ 512` is accepted, but keep `n ≤ 255`: the reply buffer sits in a 0x130-byte stack frame.
- Bank-1 writes save to flash immediately (slow).
- Writes to `0x80078000` / `0x80078055` are triggers (brightness rescale), not storage.
- `soft_version` and `cur_param_num` are firmware-owned; restores must skip them.

## Live stream `BE FD`

Sent ≈ 25 Hz (every 320 scans) while heartbeats arrive.

**1017 bytes (normal)** — header `BE FD F9 03`, then 83-word arrays in key order:

| Words | Content |
|---|---|
| 0–82 | filtered raw ADC `y` (rest ≈ 30000, bottom ≈ 18000) |
| 83–165 | travel, µm: `clamp(max_travel − hC, 0, max_travel)` |
| 166–248 | dynamic rest baseline (moved by the rest tracker) |
| 249–331 | **power-up baseline** (`rest0`, captured once with the first baseline; never tracked) |
| 332–414 | DA only: running peak-to-peak of `y − baseline`, sampled every scan |
| 415–497 | DA only: running peak-to-peak of travel, sampled per frame |
| 498 | DA flag |
| 499 | scans counted in the last second (8000) |
| 500 | engine busy time in the last second, per mille (≈ 680 on stock) |
| 501–504 | uptime in seconds, u64 LE |

then CRC and `ED`. The panel uses only raw and travel. The DA accumulators start at
min 65535 / max 0 and are never reset (toggling DA does not clear them).

**1003 bytes (calibration mode, `BE FE 01`)** — header `BE FD EB 03`, then **6 consecutive
83-word blocks**, one per snapshot (snapshot 0 … 5, each in key order), CRC, `ED`. Each scan the
88 averages go through `y += (x − y) >> 4` (seeded with the first sample after boot); after the
first 8000 calibration scans since boot a snapshot is taken every 8 scans and a frame is sent
every 6 snapshots (48 scans, ≈ 167 frames/s), without heartbeats. (Older notes said "interleaved
per key"; that was wrong.)

## RAM map

83 entries × u16 unless noted.

| Name | Address | Bytes |
|---|---|---|
| `top_value` / `bot_value` | `0x80070000` / `0x800700B0` | 166 / 166 |
| `keytype_code` (switch type) | `0x80070160` | 166 |
| `soft_version` | `0x80070210` | 4 |
| `cur_param_num` (active profile) | `0x80070218` | 4 |
| `config_name1..4` | `0x80070290 + 64·i` | 64 each |
| `key_map` / `key_map_fn` | `0x80080160` / `0x80080210` | 166 / 166 |
| `press_th` / `return_th` | `0x800802C0` / `0x80080370` | 166 / 166 |
| `press_deadzone` / `return_deadzone` | `0x80080420` / `0x800804D0` | 166 / 166 |
| `norm_press_th` / `norm_return_th` | `0x80080580` / `0x80080630` | 166 / 166 |
| `key_mode` (u8) | `0x800806E0` | 83 |
| `key_cancel_code` (SOCD) | `0x80080A50` | 166 |
| `rgb` / `rgb_solid_enable` / `rgb_config` | `0x80080B00` / `0x80080C08` / `0x80080C60` | 255 / 85 / 5 |
| `rgb_enable`, `cap_enable`, `rgb_fn_enable` | `0x80080C68`, `0x80080C69`, `0x80080C6B` | 1 each |
| `sleep_second_cnt` | `0x80080C6C` | 4 |
| `win_lock_state`, `auto_calib_enable` | `0x80080C70`, `0x80080C71` | 1 each |
| `bind_key_setting` (DKS / groups) | `0x80080D00 + 48·key` | 48 each |
| `bind_key_mode` (u8) | `0x80081DE0` | 83 |

`tools/rammap.py` is the machine-readable copy.

## Keymap codes

Signed i16: modifiers −8…−1 (LCtrl…RWin), HID usages 4–119, consumer 201–224, mouse 251–255, board
functions 301–320, Fn layer 401 (402–404 are inert), 1000 = empty. Consumer code 225 has no bit in
the 24-bit consumer report and is broken on stock (it writes past the keyboard report); ours sends
nothing for it. The 5-s reset is not keycode 319 but hard-wired Fn + physical Esc (319 is a no-op).
Details: `docs/re-engine.md` §10. Key index runs row by row:
Esc = 0, LShift = 59, LCtrl = 73, Space = 76, RCtrl = 79, Right = 82 (full layout:
`panel/src/protocol/layout.ts`).
