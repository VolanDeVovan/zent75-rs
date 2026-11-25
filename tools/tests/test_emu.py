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
    res = gate(DEFAULT_ELF, ms=250, show_mmio=False)  # full app idles after ~100 ms
    assert res.ok, res.text
    assert res.m.first_wfi is not None
    assert sum(res.m.irq_count.values()) > 0


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
    res = gate(DEFAULT_ELF, ms=250, backup=os.path.join(backups, dirs[-1]), show_mmio=False)
    assert res.ok, res.text


@needs_elf
def test_firmware_loader_exit_paths():
    img = Image.load(DEFAULT_ELF)
    if not any(n.split("::")[-1] == "enter_bootloader" for _, _, n in img.symbols):
        pytest.skip("firmware has no enter_bootloader yet")
    res = gate(DEFAULT_ELF, ms=250, enter_bootloader_ms=150, show_mmio=False)
    assert res.ok and res.reason == "bootloader", res.text
    if any("check_reset_cause" in n for _, _, n in img.symbols):
        res = gate(DEFAULT_ELF, ms=3000, reset_flags=1 << 16, expect_bootloader=True, show_mmio=False)
        assert res.ok and res.reason == "bootloader", res.text


# ---- interrupts + scan peripherals on a hand-assembled ISR ---------------------------------------
ADC0 = 0xF3080000
COUNTER = 0x00090100
MRET = 0x30200073


def csr(f3, csrno, rs1_or_imm, rd=0):
    return [(csrno << 20) | (rs1_or_imm << 15) | (f3 << 12) | (rd << 7) | 0x73]


def store(addr, value):
    return li(T0, addr) + li(T1, value) + sw(T1, T0, 0)


def isr_image(setup):
    """main: vector table entry 58 -> handler, vectored PLIC, ADC0 IRQ enabled, `setup`, idle.
    handler: COUNTER += 1, clear ADC0 int_sts, complete IRQ 58, mret."""
    handler = li(T0, COUNTER) + lw(T1, T0, 0) + addi(T1, T1, 1) + sw(T1, T0, 0)
    handler += store(ADC0 + 0x1110, 1 << 23) + store(0xE4200004, 58) + [MRET]

    def main(h):
        code = li(SP, 0x000A0000) + store(COUNTER, 0) + store(4 * 58, h)
        code += csr(1, 0x305, 0)  # mtvec = 0 (vector table at ILM 0)
        code += csr(6, 0x7D0, 2)  # mmisc_ctl.VEC_PLIC
        code += store(0xE4000000, 2) + store(0xE4000000 + 4 * 58, 1) + store(0xE4002004, 1 << 26)
        code += store(ADC0 + 0x804, 0x00090000) + store(ADC0 + 0x80C, 3)  # seq DMA, 4 words
        code += store(ADC0 + 0x810, 1) + store(ADC0 + 0x814, 3 | 0x20)  # queue: ch1, ch3+int
        code += store(ADC0 + 0x1114, 1 << 23)  # int_en seq_cvc
        code += li(T0, 0x800) + csr(2, 0x304, T0) + csr(6, 0x300, 8)  # mie.MEIE, mstatus.MIE
        return code + setup + IDLE
    h = APP + 4 + 4 * len(main(0))
    return image(main(h) + handler)


def test_adc_sw_trigger_interrupt_and_dma():
    res = gate(isr_image(store(ADC0 + 0x800, (1 << 8) | 2 | 4)), ms=1, show_mmio=False)
    assert res.ok, res.text
    m = res.m
    assert struct.unpack("<I", m.uc.mem_read(COUNTER, 4))[0] == 1
    w0, w1 = struct.unpack("<2I", m.uc.mem_read(0x00090000, 8))
    assert (w0 >> 20) & 0x1F == 1 and (w0 >> 16) & 0xF == 0
    assert (w1 >> 20) & 0x1F == 3 and (w1 >> 16) & 0xF == 1
    assert abs((w0 & 0xFFFF) - 0x8000) < 8
    assert m.irq_count.get("ADC0") == 1


def test_pwm0_hardware_trigger_rate():
    pwm0 = 0xF0318000
    setup = store(ADC0 + 0x800, (1 << 8) | 1)  # hw_trig_en, 2 conversions
    setup += store(pwm0 + 0x08, (20000 - 1) << 4)  # 125 us period at 160 MHz
    setup += store(pwm0 + 0x0C, 320 << 4) + store(pwm0 + 0x14, 10320 << 4)  # cmp[0], cmp[2]
    setup += store(pwm0 + 0x80 + 4 * 8, 0x15000000)  # chcfg[8]
    setup += store(pwm0 + 0xF0, 1 << 7)  # gcr.cen
    res = gate(isr_image(setup), ms=1, show_mmio=False)
    assert res.ok, res.text
    n = struct.unpack("<I", res.m.uc.mem_read(COUNTER, 4))[0]
    assert 14 <= n <= 17, n  # 8 periods x 2 triggers in 1 ms


def test_direct_xpi_controller_write_fails():
    res = run_words(store(0xF3000000, 1) + IDLE)
    assert "flash-gate" in kinds(res), res.text


def test_panic_handler_is_a_failure_but_keeps_running():
    words = li(T0, APP + 4 + 4 * 3) + jalr(ZERO, T0) + store(0xF0040020, ord("P")) + IDLE
    img = image(words)
    target = APP + 4 + 4 * 3
    img.symbols = [(target, 4, "rust_begin_unwind")]
    img._addrs = [target]
    res = gate(img, ms=2, show_mmio=False)
    assert "panic" in kinds(res), res.text
    assert res.m.first_wfi is not None  # ran on after the panic entry


# ---- models added for the full firmware ------------------------------------------------------------
def test_pad_names_follow_metapac():
    from regnames import pad_index, pad_name
    assert pad_name(448) == "PY00" and pad_name(416) == "PX00" and pad_name(29) == "PA29"
    assert pad_index("PY03") == 451


def test_plic_claimed_source_not_redelivered_and_preemption():
    from periph import Plic

    class M:
        def irq_changed(self):
            pass
    p = Plic(M())
    p.regs[4 * 6] = 2  # GPTMR1 prio 2
    p.regs[4 * 58] = 7  # ADC0 prio 7
    p.regs[p.ENABLE] = 1 << 6
    p.regs[p.ENABLE + 4] = 1 << 26
    p.regs[p.FEATURE] = 3  # preemptive + vectored
    p.raise_irq(6)
    assert p.claim() == 6
    p.raise_irq(6)
    assert p.best() is None  # in service: gateway holds it
    p.raise_irq(58)
    assert p.best() == 58  # higher priority preempts
    assert p.claim() == 58
    p.write(p.CLAIM, 4, 58)
    p.write(p.CLAIM, 4, 6)
    assert p.best() == 6  # re-delivered after completion
    p.regs[p.THRESHOLD] = 2
    assert p.best() is None  # threshold lock


def test_usbsts_is_write_one_to_clear():
    res = run_words(store(0xF300C144, 0) + IDLE)  # just boots; use the model directly below
    usb = next(x for x in res.m.bus.models if x.name == "USB0")
    usb.regs[0x144] = 0x5
    usb.write(0x144, 4, 0x1)
    assert usb.regs[0x144] == 0x4


def test_pwm_counter_runs_from_virtual_time():
    pwm0 = 0xF0318000
    code = store(pwm0 + 0x08, (20000 - 1) << 4) + store(pwm0 + 0xF0, 1 << 7)
    code += li(T0, pwm0 + 0x170) + lw(T1, T0, 0) + li(T0, 0x00090000) + sw(T1, T0, 0)
    for _ in range(50):
        code += jal(ZERO, 4)  # one basic block each: virtual time advances per block
    code += li(T0, pwm0 + 0x170) + lw(T1, T0, 0) + li(T0, 0x00090004) + sw(T1, T0, 0)
    res = run_words(code + IDLE)
    a, b = struct.unpack("<2I", res.m.uc.mem_read(0x00090000, 8))
    assert b > a and (b - a) >> 4 < 20000
