#!/usr/bin/env python3
"""Restore a ZENT75-HE config dump back onto the device and verify it.

Reads a backups/<ts>/dump.json produced by dump_all.py and, for each of the four profiles,
switches to that profile (`BE C0`), writes every region with `BE 03` (write RAM), commits with
`BE 02` (save to flash), and reads every region back to confirm it is byte-identical. The
originally-active profile is restored at the end.

Modes:
  restore.py backups/<ts>            full restore + verify of every profile
  restore.py backups/<ts> --verify   read-only: just compare the device to the dump, write nothing
  restore.py --selftest              harmless round-trip proof: snapshot the active profile's name,
                                     change it, save, verify the change, restore it, save, and
                                     confirm a byte-identical read-back (GOAL Phase-0 step 2)

Writes to flash. Authorized for this board; see docs/ and the project GOAL.
"""
import argparse
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from zentlib import Zent, wait_device  # noqa: E402
import rammap  # noqa: E402

# config_name1 lives here (profile name, 64 bytes); harmless to tweak for the self-test.
CONFIG_NAME1 = 0x80070290


def log(*a):
    print(*a, flush=True)


def load_dump(path):
    if os.path.isdir(path):
        path = os.path.join(path, "dump.json")
    return json.load(open(path))


def verify_profile(z, regs):
    bad = []
    for name, r in regs.items():
        live = z.read_ram_big(r["addr"], r["len"]).hex()
        if live != r["data"]:
            bad.append(name)
    return bad


def restore(dump, verify_only=False):
    z = Zent(wait_device(20))
    active = z.profile()
    log(f"firmware {z.version()} active profile {active}  ({'verify-only' if verify_only else 'RESTORE'})")
    rc = 0
    for p_str, regs in sorted(dump["profiles"].items()):
        p = int(p_str)
        z.switch_profile(p)
        if not verify_only:
            for name, r in regs.items():
                if name in rammap.READ_ONLY:
                    continue  # soft_version / cur_param_num: device-managed, not writable
                z.write_ram(r["addr"], bytes.fromhex(r["data"]))
            z.save()
            time.sleep(0.3)
        bad = verify_profile(z, regs)
        log(f"  profile {p}: {'OK (byte-identical)' if not bad else 'MISMATCH ' + ','.join(bad)}")
        if bad:
            rc = 1
    z.switch_profile(active)
    log(f"restored active profile {z.profile()}")
    z.close()
    return rc


def selftest():
    z = Zent(wait_device(20))
    active = z.profile()
    log(f"firmware {z.version()} active profile {active}  (SELF-TEST on config_name1)")
    original = z.read_ram(CONFIG_NAME1, 64)
    log(f"  original name: {original.split(bytes(1))[0][:16]!r}")
    probe = b"ZENT_RESTORE_TEST" + b"\x00" * (64 - 17)
    try:
        z.write_ram(CONFIG_NAME1, probe)
        z.save()
        time.sleep(0.3)
        got = z.read_ram(CONFIG_NAME1, 64)
        assert got == probe, "write+save did not take"
        log("  wrote probe name and read it back identical  [write/save path OK]")
    finally:
        z.write_ram(CONFIG_NAME1, original)
        z.save()
        time.sleep(0.3)
    back = z.read_ram(CONFIG_NAME1, 64)
    ok = back == original
    log(f"  restored original name, read-back identical: {ok}")
    z.close()
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dump", nargs="?", help="backups/<ts> directory or its dump.json")
    ap.add_argument("--verify", action="store_true", help="compare only; write nothing")
    ap.add_argument("--selftest", action="store_true",
                    help="harmless profile-name write/restore round trip")
    a = ap.parse_args()
    if a.selftest:
        rc = selftest()
    elif a.dump:
        rc = restore(load_dump(a.dump), verify_only=a.verify)
    else:
        ap.error("give a backups/<ts> dir, or --selftest")
    log("OK" if rc == 0 else "FAILED")
    sys.exit(rc)


if __name__ == "__main__":
    main()
