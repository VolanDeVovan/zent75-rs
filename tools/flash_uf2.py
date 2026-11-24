#!/usr/bin/env python3
"""Autonomously flash a .uf2 to the ZENT75-HE and return it to application mode.

Flow (all without a human replug):
  1. if the app is running, send `BE FC` to jump into the TinyUF2 bootloader
  2. wait for the UF2 mass-storage device (re-enumerates as USB 34B7:1001 "Zent UF2 Bootloader")
  3. mount it read-write (clearing a stale read-only flag if needed), copy the .uf2, sync, umount
  4. TinyUF2 flashes and auto-resets; wait for the app to re-enumerate as 36B0:357E
  5. verify the reported firmware version over the vendor raw-HID interface

Mount/umount/blockdev need sudo (authorized for this board).

Safety: every image is checked before anything is sent to the board (`check_uf2`). Blocks may only
target the application region 0x80040000..0x8006FFFF. 0x80000000..0x8003FFFF holds the boot header,
stage-0 (the hold-A+S+D recovery trigger) and the download loader; overwriting it removes the only
app-independent way back, so there is deliberately no override flag. See docs/boot-recovery.md.

usage: flash_uf2.py firmware/vendor/0.0.18.uf2 [--expect 0.0.18] [--no-enter]
"""
import argparse
import glob
import os
import shutil
import subprocess
import sys
import time

import uf2
from uf2 import FAMILY_ID
from zentlib import CMD_BOOTLOADER, Zent, find_hidraw, wait_device

PROTECTED = (0x80000000, 0x80040000)  # boot header + stage-0 (A+S+D recovery) + download loader
APP_REGION = (0x80040000, 0x80070000)  # application; config banks start at 0x80070000
SCRATCH = os.environ.get("ZENT_MNT", "/tmp/zent_uf2_mnt")  # mount point for the bootloader disk


class UnsafeImage(ValueError):
    """The .uf2 would write outside the application region (or is not a valid UF2)."""


def check_uf2(data: bytes) -> int:
    """Validate a UF2 image for this board; return its block count or raise UnsafeImage.

    Every block must be a well-formed UF2 block for family 0x0B4D5475 whose whole payload lies inside
    APP_REGION. Anything touching PROTECTED is refused: it would destroy the recovery path.
    """
    try:
        blocks = uf2.parse_bytes(data)  # size, magics, payload size <= 476
    except ValueError as e:
        raise UnsafeImage(str(e)) from None
    for i, b in enumerate(blocks):
        if not b.flags & uf2.FLAG_FAMILY_ID or b.family != FAMILY_ID:
            raise UnsafeImage(f"block {i}: family {b.family:#010x} (flags {b.flags:#x}), expected {FAMILY_ID:#010x}")
        if b.size == 0:
            raise UnsafeImage(f"block {i}: payload size {b.size}")
        end = b.addr + b.size
        if b.addr < PROTECTED[1] and end > PROTECTED[0]:
            raise UnsafeImage(f"block {i}: {b.addr:#010x}..{end:#010x} overlaps the protected boot region "
                              f"{PROTECTED[0]:#010x}..{PROTECTED[1]:#010x} (stage-0 / A+S+D recovery / loader)")
        if b.addr < APP_REGION[0] or end > APP_REGION[1]:
            raise UnsafeImage(f"block {i}: {b.addr:#010x}..{end:#010x} is outside the application region "
                              f"{APP_REGION[0]:#010x}..{APP_REGION[1]:#010x}")
    return len(blocks)


def log(*a):
    print(*a, flush=True)


def find_uf2_disk(timeout=25.0):
    """Return the /dev/sdX node of the TinyUF2 mass-storage device, or None."""
    end = time.time() + timeout
    while time.time() < end:
        for dev in sorted(glob.glob("/dev/sd?")):
            name = os.path.basename(dev)
            model = _sys_read(f"/sys/block/{name}/device/model")
            vendor = _sys_read(f"/sys/block/{name}/device/vendor")
            if model and ("UF2" in model or "Zent" in (vendor or "")):
                return dev
        time.sleep(0.5)
    return None


def _sys_read(path):
    try:
        return open(path).read().strip()
    except OSError:
        return None


def enter_bootloader():
    dev = find_hidraw()
    if not dev:
        log("no app-mode HID interface; assuming already in bootloader")
        return
    z = Zent(dev)
    log(f"app firmware {z.version()} on {dev}; sending BE FC (enter bootloader)")
    z.simple(CMD_BOOTLOADER)
    time.sleep(0.2)
    z.close()


def mount_rw(disk):
    os.makedirs(SCRATCH, exist_ok=True)
    subprocess.run(["sudo", "blockdev", "--setrw", disk], check=False)
    for attempt in range(3):
        subprocess.run(["sudo", "umount", SCRATCH], check=False,
                       stderr=subprocess.DEVNULL)
        subprocess.run(["sudo", "mount", "-o", f"rw,uid={os.getuid()}", disk, SCRATCH],
                       capture_output=True, text=True)
        opts = subprocess.run(["findmnt", "-no", "OPTIONS", SCRATCH],
                              capture_output=True, text=True).stdout
        if "rw," in opts or opts.strip().startswith("rw"):
            return True
        log(f"  mount attempt {attempt + 1}: landed read-only ({opts.strip()}), retrying")
        time.sleep(1)
    return False


def flash(uf2_path, enter=True, expect=None, settle=40):
    uf2_path = os.path.abspath(uf2_path)
    if not uf2_path.endswith(".uf2") or not os.path.exists(uf2_path):
        log(f"not a .uf2 file: {uf2_path}")
        return 2
    try:
        with open(uf2_path, "rb") as f:
            n = check_uf2(f.read())
    except UnsafeImage as e:
        log(f"REFUSED, nothing was sent to the board: {e}")
        return 7
    log(f"image check OK: {n} blocks, all inside {APP_REGION[0]:#010x}..{APP_REGION[1]:#010x}")
    if enter:
        enter_bootloader()
    log("waiting for the UF2 mass-storage device ...")
    disk = find_uf2_disk()
    if not disk:
        log("ERROR: UF2 bootloader disk never appeared")
        return 3
    log(f"bootloader disk: {disk}")
    if not mount_rw(disk):
        log("ERROR: could not mount the UF2 disk read-write")
        return 4
    try:
        shutil.copy(uf2_path, os.path.join(SCRATCH, os.path.basename(uf2_path)))
        subprocess.run(["sync"], check=False)
        log(f"copied {os.path.basename(uf2_path)} -> {disk}; TinyUF2 is flashing")
    finally:
        subprocess.run(["sudo", "umount", SCRATCH], check=False,
                       stderr=subprocess.DEVNULL)

    log("waiting for the app to re-enumerate as 36B0:357E ...")
    dev = wait_device(settle)
    if not dev:
        log("ERROR: app did not come back within timeout")
        return 5
    z = Zent(dev)
    ver = z.version()
    z.close()
    log(f"app is back: firmware {ver} on {dev}")
    if expect and ver != expect:
        log(f"ERROR: expected {expect}, got {ver}")
        return 6
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("uf2", help="path to the .uf2 image to flash")
    ap.add_argument("--expect", help="firmware version string to require afterwards, e.g. 0.0.18")
    ap.add_argument("--no-enter", action="store_true",
                    help="assume the board is already in the bootloader")
    a = ap.parse_args()
    rc = flash(a.uf2, enter=not a.no_enter, expect=a.expect)
    log("OK" if rc == 0 else f"FAILED (rc={rc})")
    sys.exit(rc)


if __name__ == "__main__":
    main()
