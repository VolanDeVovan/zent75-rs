# Track C — our own firmware: plan and go/no-go

Status: **GO for Rust**, with staged milestones and two hardware-gated risks called out below.
Written 2026-11-24 from the Track A reverse-engineering findings and the prior-art survey. The
exact MCU part (HPM5301 vs HPM5361 vs HPM53M1) is being finalized in `docs/hardware.md`; the plan
holds for any HPM5300-family part and notes where the choice matters.

## Decision

Write our firmware in **Rust**, on `embassy` + **`hpm-hal`** (the official HPMicro embedded-Rust
HAL), targeting `riscv32imafc-unknown-none-elf` (nightly). Use the GPL-3.0 C firmware **libhmk** as
an *algorithm reference only* (rapid trigger, continuous RT, SOCD/Rappy-Snappy, DKS, auto-calibration,
flash-emulated EEPROM) and re-implement in Rust so we are not bound by GPL. Our firmware will speak a
protocol the panel can drive (see "Protocol" below).

Why Rust is viable here (evidence):
- `hpm-hal` (github.com/hpmicro/hpm-hal, MIT/Apache-2.0, official org) lists HPM5300 as its
  best-supported family: embassy async, SYSCTL, GPIO/UART/SPI/I2C/DMA, **ADC16** (one-shot, periodic,
  sequence, preemption — exactly what a hall scan needs), **USB high-speed by default** (embassy-usb
  driver, 16 endpoints on HPM53), and XPI NOR flash via `embedded-storage`. Cargo features
  `hpm5301`/`hpm5321`/`hpm5331`/`hpm5361` plus `usb-pin-reuse-hpm5300` for the PA24/PA25 USB pins on
  small packages.
- Supporting crates exist: `hpm-metapac` (PAC from hpm-data), `hpm-riscv-rt`, `andes-riscv`.
- The toolchain is installed in this repo's environment: `rustup` nightly + target
  `riscv32imafc-unknown-none-elf` (and `riscv32imac` as a fallback).
- The boot/flash story is fully understood (Track A): TinyUF2 at `0x80020000`, app at `0x80040000`
  with boot-signature word0 = `0x0b4d5475` and entry at `0x80040004`. We can package our build as a
  UF2 the stock bootloader accepts.

Why not C: libhmk would be the fastest path (it already has the HE algorithms) but it has **no
HPMicro/RISC-V HAL** — a port needs a new ADC/flash/timer/USB HAL anyway, so the C-vs-Rust effort gap
is small, and Rust gives us memory safety and a cleaner shared protocol/codec story with the panel.
Choose C **only** if `hpm-hal` turns out unable to hit 8 kHz HS HID in practice (see Risk 1).

## Target packaging (so the stock TinyUF2 flashes our build)

- Linker: place `.start`/vector at `0x80040000`; **word 0 must be the signature `0x0B4D5475`**, entry
  at `0x80040004` (mirror the hpm_sdk `flash_uf2` linker script, app base moved to 0x80040000).
- Never emit anything into `0x80000000..0x8003FFFF` (boot header + TinyUF2). As long as that region is
  untouched, a broken app cannot brick the board — TinyUF2 survives.
- Convert the ELF/bin to UF2 with `uf2conv.py -f 0x0b4d5475 -b 0x80040000` and flash it with our
  existing `tools/flash_uf2.py` (proven end-to-end on the stock images).

## Protocol

Our firmware should talk a protocol the panel already speaks. Two options, in order of preference:
1. **Re-implement the vendor raw-HID framing** (`BE <cmd> <len LE16> … <crc16/modbus> ED`, 1024-byte
   report, usage page 0xFF60) so the panel's existing "zent" codec drives both firmwares unchanged.
   This is the least-work, highest-compat choice and the panel is already built around it.
2. Define a cleaner native protocol behind the panel's `KeyboardProtocol` interface (the panel is
   structured for a second transport/codec). Do this only if we want features the vendor framing
   can't express.
Recommendation: ship (1) first (drop-in), add (2) later if needed.

## Milestones (each ends at a green build)

- **C0 — skeleton compiles**: `firmware/` Cargo project (name e.g. `zent75-rs`), `.cargo/config.toml`
  (target, runner), `rust-toolchain.toml` (nightly), `memory.x`/linker at 0x80040000 with the
  signature, `hpm-hal` + `embassy` deps pinned to working git revs, a minimal `main` that boots and
  toggles a GPIO / writes a UART banner. `cargo build --release` produces an ELF for the target.
- **C1 — UF2 boots on hardware**: pack to UF2, flash via `tools/flash_uf2.py`, confirm it runs
  (UART/LED). *Gated on Risk 2 (app-independent recovery) being resolved first — do not flash custom
  firmware until `docs/bootloader.md` has a recovery path that works even if our app is broken.*
- **C2 — USB HS HID enumerates**: vendor interface (0xFF60) + boot keyboard; prove 8 kHz (bInterval=1
  on an HS interrupt endpoint). This is the main technical risk.
- **C3 — hall scan**: ADC16 + the analog-mux scan order from `docs/hardware.md`, producing the 83
  per-key readings; expose the `BE FD` live stream so the panel's travel view works.
- **C4 — HE engine**: travel mapping (top/bot calibration), actuation, rapid trigger, deadzones,
  SOCD, DKS/key groups, profiles, flash save — ported from libhmk's algorithms, driven by the panel.
- **C5 — RGB**: once the LED driver chip is identified (`docs/hardware.md` / open-questions).

## Risks / gates

- **Risk 1 — 8 kHz HS HID on `hpm-hal` is unproven by us.** embassy-usb negotiates HS, but a verified
  1-ms-microframe (bInterval=1) HID example for HPM5300 was not found. Validate at C2; if it can't
  hit 8 kHz, fall back to C (hpm_sdk CherryUSB/TinyUSB samples do 8 kHz) while keeping the Rust app
  logic where possible.
- **Risk 2 — app-independent bootloader entry. PARTIALLY RESOLVED (2026-11-24).** `BE FC` is handled
  by the *application*; a broken custom app can't answer it. Device experiment
  (`tools/confirm_sig_recovery.py`) **confirmed** that TinyUF2 refuses an app whose word0 ≠
  `0x0b4d5475` and stays in the UF2 drive on every reset — an app-independent recovery that needs no
  hardware. So a **signature-less build can always be recovered**: safe to flash freely during
  development. What is NOT yet resolved is recovering a **signature-valid app that hangs** (it boots,
  then can't answer `BE FC`); that needs the TinyUF2 reset GPIO/double-tap trigger or ROM ISP
  (BOOT/UART0/JTAG), which require opening the case (`docs/open-questions.md`). **Gate:** flash a
  signature-*valid* custom build only after that hardware trigger is confirmed; until then keep every
  custom flash signature-recoverable. The stock-firmware rollback via `tools/rollback.sh` is proven.
- **Risk 3 — exact MCU part & RGB driver.** The PAC feature (`hpm5301` vs `hpm5361`) and the RGB
  driver chip must be confirmed from `docs/hardware.md` before C0 deps and C5 are finalized.

## Open-source references

- `hpm-hal` https://github.com/hpmicro/hpm-hal · `hpm-metapac`/`hpm-data` · `hpm-riscv-rt`
- `libhmk` https://github.com/peppapighs/libhmk (GPL-3.0; algorithms only) + config https://github.com/peppapighs/hmkconf
- hpm_sdk TinyUF2 sample (bootloader fork origin) https://github.com/hpmicro/hpm_sdk/tree/main/samples/tinyuf2
- ISP/recovery: `hpm_isp` https://github.com/tfx2001/hpm_isp · HPM Manufacturing Tool · hpmicro/riscv-openocd · probe-rs
