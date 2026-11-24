#!/usr/bin/env python3
"""UF2 images for the ZENT75-HE: parse (summary + flat .bin per range) and pack (.bin -> .uf2).

usage: uf2.py <in.uf2> [out_prefix]                           inspect / unpack
       uf2.py pack <in.bin> -o <out.uf2> [-b BASE] [-f FAMILY]   pack

A UF2 file is a sequence of 512-byte blocks:
    0   magic0  0x0A324655 ("UF2\\n")      16  payload size (<= 476)
    4   magic1  0x9E5D5157                 20  block number
    8   flags                              24  total number of blocks
    12  target address                     28  family id (when flag 0x2000 is set)
    32  payload (476 bytes)                508 magic end 0x0AB16F30

`pack` emits the layout the stock TinyUF2 bootloader and the vendor images use: 256-byte
payloads, flag 0x2000, consecutive addresses from the base.
"""
from __future__ import annotations

import argparse
import collections
import struct
import sys
from typing import NamedTuple

BLOCK_SIZE = 512
MAX_PAYLOAD = 476
PACK_PAYLOAD = 256  # payload bytes per block written by pack()
MAGIC_START0, MAGIC_START1, MAGIC_END = 0x0A324655, 0x9E5D5157, 0x0AB16F30
FLAG_NOT_MAIN_FLASH = 0x00000001  # block must be skipped when flashing
FLAG_FAMILY_ID = 0x00002000  # the field at offset 28 is a family id

FAMILY_ID = 0x0B4D5475  # ZENT75-HE (also the boot signature in the first word of the app)
APP_BASE = 0x80040000  # flash address of the application image


class Block(NamedTuple):
    flags: int
    addr: int
    size: int
    number: int
    total: int
    family: int
    data: bytes


def parse_bytes(d: bytes, name: str = "uf2") -> list[Block]:
    """Validate and split a UF2 image. Raises ValueError on a malformed image."""
    if not d or len(d) % BLOCK_SIZE:
        raise ValueError(f"{name}: size {len(d)} is not a positive multiple of {BLOCK_SIZE}")
    blocks = []
    for o in range(0, len(d), BLOCK_SIZE):
        m0, m1, flags, addr, size, number, total, family = struct.unpack_from("<8I", d, o)
        (end,) = struct.unpack_from("<I", d, o + BLOCK_SIZE - 4)
        if (m0, m1, end) != (MAGIC_START0, MAGIC_START1, MAGIC_END):
            raise ValueError(f"{name}: bad magic in the block at offset {o:#x}")
        if size > MAX_PAYLOAD:
            raise ValueError(f"{name}: payload size {size} in the block at offset {o:#x}")
        blocks.append(Block(flags, addr, size, number, total, family, d[o + 32:o + 32 + size]))
    return blocks


def parse(path: str) -> list[Block]:
    """Read and validate every block of a UF2 file. Raises ValueError on a malformed file."""
    with open(path, "rb") as f:
        return parse_bytes(f.read(), path)


def ranges(blocks: list[Block]) -> list[tuple[int, bytearray]]:
    """Join the main-flash blocks into contiguous (address, data) ranges, sorted by address."""
    out: list[tuple[int, bytearray]] = []
    for b in sorted(blocks, key=lambda b: b.addr):
        if b.flags & FLAG_NOT_MAIN_FLASH:
            continue
        if out and b.addr < out[-1][0] + len(out[-1][1]):
            raise ValueError(f"block {b.number} at {b.addr:#010x} overlaps the previous one")
        if out and out[-1][0] + len(out[-1][1]) == b.addr:
            out[-1][1].extend(b.data)
        else:
            out.append((b.addr, bytearray(b.data)))
    return out


def pack(data: bytes, base: int = APP_BASE, family: int = FAMILY_ID) -> bytes:
    """Flat binary -> UF2 image with PACK_PAYLOAD-byte blocks starting at ``base``."""
    if not data:
        raise ValueError("empty input: nothing to pack")
    total = (len(data) + PACK_PAYLOAD - 1) // PACK_PAYLOAD
    out = bytearray()
    for i in range(total):
        chunk = data[i * PACK_PAYLOAD:(i + 1) * PACK_PAYLOAD]
        out += struct.pack("<8I", MAGIC_START0, MAGIC_START1, FLAG_FAMILY_ID,
                           base + i * PACK_PAYLOAD, len(chunk), i, total, family)
        out += chunk.ljust(MAX_PAYLOAD, b"\x00") + struct.pack("<I", MAGIC_END)
    return bytes(out)


def main_pack(argv: list[str]) -> None:
    def auto_int(value: str) -> int:
        return int(value, 0)

    ap = argparse.ArgumentParser(prog="uf2.py pack", description="Pack a flat .bin into a .uf2 image.")
    ap.add_argument("input", help="input flat .bin")
    ap.add_argument("-o", "--output", required=True, help="output .uf2")
    ap.add_argument("-b", "--base", type=auto_int, default=APP_BASE,
                    help=f"flash address of the image (default {APP_BASE:#010x})")
    ap.add_argument("-f", "--family", type=auto_int, default=FAMILY_ID,
                    help=f"UF2 family id (default {FAMILY_ID:#010x})")
    args = ap.parse_args(argv)

    with open(args.input, "rb") as f:
        data = f.read()
    image = pack(data, args.base, args.family)
    with open(args.output, "wb") as f:
        f.write(image)
    print(f"{args.input} ({len(data)} bytes) -> {args.output} "
          f"({len(image) // BLOCK_SIZE} blocks, base {args.base:#010x}, family {args.family:#010x})")


def main_inspect(argv: list[str]) -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                 epilog="To pack a .bin: uf2.py pack --help")
    ap.add_argument("uf2", help="input .uf2 image")
    ap.add_argument("out_prefix", nargs="?", help="write <out_prefix>_<addr>.bin per range")
    args = ap.parse_args(argv)

    b = parse(args.uf2)
    print("blocks", len(b), "total-field", {x.total for x in b})
    print("flags", collections.Counter(hex(x.flags) for x in b))
    print("family", collections.Counter(hex(x.family) for x in b))
    print("payload sizes", collections.Counter(x.size for x in b))
    if sorted(x.number for x in b) != list(range(len(b))) or {x.total for x in b} != {len(b)}:
        print("WARNING: block numbers / total field do not describe one complete image")
    for a, data in ranges(b):
        print(f"range {a:#010x}..{a + len(data):#010x} ({len(data)} bytes)")
        if args.out_prefix:
            fn = f"{args.out_prefix}_{a:08x}.bin"
            with open(fn, "wb") as f:
                f.write(data)
            print(" ->", fn)


def main() -> None:
    argv = sys.argv[1:]
    if argv[:1] == ["pack"]:
        main_pack(argv[1:])
    else:
        main_inspect(argv)


if __name__ == "__main__":
    main()
