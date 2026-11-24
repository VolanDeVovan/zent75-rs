#!/usr/bin/env python3
"""Minimal, self-contained .bin -> .uf2 converter for the ZENT75-HE.

The repo has no Microsoft uf2conv.py, and we must not depend on one being
installed, so this is a focused re-implementation that emits exactly the block
layout the stock TinyUF2 bootloader (and firmware/vendor/0.0.18.uf2) uses:

    * 512-byte blocks, 256-byte payloads
    * flag 0x2000 (family id present), family id 0x0B4D5475
    * target addresses starting at the base (default 0x80040000)

Verified against firmware/vendor/0.0.18.uf2 (family 0xb4d5475, 256-byte
payloads). Block layout matches re/uf2.py's parser.

usage:
    uf2conv.py input.bin -b 0x80040000 -f 0x0b4d5475 -o output.uf2

Flags are accepted in the same spirit as Microsoft's uf2conv.py; -c/--convert
is accepted and ignored (bin->uf2 is the only mode here).
"""
from __future__ import annotations

import argparse
import struct
import sys

BLOCK_SIZE = 512
PAYLOAD = 256
MAGIC_START0 = 0x0A324655  # "UF2\n"
MAGIC_START1 = 0x9E5D5157
MAGIC_END = 0x0AB16F30
FLAG_FAMILY_ID = 0x00002000


def auto_int(value: str) -> int:
    return int(value, 0)


def convert(data: bytes, base: int, family: int) -> bytes:
    nblocks = (len(data) + PAYLOAD - 1) // PAYLOAD
    if nblocks == 0:
        raise ValueError("empty input: nothing to convert")
    out = bytearray()
    for i in range(nblocks):
        chunk = data[i * PAYLOAD : (i + 1) * PAYLOAD]
        hdr = struct.pack(
            "<8I",
            MAGIC_START0,
            MAGIC_START1,
            FLAG_FAMILY_ID,
            base + i * PAYLOAD,
            len(chunk),
            i,
            nblocks,
            family,
        )
        block = hdr + chunk + b"\x00" * (PAYLOAD - len(chunk))
        block += b"\x00" * (BLOCK_SIZE - 4 - len(block))
        block += struct.pack("<I", MAGIC_END)
        assert len(block) == BLOCK_SIZE
        out += block
    return bytes(out)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("input", help="input flat .bin")
    ap.add_argument("-o", "--output", required=True, help="output .uf2")
    ap.add_argument("-b", "--base", type=auto_int, default=0x80040000,
                    help="flash base address of the image (default 0x80040000)")
    ap.add_argument("-f", "--family", type=auto_int, default=0x0B4D5475,
                    help="UF2 family id (default 0x0B4D5475)")
    ap.add_argument("-c", "--convert", action="store_true",
                    help="accepted for CLI compatibility; ignored")
    args = ap.parse_args()

    with open(args.input, "rb") as f:
        data = f.read()
    uf2 = convert(data, args.base, args.family)
    with open(args.output, "wb") as f:
        f.write(uf2)
    print(
        f"{args.input} ({len(data)} bytes) -> {args.output} "
        f"({len(uf2) // BLOCK_SIZE} blocks, base {args.base:#010x}, family {args.family:#010x})"
    )


if __name__ == "__main__":
    sys.exit(main())
