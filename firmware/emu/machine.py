"""HPM5361 machine model on unicorn: memory map, boot like stage-0, Andes CSRs, Zb* emulation,
interrupts (MCHTMR + PLIC in Andes vectored mode), virtual time, ROM API (XPI NOR + OTP) with the
flash write gate, and the failure/finding log the gate reports on."""
from __future__ import annotations

import collections
import heapq
import struct

from unicorn import (UC_ARCH_RISCV, UC_HOOK_BLOCK, UC_HOOK_CODE, UC_HOOK_INTR,
                     UC_HOOK_MEM_FETCH_PROT, UC_HOOK_MEM_READ, UC_HOOK_MEM_UNMAPPED,
                     UC_HOOK_MEM_WRITE, UC_HOOK_MEM_WRITE_PROT, UC_MODE_RISCV32, UC_PROT_ALL,
                     UC_PROT_EXEC, UC_PROT_READ, Uc, UcError)
from unicorn import riscv_const as R

import rvext
from image import APP_BASE, APP_END, FLASH_BASE, FLASH_SIZE, SIGNATURE, Image
from periph import Bus, Mchtmr, Plic, default_models
from scan import SensorSource, scan_models

M32 = 0xFFFFFFFF

ILM = (0x00000000, 0x20000)
DLM = (0x00080000, 0x20000)
AHB_SRAM = (0xF0400000, 0x8000)
ROM = (0x20000000, 0x20000)
XIP = (FLASH_BASE, FLASH_SIZE)
MMIO_MAPS = [(0x000C0000, 0x4000), (0xE0000000, 0x08000000), (0xF0000000, 0x00400000),
             (0xF0408000, 0x07BF8000)]

PROTECTED_END = 0x80040000  # boot header, stage-0, download loader
CONFIG_START = 0x80070000  # lowest flash address the firmware may erase/program
LOADER_ENTRY = 0x80020000
BOOT_FILL = b"\x5a\xa5\xb0\x07"  # recognisable filler for the (unknown) boot region

# ROM API layout (our own, only the table address is fixed by the chip)
API_TABLE = 0x2001FF00
DRV_STRUCTS = 0x2001E000
STUBS = 0x20000000
DRIVERS = ["otp", "xpi", "xpi_nor", "xpi_ram", "sdp", "sec", "exip", "api"]
COPYRIGHT = 0x2001F000
NO_TRAP_VECTOR = 0x2001F800  # mtvec before the firmware sets it
RETURN_TRAP = 0x2001F900  # return address for forced calls (test hooks)

CPU_HZ = 480_000_000
MTIME_HZ = 24_000_000

FAIL, WARN, INFO = "FAIL", "WARN", "INFO"

OTP_UID = {0x58: 0x5A11E000, 0x59: 0x7E57E3D0, 0x5A: 0x00C0FFEE, 0x5B: 0x12345678}

PANIC_SYMBOLS = ("rust_begin_unwind", "DefaultHandler", "DefaultExceptionHandler",
                 "DefaultInterruptHandler", "ExceptionHandler", "_pre_init_trap",
                 "default_start_trap", "abort", "HardFault")


class Finding:
    __slots__ = ("level", "kind", "msg", "cycle", "pc")

    def __init__(self, level, kind, msg, cycle, pc):
        self.level, self.kind, self.msg, self.cycle, self.pc = level, kind, msg, cycle, pc


class Machine:
    CYCLES_PER_MTIME = CPU_HZ // MTIME_HZ
    CPU_HZ = CPU_HZ

    def __init__(self, image: Image, names=None, align_check: bool = True,
                 trace=None, verbose: bool = False, reset_flags: int = 0) -> None:
        self.reset_flags = reset_flags
        self._tp = [0, 0]  # busy-wait accelerator: (last poll cycle, consecutive polls)
        self.time_skipped = 0
        self.img = image
        self.names = names
        self.verbose = verbose
        self.trace = trace
        self.uc = uc = Uc(UC_ARCH_RISCV, UC_MODE_RISCV32)
        self.cycles = 0
        self.end_cycles = 0
        self.findings: list[Finding] = []
        self._seen = set()
        self._stop = None
        self.csr: dict[int, int] = {}
        self.events_log: list[tuple[int, str, str]] = []
        self.uart_lines: list[tuple[int, str, str]] = []
        self.gpio_log: list[tuple[int, str, str, str, int]] = []
        self.flash_ops: list[tuple[int, str, int, int, str]] = []
        self.rom_calls = collections.Counter()
        self.irq_count = collections.Counter()
        self.wfi_count = 0
        self.first_wfi = None
        self.first_wfi_pc = None
        self.blocks = collections.deque(maxlen=24)
        self._bcount: dict[tuple[int, int], int] = {}
        self._queue: list = []
        self._qseq = 0
        self.deadline = None
        self.irq_maybe = False
        self._spin_addr = None
        self._spin = 0
        self.sp_min = None
        self.stack = None
        self._stack_armed = False
        self.bootloader_jump = None

        for base, size in (ILM, DLM):
            uc.mem_map(base, size, UC_PROT_ALL)
            uc.mem_write(base, b"\xcd" * size)  # what stage-0 left: garbage
        uc.mem_map(*AHB_SRAM, UC_PROT_ALL)
        uc.mem_write(AHB_SRAM[0], b"\xcd" * AHB_SRAM[1])
        uc.mem_map(*ROM, UC_PROT_READ | UC_PROT_EXEC)
        uc.mem_map(*XIP, UC_PROT_READ | UC_PROT_EXEC)
        uc.mem_write(FLASH_BASE, BOOT_FILL * ((PROTECTED_END - FLASH_BASE) // 4))
        uc.mem_write(PROTECTED_END, b"\xff" * (FLASH_BASE + FLASH_SIZE - PROTECTED_END))

        self.bus = Bus(self)
        self.sensor = SensorSource()
        for p in default_models(self) + scan_models(self, self.sensor):
            self.bus.add(p)
        self.plic: Plic = next(p for p in self.bus.models if isinstance(p, Plic))
        self.timer: Mchtmr = next(p for p in self.bus.models if isinstance(p, Mchtmr))
        for base, size in MMIO_MAPS:
            uc.mmio_map(base, size, self._mmio_read, base, self._mmio_write, base)

        self._setup_rom()
        uc.hook_add(UC_HOOK_BLOCK, self._on_block)
        uc.hook_add(UC_HOOK_INTR, self._on_intr)
        uc.hook_add(UC_HOOK_MEM_UNMAPPED, self._on_unmapped)
        uc.hook_add(UC_HOOK_MEM_WRITE_PROT, self._on_write_prot)
        uc.hook_add(UC_HOOK_MEM_FETCH_PROT, self._on_fetch_prot)
        uc.hook_add(UC_HOOK_CODE, self._on_rom, begin=ROM[0], end=ROM[0] + ROM[1] - 1)
        uc.hook_add(UC_HOOK_CODE, self._on_boot_region, begin=FLASH_BASE, end=PROTECTED_END - 1)
        if align_check:
            for base, size in (ILM, DLM, AHB_SRAM):
                uc.hook_add(UC_HOOK_MEM_READ | UC_HOOK_MEM_WRITE, self._on_align,
                            begin=base, end=base + size - 1)
        if trace is not None:
            uc.hook_add(UC_HOOK_CODE, self._on_trace)
            from capstone import CS_ARCH_RISCV, CS_MODE_RISCV32, CS_MODE_RISCVC, Cs
            self._cs = Cs(CS_ARCH_RISCV, CS_MODE_RISCV32 | CS_MODE_RISCVC)

    # ---- findings -----------------------------------------------------------------------------
    def pc(self) -> int:
        return self.uc.reg_read(R.UC_RISCV_REG_PC)

    def note(self, level: str, kind: str, msg: str, once_key=None) -> None:
        key = (level, kind, once_key if once_key is not None else msg)
        if key in self._seen:
            return
        self._seen.add(key)
        self.findings.append(Finding(level, kind, msg, self.cycles, self.pc()))
        if self.verbose:
            print(f"[{self.ms():9.3f} ms] {level} {kind}: {msg}")

    def fail(self, kind: str, msg: str, stop: bool = True, **_) -> None:
        self.note(FAIL, kind, msg)
        if stop:
            self.stop("fail", msg)

    def warn(self, kind: str, msg: str, once_key=None) -> None:
        self.note(WARN, kind, msg, once_key)

    def event(self, kind: str, msg: str) -> None:
        self.events_log.append((self.cycles, kind, msg))
        if self.verbose:
            print(f"[{self.ms():9.3f} ms] event {kind}: {msg}")

    def stop(self, reason: str, msg: str = "") -> None:
        if self._stop is None:
            self._stop = (reason, msg)
        self.uc.emu_stop()

    def ms(self, cycles=None) -> float:
        return (self.cycles if cycles is None else cycles) * 1000.0 / CPU_HZ

    def sym(self, addr: int) -> str:
        return self.img.sym(addr) if self.img.symbols else f"{addr:#010x}"

    # ---- loading / boot ------------------------------------------------------------------------
    def load_image(self) -> None:
        for addr, data in self.img.chunks:
            if not (FLASH_BASE <= addr and addr + len(data) <= FLASH_BASE + FLASH_SIZE):
                self.fail("image", f"load segment {addr:#x}+{len(data):#x} is outside the 1 MB XIP flash")
                continue
            if addr < APP_BASE or addr + len(data) > APP_END:
                self.fail("image", f"load segment {addr:#x}+{len(data):#x} is outside the app region "
                          f"{APP_BASE:#x}..{APP_END:#x} (the UF2 would touch boot or config flash)",
                          stop=False)
                continue
            self.uc.mem_write(addr, data)

    def preload_flash(self, writes) -> None:
        for addr, data in writes:
            self.uc.mem_write(addr, data)

    def boot(self) -> bool:
        """Stage-0's decision: word 0 of the app must be the signature, entry = APP_BASE + 4."""
        w0 = struct.unpack("<I", self.uc.mem_read(APP_BASE, 4))[0]
        if w0 != SIGNATURE:
            self.fail("boot", f"word 0 at {APP_BASE:#x} is {w0:#010x}, not {SIGNATURE:#010x}: "
                      "stage-0 would not start this image (falls back to the loader)", stop=False)
            return False
        uc = self.uc
        # Registers as stage-0 leaves them (TinyUF2 board_app_jump): unknown values, IRQs off,
        # caches off. We poison GPRs so a firmware relying on them is visible.
        for i in range(1, 32):
            uc.reg_write(R.UC_RISCV_REG_X0 + i, 0xBADC0DE0 + i)
        uc.reg_write(R.UC_RISCV_REG_SP, 0x0009FF00)  # somewhere in DLM, as stage-0's stack
        uc.reg_write(R.UC_RISCV_REG_MSTATUS, 3 << 11)  # MPP=M, MIE=0, FS=off
        uc.reg_write(R.UC_RISCV_REG_MIE, 0)
        uc.reg_write(R.UC_RISCV_REG_MTVEC, NO_TRAP_VECTOR)
        self.csr[rvext.MCACHE_CTL] = 0
        self.csr[rvext.MMISC_CTL] = 0
        st = self.img.section(".stack")
        if st:
            self.stack = (st[0], st[0] + st[2])
        elif "_sstack" in self.img.sym_by_name:
            top = self.img.sym_by_name["_sstack"]
            self.stack = (DLM[0], top)
        self._scan_code()
        self.entry = APP_BASE + 4
        return True

    def _scan_code(self) -> None:
        """Hook places QEMU would get wrong silently: reads of cycle/instret CSRs (host ticks in
        QEMU) -> virtual time; and entry points of panic / default trap handlers."""
        for vma, data in self.img.exec_ranges:
            for off in range(0, len(data) - 3, 2):
                insn = int.from_bytes(data[off:off + 4], "little")
                f = rvext.csr_fields(insn)
                if f and f[0] in rvext.CYCLE_CSRS and insn & 3 == 3:
                    a = vma + off
                    self.uc.hook_add(UC_HOOK_CODE, self._on_cycle_csr, begin=a, end=a)
        hooked = set()
        for a, n in self.img.find(*PANIC_SYMBOLS):
            short = n.split("::")[-1]
            if short in PANIC_SYMBOLS or n in PANIC_SYMBOLS or n.endswith("rust_begin_unwind"):
                if a in hooked:
                    continue
                hooked.add(a)
                self.uc.hook_add(UC_HOOK_CODE, self._on_panic, begin=a, end=a)

    # ---- ROM API ----------------------------------------------------------------------------
    def _setup_rom(self) -> None:
        uc = self.uc
        ret = struct.pack("<I", 0x00018082)  # c.jr ra ; c.nop (never executed: hooked)
        uc.mem_write(STUBS, ret * 0x800)
        uc.mem_write(COPYRIGHT, b"emulated HPM5361 ROM (zent75 emu)\0")
        table = [0x56010000, COPYRIGHT, self._stub("api", 2)]
        for d, name in enumerate(DRIVERS[:7]):
            struct_addr = DRV_STRUCTS + d * 0x100
            uc.mem_write(struct_addr, struct.pack("<I", 0x00010000))  # version
            for slot in range(1, 0x40):
                uc.mem_write(struct_addr + 4 * slot, struct.pack("<I", self._stub(name, slot * 4)))
            table.append(struct_addr)
        # table: version, copyright, run_bootloader, otp, xpi, xpi_nor, xpi_ram, sdp, sec, _r, _r, exip
        words = [table[0], table[1], table[2], table[3], table[4], table[5], table[6], table[7],
                 table[8], 0, 0, table[9]]
        uc.mem_write(API_TABLE, struct.pack(f"<{len(words)}I", *words))

    @staticmethod
    def _stub(driver: str, slot: int) -> int:
        return STUBS + DRIVERS.index(driver) * 0x100 + slot

    def _arg(self, i: int) -> int:
        return self.uc.reg_read(R.UC_RISCV_REG_A0 + i)

    def _on_rom(self, uc, addr, size, _):
        if addr == NO_TRAP_VECTOR:
            cause = uc.reg_read(R.UC_RISCV_REG_MCAUSE)
            epc = uc.reg_read(R.UC_RISCV_REG_MEPC)
            self.fail("trap", f"trap (mcause {cause:#x}) at {self.sym(epc)} before the firmware set mtvec")
            return
        if addr == RETURN_TRAP:
            self.fail("hook", "forced call returned (EnterBootloader hook did not leave the app)")
            return
        if not (STUBS <= addr < STUBS + 0x800):
            self.fail("rom", f"execution in ROM at {addr:#x} outside the API stubs")
            return
        drv, slot = divmod(addr - STUBS, 0x100)
        name = DRIVERS[drv] if drv < len(DRIVERS) else "?"
        self.rom_calls[(name, slot)] += 1
        handler = getattr(self, f"_rom_{name}", None)
        rv = handler(slot) if handler else None
        if rv is None:
            self.warn("rom", f"unmodelled ROM API call {name}+{slot:#x} from {self.sym(uc.reg_read(R.UC_RISCV_REG_RA))}"
                      " (returned 0)", once_key=(name, slot))
            rv = 0
        if self._stop is not None:
            return
        uc.reg_write(R.UC_RISCV_REG_A0, rv & M32)
        uc.reg_write(R.UC_RISCV_REG_PC, uc.reg_read(R.UC_RISCV_REG_RA))

    def _rom_api(self, slot):
        if slot == 2:
            self.fail("rom", "ROM run_bootloader() called: the chip would enter ROM ISP")
            return 0
        return None

    def _rom_otp(self, slot):
        if slot in (0x04, 0x08, 0x18):  # init, deinit, reload
            return 0
        if slot in (0x0C, 0x10):  # read_from_shadow / read_from_ip
            return OTP_UID.get(self._arg(0), 0)
        if slot in (0x14, 0x1C):
            what = "program" if slot == 0x14 else "lock"
            self.fail("otp", f"OTP {what}({self._arg(0):#x}, ...) called: irreversible on the real chip")
            return 1
        if slot in (0x20, 0x24, 0x28):
            self.warn("otp", f"OTP shadow/config change via slot {slot:#x}")
            return 0
        return None

    def _rom_xpi(self, slot):
        return 0  # XPI controller API: accept quietly

    def _flash_off(self, what: str, a: int) -> int | None:
        if FLASH_BASE <= a < FLASH_BASE + FLASH_SIZE:
            self.fail("flash", f"{what}: absolute address {a:#x} passed to the ROM NOR API "
                      "(it takes an offset from the flash start)")
            return None
        if a >= FLASH_SIZE:
            self.fail("flash", f"{what}: offset {a:#x} beyond the 1 MB flash")
            return None
        return a

    def _flash_gate(self, what: str, off: int, length: int) -> bool:
        lo, hi = FLASH_BASE + off, FLASH_BASE + off + length
        ok = lo >= CONFIG_START and hi <= FLASH_BASE + FLASH_SIZE
        src = self.sym(self.uc.reg_read(R.UC_RISCV_REG_RA))
        self.flash_ops.append((self.cycles, what, lo, length, "ok" if ok else "REFUSED"))
        self.event("flash", f"{what} {lo:#x}+{length:#x} from {src}" + ("" if ok else " REFUSED"))
        if not ok:
            if lo < APP_END:
                region = "boot header/stage-0/loader" if lo < PROTECTED_END else "the application"
                self.fail("flash-gate", f"{what} {lo:#x}..{hi:#x} touches {region} "
                          f"(below {CONFIG_START:#x}) from {src}")
            else:
                self.fail("flash-gate", f"{what} {lo:#x}..{hi:#x} out of range from {src}")
        return ok

    def _rom_xpi_nor(self, slot):
        a = self._arg
        uc = self.uc
        if slot in (0x04, 0x4C):  # get_config / auto_config(base, cfg, option)
            uc.mem_write(a(1), struct.pack("<I", 0xFCF90002) + bytes(0x100 - 4))
            return 0
        if slot in (0x08, 0x0C, 0x14):  # init, enable_write, wait_busy
            return 0
        if slot == 0x10:  # get_status(.., addr, *out)
            uc.mem_write(a(4), b"\0\0")
            return 0
        if slot == 0x50:  # get_property(base, cfg, id, *value)
            val = {0: FLASH_SIZE, 1: 256, 2: 4096, 3: 65536}.get(a(2))
            if val is None:
                return 2
            uc.mem_write(a(3), struct.pack("<I", val))
            return 0
        if slot in (0x1C, 0x3C):
            self.fail("flash-gate", "erase_chip() called: would erase boot region and loader")
            return 1
        if slot in (0x18, 0x20, 0x24, 0x34, 0x38):  # erase(start,len) / sector / block
            if slot == 0x18:
                off, length = self._flash_off("erase", a(3)), a(4)
            else:
                off = self._flash_off("erase", a(3))
                length = 0x10000 if slot in (0x24, 0x38) else 0x1000
            if off is None:
                return 2
            align = 0x10000 if slot in (0x24, 0x38) else 0x1000
            lo = off - off % align
            hi = -(-(off + length) // align) * align
            if not self._flash_gate("erase", lo, hi - lo):
                return 2
            uc.mem_write(FLASH_BASE + lo, b"\xff" * (hi - lo))
            return 0
        if slot in (0x28, 0x30):  # program(src, dst, len)
            src, dst, length = a(3), a(4), a(5)
            off = self._flash_off("program", dst)
            if off is None:
                return 2
            if not self._flash_gate("program", off, length):
                return 2
            data = bytes(uc.mem_read(src, length))
            old = bytes(uc.mem_read(FLASH_BASE + off, length))
            new = bytes(x & y for x, y in zip(old, data))
            if new != data:
                self.warn("flash", f"program {FLASH_BASE + off:#x}+{length:#x} over non-erased flash "
                          "(NOR can only clear bits; result differs from the data)")
            uc.mem_write(FLASH_BASE + off, new)
            return 0
        if slot == 0x2C:  # read(dst, start, len)
            dst, start, length = a(3), a(4), a(5)
            off = self._flash_off("read", start)
            if off is None:
                return 2
            uc.mem_write(dst, bytes(uc.mem_read(FLASH_BASE + off, length)))
            return 0
        return None

    # ---- MMIO -----------------------------------------------------------------------------------
    def _mmio_read(self, uc, offset, size, base):
        return self.bus.read(base + offset, size)

    def _mmio_write(self, uc, offset, size, value, base):
        self.bus.write(base + offset, size, value)

    def on_uart_line(self, name: str, line: str) -> None:
        self.uart_lines.append((self.cycles, name, line))
        if self.verbose:
            print(f"[{self.ms():9.3f} ms] {name}> {line}")

    def gpio_event(self, ctrl, pad, what, val) -> None:
        self.gpio_log.append((self.cycles, ctrl, pad, what, val))

    # ---- time and interrupts ----------------------------------------------------------------
    def schedule(self, at_cycles: int, fn) -> None:
        self._qseq += 1
        heapq.heappush(self._queue, (at_cycles, self._qseq, fn))
        self._recompute_deadline()

    def time_poll(self) -> None:
        """Called on every read of a time source (mtime, mcycle). Many reads in a row with only a
        few cycles between them is a busy-wait: jump virtual time ahead (never past the next
        event) so delays of seconds do not take minutes of Python."""
        tp = self._tp
        if self.cycles - tp[0] < 4000:
            tp[1] += 1
            if tp[1] > 200:
                target = self.cycles + min(40 * (tp[1] - 200), 240_000)
                if self.deadline is not None:
                    target = min(target, max(self.deadline, self.cycles))
                target = min(target, max(self.end_cycles, self.cycles))
                self.time_skipped += target - self.cycles
                self.cycles = target
        else:
            tp[1] = 0
        tp[0] = self.cycles

    def timer_changed(self) -> None:
        self._recompute_deadline()
        self.irq_changed()

    def irq_changed(self) -> None:
        self.irq_maybe = self.timer.pending() or self.plic.best() is not None

    def _recompute_deadline(self) -> None:
        cands = []
        t = self.timer.deadline_cycles()
        if t is not None and t > self.cycles:
            cands.append(t)
        if self._queue:
            cands.append(self._queue[0][0])
        self.deadline = min(cands) if cands else None

    def _service(self) -> None:
        while self._queue and self._queue[0][0] <= self.cycles:
            _, _, fn = heapq.heappop(self._queue)
            fn()
        self._recompute_deadline()
        self.irq_changed()

    def _vectored(self) -> bool:
        return bool(self.csr.get(rvext.MMISC_CTL, 0) & rvext.MMISC_VEC_PLIC)

    def _wake_pending(self) -> bool:
        mie = self.uc.reg_read(R.UC_RISCV_REG_MIE)
        return bool((mie & (1 << 7) and self.timer.pending()) or
                    (mie & (1 << 11) and self.plic.best() is not None))

    def _check_irq(self, pc: int) -> None:
        uc = self.uc
        if not uc.reg_read(R.UC_RISCV_REG_MSTATUS) & 0x8:
            return
        mie = uc.reg_read(R.UC_RISCV_REG_MIE)
        if mie & (1 << 7) and self.timer.pending():
            self.irq_count["MachineTimer"] += 1
            self._take(0x80000007, pc, None)
            return
        if mie & (1 << 11) and self.plic.best() is not None:
            if self._vectored():
                n = self.plic.claim()
                self.irq_count[self.names.irq_name(n) if self.names else f"IRQ{n}"] += 1
                self._take(0x8000000B, pc, n)
            else:
                self.irq_count["MachineExternal"] += 1
                self._take(0x8000000B, pc, None)

    def _take(self, cause: int, pc: int, irq) -> None:
        uc = self.uc
        ms = uc.reg_read(R.UC_RISCV_REG_MSTATUS)
        mie = (ms >> 3) & 1
        ms = (ms & ~((1 << 7) | (1 << 3))) | (mie << 7) | (3 << 11)
        uc.reg_write(R.UC_RISCV_REG_MSTATUS, ms)
        uc.reg_write(R.UC_RISCV_REG_MEPC, pc)
        uc.reg_write(R.UC_RISCV_REG_MCAUSE, cause)
        mtvec = uc.reg_read(R.UC_RISCV_REG_MTVEC)
        if mtvec == NO_TRAP_VECTOR:
            self.fail("trap", f"interrupt {cause:#x} taken before the firmware set mtvec")
            return
        base = mtvec & ~3
        if self._vectored():
            idx = irq if irq is not None else 0
            target = struct.unpack("<I", uc.mem_read(base + 4 * idx, 4))[0]
        elif mtvec & 1 and cause >> 31:
            target = base + 4 * (cause & 0x3FF)
        else:
            target = base
        self._spin = 0
        uc.reg_write(R.UC_RISCV_REG_PC, target)

    # ---- hooks -------------------------------------------------------------------------------
    def _ninsn(self, addr: int, size: int) -> int:
        key = (addr, size)
        n = self._bcount.get(key)
        if n is None:
            data = bytes(self.uc.mem_read(addr, size))
            n, o = 0, 0
            while o < size:
                o += 4 if data[o] & 3 == 3 else 2
                n += 1
            self._bcount[key] = n
        return n

    def _on_block(self, uc, addr, size, _):
        self.cycles += self._ninsn(addr, size)
        self.blocks.append(addr)
        sp = uc.reg_read(R.UC_RISCV_REG_SP)
        if self.stack:
            lo, hi = self.stack
            if lo <= sp <= hi:
                self._stack_armed = True
                if self.sp_min is None or sp < self.sp_min:
                    self.sp_min = sp
            elif self._stack_armed:
                self.fail("stack", f"sp {sp:#x} left the stack region {lo:#x}..{hi:#x} at {self.sym(addr)}")
                return
        if addr == self._spin_addr:
            self._spin += 1
            if self._spin > 3_000_000:
                self.fail("hang", f"CPU spinning in one block at {self.sym(addr)} with no interrupt")
                return
        else:
            self._spin_addr, self._spin = addr, 0
        if self.deadline is not None and self.cycles >= self.deadline:
            self._service()
        if self.cycles >= self.end_cycles:
            self.stop("time")
            return
        if self.irq_maybe:
            self._check_irq(addr)

    def _on_intr(self, uc, intno, _):
        pc_next = uc.reg_read(R.UC_RISCV_REG_PC)
        if intno == 2:  # illegal instruction: maybe Zb* or an Andes CSR
            for cand in (pc_next - 4, pc_next):
                try:
                    insn = struct.unpack("<I", uc.mem_read(cand, 4))[0]
                except UcError:
                    continue
                if insn & 3 != 3:
                    continue
                if self._emulate(cand, insn):
                    if self._stop is None:
                        uc.reg_write(R.UC_RISCV_REG_PC, cand + 4)
                    return
            try:
                raw = bytes(uc.mem_read(pc_next - 4, 4)).hex()
            except UcError:
                raw = "?"
            self.fail("illegal", f"illegal instruction near {self.sym(pc_next - 4)} (bytes {raw})")
            return
        names = {0: "instruction address misaligned", 1: "instruction access fault",
                 3: "breakpoint (ebreak)", 4: "load address misaligned", 5: "load access fault",
                 6: "store address misaligned", 7: "store access fault", 8: "ecall (U)",
                 11: "ecall (M)"}
        self.fail("exception", f"CPU exception {intno} ({names.get(intno, '?')}) near {self.sym(pc_next)}")

    def _emulate(self, pc: int, insn: int) -> bool:
        uc = self.uc
        rs1 = (insn >> 15) & 31
        rs2 = (insn >> 20) & 31
        r1 = uc.reg_read(R.UC_RISCV_REG_X0 + rs1) if rs1 else 0
        r2 = uc.reg_read(R.UC_RISCV_REG_X0 + rs2) if rs2 else 0
        res = rvext.bitmanip(insn, r1, r2)
        if res is not None:
            rd, v = res
            if rd:
                uc.reg_write(R.UC_RISCV_REG_X0 + rd, v)
            return True
        f = rvext.csr_fields(insn)
        if f is None:
            return False
        csr, rd, f3, src, is_imm = f
        if csr in rvext.TIME_CSRS:
            t = self.timer.mtime()
            old = t & M32 if rvext.TIME_CSRS[csr] == "lo" else t >> 32
            if f3 & 3 != 2 or src:
                self.fail("csr", f"write to read-only CSR time at {self.sym(pc)}")
            if rd:
                uc.reg_write(R.UC_RISCV_REG_X0 + rd, old)
            self.warn("csr", f"`time` CSR read at {self.sym(pc)}: Andes cores trap on it (no time CSR)",
                      once_key=pc)
            return True
        if not rvext.is_custom_csr(csr):
            self.fail("csr", f"unsupported CSR {csr:#x} at {self.sym(pc)}")
            return True
        old = self.csr.get(csr, 0)
        srcv = src if is_imm else (uc.reg_read(R.UC_RISCV_REG_X0 + src) if src else 0)
        new = rvext.csr_apply(f3, old, srcv)
        if (f3 & 3) in (2, 3) and src == 0:
            new = None
        if new is not None:
            self._csr_write(csr, new, pc)
        if rd:
            uc.reg_write(R.UC_RISCV_REG_X0 + rd, old)
        return True

    def _csr_write(self, csr: int, val: int, pc: int) -> None:
        self.csr[csr] = val
        name = rvext.CSR_NAMES.get(csr, f"csr{csr:#x}")
        if csr == 0x7CC:
            self.event("cctl", f"{name} = {val} at {self.sym(pc)}")
        elif csr in (rvext.MMISC_CTL, rvext.MCACHE_CTL):
            self.event("csr", f"{name} = {val:#x} at {self.sym(pc)}")
        if csr == rvext.MMISC_CTL:
            self.irq_changed()

    def _on_cycle_csr(self, uc, addr, size, _):
        insn = struct.unpack("<I", uc.mem_read(addr, 4))[0]
        csr, rd, f3, src, is_imm = rvext.csr_fields(insn)
        if (f3 & 3) != 2 or src:  # a write to the counter: let QEMU do it
            return
        self.time_poll()
        v = self.cycles & M32 if rvext.CYCLE_CSRS[csr] == "lo" else self.cycles >> 32
        if rd:
            uc.reg_write(R.UC_RISCV_REG_X0 + rd, v)
        uc.reg_write(R.UC_RISCV_REG_PC, addr + 4)

    def _on_panic(self, uc, addr, size, _):
        """A panic / default handler is a failure; keep running a little so the panic message
        reaches UART0 and the handler's exit path (e.g. the jump to the loader) is visible."""
        ra = uc.reg_read(R.UC_RISCV_REG_RA)
        first = not any(f.kind == "panic" for f in self.findings)
        self.fail("panic", f"reached {self.sym(addr)} (called from {self.sym(ra)})", stop=False)
        if first:
            self.end_cycles = min(self.end_cycles, self.cycles + CPU_HZ * 3)

    def _on_boot_region(self, uc, addr, size, _):
        if addr == LOADER_ENTRY:
            self.bootloader_jump = self._snapshot_at_jump()
            self.event("bootloader", "jump to the download loader at 0x80020000")
            self.stop("bootloader", "jumped to 0x80020000")
            return
        self.fail("boot-region", f"execution in the protected boot region at {addr:#x} "
                  f"(from {self.sym(self.blocks[-2]) if len(self.blocks) > 1 else '?'})")

    def _snapshot_at_jump(self) -> dict:
        uc = self.uc
        ms = uc.reg_read(R.UC_RISCV_REG_MSTATUS)
        en = [self.plic.regs.get(Plic.ENABLE + 4 * w, 0) for w in range(4)]
        on = [w * 32 + n for w in range(4) for n in range(32) if (en[w] >> n) & 1 and w * 32 + n]
        wdogs = [p.name for p in self.bus.models if type(p).__name__ == "Ewdg" and p.running()]
        usb = next((p for p in self.bus.models if p.name == "USB0"), None)
        return {"mstatus_mie": bool(ms & 8), "mcache_ctl": self.csr.get(rvext.MCACHE_CTL, 0),
                "plic_enabled": on, "mie": uc.reg_read(R.UC_RISCV_REG_MIE), "watchdogs": wdogs,
                "usb_run": bool(usb and usb.regs.get(0x140, 0) & 1), "at_ms": self.ms()}

    def _on_unmapped(self, uc, access, addr, size, value, _):
        kinds = {19: "read", 20: "write", 21: "fetch"}
        self.fail("unmapped", f"unmapped {kinds.get(access, access)} of {addr:#010x} (size {size}) "
                  f"near {self.sym(self.pc())}")
        return False

    def _on_write_prot(self, uc, access, addr, size, value, _):
        if FLASH_BASE <= addr < FLASH_BASE + FLASH_SIZE:
            self.fail("xip-store", f"plain store of {value:#x} to XIP flash {addr:#010x} near "
                      f"{self.sym(self.pc())} (flash is only written through the ROM API)")
        else:
            self.fail("rom-store", f"store to ROM {addr:#010x} near {self.sym(self.pc())}")
        return False

    def _on_fetch_prot(self, uc, access, addr, size, value, _):
        self.fail("fetch", f"instruction fetch from non-executable {addr:#010x}")
        return False

    def _on_align(self, uc, access, addr, size, value, _):
        if addr & (size - 1):
            self.fail("unaligned", f"unaligned {size}-byte {'write' if access == UC_HOOK_MEM_WRITE else 'access'}"
                      f" at {addr:#010x} near {self.sym(self.pc())}", stop=False)

    def _on_trace(self, uc, addr, size, _):
        code = bytes(uc.mem_read(addr, size))
        ins = next(self._cs.disasm(code, addr), None)
        txt = f"{ins.mnemonic} {ins.op_str}" if ins else code[::-1].hex()
        self.trace.write(f"{self.cycles:12d} {addr:08x} {txt:40s} ; {self.sym(addr)}\n")

    # ---- test hooks ---------------------------------------------------------------------------
    def force_call_at(self, at_ms: float, func: int, label: str) -> None:
        """At virtual time at_ms, divert the CPU into func() as if main code called it."""
        def fire():
            uc = self.uc
            self.event("hook", f"forcing call to {label} at {self.ms():.3f} ms")
            uc.reg_write(R.UC_RISCV_REG_RA, RETURN_TRAP)
            uc.reg_write(R.UC_RISCV_REG_PC, func)
        self.schedule(int(at_ms * CPU_HZ / 1000), fire)

    # ---- main loop ----------------------------------------------------------------------------
    def run(self, ms: float) -> str:
        self.end_cycles = int(ms * CPU_HZ / 1000)
        self._recompute_deadline()
        pc = self.entry
        uc = self.uc
        while True:
            self._stop = None
            try:
                uc.emu_start(pc, 0xFFFFFFFE)
            except UcError as e:
                if self._stop is None:
                    self.fail("emu", f"unicorn stopped: {e} at {self.sym(self.pc())}")
            pc = self.pc()
            if self._stop is not None:
                return self._stop[0]
            # no stop requested: QEMU halted on WFI (it executes wfi as 'stop')
            try:
                prev = struct.unpack("<I", uc.mem_read(pc - 4, 4))[0]
            except UcError:
                prev = None
            if prev != rvext.WFI:
                self.fail("emu", f"emulation stopped unexpectedly at {self.sym(pc)}")
                return "fail"
            self.wfi_count += 1
            if self.first_wfi is None:
                self.first_wfi, self.first_wfi_pc = self.cycles, pc - 4
                self.event("idle", f"first WFI at {self.sym(pc - 4)}")
            if self._wake_pending():
                continue
            if self.deadline is None:
                self.event("idle", "WFI with no wake-up source scheduled: idle forever")
                return "idle-forever"
            self.cycles = max(self.cycles, min(self.deadline, self.end_cycles))
            if self.cycles >= self.end_cycles:
                return "time"
            self._service()
            pc = self.pc()  # an event may have redirected the CPU
