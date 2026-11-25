#!/usr/bin/env python3
"""Statistical CPU profile of our Rust firmware on the real board (firmware/app/src/debug/prof.rs).

    uv run --with pyelftools python tools/profile.py [--elf PATH] [--captures N] [--top N]

Starts GPTMR0 sampling, reads the interrupted PCs and prints the share of samples per function
of the ELF (default: the last release build — it must be the build that is running). Uses debug
commands of our firmware; `zentlib` checks the firmware version first and never sends them to
the stock firmware. The firmware needs its `profiler` feature (on by default).
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

DEFAULT_ELF = Path(__file__).resolve().parent.parent / \
    "firmware/app/target/riscv32imafc-unknown-none-elf/release/zent75-rs"


def capture(z: zl.Zent) -> list[int]:
    """One full capture: start, wait for the buffer to fill, read it out."""
    z.debug_request(zl.CMD_DEBUG_PROF_START)
    time.sleep(0.6)
    pcs: list[int] = []
    while True:
        taken, _, raw = z.debug_request(zl.CMD_DEBUG_PROF_READ, len(pcs))
        chunk = struct.unpack(f"<{len(raw) // 4}I", raw[:len(raw) & ~3])
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
    ap.add_argument("--captures", type=int, default=10)
    ap.add_argument("--top", type=int, default=30)
    ap.add_argument("--dump", type=Path, help="also write the raw PCs (hex, one per line)")
    args = ap.parse_args()
    starts, syms = symbols(args.elf)
    pcs: list[int] = []
    try:
        with zl.Zent() as z:
            for _ in range(args.captures):
                pcs += capture(z)
    except zl.ZentError as e:
        print(e, file=sys.stderr)
        return 2
    if not pcs:
        print("no samples", file=sys.stderr)
        return 1
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
