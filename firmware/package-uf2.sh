#!/usr/bin/env bash
# Build the ZENT75-HE firmware and package it as a UF2 the stock TinyUF2
# bootloader accepts.
#
#   cargo build --release      -> ELF for riscv32imafc
#   llvm-objcopy -O binary      -> flat .bin of the flash image (app @ 0x80040000)
#   tools/uf2.py pack           -> firmware/zent75-rs.uf2
#                                  (family 0x0B4D5475, base 0x80040000)
#
# This script builds and packages ONLY. It never flashes; tools/flash_uf2.py
# does that (see firmware/README.md).
set -euo pipefail

cd "$(dirname "$0")/app"

TARGET="riscv32imafc-unknown-none-elf"
NAME="zent75-rs"
ELF="target/${TARGET}/release/${NAME}"
BIN="target/${TARGET}/release/${NAME}.bin"
UF2="../${NAME}.uf2"
BASE="0x80040000"
FAMILY="0x0b4d5475"

echo ">> cargo build --release"
cargo build --release

# Locate an objcopy that understands riscv ELF. Prefer rust's llvm-objcopy.
find_objcopy() {
    if command -v rust-objcopy >/dev/null 2>&1; then echo "rust-objcopy"; return; fi
    local sysroot; sysroot="$(rustc --print sysroot)"
    local cand; cand="$(find "$sysroot/lib/rustlib" -name llvm-objcopy 2>/dev/null | head -1)"
    if [ -n "$cand" ]; then echo "$cand"; return; fi
    if command -v llvm-objcopy >/dev/null 2>&1; then echo "llvm-objcopy"; return; fi
    for g in riscv64-unknown-elf-objcopy riscv32-unknown-elf-objcopy objcopy; do
        if command -v "$g" >/dev/null 2>&1; then echo "$g"; return; fi
    done
    echo "ERROR: no objcopy found (need rustup component 'llvm-tools')" >&2
    exit 1
}
OBJCOPY="$(find_objcopy)"

echo ">> $OBJCOPY -O binary $ELF $BIN"
"$OBJCOPY" -O binary "$ELF" "$BIN"

# Size report. zent-link.x already asserts the layout; this prints the budget.
SIZE="$(dirname "$OBJCOPY")/llvm-size"
[ -x "$SIZE" ] || SIZE="$(command -v llvm-size || true)"
if [ -n "$SIZE" ]; then
    "$SIZE" -A "$ELF" | awk '
        $1==".text" {xip=$2} $1==".fast" {ilm=$2} $1==".data" {data=$2}
        $1==".bss" || $1==".noncacheable.bss" || $1==".fast.bss" {bss+=$2}
        END {
            printf(">> size: XIP stub %d B | ILM code %d / 131072 B (%d%%) | DLM data+rodata %d + bss %d = %d / 131072 B (%d%%)\n",
                   xip, ilm, ilm*100/131072, data, bss, data+bss, (data+bss)*100/131072)
            if (xip > 1024) { print "ERROR: code in XIP .text (must run from ILM)"; exit 1 }
        }'
fi
binsize=$(stat -c %s "$BIN")
echo ">> image: $binsize B of 196608 B app region ($((binsize * 100 / 196608))%)"
if [ "$binsize" -gt 196608 ]; then
    echo "ERROR: image larger than the 192 KB application region" >&2
    exit 1
fi

# Sanity check: the first 32-bit word must be the TinyUF2 boot signature.
word0="$(od -An -tx4 -N4 "$BIN" | tr -d ' ')"
if [ "$word0" != "0b4d5475" ]; then
    echo "ERROR: first word of $BIN is 0x$word0, expected 0x0b4d5475 (boot signature)" >&2
    exit 1
fi
echo ">> boot signature OK: word0 = 0x$word0"

echo ">> python3 ../../tools/uf2.py pack $BIN -b $BASE -f $FAMILY -o $UF2"
python3 ../../tools/uf2.py pack "$BIN" -b "$BASE" -f "$FAMILY" -o "$UF2"

echo ">> done: $(pwd)/$UF2"
