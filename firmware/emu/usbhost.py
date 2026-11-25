"""A scripted USB host for the emulated ChipIdea device controller (`periph.Usb`).

It plays the bus events a real host produces around enumeration, re-enumeration and reboot, and
fails the gate when the device stops answering. The firmware side is the real one: hpm-hal's
USB driver, embassy-usb and the app's tasks, driven through the controller's registers, queue
heads and transfer descriptors.

    host = UsbHost(m)            # attaches to the USB0 model
    host.start(SCENARIOS["reboot"])

A scenario is a generator function taking the host; `yield` waits one host step (20 µs). The
helpers below are generators too (`yield from`).

What is modelled, and what is an assumption (the chip was not measured for these):

- SETUP: the 8 bytes go to queue head 0, ENDPTSETUPSTAT bit 0 and USBSTS.UI are set. While the
  bit is set, ep0 answers no IN/OUT token (setup lockout, USBMODE.SLOM = 0). Stages primed for
  an earlier request stay primed afterwards (the driver has to flush them) — pessimistic.
- IN / OUT tokens: served from the endpoint's first transfer descriptor if the endpoint is
  primed, else NAK. A completed descriptor clears ENDPTSTAT, sets ENDPTCOMPLETE and USBSTS.UI.
- Bus reset: USBSTS.URI, DEVICEADDR = 0, port connected at high speed. Primes, ENDPTSETUPSTAT
  and ENDPTCOMPLETE are left for the driver to clean up, as the reference manual asks.
- Suspend / resume: PORTSC1.SUSP and USBSTS.SLI.
- Session-valid change (a VBUS glitch that the board survives): OTGSC.ASVIS, which interrupts
  only if the driver enabled ASVIE; the interrupt line is level (`Usb.irq_asserted`).
- No packet timing, no data toggles, no stalls: a stalled ep0 simply never answers and the
  host gives the request up after `timeout_ms`.
"""
from __future__ import annotations

import struct

from machine import CPU_HZ, FAIL

STEP_US = 20
QH_SIZE, QH_NEXT, QH_TOKEN, QH_SETUP = 64, 8, 12, 40
TD_TOKEN, TD_BUF0 = 4, 8
TOKEN_ACTIVE, TOKEN_IOC = 1 << 7, 1 << 15
PORT_CCS, PORT_SUSP, PORT_HS = 1 << 0, 1 << 7, 2 << 26

GET_DESCRIPTOR, SET_ADDRESS, SET_CONFIGURATION, SET_REPORT, SET_PROTOCOL = 6, 5, 9, 9, 11
VENDOR_OUT, VENDOR_IN, KEYBOARD_IN = 3, 2, 1
#: `BE 04`: read the 4 bytes of soft_version (docs/protocol.md)
VENDOR_READ_VERSION = bytes.fromhex("be040b00100207800004ed")


class HostError(Exception):
    pass


class UsbHost:
    def __init__(self, m) -> None:
        self.m = m
        self.usb = next(x for x in m.bus.models if x.name == "USB0")
        self.log: list[str] = []
        self.done = False
        self.failed: str | None = None

    # ---- scheduling
    def start(self, scenario, at_ms: float = 0.0) -> None:
        gen = scenario(self)
        step = STEP_US * CPU_HZ // 1_000_000

        def tick():
            try:
                next(gen)
            except StopIteration:
                self.done = True
                self.note("scenario finished")
                return
            except HostError as e:
                self.failed = str(e)
                self.m.note(FAIL, "usb-host", str(e))
                return
            self.m.schedule(self.m.cycles + step, tick)
        self.m.schedule(int(at_ms * CPU_HZ / 1000), tick)

    def note(self, msg: str) -> None:
        self.log.append(f"{self.m.ms():9.3f} ms  {msg}")
        self.m.event("usb-host", msg)

    def wait_ms(self, ms: float):
        end = self.m.cycles + int(ms * CPU_HZ / 1000)
        while self.m.cycles < end:
            yield

    def wait_for(self, cond, timeout_ms: float, what: str):
        end = self.m.cycles + int(timeout_ms * CPU_HZ / 1000)
        while not cond():
            if self.m.cycles >= end:
                raise HostError(f"device did not {what} within {timeout_ms:g} ms")
            yield

    # ---- controller access
    def reg(self, off: int) -> int:
        return self.usb.regs.get(off, 0)

    def rd32(self, addr: int) -> int:
        return struct.unpack("<I", self.m.uc.mem_read(addr, 4))[0]

    def wr32(self, addr: int, v: int) -> None:
        self.m.uc.mem_write(addr, struct.pack("<I", v & 0xFFFFFFFF))

    def qh(self, ep: int, is_in: bool) -> int:
        return (self.reg(self.usb.ENDPTLISTADDR) & ~0x7FF) + QH_SIZE * (2 * ep + int(is_in))

    def attached(self) -> bool:
        return bool(self.reg(self.usb.USBCMD) & 1)

    def primed(self, ep: int, is_in: bool) -> bool:
        return bool(self.reg(self.usb.ENDPTSTAT) >> (ep + (16 if is_in else 0)) & 1)

    def enabled(self, ep: int, is_in: bool) -> bool:
        return bool(self.reg(self.usb.ENDPTCTRL + 4 * ep) >> (23 if is_in else 7) & 1)

    # ---- bus events
    def bus_reset(self) -> None:
        u = self.usb
        self.note("bus reset")
        u.regs[u.DEVICEADDR] = 0
        u.regs[u.PORTSC1] = (u.regs.get(u.PORTSC1, 0) & ~PORT_SUSP) | PORT_CCS | PORT_HS
        u.set_status(u.STS_URI | u.STS_PCI)

    def suspend(self) -> None:
        u = self.usb
        self.note("suspend")
        u.regs[u.PORTSC1] = u.regs.get(u.PORTSC1, 0) | PORT_SUSP
        u.set_status(u.STS_SLI)

    def resume(self) -> None:
        u = self.usb
        self.note("resume")
        u.regs[u.PORTSC1] = u.regs.get(u.PORTSC1, 0) & ~PORT_SUSP
        u.set_status(u.STS_PCI)

    def setup(self, bm: int, req: int, value: int, index: int, length: int) -> None:
        u = self.usb
        self.m.uc.mem_write(self.qh(0, False) + QH_SETUP, struct.pack("<BBHHH", bm, req, value, index, length))
        u.regs[u.ENDPTSETUPSTAT] = u.regs.get(u.ENDPTSETUPSTAT, 0) | 1
        u.set_status(u.STS_UI)

    def _token(self, ep: int, is_in: bool, data: bytes | None, limit: int = 1 << 15) -> bytes | None:
        """Serve one token from the endpoint's transfer descriptor; None = NAK."""
        u = self.usb
        if not self.primed(ep, is_in) or (ep == 0 and self.reg(u.ENDPTSETUPSTAT) & 1):
            return None
        qh = self.qh(ep, is_in)
        nxt = self.rd32(qh + QH_NEXT)
        td = nxt & ~0x1F
        if nxt & 1 or td == 0:
            return None
        token = self.rd32(td + TD_TOKEN)
        if not token & TOKEN_ACTIVE:
            return None
        total = (token >> 16) & 0x7FFF
        buf = self.rd32(td + TD_BUF0)
        if is_in:
            n = min(total, limit)
            out = bytes(self.m.uc.mem_read(buf, n)) if n else b""
        else:
            n = min(total, len(data))
            if n:
                self.m.uc.mem_write(buf, bytes(data[:n]))
            out = b""
        left = total - n
        mps = (self.rd32(qh) >> 16) & 0x7FF or 64
        if is_in and left and n == limit and n % mps == 0:
            # more packets of this descriptor to come: stay primed
            self.wr32(td + TD_TOKEN, (token & 0x8000FFFF) | left << 16)
            self.wr32(td + TD_BUF0, buf + n)
            return out
        token = (token & 0x8000FF7F & ~TOKEN_ACTIVE) | left << 16
        self.wr32(td + TD_TOKEN, token)
        self.wr32(qh + QH_TOKEN, token)
        bit = 1 << (ep + (16 if is_in else 0))
        u.regs[u.ENDPTSTAT] = u.regs.get(u.ENDPTSTAT, 0) & ~bit
        u.regs[u.ENDPTCOMPLETE] = u.regs.get(u.ENDPTCOMPLETE, 0) | bit
        if token & TOKEN_IOC:
            u.set_status(u.STS_UI)
        return out

    # ---- transfers
    def take_in(self, ep: int, timeout_ms: float = 50, limit: int = 1 << 15, what: str | None = None):
        """One IN transfer descriptor's worth of data (or its first `limit` bytes)."""
        got = []
        yield from self.wait_for(lambda: got.append(self._token(ep, True, None, limit)) or got[-1] is not None,
                                 timeout_ms, what or f"answer IN {ep}")
        return got[-1]

    def send_out(self, ep: int, data: bytes, timeout_ms: float = 50, what: str | None = None):
        yield from self.wait_for(lambda: self._token(ep, False, data) is not None,
                                 timeout_ms, what or f"take OUT {ep}")

    def control_in(self, bm: int, req: int, value: int, index: int, length: int, timeout_ms: float = 50):
        self.setup(bm, req, value, index, length)
        what = f"answer control IN request {req:#x} value {value:#06x}"
        data = b""
        while len(data) < length:
            part = yield from self.take_in(0, timeout_ms, what=what)
            data += part
            if len(part) % 64 or not part:
                break
        yield from self.send_out(0, b"", timeout_ms, what=what + " (status stage)")
        return data[:length]

    def control_out(self, bm: int, req: int, value: int, index: int, data: bytes = b"", timeout_ms: float = 50):
        self.setup(bm, req, value, index, len(data))
        what = f"answer control OUT request {req:#x} value {value:#06x}"
        if data:
            yield from self.send_out(0, data, timeout_ms, what=what + " (data stage)")
        yield from self.take_in(0, timeout_ms, what=what + " (status stage)")

    # ---- sequences
    def enumerate(self, boot_protocol: bool = False):
        """Reset, read the descriptors, address, configure: what every host does first."""
        self.bus_reset()
        yield from self.wait_ms(15)
        dev = yield from self.control_in(0x80, GET_DESCRIPTOR, 0x0100, 0, 64)
        if len(dev) != 18 or dev[1] != 1:
            raise HostError(f"bad device descriptor {dev.hex()}")
        self.bus_reset()  # Linux and Windows reset a second time before SET_ADDRESS
        yield from self.wait_ms(15)
        yield from self.control_out(0x00, SET_ADDRESS, 5, 0)
        yield from self.wait_ms(2)
        head = yield from self.control_in(0x80, GET_DESCRIPTOR, 0x0200, 0, 9)
        total = head[2] | head[3] << 8
        cfg = yield from self.control_in(0x80, GET_DESCRIPTOR, 0x0200, 0, total)
        if len(cfg) != total or cfg[1] != 2:
            raise HostError(f"bad configuration descriptor ({len(cfg)} of {total} bytes)")
        yield from self.control_out(0x00, SET_CONFIGURATION, cfg[5], 0)
        if boot_protocol:
            yield from self.control_out(0x21, SET_PROTOCOL, 0, 0)
        yield from self.wait_for(lambda: self.enabled(KEYBOARD_IN, True) and self.enabled(VENDOR_OUT, False),
                                 20, "enable its endpoints after SET_CONFIGURATION")
        self.note(f"enumerated: {dev[8] | dev[9] << 8:04x}:{dev[10] | dev[11] << 8:04x}, "
                  f"{cfg[4]} interfaces, configuration descriptor {total} bytes")
        return cfg

    def vendor_round_trip(self):
        """`BE 04` on the vendor OUT endpoint must be answered on the vendor IN endpoint."""
        yield from self.send_out(VENDOR_OUT, VENDOR_READ_VERSION.ljust(1024, b"\0"), what="take a vendor request")
        reply = yield from self.take_in(VENDOR_IN, what="answer a vendor request")
        if reply[:2] != b"\xBE\x04":
            raise HostError(f"vendor reply is not BE 04: {reply[:16].hex()}")
        if len(reply) == 1024:  # hpm-hal sends a zero-length packet after a full one
            yield from self.take_in(VENDOR_IN, what="finish a vendor reply")
        self.note(f"vendor BE 04 answered, soft_version {reply[10:14].hex()}")


# ------------------------------------------------------------------ scenarios

def scenario_enumerate(h: UsbHost):
    yield from h.wait_for(h.attached, 1500, "attach to the bus")
    yield from h.wait_ms(20)
    yield from h.enumerate()
    yield from h.vendor_round_trip()


def scenario_vbus_glitch(h: UsbHost):
    """The session-valid comparator trips while the board stays powered and the host is silent
    (ports often dip VBUS when the host reboots, and nothing talks to the device for seconds).
    The device must keep running — the silence is longer than the watchdog — and answer after."""
    yield from h.wait_for(h.attached, 1500, "attach to the bus")
    yield from h.wait_ms(20)
    yield from h.enumerate()
    yield from h.wait_ms(5)  # on its own: no other USB interrupt pending at the same moment
    h.usb.session_change()
    h.note(f"session-valid change: OTGSC {h.reg(h.usb.OTGSC):#010x}, interrupt "
           f"{'asserted' if h.usb.irq_asserted() else 'not enabled'}; the host stays silent for 2.2 s")
    yield from h.wait_ms(2200)
    yield from h.vendor_round_trip()


def scenario_reboot(h: UsbHost):
    """What a host reboot can throw at the device, one case after the other; after each the
    device must enumerate and serve the vendor interface again."""
    yield from h.wait_for(h.attached, 1500, "attach to the bus")
    yield from h.wait_ms(20)
    cfg = yield from h.enumerate()
    yield from h.vendor_round_trip()
    total = len(cfg)

    h.note("case 1a: a control IN transfer abandoned after its first packet, a new SETUP at once")
    h.setup(0x80, GET_DESCRIPTOR, 0x0200, 0, total)
    yield from h.take_in(0, limit=64, what="send the first descriptor packet")
    dev = yield from h.control_in(0x80, GET_DESCRIPTOR, 0x0100, 0, 18)
    if len(dev) != 18 or dev[1] != 1:
        raise HostError(f"stale data answered the request after an abandoned one: {dev.hex()}")
    yield from h.vendor_round_trip()

    h.note("case 1b: the same, the new SETUP 1 ms later (the next packet is already primed)")
    h.setup(0x80, GET_DESCRIPTOR, 0x0200, 0, total)
    yield from h.take_in(0, limit=64, what="send the first descriptor packet")
    yield from h.wait_ms(1)
    dev = yield from h.control_in(0x80, GET_DESCRIPTOR, 0x0100, 0, 18)
    if len(dev) != 18 or dev[1] != 1:
        raise HostError(f"stale data answered the request after an abandoned one: {dev.hex()}")
    yield from h.vendor_round_trip()

    h.note("case 2: SETUP of a control OUT whose data stage never comes, then a new SETUP")
    h.setup(0x21, SET_REPORT, 0x0200, 0, 1)
    yield from h.wait_ms(2)
    dev = yield from h.control_in(0x80, GET_DESCRIPTOR, 0x0100, 0, 18)
    if len(dev) != 18 or dev[1] != 1:
        raise HostError(f"bad device descriptor after an abandoned control OUT: {dev.hex()}")

    h.note("case 3: bus reset in the middle of a control transfer, re-enumeration (firmware -> OS)")
    h.setup(0x80, GET_DESCRIPTOR, 0x0200, 0, total)
    yield from h.wait_ms(1)
    yield from h.enumerate(boot_protocol=True)
    yield from h.vendor_round_trip()

    h.note("case 4: reset while a vendor reply is waiting for a host that no longer polls")
    yield from h.send_out(VENDOR_OUT, VENDOR_READ_VERSION.ljust(1024, b"\0"), what="take a vendor request")
    yield from h.wait_for(lambda: h.primed(VENDOR_IN, True), 50, "prime the vendor reply")
    yield from h.enumerate()
    yield from h.vendor_round_trip()

    h.note("case 5: suspend (host off), then resume straight into a reset and re-enumeration")
    h.suspend()
    yield from h.wait_ms(60)
    h.resume()
    yield from h.enumerate()
    yield from h.vendor_round_trip()

    h.note("case 6: deconfigure (SET_CONFIGURATION 0) and configure again without a reset")
    yield from h.control_out(0x00, SET_CONFIGURATION, 0, 0)
    yield from h.wait_ms(2)
    yield from h.control_out(0x00, SET_CONFIGURATION, cfg[5], 0)
    yield from h.wait_ms(2)
    yield from h.vendor_round_trip()

    h.note("case 7: session-valid change (VBUS glitch)")
    yield from h.wait_ms(5)
    h.usb.session_change()
    yield from h.wait_ms(5)
    yield from h.vendor_round_trip()


SCENARIOS = {"enumerate": scenario_enumerate, "reboot": scenario_reboot, "vbus-glitch": scenario_vbus_glitch}
