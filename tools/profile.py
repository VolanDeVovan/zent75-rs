#!/usr/bin/env python3
"""Statistical CPU profile of our Rust firmware on the real board (see firmware/app/src/prof.rs).

    uv run --with pyelftools python tools/profile.py [--elf PATH] [--captures N] [--top N]

Starts GPTMR0 sampling (`BE D2`), reads the interrupted PCs (`BE D1`), and prints the share of
samples per function of the ELF (default: the last release build). Version-checked: never sent
to the stock firmware.
"""
from __future__ import annotations

import argparse
import bisect
import collections
import struct
import sys
import time
from pathlib import Path

import zentlib as zl

CMD_PROF_READ, CMD_PROF_START = 0xD1, 0xD2
SAMPLES = 4096
DEFAULT_ELF = Path(__file__).resolve().parent.parent / \
    "firmware/app/target/riscv32imafc-unknown-none-elf/release/zent75-rs"


def request(z: zl.Zent, cmd: int, start: int) -> tuple[int, list[int]]:
    z.drain()
    z.send(bytes([0xBE, cmd, 9, 0]) + struct.pack("<I", start) + b"\xED")
    end = time.monotonic() + 1.0
    while time.monotonic() < end:
        r = z.recv(0.1)
        if r and r[:2] == bytes([0xBE, cmd]):
            n = r[2] | r[3] << 8
            taken, _ = struct.unpack_from("<II", r, 4)
            k = (n - 13) // 4
            return taken, list(struct.unpack_from(f"<{k}I", r, 12))
    raise zl.ZentError("no profiler reply")


def capture(z: zl.Zent) -> list[int]:
    request(z, CMD_PROF_START, 0)
    time.sleep(0.6)
    pcs: list[int] = []
    while True:
        taken, chunk = request(z, CMD_PROF_READ, len(pcs))
        pcs += chunk
        if not chunk or len(pcs) >= taken:
            return pcs


def symbols(elf: Path) -> tuple[list[int], list[tuple[int, int, str]]]:
    from elftools.elf.elffile import ELFFile
    try:
        from rust_demangler import demangle  # optional
    except ImportError:
        demangle = None
    syms = []
    with open(elf, "rb") as f:
        for s in ELFFile(f).get_section_by_name(".symtab").iter_symbols():
            if s["st_info"]["type"] == "STT_FUNC" and s["st_size"]:
                name = s.name
                if demangle:
                    try:
                        name = demangle(name)
                    except Exception:
                        pass
                syms.append((s["st_value"] & ~1, s["st_size"], name))
    syms.sort()
    return [a for a, _, _ in syms], syms


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--elf", type=Path, default=DEFAULT_ELF)
    ap.add_argument("--captures", type=int, default=5)
    ap.add_argument("--top", type=int, default=30)
    ap.add_argument("--dump", type=Path, help="also write the raw PCs (hex, one per line)")
    args = ap.parse_args()
    starts, syms = symbols(args.elf)
    pcs: list[int] = []
    with zl.Zent() as z:
        ver = z.version()
        if not ver.startswith("1."):
            print(f"firmware {ver} is not ours; profiler commands are not sent to it", file=sys.stderr)
            return 2
        for _ in range(args.captures):
            pcs += capture(z)
    if args.dump:
        args.dump.write_text("".join(f"{pc:#x}\n" for pc in pcs))
    hits: collections.Counter[str] = collections.Counter()
    for pc in pcs:
        i = bisect.bisect_right(starts, pc) - 1
        if i >= 0 and pc < syms[i][0] + syms[i][1]:
            hits[syms[i][2]] += 1
        else:
            hits[f"?{pc:#010x}"] += 1
    total = len(pcs)
    print(f"{total} samples")
    for name, n in hits.most_common(args.top):
        print(f"{100 * n / total:6.2f}%  {n:6d}  {name[:140]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
