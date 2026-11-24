#!/usr/bin/env python3
"""Phase-0 full backup of the ZENT75-HE config (read-only: BE 04 + BE C0 profile switch).

Dumps every RAM-map region for all 4 profiles into backups/<ts>/ as raw .bin + combined JSON
with sha256 sums. Restores the originally-active profile at the end. No flash/save/write.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time

from rammap import ADVANCED_KEYS_MIN_VERSION, PROFILE_COUNT, REGIONS, is_advanced
from zentlib import Zent, format_version, wait_device

BACKUP_ROOT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "backups")

Regions = dict[str, tuple[int, int]]


def dump_profile(z: Zent, regions: Regions) -> dict[str, dict]:
    """Read every region of the profile that is active right now."""
    return {name: {"addr": addr, "len": length, "data": z.read_ram_big(addr, length).hex()}
            for name, (addr, length) in regions.items()}


def dump_device(z: Zent, regions: Regions, active: int) -> dict[int, dict[str, dict]]:
    """Dump all profiles, then go back to ``active`` (also when a read fails half-way)."""
    profiles = {}
    try:
        for p in range(PROFILE_COUNT):
            z.switch_profile(p)  # raises unless the board confirms profile p
            print(f"  profile {p} (active now {z.profile()}) ...", flush=True)
            profiles[p] = dump_profile(z, regions)
    finally:
        z.switch_profile(active)
        print("restored active profile", z.profile())
    return profiles


def write_backup(directory: str, meta: dict, profiles: dict[int, dict[str, dict]]) -> int:
    """Write p<N>_<region>.bin, dump.json and SHA256SUMS; return the number of .bin files."""
    os.makedirs(directory, exist_ok=True)
    sums = {}
    for p, regs in profiles.items():
        for name, r in regs.items():
            raw = bytes.fromhex(r["data"])
            fn = f"p{p}_{name}.bin"
            with open(os.path.join(directory, fn), "wb") as f:
                f.write(raw)
            sums[fn] = hashlib.sha256(raw).hexdigest()
    with open(os.path.join(directory, "dump.json"), "w") as f:
        json.dump({**meta, "profiles": profiles, "sha256": sums}, f, indent=1)
    with open(os.path.join(directory, "SHA256SUMS"), "w") as f:
        f.write("".join(f"{v}  {k}\n" for k, v in sorted(sums.items())))
    return len(sums)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("device", nargs="?", help="hidraw node (default: auto-detect, wait up to 20 s)")
    ap.add_argument("--out", default=BACKUP_ROOT, help="backup root directory (default: %(default)s)")
    args = ap.parse_args()

    dev = args.device or wait_device(20)
    if not dev:
        print("device not present")
        sys.exit(1)
    with Zent(dev) as z:
        ver_raw = z.version_raw()
        ver, active = format_version(ver_raw), z.profile()
        print("firmware", ver, "active profile", active, "dev", dev)
        serial = z.serial()
        regions = dict(REGIONS)
        if (ver_raw[2], ver_raw[1], ver_raw[0]) < ADVANCED_KEYS_MIN_VERSION:
            # bind_key_* does not exist before 0.0.18; reading unmapped memory can hang the board
            regions = {n: r for n, r in regions.items() if not is_advanced(n)}
            print("  firmware older than 0.0.18: skipping bind_key_* regions")
        profiles = dump_device(z, regions, active)

    ts = int(time.time())
    directory = os.path.join(args.out, str(ts))
    meta = {"timestamp": ts, "firmware": ver, "active_profile": active,
            "serial_raw": serial.hex() if serial else None, "regions": regions}
    count = write_backup(directory, meta, profiles)
    print("backup ->", directory, f"({count} regions x4 profiles)")


if __name__ == "__main__":
    main()
