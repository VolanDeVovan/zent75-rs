# Stock firmware algorithms

How 0.0.18 turns ADC counts into key events — the behaviour our firmware should match or improve.
Everything runs in the per-step tick `FUN_ram_00007A88(step)`, `step = 0..10`, 8 kHz per full scan.
The sensor reads **high at rest (≈30000) and falls when pressed (≈18000 bottomed)**.

Exact reference with every constant: `docs/re-engine.md`. Our port: `firmware/core/src/engine/`
and `firmware/core/src/keys/`, diff-tested against the emulated stock engine
(`firmware/core/tests/engine_diff.rs`); deliberate fixes are listed in
`firmware/core/src/engine/compat.rs`.

## ADC → travel

1. **Average** — per input, 3 of 4 conversions (`FUN_ram_00013314`) → `0x8EE28[slot]`.
2. **Crosstalk** — once the engine is enabled, each slot subtracts a fixed fraction of the
   current `x − baseline` of slots `s−2`, `s+6`, `s−8` (`FUN_ram_00012302`).
3. **Filter** — nonlinear IIR (`FUN_ram_00013672`); jumps > 323 counts pass straight through,
   small changes move ~1/8 per scan, a constant input settles 1 count low. Its error integrator
   is **one global shared by all slots** on stock (fixed per slot in ours). The result is the
   "raw" value in `BE FD`.
4. **Normalise** — three positions per slot with the per-slot dead zone `dz` (~30 counts) as
   hysteresis: `fA = (Δ − dz)·65536/span` (press side), `fB = (Δ + dz)·65536/span` (release side),
   `fC = Δ·65536/span` (display), `Δ = baseline − raw`. `span` (single-precision float) comes from
   the **stored** `top_value`/`bot_value` (if `|top − bot| ≤ 2999`, `bot = top − 12000`).
5. **Curve** — per switch type a table `f → height` (µm, max at rest, 0 bottomed), built from the
   templates by `FUN_ram_0000E04E`. Types 0–3 built in (full travel 3420 / 3375 / 3420 / 3780 µm),
   4–7 the custom records of bank 1.
6. **Travel** (µm) = `clamp(max_travel − hC, 0, max_travel)`, words 83–165 of `BE FD`. Velocity
   `hC(prev) − hC` per scan feeds only auto-cal.

Actuation compares heights `hA`/`hB` against thresholds converted to heights.

## Baseline and warm-up

After power-up nothing runs for **12000 scans (1.5 s)**. Then 512-scan windows are summed; the first
window with no activity sets `baseline = power-up baseline (rest0) = window mean`, unless any key's
current raw is more than 40 counts off its mean (retry next window). **There is no check that keys
are released.** Afterwards the rest tracker accepts a window only if it is quiet (mean within ±8 of
the previous window, within ±450 of `rest0`, within −16…+40 of the baseline, no neighbour activity)
and moves the baseline once per 16 accepted windows (≥ 1 s) to their exponentially weighted mean.
`rest0` (words 249–331) is never updated.

## Auto-calibration (`auto_calib_enable`, per profile, default on)

A hold within the last 400 µm, entered faster than 15 µm/scan and held ≥ 0.625 s, measures the
bottom level. On stock that measurement only **gates** the update (±600 counts of the stored swing):
the clamp's bounds are inverted, so every accepted hold moves the reference halfway towards
`baseline − swing + 380`, i.e. the span shrinks by ~380 counts regardless of what was measured. With
the factory 31000/17000 and a real swing ≈ 12000 every hold is rejected, so auto-cal does nothing on
the owner's unit. It never touches `top/bot`, the baseline or `dz`. Ours clamps the measured bottom
into the intended band `[−swing−135, −swing+380]` of the stored calibration (so it really tracks)
and re-derives the reference when the calibration is rewritten.

## Travel stuck at 0 (raw moves, travel stays 0)

Two causes, both version-independent (0.0.9 behaves the same):

1. **Key held at power-up** — confirmed on hardware (S held at plug-in: raw 18169, travel 0) and in
   the emulator (`held_at_boot` trace). The baseline is captured with the key pressed; after release
   the rest tracker rejects the real rest level (`> 450` from `rest0`, `> 40` above the baseline), so
   the key stays at travel 0 until reboot. Cure on stock: replug with no keys pressed. This also
   explains the 2026-11-24 LCtrl/LShift incident. **Ours:** a quiet window whose mean is more than
   250 counts above the baseline (towards rest) re-captures baseline and `rest0` of that slot, so the
   key works again ~0.2 s after it is first released.
2. **Stored swing much larger than the real one** — `span` sized from `top/bot` (factory 31000/17000)
   while the switch swings less, so `f` never crosses the curve threshold. Cure: recalibrate
   (measured top/bot).

## Actuation (`FUN_ram_00009D10`, every step)

The 88 key objects are evaluated step by step. On stock object k is filed at step
`slot2key[k] >> 3` instead of its own sample's step, so some keys act on the previous scan's
heights (≤ 1 scan extra latency; fixed in ours).

- `key_mode = 0` normal: press at press-side travel ≥ `norm_press_th`, release at release-side travel
  ≤ `norm_return_th` (the `dz` hysteresis adds ≈ 25–60 µm).
- `key_mode = 1` rapid trigger: press on a downward move of `press_th` once below the top dead zone
  (`press_deadzone`), release on an upward move of `return_th`; inside the bottom zone
  (`return_deadzone + 10` µm) a released key re-presses at once. Debounce guards: after returning
  to the top (`max − 300` µm) presses are blocked for 80 scans; after reaching the bottom (< 101 µm)
  releases are blocked for 128 scans while below 100 µm. Space/Backspace/Enter/LShift use longer
  guards with an absolute 650 µm top threshold when `return_th < 300`.
- `key_mode = 2`: normal press point, RT only below it; an RT release leaves the key in state 2
  (not sent) until it re-presses by RT or fully releases.

All thresholds in µm.

## Layers and tap-hold

The Fn layer is active while any pressed key has main-layer code 401; it is not latched — releasing
Fn switches held keys back to their main codes. A Fn-layer code 1000 is a dead key, not a
fallthrough. FN2–FN4 do nothing. Undocumented **tap-hold**: bank-2 `+0x000`/`+0x0B0` hold codes
(per layer, 1000 = off): held > 4000 scans (0.5 s) sends the hold code, released earlier sends the
normal code for one rebuild. Bank-2 `+0x790`/`+0x7E8` are per-layer modifier masks ORed into byte 0 while the key is pressed
(also while a tap-hold is pending). A tap is in the report for one scan only (125 µs), and the
tap code is sent on release, not on press. The panel edits both in the Keymap tab.
(The 0.5 s timer belongs to this feature, not to DKS.)

## SOCD

`key_cancel_code[key]` (1000 = off) names a target code (report byte `(c>>3)+1`, or byte 0 for
modifiers). The cancel **arms only on this key's press edge if the target is already in the report**
and disarms when this key is released or the target disappears. So a single entry A→D cancels D
only if D was down first; two entries (A→D, D→A) give last-press-wins and the first key returns when
the second is released. Layer-agnostic.

## Key groups and DKS (0.0.18)

`bind_key_mode[key]`: 0 none, 1 key group, 2 DKS; record = 48 bytes at `bind_key_setting`.
Key group: while the key is pressed, all four member codes (≠ 1000) are sent. DKS: phases fired in
the panel's column order (0 = pressed past the top point, 1 = past the bottom point, 2 = back above
the bottom point, 3 = back above the top point); action byte `r[4·slot + phase]`: 1 = hold, 2 = tap
(released `delay + rnd(random)` scans, then pressed `single` scans; UI unit ms = scans/8), 0 = end a
hold (it lingers 9 scans). Byte 47 = 1 (default) suppresses the original key; 0 sends it too.

## Function keys

301 bootloader, 302 reboot, 303/304 RT off/on (RAM only), 305 profile defaults, 306 `~ESC`,
307–310 profile 1–4 (0.9 s cool-down during which **no report is rebuilt** — the last report
freezes until the next change after it), 311–318 lighting, 316 WinLock (blocks LWin/RWin only),
320 WASD↔arrows (RAM only), 319 nothing. Consumer 201–224, mouse 251–255. Keyboard report: 16-byte
NKRO (modifiers + bitmap of usages 0–119), sent on change only; more than 20 keys pressed → empty
reports until ≤ 4 are held. **Fn + Esc held 5 s** (hard-wired) erases both config banks
**including the calibration** on stock; ours keeps `top/bot` and the switch types.

## Profiles and saving

- `cur_param_num` (bank 1) = active profile 0–3. `BE C0` switches at end of scan
  (`FUN_ram_00015ABE`); only bank 2 swaps, bank 1 (names, top/bot) is global.
- Any config change re-runs `FUN_ram_0000A6F8` (span, deadzones, curves, key structs).
- `BE 02` writes the RAM shadows to the NOR config banks (`FUN_ram_00015F5A`), interrupts masked.
  Bank-1 writes save themselves.

## 0.0.9 vs 0.0.18

Same command set, loader split and config banks. 0.0.18 adds key groups/DKS (`bind_key_*`),
mouse-button keycodes (+ interface 3), a switch-curve retune and auto-cal tuning. Stay on 0.0.18.
