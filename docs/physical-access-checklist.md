# Physical-access checklist (to close the last open items)

Everything reachable from software is done (see `open-questions.md` for how each was narrowed). The
four remaining items need the case opened once. This is the exact capture list and what each closes.
No rush and no risk to daily use — the board is on stock 0.0.18 and fully recoverable via
`tools/rollback.sh`.

## 1. Photos (close the mux IC + the MCU SKU)
- **Whole PCB, both sides, sharp and well-lit.** Lets me map the layout.
- **Close-up of the main MCU silkscreen** (the top marking). → confirms HPM5361 vs sibling SKU
  (item: "package silkscreen", `open-questions.md`). Expected: an `HPM53…IEG1`-style line.
- **Close-up of every small IC near the switch rows** (the analog multiplexers), reading their top
  marking. → identifies the mux IC (expected a 74HC4067 / CD74HC4067-class 16:1, or similar). This
  closes the "analog-mux IC" item.

## 2. Continuity / probing (close the mux select routing + the ADC inputs)
With a multimeter in continuity mode, from the MCU pins we already identified in `hardware.md §3`:
- **PY00, PY01, PY02, PY03** (the likely 4-bit mux **address** lines) → trace to the mux IC select
  pins (S0..S3 / A..D). Confirms the address wiring and bit order.
- **PB00, PB01, PB08, PB09, PB10, PB11, PB12, PB13** (the 8 ADC **sense** inputs) → trace to the mux
  IC common/output pins (Z). Confirms which mux feeds which ADC channel, which in turn validates the
  scan-order tables at `0x80040528` / `0x80040580` against the physical matrix.
  (MCU pin → pad name mapping: use the HPM5361 QFN48 datasheet pinout.)

## 3. Bootloader recovery trigger (close the hung-signed-app case; lifts the custom-firmware gate)
The trigger mechanism is known (`bootloader.md §5`): an **active-high GPIO sampled ~100 ms at reset**
(normally high → app; pull low → DFU), **no double-tap**. To find THIS board's pad:
- Look for a **reset button/test-point** and any obvious **BOOT/DFU pad or button**. Photograph them.
- If there is a reset button: with the UF2 tools ready, try the normal flow — but note double-tap is
  ruled out, so a plain double-press will NOT work; it must be a *held pin at reset*.
- Easiest confirmation: while watching `dmesg`, momentarily **ground a candidate pad and tap reset**;
  if USB `34B7:1001` (the UF2 drive) appears, that pad is the trigger. Report which pad.
- This also lets me lift the gate on flashing a **signature-valid** custom build (currently blocked in
  `firmware-plan.md` Risk 2), because we'd then have a guaranteed way back from a hung signed app.

## 4. Recovery pads to locate (ultimate fallback, optional)
From `bootloader.md §5`: **BOOT_MODE = PA02/PA03**, **UART0 ROM-ISP = PA00 (TXD)/PA01 (RXD)**,
**JTAG = PA04 TDO / PA05 TDI / PA06 TCK / PA07 TMS / PA08 TRST**. Photograph/label any exposed pads or
test-points for these. With them, `hpm_isp` (UART) or `riscv-openocd`/`probe-rs`/J-Link (JTAG) can
recover the board no matter what is in app flash — the belt-and-suspenders backup before any
signature-valid custom firmware.

## What I do with it
Send the photos/continuity notes and I will: name the mux IC and finalize `hardware.md`; confirm the
MCU SKU; record the trigger pad and lift the custom-firmware flashing gate in `firmware-plan.md`; and
resolve the corresponding items in `open-questions.md`. Nothing else in the project is waiting on this.
