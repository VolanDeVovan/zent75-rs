"""Instructions the unicorn (QEMU 5) RISC-V core does not know, emulated in Python.

- Zba / Zbb / Zbc / Zbs (RV32): our firmware is built with `+zba,+zbb,+zbc,+zbs`; QEMU raises an
  illegal-instruction exception for them, the emulator executes them here and skips the insn.
- CSR instructions on Andes custom CSRs (0x7C0..0x7FF etc.), kept in a plain dict.

All functions are pure (no unicorn), so they are unit-testable.
"""
from __future__ import annotations

M32 = 0xFFFFFFFF

# CSR numbers QEMU rejects but the Andes D25 core implements (custom CSR ranges).
CUSTOM_CSR_RANGES = ((0x7C0, 0x800), (0x800, 0x900), (0xBC0, 0xC00), (0xFC0, 0x1000), (0x3A0, 0x3F0))
CSR_NAMES = {
    0x7C4: "mxstatus", 0x7C9: "mppib", 0x7CA: "mcache_ctl", 0x7CB: "mcctlbeginaddr",
    0x7CC: "mcctlcommand", 0x7CD: "mcctldata", 0x7D0: "mmisc_ctl", 0x7D2: "mrandseq",
    0x7D4: "mrandstate", 0x7D5: "mrandstate", 0x7D8: "mclk_ctl", 0x7E0: "mhsp_ctl",
    0x7E1: "msp_bound", 0x7E2: "msp_base", 0xBC0: "pmacfg0", 0xBD0: "pmaaddr0",
    0xC01: "time", 0xC81: "timeh", 0xB00: "mcycle", 0xB80: "mcycleh", 0xC00: "cycle",
    0xC80: "cycleh", 0xB02: "minstret", 0xB82: "minstreth", 0xC02: "instret", 0xC82: "instreth",
}
CYCLE_CSRS = {0xB00: "lo", 0xB80: "hi", 0xC00: "lo", 0xC80: "hi", 0xB02: "lo", 0xB82: "hi",
              0xC02: "lo", 0xC82: "hi"}
TIME_CSRS = {0xC01: "lo", 0xC81: "hi"}

MMISC_CTL = 0x7D0
MMISC_VEC_PLIC = 1 << 1
MCACHE_CTL = 0x7CA


def is_custom_csr(csr: int) -> bool:
    return any(a <= csr < b for a, b in CUSTOM_CSR_RANGES)


def sext(v: int, bits: int) -> int:
    v &= (1 << bits) - 1
    return v - (1 << bits) if v >> (bits - 1) else v


def s32(v: int) -> int:
    return sext(v, 32)


def _clmul(a: int, b: int) -> int:
    r = 0
    for i in range(32):
        if (b >> i) & 1:
            r ^= a << i
    return r


def _rol(a: int, n: int) -> int:
    n &= 31
    return ((a << n) | (a >> (32 - n))) & M32 if n else a


def _ror(a: int, n: int) -> int:
    n &= 31
    return ((a >> n) | (a << (32 - n))) & M32 if n else a


def bitmanip(insn: int, rs1v: int, rs2v: int) -> tuple[int, int] | None:
    """Execute a Zba/Zbb/Zbc/Zbs RV32 instruction. Returns (rd, value) or None if `insn` is not
    one of them. rs1v/rs2v are the unsigned register values."""
    op = insn & 0x7F
    rd = (insn >> 7) & 31
    f3 = (insn >> 12) & 7
    rs2 = (insn >> 20) & 31
    f7 = insn >> 25
    a, b = rs1v & M32, rs2v & M32
    sh = b & 31
    v = None
    if op == 0x33:  # OP
        if f7 == 0x20:
            v = {7: a & ~b, 6: a | ~b, 4: ~(a ^ b)}.get(f3)
        elif f7 == 0x10:
            v = {2: (a << 1) + b, 4: (a << 2) + b, 6: (a << 3) + b}.get(f3)
        elif f7 == 0x05:
            if f3 == 1:
                v = _clmul(a, b)
            elif f3 == 3:
                v = _clmul(a, b) >> 32
            elif f3 == 2:
                v = _clmul(a, b) >> 31
            elif f3 == 4:
                v = a if s32(a) < s32(b) else b
            elif f3 == 5:
                v = min(a, b)
            elif f3 == 6:
                v = a if s32(a) > s32(b) else b
            elif f3 == 7:
                v = max(a, b)
        elif f7 == 0x30:
            v = {1: _rol(a, sh), 5: _ror(a, sh)}.get(f3)
        elif f7 == 0x24:
            v = {1: a & ~(1 << sh), 5: (a >> sh) & 1}.get(f3)
        elif f7 == 0x14 and f3 == 1:
            v = a | (1 << sh)
        elif f7 == 0x34 and f3 == 1:
            v = a ^ (1 << sh)
        elif f7 == 0x04 and f3 == 4 and rs2 == 0:
            v = a & 0xFFFF  # zext.h
    elif op == 0x13:  # OP-IMM
        shamt = rs2
        imm12 = insn >> 20
        if f3 == 1:
            if f7 == 0x30:
                if rs2 == 0:  # clz
                    v = 32 - a.bit_length()
                elif rs2 == 1:  # ctz
                    v = 32 if a == 0 else (a & -a).bit_length() - 1
                elif rs2 == 2:  # cpop
                    v = bin(a).count("1")
                elif rs2 == 4:  # sext.b
                    v = sext(a, 8)
                elif rs2 == 5:  # sext.h
                    v = sext(a, 16)
            elif f7 == 0x24:
                v = a & ~(1 << shamt)
            elif f7 == 0x14:
                v = a | (1 << shamt)
            elif f7 == 0x34:
                v = a ^ (1 << shamt)
        elif f3 == 5:
            if f7 == 0x30:
                v = _ror(a, shamt)
            elif f7 == 0x24:
                v = (a >> shamt) & 1
            elif imm12 == 0x287:  # orc.b
                v = 0
                for i in range(4):
                    if (a >> (8 * i)) & 0xFF:
                        v |= 0xFF << (8 * i)
            elif imm12 == 0x698:  # rev8 (RV32)
                v = int.from_bytes(a.to_bytes(4, "little"), "big")
    if v is None:
        return None
    return rd, v & M32


def csr_fields(insn: int) -> tuple[int, int, int, int, bool] | None:
    """Decode a Zicsr instruction -> (csr, rd, f3, rs1_or_uimm, is_imm) or None."""
    if insn & 0x7F != 0x73:
        return None
    f3 = (insn >> 12) & 7
    if f3 in (0, 4):
        return None
    return insn >> 20, (insn >> 7) & 31, f3, (insn >> 15) & 31, f3 >= 5


def csr_apply(f3: int, old: int, src: int) -> int | None:
    """New CSR value for csrrw/csrrs/csrrc(i); None means 'no write' (rs1 = x0 for set/clear)."""
    op = f3 & 3
    if op == 1:
        return src & M32
    if op == 2:
        return (old | src) & M32
    if op == 3:
        return old & ~src & M32
    return None


def insn_len(halfword: int) -> int:
    return 4 if halfword & 3 == 3 else 2


WFI = 0x10500073
