# Stable (0.0.9) vs latest (0.0.18)

The vendor driver's firmware dropdown offers **0.0.9 = 稳定版本 (stable)** and **0.0.18 = 最新版本
(latest)**; the device runs 0.0.18. This diff matches functions between the two decompilations with
`re/match.py` (output `re/out/match_0.0.9_0.0.18.tsv`) and reads the ones that moved. Both images are
the same MCU/board; the app grew from **74 240 B** (`0x80040000..0x80052200`) to **76 032 B**
(`..0x80052900`), and `gp` moved from `0x95784` (0.0.9) to `0x91c2c` (0.0.18) as globals shifted.

The vendor changelog embedded in the driver bundle (see `panel/PROTOCOL-NOTES.md`): 0.0.18 = mouse
key codes + key groups + DKS; 0.0.17 = auto-calibration tuning; 0.0.16 = switch-curve tuning and
fixes; 0.0.10 = logic tweaks + a sleep bug fix; 0.0.8 = default switch → Jade Pro. 0.0.9 therefore
predates every switch-curve and auto-cal refinement and all of the advanced-key features.

## Command handlers — identical

The dispatcher is `FUN_ram_00002f2a` (0.0.9) ↔ `FUN_ram_0000b812` (0.0.18), match ratio 0.886
(same code, shifted addresses). **The command set is byte-for-byte the same**: the `cmd < 0xC0`
switch has `00,01,02,03,04,1D`; `0xC0` is its own branch; `cmd ≥ 0xDA` is `switch(cmd+0x26)` with
`0xDA,0xEE,0xFC,0xFE,0xFF`. So:

- **`0xDA` exists in both** (same `byte4 ∈ {2,3} → 0/1` diagnostic toggle). It is not a 0.0.18
  addition.
- Same framing (`BE … len … ED`), same CRC-16/MODBUS, same reply shapes, same serial (`BE 1D`).
- **Same address validation**: bank 1 `0x80070000..0x80078000`, bank 2 `0x80080000..0x80100000`,
  `n ≤ 0x200`; same `0x80078000`/`0x80078055` virtual-address special cases. The only numeric change
  is the RGB live-apply boundary inside case 3: **`0x80080aff` (0.0.9) → `0x80080a1f` (0.0.18)**,
  because the per-profile region layout shifted when the advanced-key regions were inserted (below).

No command was added, removed, or changed in meaning between the two.

## RAM map / flash layout

- **Config banks unchanged in place**: bank 1 (`0x8007xxxx`, board-global: top/bot, keytype,
  version, profile, names) and bank 2 (`0x8008xxxx`, per-profile) sit at the same base addresses.
  The ROM-API flash path and the `0x80040000` app / `0x80020000` loader split are identical.
- **0.0.18 adds the advanced-key regions inside bank 2**: `bind_key_setting` (`0x80080D00`,
  48 B/key) and `bind_key_mode` (`0x80081DE0`). 0.0.9's config-apply never references them
  (`re/fn.sh 0.0.9 0000468a` has no bind/DKS handling), and the vendor JS reads them only when the
  firmware version is ≥ 18. So on 0.0.9 those addresses are just unused config space; the feature is
  firmware-gated, not merely UI-gated.
- Globals (`DAT_ram_*`) all moved because of the above and the extra code, so absolute addresses are
  not comparable across the two — only structure is.

## Scan / ADC / calibration

Same skeleton in both: 8 kHz, 11 steps/scan, 2 ADCs hardware-triggered into DMA, a **12000-scan
warm-up** gate (0.0.9 `DAT_ram_00095e80 < 12000`, 0.0.18 `DAT_ram_000920ac < 12000`), a
**512-still-frame dynamic baseline** (`== 0x200`), and the stored-top/bot → per-key `span` model.
The pieces that changed:

- **Raw reduction reworked.** 0.0.9's per-group averaging `FUN_ram_0000c2a8` (removed) → 0.0.18's
  `FUN_ram_00013314` (new, 178 B): the "drop the first of 4 conversions, average the other three"
  grouping. A filtering/averaging refinement.
- **Switch-curve lookup retuned, not redesigned.** 0.0.9 `FUN_ram_0000de8a` (60 B) and 0.0.18
  `FUN_ram_00014d86` (88 B) are the *same algorithm* — per-switch-type table interpolation with the
  same `0xA4B/0xA4C` domain — but the tables and the threshold handling changed (0.0.18 stores the
  per-type threshold in `DAT_ram_00091fe4` and clamps slightly differently). This is the 0.0.16
  "switch-curve tuning". `match.py` lists `00014d86` as "new" only because the address/shape moved
  enough to defeat the matcher; functionally it is the evolved `0000de8a`.
- **Auto-calibration present in both**, bounded by the stored top/bot; 0.0.17's "tuning" is a
  parameter/logic refinement of the same refit block, not a new mechanism.

## New features in 0.0.18

- **Key groups and DKS** (`bind_key_mode` / `bind_key_setting`). Entirely new runtime:
  `FUN_ram_0000e31c` (690 B, the DKS state machine, dispatched by mode from the config-apply path),
  plus `FUN_ram_00014804` (bind-change detector) and helpers — all with **no 0.0.9 counterpart** in
  `match.py`. 0.0.9 has no DKS/group engine.
- **Mouse button key codes 251–255** (vendor JS changelog); the stock 0.0.18 keymap can emit mouse
  buttons, 0.0.9 cannot.
- Several other new-in-0.0.18 functions from `match.py` (`0x10b56` 378 B, `0x14e86`, `0x146ae`,
  `0x14148`, `0x0000e31c`) cluster around the advanced-key feature and the retuned scan pipeline.

## Bug fixes

0.0.9 predates the 0.0.10 "sleep bug fix" and the 0.0.16 "switch curve tuning and fixes". Beyond the
curve retune above, the sleep path (`sleep_second_cnt` handling) and assorted logic tweaks differ,
but no single localized "the fix for X" function stands out in the diff — the changes are spread
across the reworked scan pipeline.

## Verdict

- **What stable (0.0.9) lacks:** key groups and DKS (no `bind_key_*` engine at all), mouse-button
  key codes, the 0.0.16 switch-curve retune, the 0.0.17 auto-cal tuning, and the 0.0.10 sleep fix. If
  you rely on DKS/key-groups or mouse keys, 0.0.9 cannot do them regardless of the driver.
- **What latest (0.0.18) adds in risk:** more surface — a 690-byte DKS state machine and extra config
  regions — and the config-bank layout shift. Nothing in the command or flash path changed in a way
  that endangers recovery (same `BE FC`, same loader split, same banks).
- **Does either explain the travel-stuck-at-0 fault? No — and downgrading will not fix it.** The
  mechanism (`travel = curve((live_rest − raw − deadzone) × 65536 / span)` with `span` sized from the
  **stored** `top_value`/`bot_value`, and the curve clamping anything below its threshold to a flat 0;
  see `algorithms.md`) is **present and identical in both 0.0.9 and 0.0.18**. The 0.0.16 curve retune
  and 0.0.17 auto-cal tuning only shift *which* marginal keys land right at the clamp threshold, so
  switching versions might move the symptom between keys but cannot cure it. The real cure is correct
  per-key stored `top_value`/`bot_value` (recalibration) — version-independent.

**Recommendation:** stay on 0.0.18 (the device's shipped version; it is a strict feature superset and
nothing in the diff adds recovery risk). Treat stuck-at-0 as a calibration problem, not a firmware
one.
