# Open questions and physical-access items

What the static reverse-engineering could not settle, grouped by whether it needs hardware. The
confident findings are in `hardware.md`, `protocol.md`, `algorithms.md`, `bootloader.md`,
`stable-vs-latest.md`.

## Determinable only with physical access

- **Exact MCU part and package.** Narrowed to **HPM5361, QFN48 (`HPM5361ICB1`)** from the datasheet
  (16 analog pins total, 8 bonded on QFN48) and the scan's 8 sense lines (`hardware.md §1`). HPM5361
  vs a pin-compatible `HPM53x1ICB1` sibling, and the package for certain, can only be confirmed by
  **reading the die silkscreen** — open the case.
- **Analog multiplexer chip and its select lines.** The scan uses 8 sense lines (4→ADC0, 4→ADC1)
  stepped through 11 positions; the step is driven by a GPTMR1 pulse (`FUN_ram_00015250`), and there
  are **no steady-state GPIO writes to the select lines** in the firmware, so the mux-select pins and
  the mux IC (the Rainy75-HE sibling reportedly uses Nexperia 74HC4067-class 16:1 parts) are not
  derivable from the binary. Needs the **schematic or a logic analyzer** on the GPTMR1 outputs / mux
  select pins, plus continuity to confirm the `0x80040528`/`0x80040580` scan-order tables against the
  physical matrix.
- **RGB LED data pin.** Driven by SPI1 MOSI via an IOC pad (func 5, register `0xf40400e8`);
  the exact package pin needs the IOMUX decode against the HPM5361 QFN48 pinout, or probing.
- **TinyUF2 recovery trigger.** The loader binary at `0x80020000` is not in our dumps and the
  hpm_sdk `samples/tinyuf2` source is not in this SDK checkout, so whether the vendor fork honours a
  **reset-time GPIO button** (the EVK uses PA9-low) and/or a **double-tap reset** magic, and on which
  pad, is undetermined (`bootloader.md §5`). Resolve by **experiment** (double-reset; hold various
  keys at power-on and watch for the UF2 drive) and, if needed, **open the case** to find a
  bootloader pad. Also confirm empirically that the "**app without the `0x0b4d5475` signature at
  `0x80040000` stays in the loader**" recovery (the recommended dev safety net) actually holds on
  this board before relying on it.
- **Recovery pads to locate and label before any custom flash** (`bootloader.md §5`): UART0 ROM-ISP
  pads **PA00 (TXD) / PA01 (RXD)**, the **BOOT pins PA02/PA03**, and **JTAG PA04–PA08**. Note this is
  a RISC-V core: debug is **JTAG, not SWD**. Whether JTAG is **OTP-locked** on this unit is unknown
  and can only be checked by attaching a probe.

## Determinable from the binary with more analysis (not yet traced)

- **DKS byte 47 ("DKSOriginCover" / "trigger the original key").** Parsed into the runtime struct in
  `FUN_ram_0000a6f8`, but its exact effect — whether it suppresses or still emits the physical key's
  own code — was not traced through the actuation path (`FUN_ram_00009d10` / `FUN_ram_0000e31c`). The
  vendor side is itself ambiguous (variable name "cover" vs UI label "trigger"). Needs a deeper trace
  of the DKS state machine, or an on-device A/B test.
- **`rgb_config` low two bits always written as 1** (vendor observation). Meaning not confirmed on
  the firmware side; trace the `rgb_matrix` config consumer that reads `0x80080C60`.
- **Unnamed config bytes `0x80080C65..0x80080C67`.** Carried in the vendor's 17-byte RGB export but
  never named; firmware meaning unconfirmed.
- **FD-stream tail semantics.** The 1017 tail words beyond the scan-rate (`8000`) — `0x91fb0/1000`
  (≈680) and the 8 bytes from `0x843e8` — are internal counters whose exact meaning was not pinned
  down (`protocol.md §BE FD`). The two auxiliary per-key arrays streamed at words `[332..497]`
  (`0x90f28`, `0x8f3a8`, zero at rest) are identified as velocity/diagnostic arrays but their precise
  formula was not fully derived.
- **The `0xDA` diagnostic post-processor** `FUN_ram_00015a84(&0x801b0, &0x8f248, &0x8f3a8, &0x8f0e8)`
  rewrites the travel/auxiliary arrays from a table at `0x801b0`; the exact transform (and why the
  vendor ships but never uses it) was not fully decoded.

## Product / process questions

- **Travel-stuck-at-0 reproduction.** The root cause is understood (`algorithms.md §8`). Still worth
  doing on-device: dump a victim key's live `BE FD` raw range vs its stored `top_value`/`bot_value`
  to quantify how small the real swing is relative to the 31000/17000 default, and confirm the panel
  can flag such keys automatically (stored `top−bot` implausibly large vs the live raw span).
- **Custom-firmware recovery drill.** Before flashing our own app, run the full drill once: build a
  signature-less test app, flash it, confirm the board returns to TinyUF2 on reset, then restore the
  vendor 0.0.18 — closing the loop on `bootloader.md §5`.
