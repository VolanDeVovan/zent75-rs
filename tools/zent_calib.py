#!/usr/bin/env python3
"""Recalibrate selected ZENT 75 HE keys, mirroring hoveyworks-driver.com:
handleStartCal -> HidCmdCalib(1) -> collect min/max -> HidCmdCalib(0)
-> HidCmdSetRam(top_value), HidCmdSetRam(bot_value) -> HidCmdSave.
Only the selected keys are changed; every other key keeps its stored values.

WRITES TO THE DEVICE (RAM, then flash).
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import time

from rammap import BOT_VALUE, KEY_COUNT, KEY_NAMES, TOP_VALUE
from zentlib import Zent, decode_stream, pack_u16, unpack_u16

DEFAULT_KEYS = (59, 73)  # LShift, LCtrl
COLLECT_S = 30
MIN_RANGE = 7000  # same acceptance threshold the driver uses
SAVE_SETTLE_S = 0.9  # Zent.save adds 0.3 s: 1.2 s in total, the vendor driver waits 1.1 s
HERE = os.path.dirname(os.path.abspath(__file__))


def collect(z: Zent, keys: dict[int, str], seconds: float) -> tuple[dict[int, int], dict[int, int], int]:
    """Calibration mode on, track per-key raw ADC max/min for ``seconds``, calibration off.

    Returns (hi, lo, number of stream frames seen).
    """
    hi = {k: 0 for k in keys}
    lo = {k: 65535 for k in keys}
    frames = 0
    z.drain()
    z.calibration(True)
    try:
        end = time.monotonic() + seconds
        last = 0.0
        while time.monotonic() < end:
            if time.monotonic() - last > 1:
                z.heartbeat()
                last = time.monotonic()
            t = z.recv(0.1)
            f = decode_stream(t) if t else None
            if f is None:
                continue
            frames += 1
            for k in keys:
                hi[k] = max(hi[k], f.raw[k])
                lo[k] = min(lo[k], f.raw[k])
    finally:
        z.calibration(False)
        time.sleep(0.3)
    return hi, lo, frames


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("device", nargs="?", help="hidraw node (default: auto-detect)")
    ap.add_argument("--keys", type=int, nargs="+", default=list(DEFAULT_KEYS), metavar="INDEX",
                    help="key indices to recalibrate (default: 59 73 = LShift LCtrl)")
    ap.add_argument("--seconds", type=float, default=COLLECT_S, help="collection time (default: %(default)s)")
    args = ap.parse_args()
    if any(not 0 <= k < KEY_COUNT for k in args.keys):
        ap.error(f"key indices must be 0..{KEY_COUNT - 1}")
    keys = {k: KEY_NAMES[k] for k in args.keys}

    with Zent(args.device) as z:
        top = unpack_u16(z.read_ram(TOP_VALUE, 2 * KEY_COUNT))
        bot = unpack_u16(z.read_ram(BOT_VALUE, 2 * KEY_COUNT))
        bk = os.path.join(HERE, f"zent_calib_backup_{int(time.time())}.json")
        with open(bk, "w") as f:
            json.dump({"top": top, "bot": bot}, f)
        print("backup ->", bk)
        print("before:", {n: (top[k], bot[k]) for k, n in keys.items()})

        print(f"CALIBRATING for {args.seconds:g}s: press {' and '.join(sorted(keys.values()))} fully, "
              "several times", flush=True)
        hi, lo, frames = collect(z, keys, args.seconds)
        print("frames", frames)
        print("measured:", {n: (hi[k], lo[k], hi[k] - lo[k]) for k, n in keys.items()})
        bad = [n for k, n in keys.items() if hi[k] - lo[k] < MIN_RANGE]
        if bad:
            print("NOT WRITTEN: too little travel seen for", bad)
            sys.exit(1)

        for k in keys:
            top[k], bot[k] = hi[k], lo[k]
        z.write_ram(TOP_VALUE, pack_u16(top))  # each write is acked and read back by zentlib
        z.write_ram(BOT_VALUE, pack_u16(bot))
        top2 = unpack_u16(z.read_ram(TOP_VALUE, 2 * KEY_COUNT))
        bot2 = unpack_u16(z.read_ram(BOT_VALUE, 2 * KEY_COUNT))
        if top2 != top or bot2 != bot:
            sys.exit("read-back mismatch: NOT saved to flash")
        print("written + verified:", {n: (top2[k], bot2[k]) for k, n in keys.items()})

        z.save(settle=SAVE_SETTLE_S)  # HidCmdSave -> flash
        print("saved to flash")


if __name__ == "__main__":
    main()
