#!/usr/bin/env python3
"""Read-only dump of the ZENT 75 HE keymap over its vendor raw-HID interface.

Protocol taken from hoveyworks-driver.com (HidCmdReadRam): only command 0x04
(read RAM) is ever sent.
"""
import os, select, sys

DEV = sys.argv[1] if len(sys.argv) > 1 else "/dev/hidraw7"

NAMES = {-8: "LCtrl", -7: "LShift", -6: "LAlt", -5: "LWin", -4: "RCtrl", -3: "RShift",
         -2: "RAlt", -1: "RWin", 0: "NONE", 40: "Enter", 41: "Esc", 42: "Bksp", 43: "Tab",
         44: "Space", 57: "Caps", 79: "Right", 80: "Left", 81: "Down", 82: "Up", 118: "Menu",
         401: "FN", 402: "FN2", 403: "FN3", 404: "FN4", 316: "WinLock", 1000: "(empty)"}
for i in range(26):
    NAMES[4 + i] = chr(ord("A") + i)
for i, c in enumerate("1234567890"):
    NAMES[30 + i] = c
for i in range(12):
    NAMES[58 + i] = f"F{i+1}"
NAMES.update({45: "-", 46: "=", 47: "[", 48: "]", 49: "\\", 51: ";", 52: "'", 53: "`",
              54: ",", 55: ".", 56: "/", 70: "Print", 71: "ScrLk", 72: "Pause", 73: "Ins",
              74: "Home", 75: "PgUp", 76: "Del", 77: "End", 78: "PgDn"})


def read_ram(fd, addr, length):
    cmd = bytes([0xBE, 0x04, 0x0B, 0x00, addr & 0xFF, (addr >> 8) & 0xFF,
                 (addr >> 16) & 0xFF, (addr >> 24) & 0xFF, length >> 8, length & 0xFF, 0xED])
    os.write(fd, b"\x00" + cmd.ljust(1024, b"\x00"))
    for _ in range(50):
        r, _, _ = select.select([fd], [], [], 0.2)
        if not r:
            continue
        t = os.read(fd, 1025)
        if t[0] == 0xBE and t[1] in (1, 4):
            n = t[2] + 256 * t[3]
            if t[n - 1] == 0xED:
                return t[10:n - 3]
    raise TimeoutError(hex(addr))


def codes(buf):
    out = []
    for i in range(0, len(buf), 2):
        v = buf[i] | buf[i + 1] << 8
        if v & 0x8000:
            v -= 0x10000
        out.append(v)
    return out


fd = os.open(DEV, os.O_RDWR)
ver = read_ram(fd, 0x80070210, 4)
print("firmware", f"{ver[2]}.{ver[1]}.{ver[0]}")
print("active config", read_ram(fd, 0x80070218, 4)[0])
for name, addr in (("main", 0x80080160), ("fn", 0x80080210)):
    km = codes(read_ram(fd, addr, 166))
    print(f"== {name} layer ({len(km)} keys)")
    print(" ".join(f"{i}:{NAMES.get(c, c)}" for i, c in enumerate(km)))
mode = read_ram(fd, 0x800806E0, 83)
print("== key_mode", list(mode))
