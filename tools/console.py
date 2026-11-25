#!/usr/bin/env python3
"""Read the debug console of our Rust firmware over USB (no UART needed).

    tools/console.py            print the kept log history (last 4 KB) and exit
    tools/console.py -f         keep following new lines (Ctrl-C to stop)

Uses a debug command that only our firmware implements (firmware/app/src/debug/mod.rs).
`zentlib` checks the firmware version first and never sends it to the stock firmware.
"""
from __future__ import annotations

import argparse
import sys
import time

import zentlib as zl


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-f", "--follow", action="store_true", help="keep printing new output")
    args = ap.parse_args()
    try:
        with zl.Zent() as z:
            pos = 0
            while True:
                total, start, data = z.debug_request(zl.CMD_DEBUG_CONSOLE, pos)
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
    except zl.ZentError as e:
        print(e, file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        return 0


if __name__ == "__main__":
    sys.exit(main())
