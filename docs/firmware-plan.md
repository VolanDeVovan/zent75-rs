# Our firmware — plan

Rust on **embassy + `hpm-hal`** (feature `hpm5361`), target `riscv32imafc-unknown-none-elf`
(nightly). The project lives in `firmware/` (layout and module map: `firmware/README.md`):

| Part | Status |
|---|---|
| `core/` — `zent-core`: config banks, engine, keys, lighting, protocol; `Keyboard` API in `device.rs` | engine / keys / lighting being rewritten from `algorithms.md` |
| `app/` — board layer running `Keyboard` on the HPM5361 | complete first version, **not flashed**; needs the bring-up checklist below |
| `sim/` — host simulator (uhid) | runs `Keyboard` with a sensor model |
| `emu/` — unicorn pre-flash gate for our ELF | run before every flash |

Nothing custom has been flashed yet.

## Why Rust

`hpm-hal` (official HPMicro org) supports HPM5300 best: ADC16 sequences, USB HS via embassy-usb,
SPI/DMA, XPI NOR through `embedded-storage`. **libhmk** (GPL-3.0, C) is the algorithm reference
only (rapid trigger, SOCD, DKS, auto-calibration) — reimplement, do not copy. Fall back to C
(hpm_sdk + CherryUSB) only if 8 kHz HS HID proves impossible on `hpm-hal`.

## Packaging and flashing

- Linker (`app/zent-link.x`): word 0 at `0x80040000` = `0x0B4D5475`, entry `0x80040004`;
  nothing below `0x80040000`; all code in ILM, all rodata in DLM (flash writes never race XIP).
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

## Board-layer bring-up checklist (in order of risk)

Each step on the real keyboard only after `emu/run.py` passes, with a fresh `tools/dump_all.py`
backup, and with the owner available for A+S+D. Watch UART0 (PA00, 115 200) if the case is open.

1. **Boot + console + `BE FC`.** Banner, reset flags, flash geometry, UID, "config: … defaults
   written for empty slots: false" (the stock banks must load as-is — if `true`, stop: the empty
   rule in `flash.rs` rejected a stock bank). Then `BE FC` must land in the UF2 loader and the drive
   must **stay** up > 10 s (proves EWDG0 is stopped before the jump; else set
   `system::WATCHDOG_ENABLED = false`).
2. **USB enumeration at high speed** (`lsusb -v`: 36B0:357E, four interfaces, bInterval 1, HS),
   hidraw nodes, `BE 1D`/`BE 04` reads via `tools/` match the backup.
3. **Scan sanity**: console `scan 8000/s`, `drop`/`miss`/`badch`/`adc1late` stay 0, `seq` min/max
   inside 641..2459 ticks (else adjust `SEQ_DONE_TICKS`), `cflct` (ADC trigger while busy) 0, CPU %.
   `BE FD` stream in the panel shows ~30000 at rest per key.
4. **Slot → key map**: press keys one by one in the panel's live view. If every key lights its
   neighbour one mux step away, change `scan::STEP_OFFSET` (the open one-step question).
5. **Typing**: NKRO report, modifiers, consumer keys, Caps LED (`SET_REPORT`).
6. **Config write**: change one value in the panel + `BE 02`; console must show
   `flash: saved …`; dump + `tools/restore.py` verify; replug; value persists. Then roll back with
   `tools/restore.py` and check stock 0.0.18 still reads the banks (`tools/rollback.sh` dry run).
7. **RGB**: colours and GRB order correct, no flicker (SPI1 at 12 MHz assumed from a 24 MHz
   source; `rgb.rs` documents the alternative write-only SPI mode).
8. **Watchdog**: confirm `PPOR_FLAG_EWDG0` (bit 16) by a deliberate hang build only if needed.
9. **`BE FF`** reboot, remote wakeup from host suspend, boot-protocol keyboard in a BIOS.
