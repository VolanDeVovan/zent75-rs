#!/usr/bin/env python3
"""Live-stream check: flag keys whose sensor moves while the firmware sees ~0 travel.

Keeps the `BE FD` stream alive with heartbeats for 30 s; press every key once meanwhile.
Nothing is written to the keyboard.
"""
from __future__ import annotations

import argparse
import time

from rammap import KEY_COUNT, KEY_NAMES
from zentlib import STREAM_LEN_TRAVEL, Zent, decode_stream

ADC_SWING = 3000  # raw ADC counts that count as "the key was pressed"
DEAD_TRAVEL_UM = 500  # peak travel below this, despite the swing, means the key is broken


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("device", nargs="?", help="hidraw node (default: auto-detect)")
    ap.add_argument("--seconds", type=float, default=30, help="capture length (default: %(default)s)")
    args = ap.parse_args()

    travel_max = [0] * KEY_COUNT
    lo = [10 ** 6] * KEY_COUNT
    hi = [0] * KEY_COUNT
    with Zent(args.device) as z:
        end = time.monotonic() + args.seconds
        last = 0.0
        while time.monotonic() < end:
            if time.monotonic() - last > 1:
                z.heartbeat()
                last = time.monotonic()
            t = z.recv(0.1)
            f = decode_stream(t) if t else None
            if f is None or f.length != STREAM_LEN_TRAVEL:
                continue
            for k in range(KEY_COUNT):
                travel_max[k] = max(travel_max[k], f.travel[k])
                lo[k] = min(lo[k], f.raw[k])
                hi[k] = max(hi[k], f.raw[k])

    print(f"pressed keys (adc swing > {ADC_SWING}):")
    for k in range(KEY_COUNT):
        if hi[k] - lo[k] > ADC_SWING:
            flag = "  <-- BROKEN: sensor moved, firmware travel ~0" if travel_max[k] < DEAD_TRAVEL_UM else ""
            print(f"{KEY_NAMES[k]:7} adc {hi[k]}->{lo[k]}  travel {travel_max[k] / 1000:.2f} mm{flag}")


if __name__ == "__main__":
    main()
