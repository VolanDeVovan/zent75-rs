# zent75-rs

Our Rust firmware for the ZENT75-HE (HPM5361), on `embassy` + [`hpm-hal`](https://github.com/hpmicro/hpm-hal).
Currently a skeleton: it builds, links at `0x80040000` with the boot signature and packages to UF2.
Plan and milestones: [`../docs/firmware-plan.md`](../docs/firmware-plan.md). Before flashing:
[`../docs/boot-recovery.md`](../docs/boot-recovery.md).

## Build and package

The toolchain is pinned by `rust-toolchain.toml` (nightly, `rust-src`, `riscv32imafc-unknown-none-elf`).

```sh
./package-uf2.sh     # cargo build --release → objcopy → ../tools/uf2.py pack → zent75-rs.uf2
```

The script checks that word 0 of the image is `0x0B4D5475` before packing (family `0x0B4D5475`,
base `0x80040000`). `zent75-rs.uf2` and `*.bin` are build artifacts (git-ignored).

## Flash

```sh
python3 ../tools/flash_uf2.py zent75-rs.uf2      # BE FC → UF2 drive → copy → wait for the app
```

`flash_uf2.py` refuses anything outside `0x80040000..0x8006FFFF`. If the build hangs, the owner holds
A+S+D while plugging in and `../tools/rollback.sh` restores stock 0.0.18.

## Memory map and boot signature

| Region | Address |
|---|---|
| boot header, stage-0, loader — never write | `0x80000000..0x8003FFFF` |
| application (`XPI0_APP` in `memory.x`) | `0x80040000..0x8006FFFF` |
| config banks (stock layout) | `0x80070000..` |
| ILM / DLM | `0x00000000` / `0x00080000`, 128 KB each |

Stage-0 checks that the first word at `0x80040000` is `0x0B4D5475` and jumps to `0x80040004`.
`link-app.x` (loaded before `hpm-link.x`) moves `_stext` to `ORIGIN(REGION_TEXT) + 4` and inserts an
`.app_signature` section holding `APP_BOOT_SIGNATURE` from `src/main.rs` at the origin. We emit no
XPI boot header of our own.

## Files

| Path | Purpose |
|---|---|
| `Cargo.toml` / `Cargo.lock` | deps; `hpm-hal` pinned by git rev, `hpm-riscv-rt` pinned to `=0.3.1` |
| `.cargo/config.toml` | target, `build-std`, rustflags, linker-script order |
| `rust-toolchain.toml` | nightly + `rust-src` + target |
| `memory.x` | memory map |
| `link-app.x` | boot signature at word 0, entry at +4 |
| `build.rs` | linker-script rerun triggers |
| `src/main.rs` | minimal embassy app + signature static |
| `package-uf2.sh` | build + objcopy + pack |
| `vendor/*.uf2` | original vendor images (0.0.18 on the unit, 0.0.9) — do not modify or publish |
