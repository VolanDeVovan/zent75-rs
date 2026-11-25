"""Peripheral models for the HPM5361 emulator.

A model is a `Periph` subclass covering [base, base + size). The `Bus` routes every MMIO access
to the model that owns the address; addresses without a dedicated model but inside a known
hpm-metapac instance get a `RegFile` (memory-like register file). Addresses outside every
instance are a hard failure (bus error on the real chip).

To add a peripheral: subclass `RegFile` (or `Periph`), override `read`/`write`, and add an
instance in `default_models()`. Models talk to the machine through `self.m` (time, IRQs,
events, findings).
"""
from __future__ import annotations

import collections
import struct

from regnames import pad_name

M32 = 0xFFFFFFFF


class Periph:
    name = "?"

    def __init__(self, m, name: str, base: int, size: int) -> None:
        self.m, self.name, self.base, self.size = m, name, base, size

    def read(self, off: int, size: int) -> int:  # off is word aligned for size 4
        return 0

    def write(self, off: int, size: int, value: int) -> None:
        pass

    def __repr__(self) -> str:
        return f"<{type(self).__name__} {self.name} @{self.base:#x}>"


class RegFile(Periph):
    """Registers hold what was written. Status bits named `*busy*` in hpm-metapac read as 0 (an
    operation finishes instantly). `ro` = {offset: (and_mask, or_mask)} applied on read."""

    def __init__(self, m, name, base, size, reset: dict[int, int] | None = None) -> None:
        super().__init__(m, name, base, size)
        self.regs: dict[int, int] = dict(reset or {})
        self.ro: dict[int, tuple[int, int]] = {}
        self._busy_cache: dict[int, int] = {}

    def busy_mask(self, off: int) -> int:
        mk = self._busy_cache.get(off)
        if mk is None:
            mk = 0
            for fname, shift, mask in self.m.names.fields(self.base + off) if self.m.names else ():
                if "busy" in fname:
                    mk |= mask << shift
            self._busy_cache[off] = mk
        return mk

    def get(self, off: int) -> int:
        v = self.regs.get(off, 0)
        if off in self.ro:
            a, o = self.ro[off]
            v = (v & a) | o
        return v & ~self.busy_mask(off) & M32

    def read(self, off, size):
        return self.get(off)

    def write(self, off, size, value):
        self.regs[off] = value & M32


class Uart(RegFile):
    """16550-style HPM UART. TX bytes go to `m.uart[name]`; LSR always 'transmitter empty'."""
    THR, DLM_IER, LCR, LSR = 0x20, 0x24, 0x2C, 0x34

    def __init__(self, m, name, base):
        super().__init__(m, name, base, 0x4000)
        self.ro[self.LSR] = (0, (1 << 5) | (1 << 6) | (1 << 30))  # thre, temt, txidle; no RX data
        self.out = bytearray()
        self.line = bytearray()

    def write(self, off, size, value):
        if off == self.THR and not (self.regs.get(self.LCR, 0) & 0x80):
            b = value & 0xFF
            self.out.append(b)
            if b == 0x0A:
                self.m.on_uart_line(self.name, self.line.decode("utf-8", "replace").rstrip("\r"))
                self.line.clear()
            else:
                self.line.append(b)
            return
        super().write(off, size, value)

    def read(self, off, size):
        if off == self.THR and not (self.regs.get(self.LCR, 0) & 0x80):
            return 0  # RBR: no input
        return super().read(off, size)


class Plic(RegFile):
    """PLIC (target 0 only). `raise_irq(n)` sets the pending bit; the core auto-claims in vectored
    mode (Machine.take_external) or the firmware reads CLAIM; writing CLAIM completes."""
    FEATURE, PENDING, ENABLE, THRESHOLD, CLAIM = 0x0, 0x1000, 0x2000, 0x200000, 0x200004

    def __init__(self, m, base=0xE4000000):
        super().__init__(m, "PLIC", base, 0x400000)
        self.pending: set[int] = set()
        self.in_service: list[int] = []
        self.claimed_log = collections.Counter()

    def prio(self, n):
        return self.regs.get(4 * n, 0)

    def enabled(self, n):
        return (self.regs.get(self.ENABLE + 4 * (n // 32), 0) >> (n % 32)) & 1

    def best(self) -> int | None:
        thr = self.regs.get(self.THRESHOLD, 0)
        cands = [n for n in self.pending if self.enabled(n) and self.prio(n) > thr]
        if not cands:
            return None
        return max(cands, key=lambda n: (self.prio(n), -n))

    def claim(self) -> int:
        n = self.best()
        if n is None:
            return 0
        self.pending.discard(n)
        self.in_service.append(n)
        self.claimed_log[n] += 1
        return n

    def read(self, off, size):
        if off == self.CLAIM:
            v = self.claim()
            self.m.irq_changed()
            return v
        if self.PENDING <= off < self.PENDING + 0x80:
            w = (off - self.PENDING) // 4
            return sum(1 << (n % 32) for n in self.pending if n // 32 == w)
        return super().read(off, size)

    def write(self, off, size, value):
        if off == self.CLAIM:
            if value in self.in_service:
                self.in_service.remove(value)
            self.m.irq_changed()
            return
        if self.PENDING <= off < self.PENDING + 0x80:
            return
        super().write(off, size, value)
        self.m.irq_changed()

    def raise_irq(self, n: int):
        self.pending.add(n)
        self.m.irq_changed()


class Mchtmr(Periph):
    """64-bit machine timer: mtime @0, mtimecmp @8, counting at m.MTIME_HZ from virtual time."""

    def __init__(self, m, base=0xE6000000):
        super().__init__(m, "MCHTMR", base, 0x10000)
        self.offset = 0
        self.cmp = (1 << 64) - 1

    def mtime(self) -> int:
        return (self.m.cycles // self.m.CYCLES_PER_MTIME + self.offset) & ((1 << 64) - 1)

    def read(self, off, size):
        if off < 8:
            self.m.time_poll()
        t = self.mtime()
        return {0: t & M32, 4: t >> 32, 8: self.cmp & M32, 0xC: self.cmp >> 32}.get(off & ~3, 0)

    def write(self, off, size, value):
        if off in (0, 4):
            t = self.mtime()
            t = (t & ~M32 | value) if off == 0 else (t & M32 | value << 32)
            self.offset = t - self.m.cycles // self.m.CYCLES_PER_MTIME
        elif off in (8, 0xC):
            if off == 8:
                self.cmp = (self.cmp & ~M32) | value
            else:
                self.cmp = (self.cmp & M32) | (value << 32)
            self.m.timer_changed()

    def deadline_cycles(self) -> int | None:
        if self.cmp >= (1 << 63):
            return None
        return max(0, (self.cmp - self.offset) * self.m.CYCLES_PER_MTIME)

    def pending(self) -> bool:
        return self.mtime() >= self.cmp


class Usb(RegFile):
    """ChipIdea USB device controller: enough for init not to hang; never sees a host."""
    USBCMD, ENDPTPRIME, ENDPTFLUSH, PORTSC1 = 0x140, 0x1B0, 0x1B4, 0x184

    def __init__(self, m, name, base):
        super().__init__(m, name, base, 0x4000)
        self.ro[self.USBCMD] = (~0x2 & M32, 0)  # rst self-clears
        self.ro[self.ENDPTPRIME] = (0, 0)
        self.ro[self.ENDPTFLUSH] = (0, 0)
        self.ro[self.PORTSC1] = (~(1 << 6) & M32, 0)  # fpr clears


class Gpio(RegFile):
    """HPM GPIO: per port value/set/clear/toggle at +0/+4/+8/+C of DO (0x100) and OE (0x200).
    Output level changes of pins with OE set are logged in m.gpio_log."""
    DI, DO, OE = 0x000, 0x100, 0x200

    def __init__(self, m, name, base, size=0x4000):
        super().__init__(m, name, base, size)

    def write(self, off, size, value):
        for grp in (self.DO, self.OE):
            if grp <= off < grp + 0x100:
                port, sub = divmod(off - grp, 0x10)
                key = grp + port * 0x10
                old = self.regs.get(key, 0)
                new = {0: value, 4: old | value, 8: old & ~value, 0xC: old ^ value}[sub & 0xC] & M32
                self.regs[key] = new
                self._changed(port, grp, old, new)
                return
        super().write(off, size, value)

    def read(self, off, size):
        for grp in (self.DO, self.OE):
            if grp <= off < grp + 0x100:
                port = (off - grp) // 0x10
                return self.regs.get(grp + port * 0x10, 0)
        if off < 0x100:  # DI mirrors DO
            port = off // 0x10
            return self.regs.get(self.DO + port * 0x10, 0)
        return super().read(off, size)

    def _changed(self, port, grp, old, new):
        oe = self.regs.get(self.OE + port * 0x10, 0)
        do = self.regs.get(self.DO + port * 0x10, 0)
        diff = old ^ new
        for bit in range(32):
            if not (diff >> bit) & 1:
                continue
            pad = pad_name(port * 32 + bit)
            if grp == self.OE:
                self.m.gpio_event(self.name, pad, "oe", (new >> bit) & 1)
            elif (oe >> bit) & 1:
                self.m.gpio_event(self.name, pad, "level", (do >> bit) & 1)


class Ppor(RegFile):
    """PPOR: reset_flag (+0) is write-1-to-clear and starts as `m.reset_flags` (the cause of the
    'previous' reset); a non-zero write to software_reset (+0x1C) = chip reset (BE FF path)."""

    def __init__(self, m, name, base, size):
        super().__init__(m, name, base, size)
        self.regs[0] = m.reset_flags

    def write(self, off, size, value):
        if off == 0:
            self.regs[0] = self.regs.get(0, 0) & ~value & M32
            return
        super().write(off, size, value)
        if off == 0x1C and value:
            self.m.event("reset", f"PPOR software reset requested (counter {value})")
            self.m.stop("reset", f"software reset requested via PPOR.software_reset = {value}")


class Ewdg(RegFile):
    """EWDG watchdog: timeout = ot_rst_val << ctrl0.div_value ticks of 32768 Hz (clk_sel=1) or
    the bus clock; refreshed by writing the key to wdt_refresh_reg. Expiry with ot_rst_en set
    is a hard failure (the chip would reset)."""
    CTRL0, CTRL1, OT_RST_VAL, REFRESH, EN = 0x00, 0x04, 0x0C, 0x10, 0x20
    KEY = 0x5A45524F
    BUS_HZ = 160_000_000

    def __init__(self, m, name, base):
        super().__init__(m, name, base, 0x4000)
        self.gen = 0
        self.feeds = 0
        self.max_gap = 0
        self.last_kick = None

    def running(self) -> bool:
        return bool(self.regs.get(self.EN, 0) & 1)

    def timeout_cycles(self) -> int:
        c0 = self.regs.get(self.CTRL0, 0)
        hz = 32768 if (c0 >> 29) & 1 else self.BUS_HZ
        ticks = (self.regs.get(self.OT_RST_VAL, 0) & 0xFFFF) << ((c0 >> 25) & 7)
        return ticks * self.m.CPU_HZ // hz

    def _kick(self):
        self.gen += 1
        if self.last_kick is not None:
            self.max_gap = max(self.max_gap, self.m.cycles - self.last_kick)
        self.last_kick = self.m.cycles
        if not self.running():
            return
        gen = self.gen
        tmo = self.timeout_cycles()

        def expire():
            if gen != self.gen or not self.running():
                return
            if self.regs.get(self.CTRL1, 0) & (1 << 17):
                self.m.note("FAIL", "watchdog", f"{self.name} expired: not fed for "
                            f"{tmo * 1000 / self.m.CPU_HZ:.1f} ms (the chip resets)")
                self.m.stop("watchdog", f"{self.name} timeout")
            else:
                self.m.warn("watchdog", f"{self.name} timeout (reset not enabled)")
        self.m.schedule(self.m.cycles + tmo, expire)

    def write(self, off, size, value):
        super().write(off, size, value)
        if off == self.REFRESH and value == self.KEY:
            self.feeds += 1
            self._kick()
        elif off == self.EN:
            if value & 1:
                self.m.event("watchdog", f"{self.name} enabled, timeout "
                             f"{self.timeout_cycles() * 1000 / self.m.CPU_HZ:.1f} ms")
                self.last_kick = None
            else:
                self.m.event("watchdog", f"{self.name} disabled")
            self._kick()


class Hdma(RegFile):
    """HDMA: dmactrl.reset self-clears; enabling a channel completes the transfer at once
    (sets its inttcsts bit, W1C). The data itself is not moved (yet)."""
    DMACTRL, INTTC, CH_EN, CH0 = 0x14, 0x28, 0x34, 0x40

    def __init__(self, m, name, base):
        super().__init__(m, name, base, 0x4000)
        self.ro[self.DMACTRL] = (~1 & M32, 0)
        self.transfers = 0

    def write(self, off, size, value):
        if off in (self.INTTC, 0x24, 0x2C, 0x30):  # status registers: W1C
            self.regs[off] = self.regs.get(off, 0) & ~value & M32
            return
        super().write(off, size, value)
        if off >= self.CH0 and (off - self.CH0) % 0x20 == 0 and value & 1:
            ch = (off - self.CH0) // 0x20
            self.transfers += 1
            self.regs[off] = value & ~1
            self.regs[self.INTTC] = self.regs.get(self.INTTC, 0) | (1 << ch)


class XpiGuard(RegFile):
    """XPI0 controller registers. Any write is a hard failure: flash must only be changed through
    the ROM NOR API, where the gate can check the address range."""

    def write(self, off, size, value):
        super().write(off, size, value)
        self.m.fail("flash-gate", f"direct write {value:#x} to {self.name} controller register +{off:#x} "
                    "(bypasses the ROM API range check)")


class Ioc(RegFile):
    """Pin mux. Plain register file; Machine reports FUNC_CTL against the expected table."""


def default_models(m) -> list[Periph]:
    return [
        Plic(m),
        Mchtmr(m),
        Uart(m, "UART0", 0xF0040000),
        Usb(m, "USB0", 0xF300C000),
        Gpio(m, "GPIO0", 0xF00D0000),
        Gpio(m, "FGPIO", 0x000C0000),
        Gpio(m, "PGPIO", 0xF411C000),
        Ioc(m, "IOC", 0xF4040000, 0x4000),
        Ioc(m, "PIOC", 0xF4118000, 0x4000),
        Ppor(m, "PPOR", 0xF4100000, 0x4000),
        Ewdg(m, "EWDG0", 0xF00B0000),
        Ewdg(m, "EWDG1", 0xF00B4000),
        Hdma(m, "HDMA", 0xF00C8000),
        XpiGuard(m, "XPI0", 0xF3000000, 0x4000),
    ]


class Bus:
    """Routes MMIO, logs writes, and breaks generic poll loops on RegFile registers."""
    POLL_LIMIT = 2000  # reads of one address with no write to it ...
    POLL_WINDOW = 4_000_000  # ... within this many cycles = a poll loop

    def __init__(self, m) -> None:
        self.m = m
        self.models: list[Periph] = []
        self.by_page: dict[int, Periph] = {}
        self.writes: list[tuple[int, int, int, int, int]] = []  # (cycle, pc, addr, size, value)
        self.write_count = collections.Counter()
        self.last_write: dict[int, int] = {}
        self.read_count = collections.Counter()
        self.poll: dict[int, list[int]] = {}
        self.polled: dict[int, dict] = {}  # addr -> {'pc':..., 'count':...}
        self.unknown: collections.Counter = collections.Counter()

    def add(self, p: Periph):
        self.models.append(p)
        for page in range(p.base >> 12, (p.base + p.size + 0xFFF) >> 12):
            self.by_page[page] = p
        return p

    def model(self, addr: int) -> Periph | None:
        p = self.by_page.get(addr >> 12)
        if p is not None:
            return p
        names = self.m.names
        inst = names.instance(addr) if names and names.available else None
        if inst is None:
            return None
        base, name, mod, _ = inst
        return self.add(RegFile(self.m, name, base, 0x4000))

    # ---- access ---------------------------------------------------------------------------
    def read(self, addr: int, size: int) -> int:
        p = self.model(addr)
        if p is None:
            self.unknown[addr] += 1
            self.m.fail("mmio", f"read of unmapped peripheral address {addr:#010x}", addr=addr)
            return 0
        woff = (addr - p.base) & ~3
        v = p.read(woff, 4)
        self.read_count[addr & ~3] += 1
        if type(p) in (RegFile, Ioc):
            v = self._poll_break(addr & ~3, v)
        sh = (addr & 3) * 8
        return (v >> sh) & ((1 << (8 * size)) - 1)

    def write(self, addr: int, size: int, value: int) -> None:
        p = self.model(addr)
        pc = self.m.pc()
        self.writes.append((self.m.cycles, pc, addr, size, value))
        if len(self.writes) > 500_000:
            del self.writes[:100_000]
        self.write_count[addr & ~3] += 1
        if p is None:
            self.unknown[addr] += 1
            self.m.fail("mmio", f"write {value:#x} to unmapped peripheral address {addr:#010x}",
                        addr=addr)
            return
        woff = (addr - p.base) & ~3
        if size < 4:
            sh = (addr & 3) * 8
            mask = ((1 << (8 * size)) - 1) << sh
            old = p.regs.get(woff, 0) if isinstance(p, RegFile) else 0
            value = (old & ~mask) | ((value << sh) & mask)
        self.last_write[addr & ~3] = value & M32
        self.poll.pop(addr & ~3, None)
        p.write(woff, 4, value & M32)

    def _poll_break(self, addr: int, v: int) -> int:
        st = self.poll.get(addr)
        now = self.m.cycles
        if st is None or now - st[1] > self.POLL_WINDOW:
            self.poll[addr] = [1, now]
            return v
        st[0] += 1
        if st[0] < self.POLL_LIMIT:
            return v
        info = self.polled.setdefault(addr, {"pc": self.m.pc(), "count": 0, "value": v})
        info["count"] += 1
        return v if st[0] % 2 else (~v & M32)
