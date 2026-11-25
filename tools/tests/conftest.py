"""Make tools/ importable and provide an in-process fake keyboard (no hardware involved)."""
from __future__ import annotations

import os
import socket
import sys
import threading

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import zentlib  # noqa: E402


class FakeKeyboard(threading.Thread):
    """Device side of the protocol on one end of a SOCK_SEQPACKET pair.

    A seqpacket socket keeps report boundaries, so the host end behaves like a hidraw fd for
    os.read / os.write / select. The fake holds one global image and four profile images and
    answers like the stock firmware; the fault knobs make it misbehave on purpose.
    """

    def __init__(self, sock: socket.socket) -> None:
        super().__init__(daemon=True)
        self.sock = sock
        self.mem_global = bytearray(0x10000)  # 0x8007xxxx
        self.mem_profiles = [bytearray(0x10000) for _ in range(4)]  # 0x8008xxxx
        self.flash_profiles = [bytearray(0x10000) for _ in range(4)]
        self.mem_global[0x210:0x214] = bytes([18, 0, 0, 0])  # 0.0.18
        self.active = 0
        self.serial = bytes(range(0x10, 0x20))
        self.requests: list[bytes] = []  # raw 1025-byte reports as received
        self.saves = 0
        self.debug_requests = 0
        self.console = b""  # what a debug read returns (our firmware only)
        # fault injection
        self.drop_replies = 0  # swallow the next N replies
        self.corrupt_replies = 0  # flip a data byte (CRC then fails) in the next N read replies
        self.stale_before_reply = False  # send a reply for another address first
        self.short_replies = False  # answer reads with one byte less than asked
        self.ignore_profile_switch = False
        self.ignore_writes = False  # ack but do not store

    def _mem(self, addr: int) -> tuple[bytearray, int]:
        if 0x80070000 <= addr < 0x80080000:
            return self.mem_global, addr - 0x80070000
        if 0x80080000 <= addr < 0x80090000:
            return self.mem_profiles[self.active], addr - 0x80080000
        raise AssertionError(f"request outside fake memory: {addr:#x}")

    def _reply(self, frame: bytes) -> None:
        if self.drop_replies > 0:
            self.drop_replies -= 1
            return
        self.sock.send(bytes(frame).ljust(zentlib.REPORT_SIZE, b"\x00"))  # no report id on input

    def run(self) -> None:
        while True:
            try:
                report = self.sock.recv(4096)
            except OSError:
                return
            if not report:
                return
            self.requests.append(report)
            assert len(report) == 1025 and report[0] == 0, "host must send report id 0 + 1024 bytes"
            self.handle(report[1:])

    def handle(self, r: bytes) -> None:
        cmd, n = r[1], r[2] | r[3] << 8
        assert r[0] == 0xBE and r[n - 1] == 0xED
        assert not any(r[n:]), "padding must be zero"
        if cmd in (zentlib.CMD_READ_RAM, zentlib.CMD_READ_FLASH):
            assert n == 11
            addr, length = int.from_bytes(r[4:8], "little"), int.from_bytes(r[8:10], "big")
            mem, off = self._mem(addr)
            if cmd == zentlib.CMD_READ_FLASH and mem is not self.mem_global:
                mem = self.flash_profiles[self.active]
            data = bytes(mem[off:off + length])
            if self.short_replies:
                data = data[:-1]
            if self.stale_before_reply:
                self._reply(zentlib.encode_data_frame(cmd, addr ^ 0x100, bytes(length)))
            out = bytearray(zentlib.encode_data_frame(cmd, addr, data))
            if self.corrupt_replies > 0:
                self.corrupt_replies -= 1
                out[10] ^= 0xFF
            self._reply(bytes(out))
        elif cmd == zentlib.CMD_WRITE_RAM:
            f = zentlib.decode_data_frame(r)
            if f is None or f.declared_len != len(f.data) or n != 13 + len(f.data):
                return  # bad CRC / framing: the firmware stays silent
            if not self.ignore_writes:
                mem, off = self._mem(f.addr)
                mem[off:off + len(f.data)] = f.data
            self._reply(zentlib.WRITE_ACK)
        elif cmd == zentlib.CMD_SWITCH_PROFILE:
            assert n == 6
            if not self.ignore_profile_switch and r[4] < 4:
                self.active = r[4]
                self.mem_global[0x218] = r[4]
                self._reply(zentlib.encode_simple(zentlib.CMD_SWITCH_PROFILE, r[4]))
        elif cmd == zentlib.CMD_SERIAL:
            self._reply(bytes([0xBE, 0x1D, 21, 0]) + self.serial + b"\xED")
        elif cmd in (zentlib.CMD_DEBUG_CONSOLE, zentlib.CMD_DEBUG_PROF_READ, zentlib.CMD_DEBUG_PROF_START):
            self.debug_requests += 1
            assert self.mem_global[0x212] >= 1, "debug command sent to a stock firmware"
            assert n == 13 and r[4:8] == b"ZDBG"
            arg = int.from_bytes(r[8:12], "little")
            data = self.console[arg:arg + 1011]
            self._reply(bytes([0xBE, cmd]) + (13 + len(data)).to_bytes(2, "little")
                        + len(self.console).to_bytes(4, "little") + arg.to_bytes(4, "little")
                        + data + b"\xED")
        elif cmd == zentlib.CMD_SAVE:
            self.saves += 1
            self.flash_profiles[self.active][:] = self.mem_profiles[self.active]


@pytest.fixture
def rig():
    """(Zent, FakeKeyboard) connected through a socketpair."""
    host, device = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    kb = FakeKeyboard(device)
    kb.start()
    z = zentlib.Zent(fd=host.detach())
    yield z, kb
    z.close()
    device.close()

