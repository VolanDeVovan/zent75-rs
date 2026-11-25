# Our firmware — plan

Rust on **embassy + `hpm-hal`** (feature `hpm5361`), target `riscv32imafc-unknown-none-elf`
(nightly). The project lives in `firmware/` (layout and module map: `firmware/README.md`):

| Part | Status |
|---|---|
| `core/` — `zent-core`: config banks, engine, keys, lighting, protocol; `Keyboard` API in `device/` | complete; diff-tested against the emulated stock firmware |
| `app/` — board layer running `Keyboard` on the HPM5361 | **runs on the real keyboard**; hardware status in the checklist below |
| `sim/` — host simulator (uhid) | runs `Keyboard` with a sensor model |
| `emu/` — unicorn pre-flash gate for our ELF | run before every flash |
| `hpm-hal/` — local copy of the HAL with the USB and PLIC fixes found on the board (`PATCHES.md`) | candidates for upstream |

## Why Rust

`hpm-hal` (official HPMicro org) supports HPM5300 best: ADC16 sequences, USB HS via embassy-usb,
SPI/DMA, XPI NOR through `embedded-storage`. **libhmk** (GPL-3.0, C) is the algorithm reference
only (rapid trigger, SOCD, DKS, auto-calibration) — reimplement, do not copy. Fall back to C
(hpm_sdk + CherryUSB) only if 8 kHz HS HID proves impossible on `hpm-hal` — it did not: with the
fixes in `firmware/hpm-hal/PATCHES.md` the device enumerates at high speed and scans at 8 kHz.

## Packaging and flashing

- Linker (`app/zent-link.x`): word 0 at `0x80040000` = `0x0B4D5475`, entry `0x80040004`;
  nothing below `0x80040000`; all code in ILM, all rodata in DLM (flash writes never race XIP).
- `firmware/package-uf2.sh` → `firmware/zent75-rs.uf2` (family `0x0B4D5475`).
- Flash with `tools/flash_uf2.py` (refuses images outside the app region). Recovery and rules:
  `boot-recovery.md` — a hung build is recovered by holding A+S+D at plug-in.

## Protocol

Implement the vendor framing first (`protocol.md`) so the panel and tools work unchanged. A native
protocol can come later behind the panel's `KeyboardProtocol` interface.

## Milestones

All five are implemented; what has been seen working on the keyboard is in
[Hardware status](#hardware-status).

1. **Boots** — signature-valid image runs; console banner, `BE FC` so every build can return to
   the loader without the recovery keys. *On hardware.*
2. **USB** — HS enumeration with the four interfaces of `hardware.md`; 8 kHz on the boot keyboard
   endpoint (`bInterval = 1`). *On hardware.*
3. **Scan** — PWM1 P4–P7 mux addressing, ADC0/ADC1 sequences on PB00/01/08–13, 88 slots at 8 kHz;
   `BE FD` stream so the panel's live view works. *On hardware; the slot→key table still to be
   checked key by key.*
4. **HE engine** — baseline (not from held keys), calibration, travel, actuation, rapid trigger,
   deadzones, SOCD, DKS / key groups, profiles, config banks in NOR (range-checked writes).
   *Bit-exact against stock in tests; typing and config persistence on hardware open.*
5. **RGB** — 85 × WS2812 on SPI1 + DMA. *Colours and order on hardware open.*

## Risks

- CPU budget: a scan takes 73 µs of the 125 µs period on hardware (CPU ~68 %). Changes to the
  engine or key pipeline must be measured (`tools/profile.py`, the console statistics line).
- DLM is nearly full (about 111 KB of data + bss and the 16 KB stack in 128 KB; the linker
  script asserts the stack still fits). Big tables go to AHB SRAM.
- A config-layout change would break the stock firmware's banks: keep the stock layout as long as we
  want to switch back and forth.

## References

`hpm-hal` https://github.com/hpmicro/hpm-hal · `hpm-metapac` · libhmk
https://github.com/peppapighs/libhmk · hpm_sdk TinyUF2 sample · `hpm_isp`
https://github.com/tfx2001/hpm_isp

## Hardware status

Every step on the real keyboard only after `emu/run.py` passes, with a fresh `tools/dump_all.py`
backup, and with the owner available for A+S+D. The console is read over USB with
`tools/console.py` (or UART0 on PA00, 115 200, if the case is open).

### Verified on the keyboard

| What | How it shows |
|---|---|
| Boot | banner, clocks, `flash: 1 MB NOR ready`, UID on the console |
| Stock config banks load unchanged | `config: … defaults written for empty slots: false` |
| USB high speed, four interfaces | `lsusb -v`: `36B0:357E`, bInterval 1; `usb: configured, high speed` |
| Vendor protocol | `tools/` reads (`BE 04`, `BE 1D`) match the backup; the panel connects |
| `BE FC` | lands in the UF2 loader and the drive stays up (EWDG0 is stopped before the jump); `tools/flash_uf2.py` and `tools/rollback.sh` rely on it |
| Scan at 8 kHz | `scan 8000/s`, avg 73 µs per scan, CPU ~68 % |
| `BE FD` stream | live view in the panel |
| Debug console, crash record, profiler over USB | `tools/console.py`, `tools/profile.py` |

Found and fixed during bring-up: the hpm-hal bugs in `firmware/hpm-hal/PATCHES.md` (USB
completion clearing, endpoint enable, VBUS events without a sense pin, PLIC priority index and
completion), and ADC steps captured too late (`scan.rs`, `LATEST_TICKS`: a torn DMA buffer read
as a key press; such steps are now dropped and counted as `late`).

### Host reboot → UF2 loader (open until confirmed on the keyboard)

With an earlier build on the board, rebooting the host PC left the keyboard in the UF2 loader.
Only a panic or a watchdog reset leads there, and the record of it was lost (the board went back
to stock). The USB driver had two ways to stop thread mode from feeding the watchdog, both
reproduced with the emulator's scripted host (`emu/run.py --usb-host reboot` / `vbus-glitch`)
on that build and fixed in `firmware/hpm-hal` (`PATCHES.md`, "USB robustness"):

1. **Interrupt storm on a session-valid change.** The USB interrupt handler returned before it
   cleared OTGSC.ASVIS when no other USB interrupt was pending. A VBUS dip that the board
   survives while the host is silent (a reboot) then re-enters the handler forever; scan
   processing and thread mode starve, the watchdog resets after 2 s. Most likely the cause: it
   needs nothing but the glitch.
2. **Spin on an abandoned control transfer.** A new SETUP that arrives before the next stage
   of the previous request is primed made ep0's `transfer` wait forever for a flag that only
   the same task clears.

Next to these, bus resets after the first one never reached embassy-usb and control stages
waited without a way out, so a re-enumeration could leave the device mute (not in the loader).

To confirm on the keyboard: reboot the host a few times with the console history at hand. The
keyboard must come back each time; `usb: bus reset` / `usb: configured` lines show the
re-enumerations. If it lands in the loader again, flash the same UF2 **without replugging** and
read `tools/console.py`: the first lines must show `previous run: …` with the panic location or
the stall pcs.

### Not verified yet (in order of risk)

1. **Slot → key map and typing end to end**: every key in the panel's live view and in a text
   editor; NKRO report, modifiers, consumer keys, Caps LED (`SET_REPORT`), tap-hold, SOCD, DKS.
   If every key lights its neighbour one mux step away, change `scan::STEP_OFFSET`. Building
   with `--features key-log` logs the key objects on every HID change.
2. **Scan health under load**: `drop`/`miss`/`badch`/`adc1late`/`cflct` stay 0 while typing with
   the panel streaming; `late` may count up slowly (dropped steps), `seq` stays inside
   641..2459 ticks (else adjust `SEQ_DONE_TICKS`).
3. **Config write + persist**: change one value in the panel + `BE 02`; the console must show
   `flash: 1 slot(s) saved, 0 failed, 0 refused since boot`; dump + `tools/restore.py` verify;
   replug; the value persists. Then roll back with `tools/restore.py` and check stock 0.0.18
   still reads the banks.
4. **RGB**: colours and GRB order, no flicker (SPI1 at 12 MHz assumed from a 24 MHz source;
   `rgb.rs` documents the alternative write-only SPI mode), effects against stock.
5. **`BE FF`** reboot (PPOR software reset through stage-0).
6. **Suspend / remote wakeup**: LEDs off on host suspend, a key wakes the host.
7. **Watchdog reset path**: `PPOR_FLAG_EWDG0` (bit 16) is taken from hpm_sdk, not measured. Only
   a deliberate hang build can confirm it (the boot after the reset must go to the loader, and
   the next start must log `previous run: thread mode stalled at pc …`). The reset flags are
   logged at every boot.
8. Boot-protocol keyboard in a BIOS.
