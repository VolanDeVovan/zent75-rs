"""Pre-flash gate (firmware/emu): the emulator's safety checks on hand-assembled images, the
Zb* emulation, and the real firmware ELF when it is built.

Needs unicorn + pyelftools (skipped otherwise):
    cd tools && uv run --with pytest --with unicorn --with capstone --with pyelftools \\
        python -m pytest -q tests/test_emu.py
"""
from __future__ import annotations

import os
import struct
import sys

import pytest

pytest.importorskip("unicorn")
pytest.importorskip("elftools")

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
EMU = os.path.join(ROOT, "firmware", "emu")
sys.path.insert(0, EMU)

import rvext  # noqa: E402
from image import Image  # noqa: E402
from run import DEFAULT_ELF, gate  # noqa: E402

APP = 0x80040000
SIG = 0x0B4D5475
ZERO, RA, SP, T0, T1, A0, A1, A2, A3, A4, A5 = 0, 1, 2, 5, 6, 10, 11, 12, 13, 14, 15


# ---- a tiny RV32 assembler ---------------------------------------------------------------------
def _i(op, rd, f3, rs1, imm):
    return ((imm & 0xFFF) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op


def addi(rd, rs1, imm):
    return [_i(0x13, rd, 0, rs1, imm)]


def li(rd, v):
    v &= 0xFFFFFFFF
    lo = v & 0xFFF
    if lo >= 0x800:
        lo -= 0x1000
    hi = ((v - lo) >> 12) & 0xFFFFF
    return [(hi << 12) | (rd << 7) | 0x37] + addi(rd, rd, lo)


def lw(rd, rs1, imm):
    return [_i(0x03, rd, 2, rs1, imm)]


def sw(rs2, rs1, imm):
    return [((imm >> 5 & 0x7F) << 25) | (rs2 << 20) | (rs1 << 15) | (2 << 12) | ((imm & 31) << 7) | 0x23]


def jalr(rd, rs1, imm=0):
    return [_i(0x67, rd, 0, rs1, imm)]


def jal(rd, off):
    o = off & 0x1FFFFF
    imm = ((o >> 20) & 1) << 31 | ((o >> 1) & 0x3FF) << 21 | ((o >> 11) & 1) << 20 | ((o >> 12) & 0xFF) << 12
    return [imm | (rd << 7) | 0x6F]


WFI = [rvext.WFI]
IDLE = WFI + jal(ZERO, -4)  # wfi; j wfi


def rom_nor_call(slot, args):
    """a0..a5 = args; t1 = xpi_nor_driver_if->slot; jalr ra, t1."""
    code = li(T0, 0x2001FF14) + lw(T0, T0, 0) + lw(T1, T0, slot)
    for i, v in enumerate(args):
        code += li(A0 + i, v)
    return code + jalr(RA, T1)


def image(words, signature=SIG, extra=()):
    img = Image()
    img.path = "<test>"
    img.chunks = [(APP, struct.pack(f"<{len(words) + 1}I", signature, *words))] + list(extra)
    return img


def run_words(words, ms=2.0, **kw):
    return gate(image(words, **kw.pop("img", {})), ms=ms, show_mmio=False, **kw)


def kinds(res):
    return {f.kind for f in res.failures}


# ---- gate behaviour on synthetic images ----------------------------------------------------------
def test_idle_image_passes():
    res = run_words(li(SP, 0x000A0000) + IDLE)
    assert res.ok, res.text
    assert res.m.first_wfi is not None


def test_no_signature_is_refused():
    res = run_words(IDLE, img={"signature": 0xFFFFFFFF})
    assert not res.ok and "boot" in kinds(res)


def test_spinning_without_wfi_fails():
    res = run_words(jal(ZERO, 0), ms=0.5)
    assert "idle" in kinds(res), res.text


def test_illegal_instruction_fails():
    res = run_words([0xFFFFFFFF] + IDLE)
    assert "illegal" in kinds(res), res.text


def test_erase_below_config_is_a_hard_failure_and_does_nothing():
    xpi0 = 0xF3000000
    code = rom_nor_call(0x18, [xpi0, 4, 0, 0x40000, 0x1000]) + IDLE  # erase(app region)
    res = run_words(code)
    assert "flash-gate" in kinds(res), res.text
    assert bytes(res.m.uc.mem_read(APP, 4)) == struct.pack("<I", SIG)  # untouched


@pytest.mark.parametrize("slot,args", [
    (0x20, [0xF3000000, 4, 0, 0x3F000]),  # erase_sector in the loader
    (0x24, [0xF3000000, 4, 0, 0x60000]),  # erase_block (64K) reaching into the app
    (0x28, [0xF3000000, 4, 0, 0x00080000, 0x1000, 16]),  # program the boot header
    (0x1C, [0xF3000000, 4, 0]),  # erase_chip
])
def test_flash_gate_variants(slot, args):
    res = run_words(li(SP, 0x000A0000) + rom_nor_call(slot, args) + IDLE)
    assert "flash-gate" in kinds(res), res.text


def test_erase_and_program_config_bank_is_allowed():
    code = li(SP, 0x000A0000)
    code += rom_nor_call(0x18, [0xF3000000, 4, 0, 0x70000, 0x1000])  # erase bank 1 sector
    code += li(T0, 0x00090000) + li(T1, 0x12345678) + sw(T1, T0, 0)  # data in DLM
    code += rom_nor_call(0x28, [0xF3000000, 4, 0, 0x00090000, 0x70000, 4])  # program
    res = gate(image(code + IDLE), ms=2, show_mmio=False, preload=[(0x80070000, b"\0" * 8)])
    assert res.ok, res.text
    assert bytes(res.m.uc.mem_read(0x80070000, 8)) == struct.pack("<I", 0x12345678) + b"\xff" * 4
    assert [op[1] for op in res.m.flash_ops] == ["erase", "program"]


def test_absolute_flash_address_is_rejected():
    res = run_words(rom_nor_call(0x18, [0xF3000000, 4, 0, 0x80070000, 0x1000]) + IDLE)
    assert "flash" in kinds(res), res.text


def test_plain_store_to_xip_fails():
    res = run_words(li(T0, 0x80070000) + sw(ZERO, T0, 0) + IDLE)
    assert "xip-store" in kinds(res), res.text


def test_jump_into_boot_region_fails_and_loader_jump_is_reported():
    res = run_words(li(T0, 0x80003000) + jalr(ZERO, T0))
    assert "boot-region" in kinds(res), res.text
    res = run_words(li(T0, 0x80020000) + jalr(ZERO, T0))
    assert res.m.bootloader_jump is not None and res.reason == "bootloader"


def test_otp_uid_and_otp_program_gate():
    code = li(SP, 0x000A0000) + li(T0, 0x2001FF0C) + lw(T0, T0, 0) + lw(T1, T0, 0x0C)
    code += li(A0, 0x58) + jalr(RA, T1)  # a0 = read_from_shadow(0x58)
    code += li(T0, 0x00090000) + sw(A0, T0, 0)
    code += li(T0, 0x2001FF0C) + lw(T0, T0, 0) + lw(T1, T0, 0x14) + jalr(RA, T1)  # program()
    res = run_words(code + IDLE)
    assert struct.unpack("<I", res.m.uc.mem_read(0x00090000, 4))[0] != 0
    assert "otp" in kinds(res)


def test_unaligned_access_fails():
    res = run_words(li(T0, 0x00090001) + lw(T1, T0, 0) + IDLE)
    assert "unaligned" in kinds(res), res.text


# ---- Zb* emulation -------------------------------------------------------------------------------
def r(f7, rs2, rs1, f3, rd=10, op=0x33):
    return (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op


@pytest.mark.parametrize("insn,a,b,want", [
    (r(0x20, 2, 1, 7), 0xF0, 0x30, 0xC0),  # andn
    (r(0x10, 2, 1, 4), 3, 0x100, 0x10C),  # sh2add
    (r(0x14, 2, 1, 1), 0, 31, 0x80000000),  # bset
    (r(0x24, 2, 1, 5), 0x8, 3, 1),  # bext
    (r(0x05, 2, 1, 4), 0xFFFFFFFF, 1, 0xFFFFFFFF),  # min (signed)
    (r(0x05, 2, 1, 7), 0xFFFFFFFF, 1, 0xFFFFFFFF),  # maxu
    (r(0x30, 2, 1, 1), 0x80000001, 1, 0x3),  # rol
    (r(0x05, 2, 1, 1), 0x3, 0x3, 0x5),  # clmul
    (r(0x30, 0, 1, 1, op=0x13), 0x00010000, 0, 15),  # clz
    (r(0x30, 1, 1, 1, op=0x13), 0x00010000, 0, 16),  # ctz
    (r(0x30, 2, 1, 1, op=0x13), 0xF0F0, 0, 8),  # cpop
    (r(0x30, 4, 1, 1, op=0x13), 0x80, 0, 0xFFFFFF80),  # sext.b
    (r(0x24, 3, 1, 1, op=0x13), 0xFF, 0, 0xF7),  # bclri 3
    (0x69805513 | (1 << 15), 0x11223344, 0, 0x44332211),  # rev8
    (r(0x04, 0, 1, 4), 0x12345678, 0, 0x5678),  # zext.h
])
def test_bitmanip(insn, a, b, want):
    assert rvext.bitmanip(insn, a, b) == (10, want)


def test_plain_add_is_not_bitmanip():
    assert rvext.bitmanip(r(0x00, 2, 1, 0), 1, 2) is None


# ---- the real firmware -----------------------------------------------------------------------------
needs_elf = pytest.mark.skipif(not os.path.exists(DEFAULT_ELF),
                               reason="firmware ELF not built (firmware/package-uf2.sh)")


@needs_elf
def test_firmware_boots_to_idle():
    res = gate(DEFAULT_ELF, ms=1200, show_mmio=False)
    assert res.ok, res.text
    assert res.m.first_wfi is not None
    assert res.m.irq_count.get("MachineTimer", 0) > 0


@needs_elf
def test_firmware_uf2_matches_elf_boot():
    uf2 = os.path.join(ROOT, "firmware", "zent75-rs.uf2")
    if not os.path.exists(uf2) or os.path.getmtime(uf2) < os.path.getmtime(DEFAULT_ELF):
        pytest.skip("no UF2 packaged from the current ELF")
    res = gate(uf2, ms=300, show_mmio=False)
    assert "boot" not in kinds(res) and "image" not in kinds(res), res.text


@needs_elf
def test_firmware_with_real_config_banks():
    backups = os.path.join(ROOT, "backups")
    dirs = sorted(d for d in os.listdir(backups) if os.path.exists(os.path.join(backups, d, "dump.json"))) \
        if os.path.isdir(backups) else []
    if not dirs:
        pytest.skip("no backups/<ts>/dump.json")
    res = gate(DEFAULT_ELF, ms=600, backup=os.path.join(backups, dirs[-1]), show_mmio=False)
    assert res.ok, res.text


@needs_elf
def test_firmware_loader_exit_paths():
    img = Image.load(DEFAULT_ELF)
    if not any(n.split("::")[-1] == "enter_bootloader" for _, _, n in img.symbols):
        pytest.skip("firmware has no enter_bootloader yet")
    res = gate(DEFAULT_ELF, ms=600, enter_bootloader_ms=300, show_mmio=False)
    assert res.ok and res.reason == "bootloader", res.text
    if any("check_reset_cause" in n for _, _, n in img.symbols):
        res = gate(DEFAULT_ELF, ms=3000, reset_flags=1 << 16, expect_bootloader=True, show_mmio=False)
        assert res.ok and res.reason == "bootloader", res.text
