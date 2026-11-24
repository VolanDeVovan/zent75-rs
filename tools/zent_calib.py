#!/usr/bin/env python3
"""Recalibrate selected ZENT 75 HE keys, mirroring hoveyworks-driver.com:
handleStartCal -> HidCmdCalib(1) -> collect min/max -> HidCmdCalib(0)
-> HidCmdSetRam(top_value), HidCmdSetRam(bot_value) -> HidCmdSave.
Only the keys in KEYS are changed; every other key keeps its stored values.
"""
import json, os, select, sys, time

DEV = "/dev/hidraw7"
N = 83
KEYS = {59: "LShift", 73: "LCtrl"}
TOP, BOT = 0x80070000, 0x800700B0
COLLECT_S = 30
MIN_RANGE = 7000  # same acceptance threshold the driver uses
HERE = os.path.dirname(os.path.abspath(__file__))


def crc16(buf):
    crc = 0xFFFF
    for b in buf:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return crc


def send(fd, pkt):
    os.write(fd, b"\x00" + bytes(pkt).ljust(1024, b"\x00"))


def drain(fd):
    while select.select([fd], [], [], 0.02)[0]:
        os.read(fd, 1025)


def wait(fd, pred, timeout=1.0):
    end = time.time() + timeout
    while time.time() < end:
        if select.select([fd], [], [], 0.05)[0]:
            t = os.read(fd, 1025)
            r = pred(t)
            if r is not None:
                return r
    return None


def read_ram(fd, addr, length):
    drain(fd)
    send(fd, [0xBE, 0x04, 0x0B, 0x00, *addr.to_bytes(4, "little"), length >> 8, length & 0xFF, 0xED])

    def p(t):
        if t[0] == 0xBE and t[1] in (1, 4):
            n = t[2] + 256 * t[3]
            if t[n - 1] == 0xED and crc16(t[:n - 3]) == t[n - 3] | t[n - 2] << 8:
                return bytes(t[10:n - 3])
    r = wait(fd, p)
    if r is None:
        raise RuntimeError(f"read {addr:#x} failed")
    return r


def set_ram(fd, addr, data):
    s = len(data)
    a = bytearray(1024)
    a[0:10] = bytes([0xBE, 0x03, (13 + s) & 0xFF, (13 + s) >> 8, *addr.to_bytes(4, "little"), s >> 8, s & 0xFF])
    a[10:10 + s] = data
    c = crc16(a[:s + 10])
    a[s + 10], a[s + 11], a[s + 12] = c & 0xFF, c >> 8, 0xED
    drain(fd)
    send(fd, a)
    ok = wait(fd, lambda t: True if t[:5] == bytes([0xBE, 3, 5, 0, 0xED]) else None, 1.5)
    if not ok:
        raise RuntimeError(f"no ack for write {addr:#x}")


def u16s(b):
    return [b[i] | b[i + 1] << 8 for i in range(0, len(b), 2)]


def pack(vals):
    out = bytearray()
    for v in vals:
        out += bytes([v & 0xFF, v >> 8])
    return bytes(out)


fd = os.open(DEV, os.O_RDWR)
top = u16s(read_ram(fd, TOP, 2 * N))
bot = u16s(read_ram(fd, BOT, 2 * N))
bk = os.path.join(HERE, f"zent_calib_backup_{int(time.time())}.json")
json.dump({"top": top, "bot": bot}, open(bk, "w"))
print("backup ->", bk)
print("before:", {n: (top[k], bot[k]) for k, n in KEYS.items()})

hi = {k: 0 for k in KEYS}
lo = {k: 65535 for k in KEYS}
drain(fd)
send(fd, [0xBE, 0xFE, 0x06, 0x00, 0x01, 0xED])  # calibration on
print(f"CALIBRATING for {COLLECT_S}s: press LCtrl and LShift fully, several times", flush=True)
try:
    end = time.time() + COLLECT_S
    last = 0
    frames = 0
    while time.time() < end:
        if time.time() - last > 1:
            send(fd, [0xBE, 0x00, 0x05, 0x00, 0xED])
            last = time.time()
        if not select.select([fd], [], [], 0.1)[0]:
            continue
        t = os.read(fd, 1025)
        if t[0] != 0xBE or t[1] != 0xFD:
            continue
        a = t[2] + 256 * t[3]
        v = [t[2 * i] + 256 * t[2 * i + 1] for i in range(2, 6 * N + 2)] if a == 1003 else \
            [t[2 * i] + 256 * t[2 * i + 1] for i in range(2, 2 * N + 2)] if a == 1017 else None
        if v is None:
            continue
        frames += 1
        for k in KEYS:
            x = sum(v[k + j * N] for j in range(6)) // 6 if a == 1003 else v[k]
            hi[k] = max(hi[k], x)
            lo[k] = min(lo[k], x)
finally:
    send(fd, [0xBE, 0xFE, 0x06, 0x00, 0x00, 0xED])  # calibration off
    time.sleep(0.3)

print("frames", frames)
print("measured:", {n: (hi[k], lo[k], hi[k] - lo[k]) for k, n in KEYS.items()})
bad = [n for k, n in KEYS.items() if hi[k] - lo[k] < MIN_RANGE]
if bad:
    print("NOT WRITTEN: too little travel seen for", bad)
    sys.exit(1)

for k in KEYS:
    top[k], bot[k] = hi[k], lo[k]
set_ram(fd, TOP, pack(top))
set_ram(fd, BOT, pack(bot))
top2 = u16s(read_ram(fd, TOP, 2 * N))
bot2 = u16s(read_ram(fd, BOT, 2 * N))
assert top2 == top and bot2 == bot, "read-back mismatch"
print("written + verified:", {n: (top2[k], bot2[k]) for k, n in KEYS.items()})

drain(fd)
send(fd, [0xBE, 0x02, 0x05, 0x00, 0xED])  # HidCmdSave -> flash
time.sleep(1.2)
print("saved to flash")
