# firmware/emu — pre-flash gate

Runs **our** firmware (`firmware/app` ELF, or the packaged UF2) on an emulated HPM5361 — unicorn
(QEMU RV32) plus Python models of the parts of the chip the firmware touches — and fails if the
image could hurt the keyboard or would not boot. Run it before every `tools/flash_uf2.py`.

```sh
cd firmware && ./package-uf2.sh
uv run --with unicorn --with capstone --with pyelftools python emu/run.py            # release ELF
uv run --with unicorn --with capstone --with pyelftools python emu/run.py zent75-rs.uf2 \
    --backup ../backups/1795545170 --ms 2000 --banner 'ZENT75-HE'
```

Exit code: `0` pass, `1` at least one FAIL, `2` cannot load the image.

| Option | |
|---|---|
| `--ms N` | virtual run time (default 1200 ms) |
| `--backup DIR` | preload the config banks from a `tools/dump_all.py` backup |
| `--banner REGEX` | UART0 output must match (otherwise only a warning when silent) |
| `--strict-pinmux` | expected pads never muxed are failures, not warnings |
| `--reset-flags 0x10000 --expect-bootloader` | boot as if EWDG0 reset the chip; require a clean jump to the loader |
| `--enter-bootloader MS` | at MS force a call to the firmware's `enter_bootloader` and require a clean jump to `0x80020000` |
| `--press ADC0:1:3:100-200:0xC000` | ADC input override (adc, channel, mux step 0..10, time span, value); the HID state must change during the press |
| `--press SLOT:9:100-200:0xB96C` | same, by scan slot (`step × 8 + position`, interleave of `firmware/app/src/scan.rs`) |
| `--scan-budget N` | max average cycles per scan (default 60000 = 125 µs at 480 MHz) |
| `--adc-rest V` | ADC result of an idle input (default `0x8000`) |
| `--trace [FILE]` | instruction trace with symbols |
| `-v` | events and UART live; `--no-mmio` drops the register list |

Tests: `cd tools && uv run --with pytest --with unicorn --with capstone --with pyelftools python -m
pytest -q tests/test_emu.py` (hand-assembled images for every gate rule, Zb* emulation, the real
ELF when built; the module is skipped when unicorn is not installed).

## What the gate checks

| Check | FAIL when |
|---|---|
| boot | word 0 at `0x80040000` ≠ `0x0B4D5475`, or a load segment / UF2 block outside `0x80040000..0x8006FFFF` |
| idle | the CPU never executes `WFI` (embassy executor idle) within the run |
| traps | illegal instruction, CPU exception, unknown CSR, panic handler (`rust_begin_unwind`), default trap/IRQ handlers, a trap before `mtvec` is set, one block spinning 3 M times |
| memory | unmapped access, unaligned RAM access, access to an address that is no HPM5361 peripheral |
| **flash gate** | ROM NOR `erase`/`erase_sector`/`erase_block`/`program` (rounded to sectors/blocks) touching anything below `0x80070000`; `erase_chip`; an absolute (`0x8xxxxxxx`) address instead of an offset; any write to the XPI0 controller registers |
| XIP / ROM | a plain store to the flash window or to ROM; execution in `0x80000000..0x8003FFFF` other than a jump to the loader entry `0x80020000` |
| OTP | `program` / `lock` (irreversible) |
| stack | `sp` leaves the linker `.stack` region after entering it (peak use is reported) |
| watchdog | EWDG expires with `ot_rst_en` (feeds and the longest gap are reported) |
| scan budget | the firmware's own `scan::STATS` (mcycle around `Keyboard::scan`) average > `--scan-budget`; scans, drops, cycles/scan and ADC-ISR cost are reported |
| HID | with `--press`: `scan::LAST_GEN` (written when `Keyboard::hid_generation` moves) is not written while a press is held (+20 ms) |
| UART | `--banner` given and not seen |
| pin mux | IOC/PIOC `FUNC_CTL` of a board pad written with a different function than the table below; with `--strict-pinmux` also pads never written |
| loader jump | with `--enter-bootloader`/`--expect-bootloader`: no jump to `0x80020000`, or L1 caches still on, or a watchdog still running (MIE / PLIC enables / USB run bit left on are warnings) |

Warnings: pads never muxed, GPIO outputs driven (the board has no known GPIO outputs), BOOT/JTAG
pads re-muxed, poll loops the emulator had to break (with the register name), unmodelled ROM
calls, silent UART, an unexpected jump to the loader.

Expected pin mux (`pinmux.py`, from `docs/re-peripherals.md` §2): UART0 PA00/PA01 = 2; USB PA24/PA25
analog; SPI1_MOSI PA29 = 5; ADC PB00/PB01/PB08–PB13 analog; PY00–PY03 PIOC = 3 and IOC = 0x11
(PWM1_P4..P7, analog-mux address).

The report also lists UART0 output, every MMIO register written (decoded against hpm-metapac from
`~/.cargo/git/checkouts/hpm-metapac-*`), interrupts taken, ROM API calls and flash operations,
GPIO level changes and peripheral activity (ADC sequences, PWM periods, timer reloads, DMA).

## How it runs the firmware

- **Boot like stage-0**: the ELF's `PT_LOAD` segments go to their load (flash) addresses — the
  same bytes the UF2 carries — or the UF2 blocks via `tools/uf2.py`. `0x80000000..0x8003FFFF` is
  filled with the pattern `5a a5 b0 07`, the rest of flash is erased (`0xFF`), config banks are
  optionally preloaded (bank 1 at `0x80070000` from the active profile, profile k at
  `0x80080000 + k·0x2000`). Start at `0x80040004`.
- **CPU state at entry** (assumed; the stage-0 binary is not in the repo, TinyUF2's
  `board_app_jump` disables IRQs and both L1 caches): M-mode, `mstatus = MPP=M` (MIE=0, FS off),
  `mie = 0`, `mcache_ctl = 0`, `mmisc_ctl = 0`, `mtvec` → a sentinel that fails if a trap
  happens before the firmware sets it, `sp = 0x0009FF00`, other GPRs poisoned `0xBADC0DEx`,
  ILM/DLM/AHB SRAM filled with `0xCD`.
- **Memory**: ILM `0x0`/128 K, DLM `0x80000`/128 K, AHB SRAM `0xF0400000`/32 K, XIP
  `0x80000000`/1 M read+exec only, ROM `0x20000000`/128 K, MMIO (FGPIO `0xC0000`, `0xE0000000+`,
  `0xF0000000+`).
- **CPU extensions**: QEMU lacks Zba/Zbb/Zbc/Zbs and the Andes CSRs; they raise illegal
  instruction and are executed in `rvext.py`. `mcycle`/`cycle` reads (found by scanning the ELF's
  code) return virtual cycles. QEMU executes `WFI` as "stop": the main loop then advances virtual
  time to the next event.
- **Time**: 1 instruction = 1 cycle at 480 MHz; MCHTMR at 24 MHz derived from it. Busy-waits on a
  time source (mtime, mcycle, GPTMR cnt read hundreds of times in a row) are accelerated by
  jumping time forward, never past the next scheduled event.
- **Interrupts**: MCHTMR (`mtime`/`mtimecmp`, embassy-time's driver) and the PLIC (priority,
  enable, threshold, claim/complete; a claimed source is not re-delivered before its completion,
  and in preemptive mode the priorities in service raise the threshold). Andes vectored mode (`mmisc_ctl.VEC_PLIC`): the core
  auto-claims and jumps to `*(mtvec + 4·irq)`; core interrupts and exceptions go to entry 0.
  Taken at basic-block boundaries.
- **ROM API**: table at `0x2001FF00`; `xpi_nor_driver_if` (`+0x14`) and `otp_driver_if` (`+0x0C`)
  point to stub tables whose slots are Python handlers: `auto_config`/`get_property` (1 MB, 256 B
  page, 4 KB sector, 64 KB block), `erase*`/`program`/`read` against the emulated flash (NOR
  program = AND), OTP `read_from_shadow` returns a fixed fake UID.
- **Peripheral models** (`periph.py`, `scan.py`): UART0 (TX capture, LSR always empty), PLIC,
  MCHTMR, SYSCTL/PLLCTL (register files; every field named `*busy*` in metapac reads 0, so clock
  switches finish at once), USB0 (`usbcmd.rst` self-clears, prime/flush read 0 — it never sees a
  host), GPIO0/FGPIO/PGPIO (DO/OE set/clear/toggle, level changes logged), IOC/PIOC, PPOR
  (`reset_flag` W1C, software reset ends the run), EWDG0/1, HDMA, XPI0 (write = failure), ADC0/1
  (sequence mode + sequence DMA + `seq_cvc` IRQ 58/59), PWM0 (ADC hardware trigger at the even
  compare points while `cen`), PWM1, GPTMR0/1 (reload IRQ 5/6). Every other address inside a
  metapac peripheral is a plain register file with a **poll-breaker**: ~2000 reads of one register
  without a write inside a few ms flip the returned value and raise a warning naming the register.

## Adding a peripheral

Subclass `RegFile` in `periph.py` (or `scan.py`), override `read(off, size)` / `write(off, size,
value)` (offsets are word aligned, relative to the base), and add an instance to
`default_models()`. Use `self.m.schedule(cycle, fn)` for timed events, `self.m.plic.raise_irq(n)`
for interrupts, `self.m.fail/warn/event` for findings, `self.m.time_poll()` in time-source reads.

## Known modelling gaps

- Stage-0 hand-off state is assumed (see above), not measured; clock registers start at reset
  values, not at what stage-0 configured.
- Busy-wait acceleration applies only to time-source reads < 100 cycles apart.
- Timing is idealised: no XIP/cache latency, no interrupt latency, IPC 1. Real-time margins
  (scan deadlines, USB timing) are not validated.
- Interrupts: edge-style pending (a level source that stays asserted
  is not re-raised), external-interrupt `mcause` written as `0x8000000B`; Andes `mxstatus`/PFT and
  stack-protection CSRs are plain storage.
- Every misaligned RAM access is a failure, although the core may support some.
- `mcycle` virtualisation needs the ELF (scanned code); with a UF2 input QEMU returns host ticks.
- USB: registers only, no enumeration, no USB interrupts; HID/vendor protocol cannot be tested
  here (use `firmware/sim`).
- ADC: TRGM routing, conversion timing and the seq-DMA word layout (SDK, unverified) are
  approximations (conversion time from `conv_cfg1`); values come from `SensorSource`, not from a
  hall model; PWM CNT runs from virtual time; PWM1 (mux address) is a register file — the mux
  step is the trigger index inside the PWM0 period.
- Scan cost: IPC 1, no cache misses, so real hardware needs at least as many cycles as reported.
- The scan/HID probes read `zent75_rs::scan::STATS` (field order as declared) and
  `zent75_rs::scan::LAST_GEN`; renaming them in the app silently disables those checks.
- HDMA completes instantly without moving data; SPI1/WS2812 frames are not decoded.
- GPTMR clock assumed 24 MHz, EWDG bus clock 160 MHz.
- Pin mux checks `FUNC_CTL` only (no `PAD_CTL`); the expected table only covers pads the stock
  firmware uses.
- Config preload restores only the regions in `dump.json`; uncovered bytes stay `0xFF`.
