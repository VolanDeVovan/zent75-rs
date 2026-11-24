# Our firmware — plan

Rust on **embassy + `hpm-hal`** (feature `hpm5361`), target `riscv32imafc-unknown-none-elf`
(nightly). The project lives in `firmware/`; today it is a skeleton that builds, links at
`0x80040000` with the boot signature, and packages to UF2. Nothing custom has been flashed yet.

## Why Rust

`hpm-hal` (official HPMicro org) supports HPM5300 best: ADC16 sequences, USB HS via embassy-usb,
SPI/DMA, XPI NOR through `embedded-storage`. **libhmk** (GPL-3.0, C) is the algorithm reference
only (rapid trigger, SOCD, DKS, auto-calibration) — reimplement, do not copy. Fall back to C
(hpm_sdk + CherryUSB) only if 8 kHz HS HID proves impossible on `hpm-hal`.

## Packaging and flashing

- Linker: word 0 at `0x80040000` = `0x0B4D5475`, entry `0x80040004`; nothing below `0x80040000`.
- `firmware/package-uf2.sh` → `firmware/zent75-rs.uf2` (family `0x0B4D5475`).
- Flash with `tools/flash_uf2.py` (refuses images outside the app region). Recovery and rules:
  `boot-recovery.md` — a hung build is recovered by holding A+S+D at plug-in.
- Early bring-up: consider building without the signature first (stage-0 then never runs it) only
  to test packaging; real runs need the signature.

## Protocol

Implement the vendor framing first (`protocol.md`) so the panel and tools work unchanged. A native
protocol can come later behind the panel's `KeyboardProtocol` interface.

## Milestones

1. **Boots** — signature-valid image runs; UART0 banner, `BE FC` handler early so every later build
   can return to the loader without the recovery keys.
2. **USB** — HS enumeration with the four interfaces of `hardware.md`; 8 kHz on the boot keyboard
   endpoint (`bInterval = 1`). Main technical risk.
3. **Scan** — PWM1 P4–P7 mux addressing, ADC0/ADC1 sequences on PB00/01/08–13, 88 slots at 8 kHz;
   `BE FD` stream so the panel's live view works; verify the slot→key table on hardware.
4. **HE engine** — baseline (not from held keys), calibration, travel, actuation, rapid trigger,
   deadzones, SOCD, DKS / key groups, profiles, config banks in NOR (range-checked writes).
5. **RGB** — 85 × WS2812 on SPI1 + DMA.

## Risks

- 8 kHz HS HID on `hpm-hal` is unproven — validate at milestone 2.
- A config-layout change would break the stock firmware's banks: keep the stock layout as long as we
  want to switch back and forth.

## References

`hpm-hal` https://github.com/hpmicro/hpm-hal · `hpm-metapac` · libhmk
https://github.com/peppapighs/libhmk · hpm_sdk TinyUF2 sample · `hpm_isp`
https://github.com/tfx2001/hpm_isp
