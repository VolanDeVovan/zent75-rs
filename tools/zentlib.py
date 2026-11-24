#!/usr/bin/env python3
"""Raw-HID codec and transport for the ZENT75-HE stock firmware (USB 36b0:357e, interface 2).

Frame format (README.md, "Protocol"; mirrors panel/src/protocol/zent/frames.ts)
------------------------------------------------------------------------------
Every frame is ``BE <cmd> <len LE16> ... ED``, zero-padded to a 1024-byte report. ``len``
counts the whole frame including ``BE`` and ``ED``. The interface uses unnumbered reports,
so a write to hidraw carries a leading ``00`` report-id byte (1025 bytes) and a read returns
the 1024 payload bytes without it.

    simple       BE <cmd> 05 00 ED                       heartbeat 00, save 02, serial 1D, ...
    one argument BE <cmd> 06 00 <arg> ED                 profile C0 <0-3>, calibration FE <0/1>
    read request BE <01|04> 0B 00 <addr LE32> <n BE16> ED
    data frame   BE <cmd> <13+n LE16> <addr LE32> <n BE16> <data: n bytes> <crc LE16> ED
                 offsets: [0]=BE [1]=cmd [2:4]=len [4:8]=addr [8:10]=n [10:10+n]=data
                          [10+n:12+n]=crc [12+n]=ED
                 host -> device as the write-RAM request (cmd 03, ack ``BE 03 05 00 ED``),
                 device -> host as the read reply (cmd 01 flash / 04 RAM).

With ``L`` the frame length from [2:4], the data of a read reply is ``[10:L-3]``, its CRC is
``[L-3:L-1]`` and ``[L-1]`` is ``ED``. The CRC is CRC-16/MODBUS (reflected poly 0xA001, init
0xFFFF, stored little-endian) over everything in front of it, i.e. ``[0:L-3]``. The byte
count ``n`` is big-endian, unlike every other multi-byte field.

The module is split in two: pure functions (bytes in, bytes out; unit-tested in
tools/tests without hardware) and the :class:`Zent` transport that does the hidraw I/O.
Only the standard library is used; raw ``/dev/hidraw`` through ``os.read``/``os.write`` is
deliberate.
"""
from __future__ import annotations

import glob
import os
import select
import time
from typing import Callable, NamedTuple, Optional, Sequence, TypeVar

import rammap

VID, PID = 0x36B0, 0x357E
RAW_HID_INTERFACE = 2
N = rammap.KEY_COUNT  # 83 keys

REPORT_SIZE = 1024
REPORT_ID = 0x00
SOF, EOF = 0xBE, 0xED

CMD_HEARTBEAT = 0x00
CMD_READ_FLASH = 0x01
CMD_SAVE = 0x02
CMD_WRITE_RAM = 0x03
CMD_READ_RAM = 0x04
CMD_SERIAL = 0x1D
CMD_SWITCH_PROFILE = 0xC0
CMD_RESET_CONFIG = 0xEE
CMD_BOOTLOADER = 0xFC
CMD_STREAM = 0xFD
CMD_CALIBRATION = 0xFE
CMD_REBOOT = 0xFF

SIMPLE_FRAME_LEN = 5
READ_REQUEST_LEN = 11
DATA_FRAME_OVERHEAD = 13  # header 4 + addr 4 + n 2 + crc 2 + ED 1
MAX_DATA_PER_FRAME = REPORT_SIZE - DATA_FRAME_OVERHEAD  # 1011: what physically fits
#: Largest block the tools put into one frame. The vendor driver never moves more than
#: 255/256 bytes at once, and 255 (the rgb table) is the largest size proven on the board.
MAX_CHUNK = 255
SERIAL_REPLY_LEN = 21  # BE 1D 15 00 <16 bytes> ED
STREAM_LEN_TRAVEL = 1017  # 83 raw ADC u16, then 83 travel u16 (um)
STREAM_LEN_RAW6 = 1003  # calibration mode: 6 scans x 83 raw ADC u16

WRITE_ACK = bytes([SOF, CMD_WRITE_RAM, SIMPLE_FRAME_LEN, 0x00, EOF])

T = TypeVar("T")


class ZentError(RuntimeError):
    """The device did not answer, or answered something else than what was asked for."""


# ---------------------------------------------------------------------------- pure codec

def crc16(buf: bytes) -> int:
    """CRC-16/MODBUS: reflected poly 0xA001, init 0xFFFF, no final xor."""
    crc = 0xFFFF
    for b in buf:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return crc


def _check_addr(addr: int) -> None:
    if not 0 <= addr <= 0xFFFFFFFF:
        raise ValueError(f"bad address {addr:#x}")


def encode_simple(cmd: int, *args: int) -> bytes:
    """``BE <cmd> <5+len(args) LE16> <args...> ED`` (no CRC)."""
    n = SIMPLE_FRAME_LEN + len(args)
    return bytes([SOF, cmd, n & 0xFF, n >> 8, *args, EOF])


def encode_read(cmd: int, addr: int, length: int) -> bytes:
    """``BE <cmd> 0B 00 <addr LE32> <length BE16> ED`` for cmd 01 (flash) / 04 (RAM)."""
    _check_addr(addr)
    if not 1 <= length <= MAX_DATA_PER_FRAME:
        raise ValueError(f"read length {length} out of range 1..{MAX_DATA_PER_FRAME}")
    return bytes([SOF, cmd, READ_REQUEST_LEN, 0x00, *addr.to_bytes(4, "little"),
                  *length.to_bytes(2, "big"), EOF])


def encode_data_frame(cmd: int, addr: int, data: bytes) -> bytes:
    """``BE <cmd> <13+n LE16> <addr LE32> <n BE16> <data> <crc LE16> ED``.

    The write-RAM request (cmd 03) and the read replies (cmd 01 / 04) share this shape.
    The CRC covers the first 10+n bytes.
    """
    _check_addr(addr)
    n = len(data)
    if not 1 <= n <= MAX_DATA_PER_FRAME:
        raise ValueError(f"data length {n} out of range 1..{MAX_DATA_PER_FRAME}")
    body = bytes([SOF, cmd]) + (DATA_FRAME_OVERHEAD + n).to_bytes(2, "little") \
        + addr.to_bytes(4, "little") + n.to_bytes(2, "big") + bytes(data)
    return body + crc16(body).to_bytes(2, "little") + bytes([EOF])


def encode_write(addr: int, data: bytes) -> bytes:
    """Write-RAM request: ``BE 03 <13+n LE16> <addr LE32> <n BE16> <data> <crc LE16> ED``."""
    return encode_data_frame(CMD_WRITE_RAM, addr, data)


def to_report(frame: bytes) -> bytes:
    """Frame -> the 1025 bytes written to hidraw: report id 0, then the zero-padded frame."""
    if len(frame) > REPORT_SIZE:
        raise ValueError(f"frame of {len(frame)} bytes exceeds one {REPORT_SIZE}-byte report")
    return bytes([REPORT_ID]) + bytes(frame).ljust(REPORT_SIZE, b"\x00")


def normalize_report(report: bytes) -> bytes:
    """Strip a leading report-id 0 if a backend delivered 1025 bytes (hidraw itself does not)."""
    if len(report) == REPORT_SIZE + 1 and report[0] == REPORT_ID and report[1] == SOF:
        return report[1:]
    return report


def frame_len(report: bytes) -> Optional[int]:
    """Length field of a frame, or None when the report is not a plausible ``BE..`` frame."""
    if len(report) < SIMPLE_FRAME_LEN or report[0] != SOF:
        return None
    n = report[2] | report[3] << 8
    return n if SIMPLE_FRAME_LEN <= n <= len(report) else None


class DataFrame(NamedTuple):
    """A decoded data frame (read reply or write request)."""
    cmd: int
    addr: int
    declared_len: int  # the BE16 at [8:10]
    data: bytes  # [10:L-3]


def decode_data_frame(report: bytes) -> Optional[DataFrame]:
    """Decode ``BE <cmd> <L LE16> <addr LE32> <n BE16> <data> <crc LE16> ED``.

    Returns None unless the length is plausible, ``[L-1]`` is ``ED`` and the CRC over
    ``[0:L-3]`` matches ``[L-3:L-1]``. The data is ``[10:L-3]``, exactly as the vendor driver
    slices it; ``declared_len`` is reported but not enforced here.
    """
    n = frame_len(report)
    if n is None or n < DATA_FRAME_OVERHEAD + 1 or report[n - 1] != EOF:
        return None
    if crc16(report[:n - 3]) != (report[n - 3] | report[n - 2] << 8):
        return None
    return DataFrame(report[1], int.from_bytes(report[4:8], "little"),
                     int.from_bytes(report[8:10], "big"), bytes(report[10:n - 3]))


def match_read_reply(report: bytes, cmd: int, addr: int, length: int) -> Optional[bytes]:
    """Data of the reply to ``encode_read(cmd, addr, length)``, or None if it is not that reply.

    The command, the echoed address and the number of data bytes must all match the request,
    so a stale or truncated reply can never be mistaken for the answer.
    """
    f = decode_data_frame(normalize_report(report))
    if f is None or f.cmd != cmd or f.addr != addr or len(f.data) != length:
        return None
    return f.data


def is_write_ack(report: bytes) -> bool:
    """``BE 03 05 00 ED``."""
    return bytes(normalize_report(report)[:SIMPLE_FRAME_LEN]) == WRITE_ACK


def decode_serial(report: bytes) -> Optional[bytes]:
    """The whole serial reply frame ``BE 1D 15 00 <16 bytes> ED`` (21 bytes), or None."""
    r = normalize_report(report)
    if frame_len(r) != SERIAL_REPLY_LEN or r[1] != CMD_SERIAL or r[SERIAL_REPLY_LEN - 1] != EOF:
        return None
    return bytes(r[:SERIAL_REPLY_LEN])


def decode_profile_notice(report: bytes) -> Optional[int]:
    """Profile index of a pushed ``BE C0 06 00 <n> ED``, or None."""
    r = normalize_report(report)
    if frame_len(r) != SIMPLE_FRAME_LEN + 1 or r[1] != CMD_SWITCH_PROFILE:
        return None
    return r[4]


class StreamFrame(NamedTuple):
    """One ``BE FD`` live frame."""
    length: int  # STREAM_LEN_TRAVEL or STREAM_LEN_RAW6
    raw: list[int]  # raw ADC per key (RAW6: floor of the mean of the 6 scans)
    travel: Optional[list[int]]  # travel in um per key; None in a RAW6 frame


def _words(buf: bytes, offset: int, count: int) -> list[int]:
    return [buf[offset + 2 * i] | buf[offset + 2 * i + 1] << 8 for i in range(count)]


def decode_stream(report: bytes, keys: int = N) -> Optional[StreamFrame]:
    """Decode a ``BE FD`` frame; None for anything else. The payload starts at [4]."""
    r = normalize_report(report)
    if len(r) < 4 or r[0] != SOF or r[1] != CMD_STREAM:
        return None
    n = r[2] | r[3] << 8
    if n == STREAM_LEN_TRAVEL and len(r) >= 4 + 4 * keys:
        w = _words(r, 4, 2 * keys)
        return StreamFrame(n, w[:keys], w[keys:])
    if n == STREAM_LEN_RAW6 and len(r) >= 4 + 12 * keys:
        w = _words(r, 4, 6 * keys)
        return StreamFrame(n, [sum(w[k + j * keys] for j in range(6)) // 6 for k in range(keys)], None)
    return None


def format_version(raw: bytes) -> str:
    """soft_version bytes ``<patch> <minor> <major> 00`` -> "major.minor.patch"."""
    return f"{raw[2]}.{raw[1]}.{raw[0]}"


def unpack_u16(buf: bytes) -> list[int]:
    """Little-endian u16 table -> list."""
    if len(buf) % 2:
        raise ValueError("odd number of bytes in a u16 table")
    return _words(buf, 0, len(buf) // 2)


def unpack_i16(buf: bytes) -> list[int]:
    """Little-endian i16 table -> list (keymap codes are signed)."""
    return [v - 0x10000 if v & 0x8000 else v for v in unpack_u16(buf)]


def pack_u16(values: Sequence[int]) -> bytes:
    """List -> little-endian u16 table. Raises on values outside 0..65535."""
    return b"".join(v.to_bytes(2, "little") for v in values)


# ---------------------------------------------------------------------------- device discovery

def find_hidraw(iface: int = RAW_HID_INTERFACE) -> Optional[str]:
    """Return /dev/hidrawN of the vendor raw-HID interface, or None."""
    for p in sorted(glob.glob("/sys/class/hidraw/hidraw*")):
        try:
            with open(p + "/device/uevent") as f:
                ue = f.read()
        except OSError:
            continue
        if f"HID_ID=0003:{VID:08X}:{PID:08X}" not in ue:
            continue
        # .../1-2:1.2/0003:36B0:357E.xxxx -> the USB interface directory is "1-2:1.<iface>"
        intf = os.path.basename(os.path.dirname(os.path.realpath(p + "/device")))
        if intf.rpartition(".")[2] == str(iface):
            return "/dev/" + os.path.basename(p)
    return None


def wait_device(timeout: float = 30.0, iface: int = RAW_HID_INTERFACE) -> Optional[str]:
    """Wait until the raw-HID node exists and is readable and writable; None on timeout."""
    end = time.monotonic() + timeout
    while True:
        d = find_hidraw(iface)
        if d and os.access(d, os.R_OK | os.W_OK):
            return d
        if time.monotonic() >= end:
            return None
        time.sleep(0.25)


# ---------------------------------------------------------------------------- transport

class Zent:
    """Blocking request/reply transport on the vendor raw-HID interface.

    Reads and writes are confined to the README RAM map (``rammap``) because requests at
    unmapped addresses have hung the stock firmware; pass ``unchecked=True`` to lift that.
    """

    def __init__(self, dev: Optional[str] = None, *, fd: Optional[int] = None) -> None:
        """Open ``dev`` (default: auto-detect). ``fd`` adopts an already open descriptor."""
        if fd is not None:
            self.dev, self.fd = dev or f"<fd {fd}>", fd
            return
        self.dev = dev or find_hidraw()
        if not self.dev:
            raise ZentError("ZENT75-HE raw HID interface not found")
        self.fd = os.open(self.dev, os.O_RDWR)

    def __enter__(self) -> "Zent":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def close(self) -> None:
        if self.fd >= 0:
            os.close(self.fd)
            self.fd = -1

    # ---- raw reports

    def send(self, frame: Sequence[int]) -> None:
        """Send one frame as a full report (report id 0 + frame zero-padded to 1024)."""
        report = to_report(bytes(frame))
        written = os.write(self.fd, report)
        if written != len(report):
            raise ZentError(f"short hidraw write: {written} of {len(report)} bytes")

    def recv(self, timeout: float = 0.05) -> Optional[bytes]:
        """One input report, or None when nothing arrives within ``timeout`` seconds."""
        if select.select([self.fd], [], [], max(0.0, timeout))[0]:
            return os.read(self.fd, REPORT_SIZE + 1)
        return None

    def drain(self, quiet: float = 0.02, limit: float = 0.5) -> None:
        """Discard pending input until the line is quiet for ``quiet`` s (at most ``limit`` s,
        so a running ``BE FD`` stream cannot hold this up forever)."""
        end = time.monotonic() + limit
        while time.monotonic() < end and self.recv(quiet) is not None:
            pass

    def wait(self, pred: Callable[[bytes], Optional[T]], timeout: float = 1.0) -> Optional[T]:
        """Feed input reports to ``pred`` until it returns something other than None."""
        end = time.monotonic() + timeout
        while True:
            left = end - time.monotonic()
            if left <= 0:
                return None
            t = self.recv(min(left, 0.05))
            if t:
                r = pred(t)
                if r is not None:
                    return r

    def simple(self, cmd: int, *args: int) -> None:
        """Send ``BE <cmd> <len> <args...> ED`` without waiting for anything."""
        self.send(encode_simple(cmd, *args))

    # ---- memory

    @staticmethod
    def _check_map(what: str, addr: int, length: int, unchecked: bool) -> None:
        if not unchecked and not rammap.in_map(addr, length):
            raise ValueError(f"{what} {addr:#x}+{length} is outside the known RAM map "
                             "(unmapped requests have hung the firmware; unchecked=True to force)")

    def _read(self, cmd: int, addr: int, length: int, timeout: float = 1.0, tries: int = 3,
              unchecked: bool = False) -> bytes:
        self._check_map("read", addr, length, unchecked)
        request = encode_read(cmd, addr, length)
        for _ in range(tries):
            self.drain()
            self.send(request)
            r = self.wait(lambda t: match_read_reply(t, cmd, addr, length), timeout)
            if r is not None:
                return r
        raise ZentError(f"read cmd {cmd:#x} addr {addr:#x} len {length} failed")

    def read_ram(self, addr: int, length: int, **kw) -> bytes:
        """``BE 04``: exactly ``length`` bytes of RAM (one frame). Idempotent, retried."""
        return self._read(CMD_READ_RAM, addr, length, **kw)

    def read_flash(self, addr: int, length: int, **kw) -> bytes:
        """``BE 01``: exactly ``length`` bytes of the saved (flash) copy, same addressing."""
        return self._read(CMD_READ_FLASH, addr, length, **kw)

    def read_ram_big(self, addr: int, length: int, chunk: int = MAX_CHUNK, **kw) -> bytes:
        """Read ``length`` bytes of RAM in frames of at most ``chunk`` bytes."""
        if length < 1 or not 1 <= chunk <= MAX_DATA_PER_FRAME:
            raise ValueError(f"bad length {length} / chunk {chunk}")
        self._check_map("read", addr, length, kw.get("unchecked", False))
        out = bytearray()
        while len(out) < length:
            out += self.read_ram(addr + len(out), min(chunk, length - len(out)), **kw)
        return bytes(out)

    def write_ram(self, addr: int, data: bytes, timeout: float = 1.5, verify: bool = True,
                  chunk: int = MAX_CHUNK, unchecked: bool = False) -> None:
        """``BE 03``: write ``data`` to RAM (not to flash; see :meth:`save`).

        Sent in frames of at most ``chunk`` bytes, each acknowledged by ``BE 03 05 00 ED``.
        The ack names no address, so with ``verify`` every frame is read back and compared;
        a mismatch or a missing ack raises :class:`ZentError`. A write is never retried here.
        """
        data = bytes(data)
        if not data or not 1 <= chunk <= MAX_DATA_PER_FRAME:
            raise ValueError(f"bad data length {len(data)} / chunk {chunk}")
        self._check_map("write", addr, len(data), unchecked)
        if not unchecked and rammap.overlaps_read_only(addr, len(data)):
            raise ValueError(f"write {addr:#x}+{len(data)} touches a firmware-owned region "
                             "(soft_version / cur_param_num)")
        for off in range(0, len(data), chunk):
            part, a = data[off:off + chunk], addr + off
            self.drain()
            self.send(encode_write(a, part))
            if not self.wait(lambda t: True if is_write_ack(t) else None, timeout):
                raise ZentError(f"no ack for write {a:#x}+{len(part)}")
            if verify:
                back = self.read_ram(a, len(part), unchecked=unchecked)
                if back != part:
                    bad = next(i for i in range(len(part)) if back[i] != part[i])
                    raise ZentError(f"write verify failed at {a + bad:#x}: "
                                    f"wrote {part[bad]:#04x}, read {back[bad]:#04x}")

    # ---- commands

    def save(self, settle: float = 1.5) -> Optional[bytes]:
        """``BE 02``: commit RAM to flash, then keep quiet for ``settle`` seconds.

        The firmware sends no ack (the vendor driver just waits 1100 ms). Should a ``BE 02``
        frame show up anyway, its first 8 bytes are returned; normally the result is None.
        """
        self.drain()
        end = time.monotonic() + settle
        self.simple(CMD_SAVE)
        r = self.wait(lambda t: bytes(t[:8]) if t[:2] == bytes([SOF, CMD_SAVE]) else None, settle)
        time.sleep(max(0.0, end - time.monotonic()) + 0.3)
        return r

    def switch_profile(self, n: int, settle: float = 0.6, verify: bool = True,
                       timeout: float = 3.0) -> None:
        """``BE C0 <n>``: make profile ``n`` (0-3) the active one.

        Unsaved RAM edits of the old profile are lost. After ``settle`` seconds the switch is
        confirmed by reading cur_param_num (polled for up to ``timeout`` s); a board that
        stays on another profile raises :class:`ZentError` instead of silently handing out
        the wrong profile's memory.
        """
        if n not in range(rammap.PROFILE_COUNT):
            raise ValueError(f"profile {n} out of range 0..{rammap.PROFILE_COUNT - 1}")
        self.drain()
        self.simple(CMD_SWITCH_PROFILE, n)
        time.sleep(settle)
        if not verify:
            return
        end = time.monotonic() + timeout
        while True:
            got = self.profile()
            if got == n:
                return
            if time.monotonic() >= end:
                raise ZentError(f"profile switch to {n} not confirmed (board reports {got})")
            time.sleep(0.25)

    def heartbeat(self) -> None:
        """``BE 00``: while it keeps arriving (about 1/s) the board streams ``BE FD`` frames."""
        self.simple(CMD_HEARTBEAT)

    def calibration(self, on: bool) -> None:
        """``BE FE <1/0>``: calibration mode (the stream turns into 6-scan raw frames)."""
        self.simple(CMD_CALIBRATION, 1 if on else 0)

    def version_raw(self) -> bytes:
        """soft_version: ``<patch> <minor> <major> 00``."""
        return self.read_ram(rammap.SOFT_VERSION, 4)

    def version(self) -> str:
        """Firmware version as "major.minor.patch"."""
        return format_version(self.version_raw())

    def profile(self) -> int:
        """Index of the active profile (cur_param_num)."""
        return self.read_ram(rammap.CUR_PARAM_NUM, 4)[0]

    def serial(self, timeout: float = 1.0) -> Optional[bytes]:
        """``BE 1D``: the raw 21-byte reply frame ``BE 1D 15 00 <16 bytes> ED``, or None."""
        self.drain()
        self.simple(CMD_SERIAL)
        return self.wait(decode_serial, timeout)
