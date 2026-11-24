#!/usr/bin/env python3
"""Read-only dump of the ZENT 75 HE keymap over its vendor raw-HID interface.

Protocol taken from hoveyworks-driver.com (HidCmdReadRam): only command 0x04
(read RAM) is ever sent.
"""
from __future__ import annotations

import argparse

from rammap import KEY_COUNT, KEY_MAP, KEY_MAP_FN, KEY_MODE
from zentlib import Zent, unpack_i16


def _key_names() -> dict[int, str]:
    """Keymap code (signed i16) -> label, for the codes worth naming."""
    names = {-8: "LCtrl", -7: "LShift", -6: "LAlt", -5: "LWin", -4: "RCtrl", -3: "RShift",
             -2: "RAlt", -1: "RWin", 0: "NONE", 40: "Enter", 41: "Esc", 42: "Bksp", 43: "Tab",
             44: "Space", 57: "Caps", 79: "Right", 80: "Left", 81: "Down", 82: "Up", 118: "Menu",
             401: "FN", 402: "FN2", 403: "FN3", 404: "FN4", 316: "WinLock", 1000: "(empty)"}
    names.update({4 + i: chr(ord("A") + i) for i in range(26)})
    names.update({30 + i: c for i, c in enumerate("1234567890")})
    names.update({58 + i: f"F{i + 1}" for i in range(12)})
    names.update({45: "-", 46: "=", 47: "[", 48: "]", 49: "\\", 51: ";", 52: "'", 53: "`",
                  54: ",", 55: ".", 56: "/", 70: "Print", 71: "ScrLk", 72: "Pause", 73: "Ins",
                  74: "Home", 75: "PgUp", 76: "Del", 77: "End", 78: "PgDn"})
    return names


NAMES = _key_names()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("device", nargs="?", help="hidraw node (default: auto-detect)")
    args = ap.parse_args()

    with Zent(args.device) as z:
        print("firmware", z.version())
        print("active config", z.profile())
        for name, addr in (("main", KEY_MAP), ("fn", KEY_MAP_FN)):
            km = unpack_i16(z.read_ram(addr, 2 * KEY_COUNT))
            print(f"== {name} layer ({len(km)} keys)")
            print(" ".join(f"{i}:{NAMES.get(c, c)}" for i, c in enumerate(km)))
        print("== key_mode", list(z.read_ram(KEY_MODE, KEY_COUNT)))


if __name__ == "__main__":
    main()
