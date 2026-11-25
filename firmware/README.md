# zent75-rs

Our Rust firmware for the ZENT75-HE (HPM5361), on `embassy` + [`hpm-hal`](https://github.com/hpmicro/hpm-hal).
Plan and milestones: [`../docs/firmware-plan.md`](../docs/firmware-plan.md). Before flashing:
[`../docs/boot-recovery.md`](../docs/boot-recovery.md). Nothing of ours has been flashed yet.

## Layout

| Dir | Crate / tool | What |
|---|---|---|
| `core/` | `zent-core` (`no_std`, `unsafe` forbidden, host-testable) | everything hardware-independent, see [Core](#core-coresrc) |
| `app/` | `zent75-rs` (RISC-V binary) | the **board layer**: clocks, scan, USB, RGB, flash, reset/bootloader, runs `Keyboard` |
| `sim/` | `zent-sim` (host) | simulator: sensor model + `Keyboard` behind a virtual uhid device (`36B0:5A11`), see `sim/README.md` |
| `emu/` | Python (unicorn) | **pre-flash gate**: runs our ELF on an emulated HPM5361 and fails on anything that could hurt the keyboard, see `emu/README.md` |

## Build and package

The toolchain is pinned by `rust-toolchain.toml` (nightly, `rust-src`, `riscv32imafc-unknown-none-elf`).

```sh
./package-uf2.sh     # cargo build --release → size report → objcopy → ../tools/uf2.py pack → zent75-rs.uf2
cd core && cargo test
cd app && cargo clippy --release
```

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
| DLM: data, **all rodata**, bss, stack | `0x00080000`, 128 KB |

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
| `main.rs` | clocks (CPU 480 MHz, AHB 160 MHz), init order, executor tasks: console, service (+ debounced flash commits), RGB 100 Hz, health (watchdog + stats every 5 s) |
| `kb.rs` | the one `Keyboard` (link-time static) behind the `ScanLock` mutex |
| `plic.rs` | interrupt priorities, the hpm-hal PLIC workarounds, `ScanLock` (raises the PLIC threshold: masks scan processing, never the ADC capture) |
| `scan.rs` | PWM1 mux address, PWM0 + TRGM0 ADC triggers, ADC0/ADC1 sequences + DMA, ADC0 ISR per step (prio 7), GPTMR1 frame processing → `Keyboard::scan` (prio 2, preemptible) |
| `flash.rs` | `RomNor`: the raw NOR for the core's `BankStore` (erase/program through hpm-hal's ROM-API driver, re-checked against `check_write_range`; reads from XIP); OTP UID |
| `usb.rs` | embassy-usb HS device `36B0:357E` with the stock descriptors, one task per IN endpoint, vendor protocol |
| `rgb.rs` | SPI1 (12 MHz) + HDMA WS2812 stream, non-blocking |
| `system.rs` | `BE FC` → stock loader sequence (quiesce, PLIC cleanup, caches off, jump `0x80020000`); `BE FF` → PPOR software reset; panic → log + loader; EWDG0 watchdog (2 s), watchdog reset → loader on the next boot |
| `console.rs` | UART0 PA00/PA01 115 200 8N1; synchronous during boot, then a ring drained by a task |

Interrupts: ADC0 `seq_cvc` prio 7 (capture), USB0 prio 3, GPTMR1 prio 2 (processing, re-enables
MIE), MCHTMR (embassy time). The PLIC runs vectored + preemptive like stock. Two hpm-hal quirks
are worked around in `plic.rs`: `InterruptExt::set_priority` programs the *next* source's register
(metapac `PLIC.priority(n)` is source n+1), and `InterruptExt::complete` reads the claim register
(which claims a pending higher-priority source); `plic::set_priority` / `plic::complete` do it
right, and USB0 is bound by hand for the same reason.

### `unsafe` in the board layer

State shared between contexts is atomics (scan buffers, statistics), `critical-section` cells
(console, signals) or the `ScanLock` mutex (keyboard); peripherals are driven through the PAC's
safe register API. What remains, each with a `// SAFETY:` comment:

| Where | Why it cannot be safe |
|---|---|
| `main.rs` `link_section` | the boot signature must be word 0 of the image |
| `plic.rs` `unsafe impl RawMutex for ScanLock` | a lock made of a PLIC threshold; the trait is `unsafe` to implement |
| `plic.rs` `irq.enable()` | unmasking an interrupt is `unsafe` in hpm-hal |
| `scan.rs` `riscv::interrupt::enable()` | re-enabling MIE inside the processing handler (nested interrupts) |
| `usb.rs` `unsafe impl Binding`, `on_interrupt()` | hand-written USB0 vector instead of `bind_interrupts!` |
| `flash.rs` XIP read, D-cache flush, OTP ROM call | raw memory-mapped flash, an Andes CSR, a mask-ROM function |
| `system.rs` jump to the loader | `asm!`, leaves the program |

## Console output

Boot banner, reset flags, flash init result, UID, config load result, a `flash: … saved` line
after config writes, and every 5 s:
`scan <rate>/s avg/max µs per scan, adc isr max cycles, cpu % | seq <min>..<max> ticks | drop miss
badch cflct adc1late`. `seq` is the measured trigger → sequence-complete time used to locate the
mux step (`SEQ_DONE_TICKS` in `scan.rs`).
