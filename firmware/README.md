# zent75-rs

Our Rust firmware for the ZENT75-HE (HPM5361), on `embassy` + [`hpm-hal`](https://github.com/hpmicro/hpm-hal).
Plan and milestones: [`../docs/firmware-plan.md`](../docs/firmware-plan.md). Before flashing:
[`../docs/boot-recovery.md`](../docs/boot-recovery.md). It runs on the real keyboard; what is
verified there and what is not: the checklist in the plan.

## Layout

| Dir | Crate / tool | What |
|---|---|---|
| `core/` | `zent-core` (`no_std`, `unsafe` forbidden, host-testable) | everything hardware-independent, see [Core](#core-coresrc) |
| `app/` | `zent75-rs` (RISC-V binary) | the **board layer**: clocks, scan, USB, RGB, flash, reset/bootloader, runs `Keyboard` |
| `sim/` | `zent-sim` (host) | simulator: sensor model + `Keyboard` behind a virtual uhid device (`36B0:5A11`), see `sim/README.md` |
| `emu/` | Python (unicorn) | **pre-flash gate**: runs our ELF on an emulated HPM5361 and fails on anything that could hurt the keyboard, see `emu/README.md` |
| `hpm-hal/` | `hpm-hal` (local copy) | upstream HAL at a pinned revision plus the USB and PLIC fixes this board needed, see `hpm-hal/PATCHES.md` |

## Build and package

The toolchain is pinned by `rust-toolchain.toml` (nightly, `rust-src`, `riscv32imafc-unknown-none-elf`).

```sh
./package-uf2.sh     # cargo build --release → size report → objcopy → ../tools/uf2.py pack → zent75-rs.uf2
cd core && cargo test --features std
cd app && cargo clippy --release
```

The release profile sets `trim-paths = "object"`: panic locations and debug info name
`/cargo/deps/zent-core-0.1.0/src/…` instead of absolute paths, so the image (and its DLM use)
does not depend on where the repo is checked out and does not carry the builder's home directory.

Cargo features of `app`: `profiler` (default; the statistical profiler, GPTMR0 + 8 KB of AHB
SRAM) and `key-log` (off; logs every active key object on each HID change, costs scan time).

`package-uf2.sh` checks the boot signature (word 0 = `0x0B4D5475`), that code did not leak into
XIP and that the image fits the 192 KB app region, and prints the ILM/DLM budget.
`zent75-rs.uf2` and `*.bin` are build artifacts (git-ignored).

## Flash

```sh
uv run --with unicorn --with capstone --with pyelftools python emu/run.py   # gate first
python3 ../tools/flash_uf2.py zent75-rs.uf2      # BE FC → UF2 drive → copy → wait for the app
```

`flash_uf2.py` refuses anything outside `0x80040000..0x8006FFFF`. If the build hangs, the owner holds
A+S+D while plugging in and `../tools/rollback.sh` restores stock 0.0.18.

## Memory map and boot

| Region | Address |
|---|---|
| boot header, stage-0, loader — never write | `0x80000000..0x8003FFFF` |
| application (`XPI0_APP` in `app/memory.x`) | `0x80040000..0x8006FFFF` |
| config banks (stock layout, written only through `core`'s `BankStore` → `app/src/flash.rs`) | `0x80070000..0x800FFFFF` |
| ILM: vector table + **all code** | `0x00000000`, 128 KB |
| DLM: data, **all rodata**, bss, 16 KB stack | `0x00080000`, 128 KB |
| AHB SRAM (not loaded, not zeroed): engine height tables 21.5 KB, profiler buffer 8 KB, crash record 0.5 KB | `0xF0400000`, 32 KB |

Stage-0 checks that the first word at `0x80040000` is `0x0B4D5475` and jumps to `0x80040004`.
`app/zent-link.x` (our copy of hpm-riscv-rt's `hpm-link.x`, which it replaces) puts the signature
at the origin, the reset stub at +4, and everything else in ILM/DLM, copied by the reset stub. The
CPU therefore never touches XIP after start-up, so NOR erase/program runs with interrupts on and the
8 kHz scan keeps going during config saves. The script asserts the XIP stub stays under 1 KB.

## Core (`core/src`)

| Module | Role |
|---|---|
| `device/` | `Keyboard`: the whole keyboard as one state machine (`mod.rs`: boot, `scan`, deferred work; `vendor.rs`: the configurator-protocol dispatcher). The only type the platforms drive |
| `config.rs`, `defaults.rs` | the two config banks as byte images in the stock layout, with typed accessors; **the only place with bank offsets**; stock validation/repair and factory images |
| `protocol.rs`, `crc.rs` | vendor frame parsing and building |
| `engine/` | ADC samples → filtered raw → baseline → curve heights → travel; `compat.rs` holds every deliberate deviation from the stock engine as a switch |
| `keys/` | actuation (normal, rapid trigger, mode 2), layers, tap-hold, SOCD, key groups / DKS, function codes → HID reports |
| `lighting/` | the stock RGB engine (effects, overlays, live preview) |
| `hid.rs`, `keycode.rs`, `layout.rs` | report formats + stock report descriptors, keymap codes, key/slot/LED tables |
| `storage/` | `Storage` trait (what `Keyboard` persists through), the flash write-range guard, and `nor::BankStore`: the stock NOR layout on a raw `Nor` device with RAM staging, debounced commits and read-back verification |

Layering: `app` and `sim` depend on `core`, never the other way round; `core` knows no hardware
and no clock (time comes in as scan counts or as arguments). The config byte layout is the
interface shared with the panel, `tools/` and the stock firmware, so it is kept as-is and
wrapped, not converted into typed structs.

Tests (`core/tests`): `engine_diff` / `engine_fixed` replay traces of the emulated stock firmware
(bit-exact with `Compat::STOCK`, equivalent-or-documented with the fixes), `lighting` compares
frames with stock renders, `config` compares factory images and repairs, `dispatcher` /
`device` / `keys_device` drive `Keyboard` through its public API; `storage::nor` has unit tests
on a fake NOR, including the write gate.

## Board layer (`app/src`)

| File | Role |
|---|---|
| `main.rs` | clocks (CPU 480 MHz, AHB 160 MHz), init order, executor tasks: console, service (+ debounced flash commits), RGB 100 Hz, health (watchdog, `BE FD` tail, statistics every 5 s) |
| `kb.rs` | the one `Keyboard` (link-time static) behind the `ScanLock` mutex; the engine's height tables in AHB SRAM |
| `plic.rs` | interrupt priorities, `ScanLock` (raises the PLIC threshold: masks scan processing, never the ADC capture) |
| `scan.rs` | PWM1 mux address, PWM0 + TRGM0 ADC triggers, ADC0/ADC1 sequences + DMA, ADC0 ISR per step (prio 7), GPTMR1 frame processing → `Keyboard::scan` (prio 2, preemptible) |
| `flash.rs` | `RomNor`: the raw NOR for the core's `BankStore` (erase/program through hpm-hal's ROM-API driver, re-checked against `check_write_range`; reads from XIP); OTP UID |
| `usb.rs` | embassy-usb HS device `36B0:357E` with the stock descriptors, one task per IN endpoint; vendor reports go to `debug::vendor_command`, then to `Keyboard::vendor_in` |
| `rgb.rs` | SPI1 (12 MHz) + HDMA WS2812 stream, non-blocking |
| `system.rs` | `BE FC` → stock loader sequence (quiesce, PLIC cleanup, caches off, jump `0x80020000`); `BE FF` → PPOR software reset; panic → crash record + loader; EWDG0 watchdog (2 s), watchdog reset → loader on the next boot |
| `console.rs` | `log!`: UART0 PA00/PA01 115 200 8N1 (synchronous during boot, then a ring drained by a task) and a 4 KB history for the USB console |
| `debug/` | everything stock does not have: debug vendor commands (`mod.rs`), crash record + stall detector (`crash.rs`), statistical profiler (`prof.rs`) — see [Debugging](#debugging) |

Interrupts: ADC0 `seq_cvc` prio 7 (capture), GPTMR0 prio 5 (profiler sample, only during a
capture), USB0 prio 3, GPTMR1 prio 2 (processing, re-enables MIE), MCHTMR (embassy time). The
PLIC runs vectored + preemptive like stock.

**Late ADC steps.** Under load the capture ISR can run after the next mux step's first
conversion has landed in the DMA buffer; such a torn buffer mixes two steps and looked like a
full key press on hardware. `scan.rs` drops a step captured later than `LATEST_TICKS` after its
trigger (`late` on the console) and the frame keeps the samples that step had two scans ago.

### `unsafe` in the board layer

State shared between contexts is atomics (scan buffers, statistics, AHB-SRAM records),
`critical-section` cells (console, signals) or the `ScanLock` mutex (keyboard); peripherals are
driven through the PAC's safe register API. What remains, each with a `// SAFETY:` comment:

| Where | Why it cannot be safe |
|---|---|
| `main.rs`, `kb.rs`, `debug/crash.rs`, `debug/prof.rs` `link_section` | the boot signature must be word 0 of the image; three statics live in AHB SRAM |
| `kb.rs` `attach_height_tables` | the one `&'static mut` to the uninitialised 21 KB table storage (zeroed in place, too big for the stack) |
| `plic.rs` `unsafe impl RawMutex for ScanLock` | a lock made of a PLIC threshold; the trait is `unsafe` to implement |
| `plic.rs` `irq.enable()` | unmasking an interrupt is `unsafe` in hpm-hal |
| `scan.rs` `riscv::interrupt::enable()` | re-enabling MIE inside the processing handler (nested interrupts) |
| `flash.rs` XIP read, D-cache flush, OTP ROM call | raw memory-mapped flash, an Andes CSR, a mask-ROM function |
| `system.rs` jump to the loader | `asm!`, leaves the program |

## Debugging

All of this is ours; the stock firmware has none of it and never sees the commands (the tools
read the firmware version first — ours reports 1.x, stock 0.0.x — in `zentlib.Zent.debug_request`).

| Tool | What |
|---|---|
| `tools/console.py [-f]` | the console over USB: the last 4 KB of log, optionally following |
| `tools/profile.py` | statistical CPU profile: share of 10 kHz PC samples per function of the ELF (feature `profiler`) |
| crash record | a panic message with its location, or — when the watchdog has not been fed for 1.25 s — where the firmware is stuck (the pc of thread mode, the pc below the ADC interrupt, and whether scans still ran: tells a spinning task from an interrupt that keeps firing), is kept in AHB SRAM and logged as `previous run: …` by the next start of the app. Panic and watchdog reset both end in the UF2 loader; the record survives that as long as the board stays powered, so flash again (or let the loader start the app) **without replugging**, then read `tools/console.py` |
| `cargo build --release --features key-log` | log the HID report and every active key object (thresholds, travel, raw) on each HID change |

Debug commands on the vendor interface (`app/src/debug/mod.rs`): request
`BE <cmd> 0D 00 "ZDBG" <arg LE32> ED`, reply `BE <cmd> <len LE16> <a LE32> <b LE32> <payload> ED`
with `D0` console read, `D1` profiler read, `D2` profiler start + read. Stock 0.0.18 does not use
these command bytes and drops unknown commands; the signature and fixed length keep a debug frame
from being taken for anything else, and whatever is not exactly a debug request goes to the core's
stock dispatcher unchanged.

## Console output

Boot banner, `previous run: …` if the last run left a crash record, clocks, reset flags, flash
init result, UID, config load result, `usb: bus reset` / `usb: configured, high speed`, a `flash: … saved` line
after config writes, and every 5 s:

```
scan 8000/s avg 73.0 max 98 us | adc isr max 420 cyc | cpu 68% | seq 1502..1688 ticks | drop 0 miss 0 late 0 badch 0 cflct 0 adc1late 0 logdrop 0
```

`avg`/`max`: `Keyboard::scan` time per frame; `cpu`: share of the 125 µs period spent in scan
processing plus the 11 ADC captures; `seq`: measured trigger → capture-ISR time in PWM ticks,
used to locate the mux step (`SEQ_DONE_TICKS` in `scan.rs`); then counters since boot: frames
dropped, frames with a missing step, steps captured too late, DMA buffers ending on the wrong
channel, ADC trigger conflicts, ADC1 not finished in time, log lines that did not fit the UART
ring.
