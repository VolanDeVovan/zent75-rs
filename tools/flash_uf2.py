#!/usr/bin/env python3
"""Autonomously flash a .uf2 to the ZENT75-HE and return it to application mode.

Flow (all without a human replug):
  1. if the app is running, send `BE FC` to jump into the TinyUF2 bootloader
  2. wait for the UF2 mass-storage device (re-enumerates as USB 34B7:1001 "Zent UF2 Bootloader")
  3. mount it read-write (clearing a stale read-only flag if needed), copy the .uf2, sync, umount
  4. TinyUF2 flashes and auto-resets; wait for the app to re-enumerate as 36B0:357E
  5. verify the reported firmware version over the vendor raw-HID interface

Mount/umount/blockdev need sudo (authorized for this board). See docs/bootloader.md.

Safety: every image is checked before anything is sent to the board (`check_uf2`). Blocks may only
target the application region 0x80040000..0x8006FFFF. 0x80000000..0x8003FFFF holds the boot header,
stage-0 (the hold-A+S+D recovery trigger) and the download loader; overwriting it removes the only
app-independent way back, so there is deliberately no override flag. See docs/bootloader.md §7.

usage: flash_uf2.py firmware/0.0.18.uf2 [--expect 0.0.18] [--no-enter]
"""
import argparse
import glob
import os
import struct
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from zentlib import Zent, find_hidraw, wait_device  # noqa: E402

BL_VID = "34b7"          # bootloader-mode USB vendor id

UF2_MAGIC0, UF2_MAGIC1, UF2_MAGIC_END = 0x0A324655, 0x9E5D5157, 0x0AB16F30
UF2_FLAG_FAMILY = 0x2000
FAMILY_ID = 0x0B4D5475
PROTECTED = (0x80000000, 0x80040000)  # boot header + stage-0 (A+S+D recovery) + download loader
APP_REGION = (0x80040000, 0x80070000)  # application; config banks start at 0x80070000


class UnsafeImage(ValueError):
    """The .uf2 would write outside the application region (or is not a valid UF2)."""


def check_uf2(data: bytes) -> int:
    """Validate a UF2 image for this board; return its block count or raise UnsafeImage.

    Every block must be a well-formed UF2 block for family 0x0B4D5475 whose whole payload lies inside
    APP_REGION. Anything touching PROTECTED is refused: it would destroy the recovery path.
    """
    if not data or len(data) % 512:
        raise UnsafeImage(f"size {len(data)} is not a non-zero multiple of 512")
    blocks = len(data) // 512
    for i in range(blocks):
        blk = data[i * 512:(i + 1) * 512]
        m0, m1, flags, addr, size, _no, _total, family = struct.unpack("<8I", blk[:32])
        (m_end,) = struct.unpack("<I", blk[508:512])
        if (m0, m1, m_end) != (UF2_MAGIC0, UF2_MAGIC1, UF2_MAGIC_END):
            raise UnsafeImage(f"block {i}: bad UF2 magic")
        if not flags & UF2_FLAG_FAMILY or family != FAMILY_ID:
            raise UnsafeImage(f"block {i}: family {family:#010x} (flags {flags:#x}), expected {FAMILY_ID:#010x}")
        if not 0 < size <= 476:
            raise UnsafeImage(f"block {i}: payload size {size}")
        end = addr + size
        if addr < PROTECTED[1] and end > PROTECTED[0]:
            raise UnsafeImage(f"block {i}: {addr:#010x}..{end:#010x} overlaps the protected boot region "
                              f"{PROTECTED[0]:#010x}..{PROTECTED[1]:#010x} (stage-0 / A+S+D recovery / loader)")
        if addr < APP_REGION[0] or end > APP_REGION[1]:
            raise UnsafeImage(f"block {i}: {addr:#010x}..{end:#010x} is outside the application region "
                              f"{APP_REGION[0]:#010x}..{APP_REGION[1]:#010x}")
    return blocks
SCRATCH = os.environ.get("ZENT_MNT", "/tmp/zent_uf2_mnt")


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
        # fall back: any single removable sd that just appeared
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
    z.simple(0xFC)
    time.sleep(0.2)
    z.close()


def mount_rw(disk):
    os.makedirs(SCRATCH, exist_ok=True)
    subprocess.run(["sudo", "blockdev", "--setrw", disk], check=False)
    for attempt in range(3):
        subprocess.run(["sudo", "umount", SCRATCH], check=False,
                       stderr=subprocess.DEVNULL)
        r = subprocess.run(["sudo", "mount", "-o", f"rw,uid={os.getuid()}", disk, SCRATCH],
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
        import shutil
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
