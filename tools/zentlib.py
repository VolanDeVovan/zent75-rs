#!/usr/bin/env python3
"""Shared raw-HID transport for the ZENT75-HE stock firmware (USB 36b0:357e, interface 2)."""
import glob, os, select, time

VID, PID = 0x36B0, 0x357E
N = 83


def crc16(buf):
    crc = 0xFFFF
    for b in buf:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return crc


def find_hidraw(iface=2):
    """Return /dev/hidrawN of the vendor raw-HID interface, or None."""
    for p in sorted(glob.glob("/sys/class/hidraw/hidraw*")):
        try:
            ue = open(p + "/device/uevent").read()
        except OSError:
            continue
        if f"HID_ID=0003:{VID:08X}:{PID:08X}" not in ue:
            continue
        real = os.path.realpath(p + "/device")
        # .../1-2:1.2/0003:36B0:357E.xxxx
        intf = os.path.basename(os.path.dirname(real))
        if intf.endswith(f".{iface}"):
            return "/dev/" + os.path.basename(p)
    return None


def wait_device(timeout=30.0, iface=2):
    end = time.time() + timeout
    while time.time() < end:
        d = find_hidraw(iface)
        if d and os.access(d, os.R_OK | os.W_OK):
            return d
        time.sleep(0.25)
    return None


class Zent:
    def __init__(self, dev=None):
        self.dev = dev or find_hidraw()
        if not self.dev:
            raise RuntimeError("ZENT75-HE raw HID interface not found")
        self.fd = os.open(self.dev, os.O_RDWR)

    def close(self):
        os.close(self.fd)

    def send(self, pkt):
        os.write(self.fd, b"\x00" + bytes(pkt).ljust(1024, b"\x00"))

    def drain(self):
        while select.select([self.fd], [], [], 0.02)[0]:
            os.read(self.fd, 1025)

    def recv(self, timeout=0.05):
        if select.select([self.fd], [], [], timeout)[0]:
            return os.read(self.fd, 1025)
        return None

    def wait(self, pred, timeout=1.0):
        end = time.time() + timeout
        while time.time() < end:
            t = self.recv()
            if t is not None:
                r = pred(t)
                if r is not None:
                    return r
        return None

    def _read(self, cmd, addr, length, timeout=1.0, tries=3):
        for _ in range(tries):
            self.drain()
            self.send([0xBE, cmd, 0x0B, 0x00, *addr.to_bytes(4, "little"), length >> 8, length & 0xFF, 0xED])

            def p(t):
                if t[0] == 0xBE and t[1] == cmd:
                    n = t[2] | t[3] << 8
                    if 13 <= n <= len(t) and t[n - 1] == 0xED and int.from_bytes(t[4:8], "little") == addr \
                            and crc16(t[:n - 3]) == (t[n - 3] | t[n - 2] << 8):
                        return bytes(t[10:n - 3])
            r = self.wait(p, timeout)
            if r is not None:
                return r
        raise RuntimeError(f"read cmd {cmd:#x} addr {addr:#x} len {length} failed")

    def read_ram(self, addr, length, **kw):
        return self._read(0x04, addr, length, **kw)

    def read_flash(self, addr, length, **kw):
        return self._read(0x01, addr, length, **kw)

    def read_ram_big(self, addr, length, chunk=512):
        out = bytearray()
        while len(out) < length:
            n = min(chunk, length - len(out))
            out += self.read_ram(addr + len(out), n)
        return bytes(out)

    def write_ram(self, addr, data, timeout=1.5):
        s = len(data)
        a = bytearray(10 + s + 3)
        a[0:10] = bytes([0xBE, 0x03, (13 + s) & 0xFF, (13 + s) >> 8, *addr.to_bytes(4, "little"), s >> 8, s & 0xFF])
        a[10:10 + s] = data
        c = crc16(a[:s + 10])
        a[s + 10], a[s + 11], a[s + 12] = c & 0xFF, c >> 8, 0xED
        self.drain()
        self.send(a)
        if not self.wait(lambda t: True if t[:5] == bytes([0xBE, 3, 5, 0, 0xED]) else None, timeout):
            raise RuntimeError(f"no ack for write {addr:#x}")

    def simple(self, cmd, *args):
        self.send([0xBE, cmd, 5 + len(args), 0, *args, 0xED])

    def save(self, settle=1.5):
        self.drain()
        self.simple(0x02)
        r = self.wait(lambda t: bytes(t[:8]) if t[0] == 0xBE and t[1] == 0x02 else None, settle)
        time.sleep(0.3)
        return r

    def switch_profile(self, n, settle=0.6):
        self.drain()
        self.simple(0xC0, n)
        time.sleep(settle)

    def version(self):
        v = self.read_ram(0x80070210, 4)
        return f"{v[2]}.{v[1]}.{v[0]}"

    def profile(self):
        return self.read_ram(0x80070218, 4)[0]

    def serial(self):
        self.drain()
        self.simple(0x1D)
        return self.wait(lambda t: bytes(t[:(t[2] | t[3] << 8)]) if t[0] == 0xBE and t[1] == 0x1D else None, 1.0)
