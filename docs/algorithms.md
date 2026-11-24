# Algorithms

How the firmware turns Hall-sensor ADC counts into key events, and why a key's travel can stick at
~0 after power-up. Everything runs inside the 8 kHz per-step tick `FUN_ram_00007a88(step)`
(`step = 0..10`), driven by the scan dispatcher `FUN_ram_0000057a`. Addresses are 0.0.18.

Convention: the sensor reads **high at rest (≈30000)** and **drops when pressed (≈18000 bottomed)**.
"Press depth" therefore grows as the ADC falls.

## 1. ADC → travel pipeline

Per step, for its 8 keys:

1. **Average** the raw conversions — `FUN_ram_00013314`. Each key's 4-conversion ADC group drops
   the first sample and averages the other three → `DAT_ram_0008ee28[slot]` (one per the 88 slots).
2. **Filter** — on the first scan of a step the averaged value is copied into a working buffer
   (`0x8d828`); thereafter each value is run through `FUN_ram_00013672` (a per-key IIR using state
   at `0x8dda8`/`0x90428`). The filtered result overwrites `0x8ee28` and is what the stream reports
   as "raw".
3. **Displacement** — `DAT_ram_00090b08[key] = raw − baseline`, where `baseline = DAT_ram_00090588`
   is the per-key dynamic rest level (§2).
4. **Normalise + curve** — the normalised press fraction
   `((baseline − raw) − deadzone) × 65536 / span` is passed to `FUN_ram_00014d86(switch_type, f)`,
   a per-switch-type lookup/interpolation table:
   - `span = DAT_ram_0008e748[key]` — per-key scale, derived from the **stored** `top_value`/
     `bot_value` (§2);
   - `deadzone = DAT_ram_000906e8[key]` — the starting dead zone in counts;
   - `switch_type` index `DAT_ram_000918ec[key]` comes from `keytype_code`.

   `FUN_ram_00014d86` clamps `f` to `[threshold, 0xA4C]`: below `DAT_ram_00091e74[type]` it returns
   the table's first entry, above `0xA4C` the last, else it interpolates
   `table[f − threshold]`. The per-type tables (`DAT_ram_00091e94[]`, thresholds `0x91e74[]`) are
   built from switch templates by `FUN_ram_0000e04e`, called from the config-apply path
   `FUN_ram_0000a6f8`.
5. The curve output (a "height", max at rest) is stored at `0x8e5e8`/`0x8e328`/`0x8e488` (three
   related forms) and the **reported travel** is `DAT_ram_0008f0e8[key] = clamp(max_travel − height,
   0, max_travel)` — 0 at rest, growing with press, saturating at the switch's physical travel
   (`max_travel` from the per-type block at `0x800804f8`, stride 0x2e). This is `BE FD` words
   `[83..165]`. The per-scan travel delta (velocity) `DAT_ram_0009178c[key]` and the previous travel
   `0x2490[key]` feed rapid trigger (§4).

## 2. Calibration — stored top/bot vs the dynamic baseline

There are **two** reference levels, and conflating them is the source of the stuck-at-0 bug.

- **Stored `top_value`/`bot_value`** (`0x80070000`/`0x800700B0`, shadows `DAT_ram_00007084` /
  `DAT_ram_00007134`). These are written by the vendor's manual calibration (`BE FE` stream → host
  computes top/bot → `BE 03` + `BE 02`), and default to 31000/17000. They are turned into the
  per-key `span` (`0x8e748`) and curve domain in `FUN_ram_0000a6f8`: the first loop validates each
  pair (if `top − bot ≤ ~3000` it falls back to `bot = top − 12000`) into targets
  `0x9199c`/`0x91a4c`, and the second loop computes
  `span = FUN_ram_80042782( f(top_target − bot_target, deadzone×2) )` with the float helpers
  (`FUN_ram_80042830/800428e4/8004238e/800423f2/80045636/800457f6`) that model the Hall-field
  non-linearity. So **span is sized from the stored top/bot swing.**
- **Dynamic baseline** (`DAT_ram_00090588`). Captured live: while no key is active the tick sums
  raw into an accumulator (`0x8dc48`); after 512 still scans with zero activity
  (`DAT_ram_0009205c == 0x200` and the activity sum `iVar44 == 0`) it sets
  `baseline = accumulator / 512`, marks itself captured (`DAT_ram_00091fdc = 1`), and gates
  processing on via `DAT_ram_000920bc`. A slow rest-tracker (the `0x90588` update under
  `DAT_ram_0009205c == 0x200`) nudges the baseline during use.

So `travel = curve( (live_rest − raw − deadzone) × 65536 / span_from_stored_top_bot )`.

### Boot warm-up

Processing does not start immediately. `DAT_ram_000920ac` counts completed scans, capped at 12000,
and the tick returns early until it reaches **12000** (≈1.5 s at 8 kHz). Then the 512-still-frame
baseline capture must also complete before `DAT_ram_000920bc` turns travel on. Keys are therefore
"blind" for the first ~1.5–2 s after power-up, and whatever the sensors were doing during that
window shapes the first baseline.

## 3. Auto-calibration (`auto_calib_enable`, "for stability")

`auto_calib_enable` (`0x80080C71`, shadow `DAT_ram_000012c9`) gates the per-key refit block
`LAB_ram_00008634` in the tick (runs when it is `1`, the factory default on all four profiles). For
each key it watches the actuation level (`0x8e488`) and velocity (`0x9178c`); on a settled
press/release it nudges an adaptive actuation reference `DAT_ram_00091088[key]` and recomputes
`span` (`0x8e748`) with the same float helpers — **but always bounded by the stored top/bot**
(`(&DAT_ram_00007084)[key]` and `[key+0x58]`): the correction is clamped to ±`|top−bot|` bands
(`0x87`, `0x17c` limits around the stored values). It also drifts the rest baseline between presses.

Consequence: auto-calibration keeps the thresholds tracking small drift, exactly as the vendor
claims, but it **cannot escape wrong stored top/bot** — those remain the bounds. It neither causes
nor cures the stuck-at-0 fault below; disabling it would not help, and leaving it on does not
self-heal the fault.

## 4. Actuation and rapid trigger

The actuation state machine is `FUN_ram_00009d10(flag, step, 8)`, called per step from the tick (and
once over all 11 steps on step 10). It walks per-key runtime structs (base `0x8df08`, built by
`FUN_ram_0000a6f8`) whose fields include the HID state (`+0x16`), the actuated flag (`+0x1b`), a
64-bit hold counter (`+0x20/+0x24`, a DKS timer that fires at 4000 ticks = 0.5 s), a pressed-bitmap
byte/mask (`+0x2a/+0x2b` into `0x91f54`/`0x91f64`), and DKS/group flags (`+0x2c/+0x2d`). It
aggregates the pressed keycodes into the HID report buffers (`0x8b4c8`/`0x8af48`/`0x8bfc8`/`0x8ba48`)
sent on the boot-keyboard / consumer / mouse endpoints.

The thresholds come from the config (semantics from `panel/PROTOCOL-NOTES.md`, all µm):

- `key_mode[i] = 0` (**normal**): a press fires when `travel ≥ norm_press_th`, releases at
  `travel ≤ norm_return_th`.
- `key_mode[i] = 1` (**rapid trigger**): uses the travel **delta** (`0x9178c`) — press on a downward
  move of `press_th` past the `press_deadzone` (top), release on an upward move of `return_th`
  before the `return_deadzone` (bottom). This is continuous re-trigger.

## 5. SOCD (`key_cancel_code`)

`key_cancel_code` (`0x80080A50`, i16 per physical key, `1000` = off) names a keycode that is
suppressed while this key is held. In `FUN_ram_00009d10` the suppression is done against the live
pressed-key bitmap (`0x91f54`/`0x91f64`): when a key with a non-1000 cancel code is actuated, the
matching bit of the named target is masked out of the report — "last press wins". It is
layer-agnostic; function codes are not valid targets (the vendor UI enforces that, hint 56). A
classic A/D pair needs two entries (A→D and D→A).

## 6. Key groups and DKS (`bind_key_*`, firmware ≥ 0.0.18)

`bind_key_mode[i]` (`0x80081DE0`, shadow `DAT_ram_00002438`): `0` none, `1` key group, `2` DKS.
`bind_key_setting` (`0x80080D00`, shadow `DAT_ram_00001358`, 48 B/key) holds the per-key record.
`FUN_ram_00014804(&0x1358, &0x2438, 0x58)` detects a change and triggers a rebuild; `FUN_ram_0000a6f8`
(≈ its lines parsing `0x2438`/`0x1358`, building the lists at `0x90008` (DKS/group) and `0x90168`
(callbacks)) turns each record into runtime structs. The 48-byte layout (4 action slots × 4 phases,
4 member keycodes, DKS top/bottom µm, tap/delay/random timings ×8) is documented from the vendor
side in `panel/PROTOCOL-NOTES.md`.

**Byte 47 ("DKSOriginCover" / "trigger the original key"):** this is parsed into the runtime struct
in `FUN_ram_0000a6f8` but its exact effect (whether it suppresses or still emits the physical key's
own code) was **not fully traced** through `FUN_ram_00009d10`; the vendor side itself is ambiguous
(variable "cover" vs label "trigger"). Left as an open item in `open-questions.md`.

## 7. Profiles, save, flash layout

- **Profiles.** `cur_param_num` (`0x80070218`) is the active profile 0..3. `BE C0` sets
  `DAT_ram_00091fd0`/`DAT_ram_00091fd4 = 1`; the switch is applied at end-of-scan by
  `FUN_ram_00015abe` (re-arms the PWM/ADC for the new bank). Only bank 2 (`0x8008xxxx`) swaps;
  bank 1 (`0x8007xxxx`, incl. the 4 profile names and top/bot) is global.
- **Apply.** Any config change (`BE 03` success, `BE EE` reset, profile switch) re-runs
  `FUN_ram_0000a6f8` → recompute `span`, deadzones, curves (`FUN_ram_0000e04e`) and rebuild the
  runtime key structs. This also runs at boot, before the 8 kHz processing produces travel.
- **Save.** `BE 02` → `FUN_ram_00015f5a` commits the RAM shadows to the XIP NOR config banks; bank-1
  writes additionally self-save. Interrupts are masked around the write (PLIC bit 19 at
  `0xe4002004`). The config banks live in NOR at `0x80070000`/`0x80080000` (outside the app image
  `0x80040000..0x80052900`), which is why a firmware reflash leaves them byte-identical.

## 8. Root cause — travel stuck at ~0 after power-up

**Mechanism (confirmed in code).** Reported travel is
`curve( (dynamic_baseline − raw − deadzone) × 65536 / span )`, with `span` derived in
`FUN_ram_0000a6f8` from the **stored `top_value`/`bot_value`** and the curve clamping anything below
`DAT_ram_00091e74[type]` to its flat first entry. If a key's **real** ADC swing is much smaller than
the swing implied by its stored top/bot, the normalised fraction never climbs past the curve
threshold across the whole stroke, so `FUN_ram_00014d86` returns the same clamped value for rest and
for full press → `0x8f0e8` (travel) stays at 0 **even though the raw ADC (`0x8ee28`) visibly moves**.

**Why it hit LCtrl/LShift (indices 73/59).** They carried the factory default `top=31000`,
`bot=17000` (span sized for a 14000-count swing), while those switches actually produced a much
smaller swing (hence the fix: recalibration replaced 31000/17000 with *measured* values, i.e.
top/bot that bracket the real ≈30000 rest and the real bottom). A measured, tight top/bot shrinks
`span` so the same physical motion reaches full fraction → travel works. This is precisely the
observed cure.

**Why a replug (sometimes) recovered it without recalibration.** A boundary-case key sits right at
the curve threshold. Each boot re-captures the dynamic baseline over 512 still frames and re-runs
the auto-cal refit; small boot-to-boot differences in the captured baseline (a few counts) push the
full-press fraction just above or just below the threshold, flipping the key between "works" and
"stuck". That is why LShift came back after one replug and LCtrl needed a second — the dice were
re-rolled each time. Disturbing a key during the ~1.5–2 s warm-up (bad baseline) makes this worse.

**Is `auto_calib_enable` implicated?** Indirectly. With it on (the default) the refit tracks drift
but is clamped to the stored top/bot bounds (§3), so it neither triggers nor fixes the fault — it
just perpetuates reliance on the stored values. The reliable fix is correct per-key `top_value`/
`bot_value`, which is why the panel should always offer recalibration and should flag keys whose
stored swing (`top−bot`) is implausibly large relative to the live `BE FD` raw range. The vendor's
calibration flow (host-computed top/bot from the 1003 stream) is the intended remedy.
