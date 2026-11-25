"""Hall-scan peripherals: ADC16 (sequence mode + sequence DMA), PWM (counter period and the
PWM0 -> TRGM0 -> ADC hardware trigger), GPTMR (reload interrupt) and the sensor source that
supplies conversion results.

Model (docs/re-peripherals.md section 3):
- PWM0 counts at AHB 160 MHz; one period = (rld >> 4) + 1 ticks. While gcr.cen is set, every
  rising edge of the ADC trigger (the even compare values cmp[0], cmp[2], ... when chcfg[8] is
  configured, else once per period at 0) starts the sequence of every ADC with hw_trig_en.
  TRGM routing is assumed (not checked).
- A sequence converts seq_len+1 queue entries back to back (ADC_CONV_CYCLES each), writes the
  seq-DMA words (result 15:0, queue index 19:16, channel 24:20, cycle bit 31) into the circular
  buffer at seq_dma_addr (buf_len+1 words), sets int_sts.seq_cvc and, if int_en allows it,
  raises the ADC's PLIC interrupt. seq_cfg0.sw_trig starts one sequence the same way.
- The mux step (0..10) of a trigger is its index inside the PWM period; `SensorSource.value`
  gets (adc, channel, step, t_ms) and returns the 16-bit result.
"""
from __future__ import annotations

from periph import M32, RegFile

AHB_HZ = 160_000_000
ADC_CONV_CYCLES = 22 * 6  # ~22 ADC clocks at 80 MHz, in 480 MHz CPU cycles
MUX_ADDR = (1, 2, 3, 4, 5, 9, 10, 11, 12, 13, 14)  # mux address of step k (hardware.md)
ADC_IRQ = {"ADC0": 58, "ADC1": 59}


class SensorSource:
    """Conversion results. Default: every input rests at `rest` (+ a tiny deterministic ripple,
    plus deterministic pseudo-random noise of ±`noise` counts — real sensors move the filtered
    value every scan, which defeats the engine's unchanged-input shortcut).
    `press(adc, channel, step, start_ms, end_ms, value)` overrides one slot for a time span."""

    def __init__(self, rest: int = 0x8000, noise: int = 0) -> None:
        self.rest = rest
        self.noise = noise
        self.lcg = 1
        self.overrides: list[tuple[str, int, int, float, float, int]] = []
        self.samples = 0

    def press(self, adc: str, channel: int, step: int, start_ms: float, end_ms: float, value: int):
        self.overrides.append((adc, channel, step, start_ms, end_ms, value))

    def value(self, adc: str, channel: int, step: int, t_ms: float) -> int:
        self.samples += 1
        for a, c, s, t0, t1, v in self.overrides:
            if a == adc and c == channel and s == step and t0 <= t_ms < t1:
                return v & 0xFFFF
        v = self.rest + ((channel * 7 + step * 3 + self.samples) % 5) - 2
        if self.noise:
            self.lcg = (self.lcg * 1103515245 + 12345) & 0x7FFFFFFF
            v += (self.lcg >> 8) % (2 * self.noise + 1) - self.noise
        return v & 0xFFFF


class Adc16(RegFile):
    SEQ_CFG0, SEQ_DMA_ADDR, SEQ_WR_ADDR, SEQ_DMA_CFG, SEQ_QUE = 0x800, 0x804, 0x808, 0x80C, 0x810
    INT_STS, INT_EN, ANA_CTRL0, ANA_STATUS = 0x1110, 0x1114, 0x1200, 0x1210
    SEQ_CVC = 1 << 23

    def __init__(self, m, name, base, source: SensorSource):
        super().__init__(m, name, base, 0x4000)
        self.source = source
        self.ro[self.ANA_STATUS] = (~(1 << 7) & M32, 0)  # calon: calibration done at once
        self.wr = 0
        self.cycle = 0
        self.sequences = 0
        self.busy = False

    def write(self, off, size, value):
        if off == self.INT_STS:  # W1C
            self.regs[off] = self.regs.get(off, 0) & ~value & M32
            return
        if off == self.ANA_CTRL0:
            value &= ~(1 << 2)  # startcal self-clears
        super().write(off, size, value)
        if off == self.SEQ_DMA_CFG and value & (1 << 13):  # dma_rst
            self.wr, self.cycle = 0, 0
            self.regs[off] = value & ~(1 << 13)
        if off == self.SEQ_CFG0 and value & (1 << 2):  # sw_trig
            self.regs[off] = value & ~(1 << 2)
            if value & (1 << 1):
                self.trigger(step=0)

    def hw_enabled(self) -> bool:
        return bool(self.regs.get(self.SEQ_CFG0, 0) & 1)

    def trigger(self, step: int):
        if self.busy:
            self.regs[self.INT_STS] = self.regs.get(self.INT_STS, 0) | (1 << 26)  # seq_hw_cflct
            return
        cfg = self.regs.get(self.SEQ_CFG0, 0)
        n = ((cfg >> 8) & 0xF) + 1
        self.busy = True
        self.m.schedule(self.m.cycles + n * self.conv_cycles(), lambda: self._complete(step, n))

    def conv_cycles(self) -> int:
        """CPU cycles per conversion from conv_cfg1: (convert_clock_number + 1 + ~2 sample /
        overhead clocks) ADC clocks of AHB / (clock_divider + 1). Default ~ADC_CONV_CYCLES."""
        c = self.regs.get(0x1104)
        if c is None:
            return ADC_CONV_CYCLES
        clocks = ((c >> 4) & 0x1F) + 3
        return clocks * ((c & 0xF) + 1) * self.m.CPU_HZ // AHB_HZ

    def _complete(self, step: int, n: int):
        self.busy = False
        dma = self.regs.get(self.SEQ_DMA_ADDR, 0) & ~3
        buf_len = (self.regs.get(self.SEQ_DMA_CFG, 0) & 0xFFF) + 1
        t_ms = self.m.ms()
        words = []
        for i in range(n):
            q = self.regs.get(self.SEQ_QUE + 4 * i, 0)
            ch = q & 0x1F
            v = self.source.value(self.name, ch, step, t_ms)
            words.append((self.cycle << 31) | (ch << 20) | (i << 16) | v)
        if dma:
            for w in words:
                try:
                    self.m.uc.mem_write(dma + 4 * self.wr, w.to_bytes(4, "little"))
                except Exception:  # noqa: BLE001 - unicorn raises UcError
                    self.m.fail("adc", f"{self.name} seq DMA to unmapped {dma + 4 * self.wr:#x}")
                    return
                self.wr += 1
                if self.wr >= buf_len:
                    self.wr, self.cycle = 0, self.cycle ^ 1
            self.regs[self.SEQ_WR_ADDR] = dma + 4 * self.wr
        self.sequences += 1
        self.regs[self.INT_STS] = self.regs.get(self.INT_STS, 0) | self.SEQ_CVC | (1 << 24)
        if self.regs.get(self.INT_EN, 0) & self.regs[self.INT_STS]:
            irq = ADC_IRQ.get(self.name)
            if irq is not None:
                self.m.plic.raise_irq(irq)


class Pwm(RegFile):
    """PWM counter. CNT (+0x170, ticks << 4) runs from virtual time while gcr.cen is set.
    PWM0 with cen generates the ADC hardware triggers (see module doc)."""
    RLD, CMP, CHCFG, GCR, CNT = 0x08, 0x0C, 0x80, 0xF0, 0x170
    CEN = 1 << 7

    def read(self, off, size):
        if off == self.CNT and self.regs.get(self.GCR, 0) & self.CEN:
            ticks = ((self.regs.get(self.RLD, 0) >> 4) & 0xFFFFFF) + 1
            t = (self.m.cycles - self.t0) * AHB_HZ // self.m.CPU_HZ
            return ((t % ticks) << 4) & M32
        return super().read(off, size)

    def __init__(self, m, name, base, adcs=()):
        super().__init__(m, name, base, 0x4000)
        self.adcs = list(adcs)
        self.gen = 0
        self.periods = 0
        self.t0 = 0

    def write(self, off, size, value):
        was = self.regs.get(self.GCR, 0) & self.CEN
        super().write(off, size, value)
        if off == self.GCR:
            now = value & self.CEN
            if now and not was:
                self.t0 = self.m.cycles
            if now and not was and self.adcs:
                self.gen += 1
                self._period(self.gen)
            elif not now:
                self.gen += 1

    def period_cycles(self) -> int:
        ticks = ((self.regs.get(self.RLD, 0) >> 4) & 0xFFFFFF) + 1
        return max(ticks * self.m.CPU_HZ // AHB_HZ, 1000)

    def trigger_offsets(self) -> list[int]:
        if not self.regs.get(self.CHCFG + 4 * 8, 0):
            return [0]
        offs = []
        for k in range(0, 24, 2):
            c = self.regs.get(self.CMP + 4 * k)
            if c is None:
                break
            offs.append(((c >> 4) & 0xFFFFFF) * self.m.CPU_HZ // AHB_HZ)
        return offs or [0]

    def _period(self, gen, start=None):
        """One PWM period starting at cycle `start`. Periods follow each other on the nominal
        grid (not from "now", which is up to one basic block late): the triggers must not
        drift against CNT, which the firmware reads to tell the mux step and to drop late
        captures."""
        if gen != self.gen:
            return
        if start is None:
            start = self.m.cycles
        for step, off in enumerate(self.trigger_offsets()):
            def fire(step=step):
                if gen == self.gen:
                    for a in self.adcs:
                        if a.hw_enabled():
                            a.trigger(step)
            self.m.schedule(start + off, fire)
        self.periods += 1
        nxt = start + self.period_cycles()
        self.m.schedule(nxt, lambda: self._period(gen, nxt))


class Gptmr(RegFile):
    """General-purpose timer: per channel (0x40 stride) cr/cmp/rld/cnt, sr (W1C) @0x200, irqen
    @0x204. With cr.cen a channel reloads every rld+1 ticks (TMR_HZ) and sets sr.chrldf; the IRQ
    fires when irqen has that bit. cr.cntrst restarts the count."""
    SR, IRQEN = 0x200, 0x204
    TMR_HZ = 24_000_000  # assumption: GPTMR clock left at the 24 MHz default (re-peripherals)

    def __init__(self, m, name, base, irq):
        super().__init__(m, name, base, 0x4000)
        self.irq = irq
        self.gen = [0, 0, 0, 0]
        self.start = [0, 0, 0, 0]
        self.reloads = 0

    def read(self, off, size):
        if off < 0x100 and off % 0x40 == 0x30:  # cnt: ticks since the channel (re)started
            ch = off // 0x40
            if self.regs.get(ch * 0x40, 0) & (1 << 10):
                self.m.time_poll()
                rld = self.regs.get(ch * 0x40 + 0x0C, 0) or M32
                ticks = (self.m.cycles - self.start[ch]) * self.TMR_HZ // self.m.CPU_HZ
                return ticks % (rld + 1)
        return super().read(off, size)

    def write(self, off, size, value):
        if off == self.SR:
            self.regs[off] = self.regs.get(off, 0) & ~value & M32
            return
        super().write(off, size, value)
        if off < 0x100 and off % 0x40 == 0:
            self.start[off // 0x40] = self.m.cycles
            ch = off // 0x40
            if value & (1 << 14):  # cntrst self-clears
                self.regs[off] = value & ~(1 << 14)
            self.gen[ch] += 1
            if value & (1 << 10):
                self._arm(ch, self.gen[ch])

    def _arm(self, ch, gen, slow=False):
        rld = self.regs.get(ch * 0x40 + 0x0C, 0)
        cyc = max((rld + 1) * self.m.CPU_HZ // self.TMR_HZ, 20)

        def reload():
            if gen != self.gen[ch]:
                return
            self.reloads += 1
            flag = 1 << (4 * ch)
            already = self.regs.get(self.SR, 0) & flag
            self.regs[self.SR] = self.regs.get(self.SR, 0) | flag
            if self.regs.get(self.IRQEN, 0) & self.regs[self.SR] and self.irq is not None:
                self.m.plic.raise_irq(self.irq)
            # Flag still set from the previous reload: further reloads change nothing visible
            # until software clears it, so poll less often (keeps fast timers cheap).
            self._arm(ch, gen, slow=bool(already))
        self.m.schedule(self.m.cycles + (max(cyc, 4000) if slow else cyc), reload)


def scan_models(m, source: SensorSource):
    adc0 = Adc16(m, "ADC0", 0xF3080000, source)
    adc1 = Adc16(m, "ADC1", 0xF3084000, source)
    return [adc0, adc1,
            Pwm(m, "PWM0", 0xF0318000, adcs=(adc0, adc1)),
            Pwm(m, "PWM1", 0xF031C000),
            Gptmr(m, "GPTMR0", 0xF0000000, 5),
            Gptmr(m, "GPTMR1", 0xF0004000, 6)]
