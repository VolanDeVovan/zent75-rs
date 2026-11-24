#!/usr/bin/env python3
"""Prove the app-independent recovery lever: the TinyUF2 loader refuses an app whose boot signature
(word0 at 0x80040000) is not 0x0b4d5475 and stays in UF2 mass-storage mode, with no app, button or host.

Non-destructive: it flashes a copy of the stock image with ONLY word0 zeroed (the real reset vector is
at 0x80040004, so even if the signature were ignored the stock code would still run), checks the board
lands in the bootloader rather than the app, then re-flashes the real stock image and verifies the
config is byte-identical to the given backup. Both branches are recoverable; the board is never bricked.

usage: confirm_sig_recovery.py [--stock firmware/vendor/0.0.18.uf2] [--app-bin re/out/0.0.18_80040000.bin]
                               [--backup backups/<ts>]
"""
import argparse
import glob
import os
import struct
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from zentlib import Zent, find_hidraw, wait_device  # noqa: E402

SIG = 0x0B4D5475
BASE = 0x80040000


def pack_uf2(app: bytes, path: str):
    blocks = (len(app) + 255) // 256
    out = bytearray()
    for i in range(blocks):
        chunk = app[i * 256:(i + 1) * 256]
        pad = chunk + b"\x00" * (256 - len(chunk))
        hdr = struct.pack("<8I", 0x0A324655, 0x9E5D5157, 0x2000, BASE + i * 256, len(chunk), i, blocks, SIG)
        out += hdr + pad + b"\x00" * (476 - 256) + struct.pack("<I", 0x0AB16F30)
    open(path, "wb").write(out)
    return blocks


def usb_present():
    bl = app = False
    for d in glob.glob("/sys/bus/usb/devices/*"):
        try:
            v = open(os.path.join(d, "idVendor")).read().strip()
            p = open(os.path.join(d, "idProduct")).read().strip()
        except OSError:
            continue
        if (v, p) == ("34b7", "1001"):
            bl = True
        if (v, p) == ("36b0", "357e"):
            app = True
    return bl, app


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--stock", default=os.path.join(ROOT, "firmware/vendor/0.0.18.uf2"))
    ap.add_argument("--app-bin", default=os.path.join(ROOT, "re/out/0.0.18_80040000.bin"))
    ap.add_argument("--backup", default=None)
    a = ap.parse_args()

    scratch = os.environ.get("ZENT_SCRATCH", "/tmp")
    nosig = os.path.join(scratch, "zent_nosig.uf2")
    app = bytearray(open(a.app_bin, "rb").read())
    if struct.unpack_from("<I", app, 0)[0] != SIG:
        print(f"app-bin word0 is not the signature {SIG:#x}; refusing"); sys.exit(2)
    struct.pack_into("<I", app, 0, 0)  # invalidate signature; code at +4 stays intact
    n = pack_uf2(bytes(app), nosig)
    print(f"built signature-invalid image: {n} blocks -> {nosig}")

    # 1. enter bootloader from the running app
    dev = find_hidraw()
    if dev:
        z = Zent(dev); print(f"app {z.version()}; BE FC -> bootloader"); z.simple(0xFC); time.sleep(0.3); z.close()
    # 2. flash the signature-invalid image
    print("flashing signature-invalid image ...")
    rc = subprocess.run([sys.executable, os.path.join(HERE, "flash_uf2.py"), nosig, "--no-enter"],
                        cwd=ROOT).returncode
    # flash_uf2 waits for the app and will FAIL (rc!=0) because the app must NOT come up — that is the pass
    time.sleep(2)
    bl, appup = usb_present()
    passed = bl and not appup
    print(f"after signature-invalid flash: bootloader={bl} app={appup}  -> "
          f"{'PASS (loader held, app refused)' if passed else 'UNEXPECTED (app booted)'}")

    # 3. restore stock (signature valid)
    print("restoring stock ...")
    rc = subprocess.run([sys.executable, os.path.join(HERE, "flash_uf2.py"), a.stock,
                         "--expect", os.path.basename(a.stock)[:-4], "--no-enter"], cwd=ROOT).returncode
    if rc != 0:
        print("STOCK RESTORE FAILED — board may be in the bootloader; re-run flash_uf2.py manually"); sys.exit(3)

    # 4. verify config against the backup if given
    backup = a.backup or (sorted(glob.glob(os.path.join(ROOT, "backups/*/")))[-1] if glob.glob(os.path.join(ROOT, "backups/*/")) else None)
    if backup:
        import json
        m = json.load(open(os.path.join(backup, "dump.json")))
        z = Zent(wait_device(15)); act = z.profile()
        regs = m["profiles"][str(act)]
        bad = [k for k, r in regs.items() if z.read_ram_big(r["addr"], r["len"]).hex() != r["data"]]
        z.close()
        print(f"config vs backup: {len(regs) - len(bad)}/{len(regs)} identical" + ("" if not bad else " DIFF " + ",".join(bad)))
        passed = passed and not bad
    print("RESULT:", "CONFIRMED app-independent signature recovery" if passed else "CHECK OUTPUT")
    sys.exit(0 if passed else 1)


if __name__ == "__main__":
    main()
