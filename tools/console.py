#!/usr/bin/env python3
"""Read the debug console of our Rust firmware over USB (no UART needed).

    tools/console.py            print the kept log history (last 4 KB) and exit
    tools/console.py -f         keep following new lines (Ctrl-C to stop)

Uses the debug vendor command `BE D0` that only our firmware implements. The stock firmware never
gets it: the firmware version is checked first (ours reports 1.x, stock 0.0.x).
"""
from __future__ import annotations

import argparse
import struct
import sys
import time

import zentlib as zl

CMD_CONSOLE = 0xD0


def read_chunk(z: zl.Zent, pos: int) -> tuple[int, int, bytes]:
    z.drain()
    z.send(bytes([0xBE, CMD_CONSOLE, 9, 0]) + struct.pack("<I", pos) + b"\xED")
    end = time.monotonic() + 1.0
    while time.monotonic() < end:
        r = z.recv(0.1)
        if r and r[:2] == bytes([0xBE, CMD_CONSOLE]):
            n = r[2] | r[3] << 8
            total, start = struct.unpack_from("<II", r, 4)
            return total, start, bytes(r[12:n - 1])
    raise zl.ZentError("no console reply")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-f", "--follow", action="store_true", help="keep printing new output")
    args = ap.parse_args()
    with zl.Zent() as z:
        ver = z.version()
        if not ver.startswith("1."):
            print(f"firmware {ver} is not ours; BE D0 is not sent to it", file=sys.stderr)
            return 2
        pos = 0
        while True:
            total, start, data = read_chunk(z, pos)
            if start > pos and pos:
                print(f"\n[... {start - pos} bytes lost ...]", flush=True)
            sys.stdout.write(data.decode("utf-8", "replace").replace("\r\n", "\n"))
            sys.stdout.flush()
            pos = start + len(data)
            if pos < total:
                continue
            if not args.follow:
                return 0
            time.sleep(0.2)


if __name__ == "__main__":
    sys.exit(main())
