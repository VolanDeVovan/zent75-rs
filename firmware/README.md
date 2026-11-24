# zent75-rs — ZENT75-HE firmware (Track C skeleton)

Our own Rust firmware for the HOVEY WORKS ZENT75-HE hall-effect keyboard
(USB `36b0:357e`), targeting the HPM5300-family RISC-V MCU on `embassy` +
[`hpm-hal`](https://github.com/hpmicro/hpm-hal). This is the **C0 skeleton**:
it only proves the toolchain, the memory map and the embassy runtime build and
link for the real target. The hall scan, USB HID and the vendor protocol are
later milestones (see [`../docs/firmware-plan.md`](../docs/firmware-plan.md)).

> **Do not flash this image.** See [Flashing is gated](#flashing-is-gated).

## Status

`cargo build --release` compiles and links cleanly for
`riscv32imafc-unknown-none-elf`, producing an ELF whose first image word is the
TinyUF2 boot signature `0x0B4D5475` at `0x80040000` and whose entry is at
`0x80040004` — matching `vendor/0.0.18.uf2`. `./package-uf2.sh` turns it into a
UF2 with the same block layout as the vendor images.

## Build

The toolchain is pinned by [`rust-toolchain.toml`](rust-toolchain.toml)
(nightly + `rust-src` + the `riscv32imafc-unknown-none-elf` target).
`build-std=["core"]` is set in [`.cargo/config.toml`](.cargo/config.toml), so a
plain build works:

```sh
cd firmware
cargo build --release
# -> target/riscv32imafc-unknown-none-elf/release/zent75-rs  (ELF)
```

## Produce the UF2

```sh
./package-uf2.sh
# cargo build --release
#   -> llvm-objcopy -O binary  (flat .bin of the flash image)
#   -> uf2conv.py -f 0x0b4d5475 -b 0x80040000
#   -> firmware/zent75-rs.uf2
```

The script asserts that word 0 of the `.bin` is `0x0B4D5475` before packaging.
Equivalent manual steps:

```sh
cargo build --release
rust-objcopy -O binary \
    target/riscv32imafc-unknown-none-elf/release/zent75-rs zent75-rs.bin
python3 uf2conv.py zent75-rs.bin -b 0x80040000 -f 0x0b4d5475 -o zent75-rs.uf2
```

`uf2conv.py` is a small self-contained converter (the repo has no Microsoft
`uf2conv.py`); it emits the exact block layout the stock bootloader uses
(512-byte blocks, 256-byte payloads, flag `0x2000`, family id `0x0B4D5475`),
verified against `vendor/0.0.18.uf2`. The produced `zent75-rs.uf2` is a build
artifact and is git-ignored.

## Memory map

This board ships a **TinyUF2** bootloader; the layout differs from the stock
hpm-hal example (which links a boot header at `0x80000000` and the app right
after at `0x80003000`).

| Region | Address | Notes |
|---|---|---|
| Factory XPI boot header + TinyUF2 | `0x80000000 .. 0x8003FFFF` | **Never write** (see below). TinyUF2 lives at `0x80020000`; factory boot header at `0x80000000`. |
| **Our application image** | `0x80040000 ..` | `REGION_TEXT`. Word 0 = signature `0x0B4D5475`; entry at `0x80040004`. |
| ILM (instruction local mem) | `0x00000000`, 128K | fast text / vector table |
| DLM (data local mem) | `0x00080000`, 128K | `.data`/`.bss`/stack |
| AHB SRAM | `0xF0400000`, 32K | |

The app region length is `0x80100000 - 0x80040000 = 0xC0000` (768K), assuming a
1 MB NOR (flash end `0x80100000`). See [`memory.x`](memory.x).

### How word 0 becomes the boot signature

The stock bootloader validates that the first 32-bit word of the image at
`0x80040000` is `0x0B4D5475`, then jumps to `0x80040004`. hpm-riscv-rt's
`hpm-link.x` would otherwise place the reset handler first at
`_stext = ORIGIN(REGION_TEXT)`. [`link-app.x`](link-app.x) (loaded *before*
`hpm-link.x`) instead:

1. reassigns `_stext = ORIGIN(REGION_TEXT) + 4`, and
2. `INSERT`s a `.app_signature` output section at `ORIGIN(REGION_TEXT)` holding
   the signature word (`APP_BOOT_SIGNATURE` in `src/main.rs`, kept by `#[used]`
   + linker `KEEP`).

Result: `0x80040000` = `0x0B4D5475`, `0x80040004` = reset handler (ELF entry).
We deliberately emit **no** XPI NOR boot header of our own — the factory one at
`0x80000000` already exists and must stay untouched.

## Hard rule: never write `0x80000000 .. 0x8003FFFF`

That range holds the factory XPI boot header and the TinyUF2 bootloader — our
only recovery path. A broken application at `0x80040000` cannot brick the board
**as long as TinyUF2 survives**. Nothing we build or flash may ever touch that
range. `memory.x` places no section there, and `package-uf2.sh` only emits
blocks from `0x80040000` up.

## Flashing is gated

**Do not flash this firmware to the device yet.** `BE FC` (enter bootloader) is
handled by the *application*, so a broken custom app cannot re-enter the
bootloader on its own. Flashing custom firmware is allowed only after
[`../docs/bootloader.md`](../docs/bootloader.md) establishes a bootloader entry
that works **even if our app is broken** (TinyUF2 GPIO-at-reset / double-reset /
backup-register magic, or the MCU ROM ISP via BOOT pins / UART / SWD). This is
Risk 2 in [`../docs/firmware-plan.md`](../docs/firmware-plan.md). Until then,
`package-uf2.sh` builds and packages only; it never flashes.

## Layout

| Path | Purpose |
|---|---|
| `Cargo.toml` / `Cargo.lock` | deps; `hpm-hal` pinned by git rev, `hpm-riscv-rt` pinned to `=0.3.1` |
| `.cargo/config.toml` | target, `build-std`, rustflags, linker-script order |
| `rust-toolchain.toml` | nightly + `rust-src` + target |
| `memory.x` | memory map (app at `0x80040000`) |
| `link-app.x` | boot signature at word 0, entry at +4 |
| `build.rs` | linker-script rerun triggers |
| `src/main.rs` | minimal embassy app (GPIO toggle) + signature static |
| `uf2conv.py` | self-contained `.bin -> .uf2` converter |
| `package-uf2.sh` | build + objcopy + uf2conv |
| `vendor/*.uf2` | vendor firmware images (do not modify) |
