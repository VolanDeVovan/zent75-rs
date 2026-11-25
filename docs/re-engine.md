# Stock key engine (0.0.18): ADC samples → HID reports

This is the reference for reimplementing the stock key processing bit-for-bit. It comes from
static analysis of the unpacked 0.0.18 app (Ghidra and capstone), checked by emulating the real
code with unicorn.

**Conventions:**
- Addresses are ILM/DLM addresses of the unpack.
- `gp = 0x91C2C`.
- *Slot* = scan slot `step·8 + position` (0..87, see `hardware.md`). *Key* = key index (Esc = 0 … Right = 82).
- Per-slot arrays are `i32[88]` unless noted.
- Divisions are C/RISC-V truncating divisions unless written `>>`.
- A *scan* is one full 11-step pass, at 8000 scans/s (125 µs each).

**Evidence harness:** `re-work/engine/` (git-ignored; vendor-derived).
- `harness.py` runs the stock engine on a synthetic ADC trace. Usage is at the top of the script.
- `model.py` is an independent Python model of §2–§5. It matches the emulator over 12 000 scans with noise and drift, with 0 mismatches.
- `curves.py` rebuilds the switch tables and matches the firmware's tables exactly.

Corrections to the older docs:
- **SOCD** is not "last press wins" for a single entry (§9).
- **Auto-cal** ignores what it measures (§6).
- **BE FD words 249–331** are the power-up baseline, not a tracked value (§12).
- The **1003-byte frame** is snapshot-major (§12).
- **DA** does not change travel (§13).
- The **0.5 s timer** belongs to an undocumented tap-hold feature, not DKS (§8).
- **Fn+Esc held for 5 s** erases *both* config banks, including calibration (§10).

## 1. Per-step tick `FUN_7A88(step)`

The ADC ISRs (`FUN_2A0` ADC0, `FUN_3A6` ADC1) handle one mux step at a time:
- Each copies its 64-byte DMA block to `0x8D408 + 0x40·step` (ADC0) or `0x8D148 + 0x40·step` (ADC1).
- When both ADCs are done, it sets `ready[step]` at `0x91D98`.
- After step 10 it sets the *scan flag* `0x86D7E`.

The GPTMR1 handler `FUN_57A` then calls `FUN_7A88(step)` for every ready step, in order.

**Order inside one call:**

1. Compute `gate = (0x91F7C ≠ 0) && usb_configured(0x92588)`. This later enables report sends.
2. If a vendor OUT frame is pending (`0x92024`), run the dispatcher `FUN_B812`.
3. `FUN_133C6`: send a pending vendor IN frame (BE FD), 1024 bytes on EP 0x82.
4. `FUN_1170E`: deferred profile switch.
5. If calibration mode is on (`0x91F80`, set by `BE FE 01`), run `FUN_E860` (§12) and return. **The keyboard sends no keys in calibration mode.**
6. If the scan flag is set (true only on the step-10 call), clear it and do the per-scan housekeeping:

   | Counter | Rule |
   |---|---|
   | profile cooldown `0x9214C` | −1, not below 0 |
   | heartbeat timeout `0x92078` | ++. At 16000 (2 s): stream off, DA off |
   | warm-up `0x920AC` | ++, capped at 12000 |
   | stream divider `0x92018` | ++. `stream_due = (0x92018 % 320 == 0)` |
   | scans-per-second `0x920D4` | ++ |
7. While warm-up < 12000 (the first **1.5 s** after boot), return.
8. Sample processing for the 8 slots of this step (§2):
   - Average the conversions (`FUN_13314`).
   - If the engine is *enabled* (`0x920BC`, §3): crosstalk correction (`FUN_12302`).
   - IIR filter (`FUN_13672`).
9. If the engine is enabled:
   - Compute the three curve heights for the 8 slots (§4).
   - Run actuation and report building `FUN_9D10(gate && !suspended(0x92070), step, 8)`.
   - The very first time, nothing runs per step; instead `FUN_9D10` runs once for each of the 11 steps at step 10, and `0x9206C` is set.
10. If `step ≠ 10`, return. Step 10 continues with:
    - baseline, activity and rest tracker (§3);
    - auto-calibration (§6);
    - DA min/max (§13);
    - the 5-s Fn+Esc reset counter (§10);
    - USB suspend detection and remote wake-up;
    - 1-s housekeeping (uptime, load counter);
    - the BE FD frame build, if `stream_due && heartbeat && configured` (§12).

## 2. Sample path

**Averaging (`FUN_13314`).** Each input has 4 conversions (u32 each). Only the low 16 bits are used.

`x = (c1 + c2 + c3) / 3` (conversion c0 is discarded). The result goes to `0x8EE28[slot]`.

**Crosstalk correction (`FUN_12302`).** This runs only once the engine is enabled. For each slot s of the step, in slot order:

```
comp     = (d[(s−2) mod 88]·C1[s] + d[(s+6) mod 88]·C2[s] + d[(s−8) mod 88]·C3[s]) >> 20   (i32 wrap)
x        = x − comp
d[s]     = x − baseline[s]          # d = 0x8EA08, stored immediately (later slots see the new value)
```

The `d` values used are the latest ones: from this scan if that slot is already processed, otherwise from the previous scan. The coefficients are built by `FUN_D606` as `trunc(k · 2^20)`.

Coefficients by `pos = s & 7`:

| Coefficient | pos 0 | pos 1 | pos 2 | pos 3 | pos 4 | pos 5 | pos 6 | pos 7 |
|---|---|---|---|---|---|---|---|---|
| C1 | 1048 | 1048 | 2097 | 2097 | 2097 | 2097 | 2097 | 2097 |
| C2 | 1048 | 1048 | 0 | 0 | 0 | 0 | 0 | 0 |
| C3 | 7392 | 8304 | 5138 | 2936 | −209 | −1363 | −4508 | −5767 |

Exceptions (slot: term = value):
- Set to 0:
  - slot 1: C1 and C3
  - slots 4, 5, 7, 52: C3
  - slot 81: C2
  - slots 83, 86, 87: C1
- Changed values:
  - slot 17: C3 = 10388
  - slot 43: C3 = 4445
  - slot 72: C3 = 10255

**IIR filter (`FUN_13672(x, y_prev, &st[slot])`).**

State:
- `y` = `0x8D828[slot]`. After the step, `y` is copied back to `0x8EE28`; this is the "raw" value seen everywhere downstream.
- `st` = `0x8DDA8[slot]`.
- `acc` = `0x92104`. This is **one global variable shared by all 88 slots**, updated in processing order (a vendor bug, but part of the behaviour).

The first time each step is processed after boot (`0x91D6C[step] == 0`), the filter is skipped and `y = x`.

```
d = x − y_prev
if |d| > 323:   st = d << 15;                     y = y_prev + d          (acc untouched)
else:           g   = T(d) << 15
                acc = clamp(((g − st)·92 + acc) >> 7, −0x70000, +0x70000)   (i32 wrap, arithmetic >>)
                st  = g − acc;                    y = y_prev + (st >> 15)
T(d) = sign(d)·( |d|/8+1 if |d|<64,  |d|/4 if |d|<128,  |d|/2 otherwise ),  with T(0) = −1
```

`T` is the table at `0x89708` (1024 i32, index `d + 512`).

Properties:
- Small changes move at about 1/8 per step and large ones at about 1/2.
- Jumps of more than 323 counts pass straight through.
- Because `T(0) = −1`, a constant input settles **1 count low** (30000 → 29999).

## 3. Warm-up, baseline capture, activity, rest tracker

Arrays per slot:

| Array | Address | Meaning |
|---|---|---|
| `baseline` | `0x90588` | dynamic rest level (BE FD words 166–248) |
| `rest0` | `0x8EF88` | baseline captured at power-up, never updated afterwards (words 249–331) |
| `sum` | `0x8DC48` | 512-scan accumulator |
| `pmean` | `0x9183C` | u16, previous window mean |
| `act` | `0x909A8` | activity counter |
| `wsum` / `wn` | `0x902C8` / `0x91AFC` | tracker accumulator / u16 count |

**Window.** Every scan (at step 10) `sum[s] += y[s]` and the window counter `0x9205C` is incremented. Every 512 scans:
- the window closes and the counter resets;
- activity decays: `act ≥ 4 → 2`, `1..3 → act − 1`;
- the steps below run.

**Capture** (engine not yet enabled). This needs the total activity to be 0, which is always true before capture because activity is only counted once enabled. When the first window after warm-up closes:
1. `baseline = rest0 = pmean = sum/512` (truncating).
2. `wsum = wn = 0`, velocity 0.
3. `hist[slot][0..63] = max_travel` of the slot's key.
4. **Rejection test:** if any key slot (key < 83) has `|baseline − y_now| > 40`, the capture is discarded and retried at the next window.
5. Otherwise the engine is **enabled** (`0x920BC = 1`).

There is no check that keys are *released*. Any key held steadily is captured at its pressed level, and its travel stays 0 until reboot (`algorithms.md`, "travel stuck at 0").

**Activity** (every scan once enabled):
- For every slot whose press-side height `hA < max_travel − 100` (more than about 100 µm pressed), each of the 8 *neighbour slots* of its key gets `act += 1`.
- The neighbour table is at `0x800405D8`, 8 slot indices per key; the key itself is not in its own list.

**Rest tracker** (at each window close, once enabled). For each slot, with `m = sum >> 9`, the window counts as *quiet* if all four hold:
- `|m − pmean| ≤ 8`
- `|m − rest0| ≤ 450`
- `−16 ≤ m − baseline ≤ 40`
- `act == 0`

The upper/lower limits 40/−16 become 16/−40 if `0x90DC8[0] == 1`, which is a vendor quirk that never happens.

For each quiet window: `wsum += m << wn; wn += 1` (u32). When `wn` reaches 16: `baseline = wsum / 65535`, then `wsum = wn = 0`.

So the baseline moves at most once per 16 quiet windows (≥ 8192 scans ≈ 1.02 s). It is updated to an exponentially weighted mean, with weight 2^i, of those windows. Quiet windows do not need to be consecutive. Then `pmean = m` for every slot.

## 4. Normalisation, span, curve, travel

**Constants per slot, set in `FUN_A6F8` (re-run after every config change) and `FUN_10E28` (init):**

`dz[slot]` (`0x906E8`):
- A fixed per-slot table at XIP `0x80040898`, the same for every unit.
- Negated if the sensor rests above 0x8000.
- Values per key (Esc …):
  `28 29 28 30 29 33 30 33 33 32 29 29 30 32 32 30 31 26 31 31 27 32 30 34 34 31 27 29 29 29 33 30 26 30 30 29 29 28 27 27 28 36 34 35 31 31 28 29 28 23 29 29 27 27 27 28 33 33 32 30 31 26 30 30 29 32 32 28 29 32 31 30 30 32 28 36 28 27 30 29 29 30 34` (ADC counts).

`bot_eff`:
- Equals `bot_value[key]`.
- If `|top − bot| ≤ 2999`, it is `top − 12000` instead (u16 wrap).

`span` (`0x8E748`), computed in single-precision soft-float:

```
span = (int)( −(((float)bot_eff − (float)top) + (float)(2·dz)) · 65536.0f / 2636.0f )
```

That is, `span ≈ (top − bot_eff − 2·dz) · 65536 / 2636`, truncated toward zero. With factory calibration 31000/17000 and dz = 28, span = 346674.

Auto-cal (§6) recomputes span as `(baseline − act_ref − 2·dz)·65536/2636`, using `act_ref = 0x91088` (initialised to `bot_eff`) in place of `bot_eff`.

**Per step, for each slot:**

```
Δ    = baseline − y                         (positive when pressed)
fA   = ((Δ − dz) · 65536) / span            → hA = curve(type, fA)   0x8E328   press side
fB   = ((Δ + dz) · 65536) / span            → hB = curve(type, fB)   0x8E488   release side
fC   = (Δ · 65536) / span                   → hC = curve(type, fC)   0x8E5E8   plain
vel  = hist0 − hC;  hist0 = hC              (i16, 0x9178C; hist0 = 0x2490 + 0x80·slot)
```

`curve()` returns a **height** in µm (`max_travel` at rest, 0 at bottom-out).

- A full swing maps to f ≈ 0..2636.
- `dz` acts as a hysteresis in ADC counts: press decisions use the shallower `hA`, release decisions the deeper `hB`.
- Travel shown in BE FD = `clamp(max_travel − hC, 0, max_travel)`, computed only when a frame is built (`0x8F0E8`).

## 5. Switch curves (`FUN_14D86`, builder `FUN_E04E` + `FUN_13510`)

**Switch types and max travel.**
- The type comes from `keytype_code` via `FUN_16146`: 0..3 as-is; 4092..4095 → custom types 7..4; anything else → 0.
- Max travel per type (`0x92130`): **0 = 3420, 1 = 3375, 2 = 3420, 3 = 3780 µm**.
- Custom types 4..7 live in bank 1 at `0x80070390 + 0x80·(t−4)`:
  - u32 max travel;
  - u32 point count;
  - i16 template at +8.

  Init fills them with type 0's template (3420, 71 points) and names them "Custom1..4".

**Templates.** Each template lists f values at travel `−50, 0, 50, 100, …` µm in 50 µm steps; the **last point is the max travel** instead of the next multiple of 50. Point count n = 71 / 70 / 71 / 78. Addresses:

| Type | Template address |
|---|---|
| 0 | `0x80041DFC` |
| 1 | `0x80040B28` |
| 2 | `0x80041C9C` |
| 3 | `0x80040A8C` |

```
type 0: -12 0 12 25 39 50 65 78 91 108 121 136 151 166 184 200 217 235 254 271 289 308 329 350 371 394 416 441 463 492 516 539 565 592 621 649 681 713 746 777 818 851 886 922 963 1003 1044 1088 1134 1181 1227 1280 1325 1387 1440 1499 1554 1620 1682 1751 1820 1896 1966 2046 2124 2221 2304 2394 2488 2594 2636
type 1: -11 0 11 22 35 47 59 71 87 101 114 128 144 159 175 191 207 225 242 263 281 299 319 339 361 381 404 429 452 477 502 530 556 584 612 642 675 708 740 773 806 850 884 921 962 1000 1047 1090 1141 1189 1237 1288 1344 1397 1457 1521 1583 1647 1721 1791 1875 1940 2021 2107 2192 2289 2386 2478 2584 2636
type 2: -7 0 7 17 28 40 52 64 78 91 105 119 135 152 169 185 202 219 236 255 272 291 310 329 349 370 392 414 437 461 486 513 540 570 600 629 660 691 724 756 792 826 861 897 934 973 1016 1059 1104 1152 1202 1254 1309 1366 1425 1485 1547 1613 1679 1747 1817 1892 1965 2044 2123 2208 2298 2392 2493 2597 2636
type 3: -10 0 10 19 30 39 49 60 70 82 93 104 116 130 143 156 170 184 198 212 226 240 256 272 287 305 322 339 358 376 395 417 436 457 480 502 524 550 572 597 627 654 683 713 739 769 803 832 866 903 938 978 1016 1055 1096 1138 1186 1232 1281 1329 1380 1430 1488 1544 1604 1679 1740 1813 1880 1964 2037 2115 2194 2280 2366 2464 2566 2636
```

For type 0 the last point gives 2636 ↔ 3420, and the second-to-last gives 2594 ↔ 3400.

**Table build.**

Setup:
- `t0 = tpl[0]`.
- `xs[i] = 50·(i−1)`, then `xs[n−1] = max_travel`.
- `out` is i16[2657], placed at `0x88244`, `0x86D80`, `0x858BC`, `0x843F8`. These are assigned in type order to the types actually in use, at most 4.

```
for i in 0..n:            out[tpl[i] − t0] = xs[n−1] − xs[i]        # height; NOTE i runs to n inclusive
for each segment (f1, f2) = (tpl[i−1], tpl[i]), i = 1..n−1, and f1 < j < f2:
    out[j − t0] = ((j − f1)·out[f2 − t0] + (f2 − j)·out[f1 − t0]) / (f2 − f1)     # truncating
```

The `i = n` iteration reads one element past the template, and `xs[n] = 0` (zeroed buffer). It writes `max_travel` at `tpl[n] − t0`. For the stock data that lands:
- outside the table for types 0 and 2: DLM `0x8D282` (ADC1 buffer) and `0x8F0F0` (travel array), both rewritten every scan;
- at index 11 for type 1, which is harmless;
- at index −1 for type 3 (`0x843F6`, inside the RGB speed/brightness working bytes).

Our firmware must not copy these stray writes.

**Lookup** `curve(t, f)`:
- `f < t0` → `out[0]` (= max + 50, so travel clamps to 0);
- `f > 2635` → `out[2636 − t0]` (= 0);
- otherwise `out[f − t0]`.

The threshold `t0` (`0x91E74[t]`) is −12 / −11 / −7 / −10.

Full dumps: `re-work/engine/curves_t<N>.txt` (f, height). Type 2 samples:

| f | 0 | 3 | 100 | 500 | 1000 | 1500 | 2000 | 2268 | 2500 | 2620 | 2636 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| travel µm | 0 | 22 | 433 | 1476 | 2232 | 2713 | 3073 | 3234 | 3354 | 3412 | 3420 |

`f10[t]` (`0x91EB4`) = mean f of the entries whose height is 8..12 µm: 2626 / 2625 / 2622 / 2621. It is used only by auto-cal.

## 6. Auto-calibration (`auto_calib_enable` = bank-2 byte `0xC71` == 1)

This runs every scan at step 10 once the engine is enabled. The per-slot state is:

| Field | Address | Type |
|---|---|---|
| state | `0x91CBC` | u8 |
| ok | `0x91C64` | u8 |
| cnt | `0x916DC` | u16 |
| accum | `0x8DAE8` | i32 |
| fit | `0x9162C` | u16 |

Inputs are `hB` and `vel` from §4.

- **Enter.** When `state == 0`, `hB < 400` and `vel > 15` (falling faster than 15 µm/scan ≈ 120 mm/s inside the last 400 µm): set `state = 1`, `cnt = 1`, `accum = 0`, `ok = 0`.
- **Hold.** While `state == 1` and still inside the zone (`hB < 400`, or exactly 400) with `vel ≥ −15`:
  - `cnt++`; samples from `cnt > 2000` onward are added to `accum` (`+= y`).
  - At `cnt == 4000`:
    ```
    mean = accum/2000 (u16)
    k    = (f10[type]/2636.0f + 1.0f) · 0.5f
    fit  = (u16)( base + (mean − base)/k )
    ok   = 1
    if | |fit − base| − |bot_eff − top| | > 600:   state = 0     (reject; no update)
    ```
- **Exit.** On `vel < −15` or `hB > 400`: `state = 0`. If `ok` was 1 **and** `cnt > 5000` (held ≥ 0.625 s), apply the update:
  ```
  swing = |top − bot|  (stored calibration)
  d     = clamp(fit − base,  ≤ swing+135, ≥ max(−135−swing, swing−380), ≤ 380−swing)   # evaluated in this order
  new   = (u16)(base + d)
  act_ref = act_ref + (new − act_ref)/2;  fit = new
  span  = (int)( −((float)act_ref − (float)base + (float)(2·dz)) · 65536 / 2636 )
  ```

The bounds are inverted (lower > upper) for any `swing > 257`, so **`d` always ends up as `380 − swing`**. The measured `fit` only *gates* the update (the ±600 check). Each qualifying hold moves `act_ref` halfway toward `base − swing + 380`, which shrinks the span by about 380 counts compared with the stored calibration.

Emulator check with top/bot 30000/18000: `act_ref` 18000 → 18189 → 18284 → 18331 → 18355, converging to 18379.

With the unit's factory 31000/17000 and a real swing of about 12000, every attempt is rejected, so auto-cal never does anything. Auto-cal never touches `top/bot`, `baseline` or `dz`.

## 7. Actuation (per key object, `FUN_9D10` → handler at obj+0x58)

The key objects are 88 × 0xB8 bytes at `0x804A8 + 0xB8·key`; the full field map is in the appendix. A handler sees:
- `hA`/`hB` through pointers;
- thresholds converted to heights in `FUN_A6F8`.

Height thresholds:

| Name | Value |
|---|---|
| `P` (+08) | `max − norm_press_th` |
| `R` (+0A) | `max − norm_return_th`, forced ≥ `P` |
| `TOP` (+04) | `max − press_deadzone`. If `press_deadzone == 0 && press_th < 20`, it is `max − 10`. |
| `BOT` (+06) | `min(return_deadzone + 10, max)` |

Edges set `+2D` (press) / `+2E` (release), which are used by edge-triggered function keys.

**Normal (`key_mode` 0, `0x15898`):**
- Released: press if `hA ≤ P` (that is, press-side travel ≥ `norm_press_th`). Record `low = hA`.
- Pressed: release if `hB ≥ R` (release-side travel ≤ `norm_return_th`). Record `peak = hB`.

**Rapid trigger (`key_mode` 1, `0x10B56`).** `low` is +30 (lowest height while pressed); `peak` is +32 (highest height while released).

Released:
1. Top guard:
   - When the key first comes back into the top zone (`hA ≥ topThr`, flag +38 clear), set the flag and arm counter +3A = `topLen`.
   - While the counter runs it counts down; it is cleared early if `hA < topThr`.
   - Presses are blocked while it is non-zero, unless `y` is outside 6000..59999.
2. Track the peak: if `peak < hB ≤ TOP`, set `peak = hB`.
3. Press if (`hA ≤ TOP − press_th` and `hA ≤ peak − press_th`) or `hA ≤ BOT`. Then set `low = hA` and clear the top-guard flag.

Pressed (the mirror image):
1. Bottom guard: when `hB < 101` first happens, arm +36 = `botLen`. It holds while `hB < botThr`.
2. Track the low point: if `low > hA ≥ BOT`, set `low = hA`.
3. Release if (`hB ≥ BOT + return_th` and `hB ≥ low + return_th`) or `hB ≥ TOP`. Then set `peak = hB` and clear the bottom-guard flag.

Consequences:
- RT is continuous anywhere between the dead zones.
- Inside the top dead zone the key is always released.
- Inside the bottom zone (`return_deadzone + 10` µm from the bottom) a released key re-presses at once and cannot be RT-released.

Guard constants (hard-coded):
- `botLen`/`botThr`/`topLen` = 128/100/80 and `topThr = max − 300` (10 ms and 16 ms).
- Space, Backspace, Enter and LShift use bigger values when `return_th < 300`.
- The guard table and the mode-2 handlers (`0x11EC2`, `0x1085A`: RT only after the normal actuation point, pressed flag 2 = "RT-released inside the RT zone") are in the appendix.

**Verified in the emulator:**
- Settings: `press_th` = `return_th` = 100, dead zones 200.
- A ramp 30000→22000 presses at 366 µm (plain travel).
- A lift releases at 2606 after a peak of 2713.
- Pressing again re-presses at 2501 after a peak of about 2350.
- Normal mode (1600/800) presses at 1624 and releases at 756.

The extra ≈ 25–60 µm beyond each threshold is the `dz` hysteresis.

**Object ↔ step quirk.** `FUN_A6F8` files object k in the per-step table at position `slot2key[k]` instead of `key2slot[k]`. Every object is still evaluated once per scan, but up to 10 steps away from its sample, so a key edge can lag by up to one scan (125 µs).

## 8. Layers and tap-hold

**Fn layer:**
- The Fn flag (`0x91F9C[0]`) is recomputed on every report rebuild. It is set if any listed key has **main-layer** code 401.
- Each key then uses `key_map_fn` instead of `key_map`.
- The layer is not latched: releasing Fn switches still-held keys back to their main-layer codes at the next rebuild.
- A Fn-layer value of 1000 is **not** a fallthrough. The key sends nothing while Fn is held, which is why the vendor maps the modifiers to themselves.

**FN2..FN4 (402–404) do nothing.** Codes ≥ 401 are ignored by `FUN_C698`.

`0x91F9C[1..7]` flag that LCtrl, LShift, LAlt, LWin, RCtrl, RShift or RAlt is held on the current layer (codes −8..−2, table `0x800410A8`). Only `~ESC` uses them.

**Undocumented tap-hold (mod-tap).** Bank-2 `+0x000`/`+0x0B0` (u16/key, `0x80080000`/`0x800800B0`, factory 1000) hold a per-layer *hold code*. When it is not 1000, the tap-hold state at +16 runs (flag `0x92048`):

| Situation | Result |
|---|---|
| Pressed, ≤ 4000 scans | state 1, nothing sent |
| Pressed, > 4000 scans (0.5 s) | state 2, hold code sent |
| Released from state 1 | state 3 for one rebuild, normal code sent as a one-scan tap |
| Released otherwise | state 0 |

Bank-2 `+0x790`/`+0x7E8` (u8/key, factory 0) are per-layer **modifier masks**, ORed into report byte 0 while the key is down (flag `0x920C0`). `+0x8F0`/`+0x9A0` (u16, factory 100) are not read by the engine.

## 9. SOCD, key groups, DKS

**SOCD (`key_cancel_code`).** For code c ≠ 1000, the target is report byte `(c>>3)+1`, bit `c&7` for c in 1..119. Otherwise it is byte 0, bit `c&7`. Modifiers −8..−1 therefore work as targets; consumer, mouse and function codes wrongly map onto a modifier bit.

The cancel flag (+2C):
- **arms only on this key's press edge**, if the target bit is currently set in the report;
- **disarms** when this key is released, or when the target is gone from the un-cancelled snapshot `0x91F64`.

A report rebuild works in this order:
1. Build the report.
2. Apply the WinLock mask.
3. Copy the report to the snapshot.
4. Clear the bit of every armed target.

So a single entry A→D cancels D only if D was down first. Two entries (A→D, D→A) give last-press-wins, and the first key returns when the second is released. It is layer-agnostic.

**Key groups (`bind_key_mode` 1, handler `0x15E68`).** While the key is pressed (by its own actuation), every slot code ≠ 1000 in `bind_key_setting[16..23]` is also sent. Member codes do not depend on the layer.

**DKS (`bind_key_mode` 2, `FUN_E31C`).** Record r = `bind_key_setting` (shadow `0x1358 + 48k`). The points are converted to heights: `TP = max − r[24]`, `BP = max − r[26]`. Phase order is the panel's column order:

| State | Event | Phase fired | Next state |
|---|---|---|---|
| 0 | `hA ≤ TP` (pressed past top point), unless the top guard is active | 0 | 1 |
| 1 | `hA ≤ BP` (past bottom point) | 1 | 2 |
| 1 | `hB ≥ TP` (back above top point) | 3 | 0 |
| 2 | `hB ≥ BP` (back above bottom point), unless the bottom guard is active | 2 | 3 |
| 3 | `hB ≥ TP` | 3 | 0 |
| 3 | `hA ≤ BP` | 1 | 2 |

The action byte for slot i is `r[4·i + phase]` (slot-major), and a slot whose code is 1000 is ignored:

| Action byte | Effect |
|---|---|
| 1 | Hold until a later phase applies 0 or 2 |
| 2 | Tap: released for `delay + rnd(random)` scans, then pressed for `single` scans |
| 0 | End a hold; a running tap still completes |

- The timing fields `r[28/30/32]` (`singleTime/delayTime/randomTime`) are in **scans**. The UI stores ms × 8, so the UI unit is ms (factory 8/24/16 = 1/3/2 ms).
- `rnd(n) = ((s>>8)+s & 0xFF)·n >> 8` with `s = s·0x805 + 0x3619` (u16 at `0x801AE`). Only the low byte of `randomTime` is used.
- A slot whose hold just ended stays in the report for 9 more scans.

**Byte 47** (all modes):
- `r[47] = 1` (default, UI "trigger original key" off): the key's own codes are forced to 1000 and its handler to normal mode.
- `r[47] = 0`: the original key is sent too.

The UI label is accurate.

## 10. Function codes 301–320 (`FUN_C698`)

"Edge" means the code acts once per press (needs +2D). "Level" means it repeats on every rebuild while the key is held.

| Code | Behaviour |
|---|---|
| 301 | Level. Jump to the download loader `0x80020000`. |
| 302 | Level. Watchdog reboot. |
| 303 | Level. All keys to normal mode (RAM only; any config apply restores). |
| 304 | Level. Handlers from `key_mode` again. Applies the Space/Bksp/Enter/LShift guard constants without the `return_th < 300` gate, and ignores DKS byte 47. |
| 305 | Level, repeats while held. Factory-reset the **active profile** and save to flash. |
| 306 ~ESC | Level. Sends Esc (0x29), or `` ` `` (0x35) with the held Shift bit(s) forced, if LShift/RShift is held on the layer. |
| 307–310 | Edge, only when the cooldown is 0. Switch profile code − 307 at the end of the scan. Cooldown is 7200 scans = **0.9 s**; during it **no report is rebuilt** (the last report freezes). `BE C0` sets the same cooldown. |
| 311/312 | Edge. Brightness ±12 on 0..180 (`rgb_config[3]` = value/6). |
| 313/314 | Edge. Next/previous effect (next skips 2, wraps 51 → 1). |
| 315 | Edge. `rgb_enable` toggle. |
| 316 WinLock | Edge. RAM toggle `0x804A4` (0 at boot). It masks report byte 0 with `~0x88`, blocking **LWin and RWin only**. `win_lock_state` is just the LED option. |
| 317/318 | Edge. Effect speed ±17. |
| 319 | No-op as a keycode. |
| 320 | Edge. Swap A↔Left, W↔Up, S↔Down, D↔Right in the main-layer codes (RAM only). |

**The real 5-s reset is hard-wired:** while the Fn flag is set and **physical key 0 (Esc)** is pressed, a counter runs once per scan. At 40000 scans (5 s) it erases flash `0x80070000..0x80087FFF`, which is **both banks, including `top/bot` calibration**, blinks white 3× and reboots. This happens regardless of the keymap.

Factory Fn layer: Esc 319, Bksp 315, Del 320, `\` 313, ↑ 311, ↓ 312, ← 318, → 317, LWin 316, C = next profile.

**Consumer 201–225.** Code `201 + n` sets bit n of the 3-byte report on EP 0x83 (interface 1, descriptor `0x80040A1C`):

| n | usage | n | usage | n | usage | n | usage |
|---|---|---|---|---|---|---|---|
| 0 | E9 | 6 | B5 | 12 | 224 | 18 | 183 |
| 1 | EA | 7 | 192 | 13 | 226 | 19 | 6F |
| 2 | E2 | 8 | 18A | 14 | 225 | 20 | 70 |
| 3 | CD | 9 | 194 | 15 | 227 | 21 | 2A0 |
| 4 | B7 | 10 | 221 | 16 | 22A | 22 | 29F |
| 5 | B6 | 11 | 223 | 17 | 19F | 23 | 1CB |

Code 225 (n = 24) has no bit: it falls into the keyboard path and writes past the report, so it is broken. There is no System Control report.

**Mouse 251–255** are bits 0..4 (left, right, middle, back, forward) of byte 0 of the 3-byte report on EP 0x84 (interface 3, boot mouse, X/Y always 0).

## 11. HID reports (`FUN_9D10`)

**Keyboard report (EP 0x81, interface 0, descriptor `0x800420D4`).**
- 16 bytes, no report id: byte 0 = modifiers E0–E7; bytes 1..15 = **NKRO bitmap** of usages 0x00–0x77.
- Code c ≤ 0 sets bit `c&7` of byte 0 (−8 LCtrl … −1 RWin; code 0 also sets LCtrl). Codes 1..120 set byte `(c>>3)+1`, bit `c&7`.
- The interface claims boot subclass/protocol, but the report is never in boot (6KRO) format.

**Rebuild.** The pressed list `0x8E1C8` holds objects with pressed == 1 or tap-hold state 3, in press order. A rebuild happens only when something changes (list add/remove, tap-hold state change, DKS/group list change) **and** the profile cooldown is 0. It runs these steps in order:
1. Clear the reports.
2. Compute the Fn and modifier flags.
3. OR in the modifier masks.
4. Pressed keys: tap-hold state 0 → layer code, 2 → hold code, 1 → nothing, 3 → layer code.
5. DKS/group slots.
6. WinLock mask.
7. SOCD.
8. Clear the edge flags of this step's objects.

**Send.**
- Each `FUN_9D10` call (11 per scan) compares the keyboard report with the last-sent copy (`0x80494`, initialised to `FF…` after each config apply) and sends it if different.
- Consumer and mouse reports are compared and sent the same way (`0x92148` / `0x92144`).
- A send needs:
  - USB configured;
  - the endpoint idle (`0x88242 == 0`);
  - `gate && !suspended`.
- After a send the ISR busy-waits up to 50 ticks (about 0.57 ms) for completion. On timeout it sets suspended, re-inits USB and clears the reports.
- Reports are thus sent **on change only**, at most once per step.

**Rollover limit.** With more than 20 keys in the list, an empty report is sent and reports stay empty (lock `0x92028`) until a call sees ≤ 4 keys held.

## 12. BE FD stream

**Normal frame (1017 bytes).** Built at step 10 every 320 scans (25 Hz), only while the heartbeat is alive and USB is configured. It is sent at once on EP 0x82. Words are u16 LE in key order (through `key2slot`):

| Words | Source |
|---|---|
| 0–82 | filtered raw `y` (`0x8EE28`) |
| 83–165 | `clamp(max − hC, 0, max)` µm |
| 166–248 | dynamic `baseline` (`0x90588`) |
| 249–331 | power-up baseline `rest0` (`0x8EF88`) |
| 332–414 | DA only: running peak-to-peak of `y − baseline` (`0x90F28`), sampled at each 512-scan window |
| 415–497 | DA only: running peak-to-peak of travel (`0x8F3A8`), sampled per frame |
| 498 | DA flag |
| 499 | scans counted in the last second (8000) |
| 500 | engine busy time in the last second / 1000 (GPTMR0 runs only while the engine works; 680 ≈ 68 % load) |
| 501–504 | uptime in seconds, u64 LE |

The min/max accumulators are not reset when DA is toggled.

**Calibration frame (1003 bytes, `BE FE 01`, `FUN_E860`).** In calibration mode the normal engine is bypassed:
- Each scan the 88 averages are filtered by `y += (x − y) >> 4`.
- The state is seeded with `x` the first time calibration mode is entered after boot.
- The result is written to `0x8EE28`.

After the first 8000 calibration scans since boot (`0x920A8`, never reset):
- a snapshot of all 88 values is taken every 8 scans;
- after 6 snapshots (every 48 scans, about **167 frames/s**, with no heartbeat needed) a frame is sent: `BE FD EB 03`, then **6 consecutive 83-word blocks** (snapshot 0 … 5, key order), CRC, `ED`.

`protocol.md`'s "interleaved per key" is wrong.

## 13. `BE DA`

Byte 4 = 2 → flag 0, 3 → flag 1 (`0x91F78`). The flag only enables the two min/max accumulators (words 332–497) and is echoed in word 498. It is cleared together with the stream when heartbeats stop for 16000 scans. **It does not change travel or actuation.**

## Appendix: key object (0xB8 bytes @ `0x804A8 + 0xB8·key`)

| Off | Field |
|---|---|
| +00/+02 | `press_th` / `return_th` (µm). If either is 0 in the profile, the shadow is rewritten from the object. |
| +04/+06 | TOP / BOT heights (§7) |
| +08/+0A | P / R heights |
| +0C/+0E | `key_map` / `key_map_fn` |
| +10/+12 | hold codes (§8) |
| +14/+15 | modifier masks |
| +16/+18 | tap-hold state / previous state |
| +1A | key index |
| +1B | pressed: 0, 1, or 2 = mode-2 RT-released |
| +20 | u64 hold counter |
| +28 | `key_cancel_code` |
| +2A/+2B | SOCD byte / mask |
| +2C | SOCD armed |
| +2D/+2E | press / release edge |
| +30/+32 | low / peak (RT references) |
| +34/+36 | bottom guard flag / counter |
| +38/+3A | top guard flag / counter |
| +3C/+3E/+40/+42 | botLen / botThr / topLen / topThr |
| +44/+48/+4C/+54 | pointers to hA / hB / hC / y of the key's slot |
| +50 | max travel (i32) |
| +58 | actuation handler: 0 `0x15898`, 1 `0x10B56`, 2 `0x11EC2` (keys 76, 43, 57, 59: `0x1085A` if `return_th < 300`), other `0x15898` |
| +5C/+5E | list membership flags |
| +60.. | group/DKS part: +60 handler, +64/+66 BP/TP, +68 DKS state, +6A/+6C/+6E times, +80 slot codes, +8C actions, +9C slot outputs, +A8..+B6 guards |

Guard constants for mode-1 keys:

| Keys | botLen | botThr | topLen | topThr |
|---|---|---|---|---|
| default | 128 | 100 | 80 | `max − 300` |
| Space (76), if `return_th < 300` | 440 | 550 | 440 | 650 |
| Bksp (28) / Enter (57), if `return_th < 300` | 160 | 100 | 120 | 650 |
| LShift (59), if `return_th < 300` | 440 | 100 | 440 | 650 |

The value 650 is an absolute height (vendor bug, probably meant `max − 300`).

**Factory profile (`FUN_F128`):**
- `press_th` 50, `return_th` 30, norm 1600/800, `press_deadzone` 55, `return_deadzone` 0.
- Hold codes 1000, masks 0, SOCD 1000, `bind_key_mode` 0.
- DKS record: actions 0, keys 1000, 100/3000 µm, times 8/24/16, byte 47 = 1.
- `key_mode` W, A, S, D = 1 (rapid trigger); all others 0.
