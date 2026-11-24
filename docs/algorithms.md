# Stock firmware algorithms

How 0.0.18 turns ADC counts into key events — the behaviour our firmware should match or improve.
Everything runs in the per-step tick `FUN_ram_00007A88(step)`, `step = 0..10`, 8 kHz per full scan.
The sensor reads **high at rest (≈30000) and falls when pressed (≈18000 bottomed)**.

## ADC → travel

1. **Average** — per input, 3 of 4 conversions (`FUN_ram_00013314`) → `0x8EE28[slot]`.
2. **Filter** — per-key IIR (`FUN_ram_00013672`); the result is the "raw" value in `BE FD`.
3. **Normalise** — `f = ((baseline − raw) − deadzone) × 65536 / span`, where
   `baseline` = dynamic rest level (`0x90588`), `deadzone` = `0x906E8[key]`, and `span` (`0x8E748`)
   is derived from the **stored** `top_value`/`bot_value` (if `top − bot ≤ ~3000`, `bot = top − 12000`
   is used instead).
4. **Curve** — `FUN_ram_00014D86(switch_type, f)`: per switch type (`keytype_code`), a lookup table
   with a threshold; `f` below it returns the flat first entry. Tables are built from switch
   templates by `FUN_ram_0000E04E`.
5. **Travel** (µm) = `clamp(max_travel − curve_height, 0, max_travel)` → `0x8F0E8`, words 83–165 of
   `BE FD`. Per-scan delta (velocity) `0x9178C` feeds rapid trigger.

## Baseline and warm-up

After power-up the tick does nothing for **12000 scans (~1.5 s)**. Then, with no activity for 512
scans, it sets `baseline = mean of those 512 frames` and only then enables travel. A slow tracker
nudges the baseline during use.

## Auto-calibration (`auto_calib_enable`, per profile, default on)

On settled presses/releases it refits an actuation reference (`0x91088`) and `span`, but always
clamped to bands around the stored top/bot. It tracks drift; it cannot fix wrong stored top/bot.

## Travel stuck at 0 (raw moves, travel stays 0)

Two causes, both version-independent (0.0.9 behaves the same):

1. **Key held at power-up** — confirmed on hardware (S held at plug-in: raw 18169, travel 0). The
   baseline is captured with the key pressed, so `baseline − raw` never grows. Cure: replug with no
   keys pressed. This also explains the 2026-11-24 LCtrl/LShift incident.
2. **Stored swing much larger than the real one** — `span` sized from `top/bot` (factory 31000/17000)
   while the switch swings less, so `f` never crosses the curve threshold. Cure: recalibrate
   (measured top/bot).

Our firmware should not take the baseline from a held key (e.g. reject a power-up baseline far below
the stored top, or keep tracking rest until the key is first released).

## Actuation

`FUN_ram_00009D10` — per-key state machine; aggregates pressed keycodes into the HID reports.

- `key_mode = 0` normal: press at `travel ≥ norm_press_th`, release at `travel ≤ norm_return_th`.
- `key_mode = 1` rapid trigger: press on a downward move of `press_th` once past `press_deadzone`
  (top), release on an upward move of `return_th`, `return_deadzone` at the bottom. Continuous.

All thresholds in µm.

## SOCD

`key_cancel_code[key]` (i16, `1000` = off): while this key is held, the named keycode is masked from
the report — last press wins. A/D needs two entries (A→D, D→A). Layer-agnostic.

## Key groups and DKS (0.0.18)

`bind_key_mode[key]`: 0 none, 1 key group, 2 DKS. `bind_key_setting` = 48 bytes per key (4 action
slots × 4 phases, member keycodes, DKS top/bottom µm, timings). A record whose byte 47 is `0xFF` is
unused (`FUN_ram_00014804`). The DKS hold timer fires at 4000 ticks (0.5 s). The exact effect of byte
47's "trigger the original key" flag in the actuation path is not traced. Byte-level layout and vendor
semantics: `panel/PROTOCOL-NOTES.md`.

## Profiles and saving

- `cur_param_num` (bank 1) = active profile 0–3. `BE C0` switches at end of scan
  (`FUN_ram_00015ABE`); only bank 2 swaps, bank 1 (names, top/bot) is global.
- Any config change re-runs `FUN_ram_0000A6F8` (span, deadzones, curves, key structs).
- `BE 02` writes the RAM shadows to the NOR config banks (`FUN_ram_00015F5A`), interrupts masked.
  Bank-1 writes save themselves.

## 0.0.9 vs 0.0.18

Same command set, loader split and config banks. 0.0.18 adds key groups/DKS (`bind_key_*`),
mouse-button keycodes (+ interface 3), a switch-curve retune and auto-cal tuning. Stay on 0.0.18.
