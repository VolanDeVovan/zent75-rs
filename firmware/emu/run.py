#!/usr/bin/env python3
"""Pre-flash gate: run our firmware ELF (or UF2) on an emulated HPM5361 and check it.

usage: uv run --with unicorn --with capstone --with pyelftools python firmware/emu/run.py \\
           <elf|uf2> [--backup DIR] [--ms N] [--trace [FILE]] [--banner REGEX] ...

Exit code 0 = no hard failure, 1 = at least one FAIL, 2 = usage / load error.
See firmware/emu/README.md for what is checked and what is not modelled.
"""
from __future__ import annotations

import argparse
import collections
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)

from image import Image, config_banks_from_backup  # noqa: E402
from machine import CPU_HZ, FAIL, INFO, WARN, Machine  # noqa: E402
import pinmux  # noqa: E402
from regnames import RegNames  # noqa: E402

DEFAULT_ELF = os.path.join(os.path.dirname(HERE), "app", "target", "riscv32imafc-unknown-none-elf",
                           "release", "zent75-rs")
HOOK_SYMBOLS = ("enter_bootloader", "EnterBootloader", "jump_to_bootloader", "jump_to_loader")


class Result:
    def __init__(self, m: Machine, reason: str, lines: list[str]):
        self.m, self.reason, self.lines = m, reason, lines
        self.failures = [f for f in m.findings if f.level == FAIL]
        self.warnings = [f for f in m.findings if f.level == WARN]
        self.ok = not self.failures

    @property
    def text(self) -> str:
        return "\n".join(self.lines)


def gate(path: str, ms: float = 1200.0, backup: str | None = None, banner: str | None = None,
         strict_pinmux: bool = False, align_check: bool = True, trace=None, verbose=False,
         enter_bootloader_ms: float | None = None, show_mmio: bool = True,
         reset_flags: int = 0, expect_bootloader: bool = False) -> Result:
    img = Image.load(path)
    names = RegNames()
    m = Machine(img, names if names.available else None, align_check=align_check, trace=trace,
                verbose=verbose, reset_flags=reset_flags)
    m.load_image()
    if backup:
        m.preload_flash(config_banks_from_backup(backup))
    reason = "not-started"
    hook_target = None
    if m.boot():
        if enter_bootloader_ms is not None:
            cands = []
            for want in HOOK_SYMBOLS:
                cands += [(a, n) for a, _, n in img.symbols if n.split("::")[-1] == want]
            if not cands:
                m.fail("hook", f"--enter-bootloader: none of {HOOK_SYMBOLS} in the ELF symbols",
                       stop=False)
            else:
                hook_target = cands[0]
                m.force_call_at(enter_bootloader_ms, cands[0][0], cands[0][1])
                ms = max(ms, enter_bootloader_ms + 2000)  # stock-like exits wait 1 s first
        if not m.findings or all(f.level != FAIL for f in m.findings):
            reason = m.run(ms)
    lines = report(m, reason, img, names, banner, strict_pinmux, ms, backup, hook_target,
                   enter_bootloader_ms, show_mmio, expect_bootloader)
    return Result(m, reason, lines)


def report(m: Machine, reason, img, names, banner, strict_pinmux, ms, backup, hook_target,
           enter_bootloader_ms, show_mmio, expect_bootloader=False) -> list[str]:
    out: list[str] = []
    p = out.append
    p(f"image   : {img.path}")
    p(f"metapac : {names.src if names.available else 'not found (raw addresses)'}")
    if backup:
        p(f"config  : preloaded from {backup}")
    p(f"run     : {m.ms():.3f} ms virtual ({m.cycles} cycles @ {CPU_HZ // 1_000_000} MHz), "
      f"stop reason: {reason}")

    # ---- derived checks
    if reason not in ("not-started",):
        if m.first_wfi is None and reason != "bootloader":
            m.note(FAIL, "idle", "never reached WFI (main loop / executor idle)")
        if m.stack and m.sp_min is None and reason != "not-started":
            m.note(FAIL, "stack", "sp never entered the linker stack region")
    uart_text = "\n".join(line for _, n, line in m.uart_lines if n == "UART0")
    pending = m.bus.models and next((x for x in m.bus.models if x.name == "UART0"), None)
    if pending is not None and len(pending.line):
        uart_text += ("\n" if uart_text else "") + pending.line.decode("utf-8", "replace")
    if banner:
        if not re.search(banner, uart_text):
            m.note(FAIL, "uart", f"UART0 banner /{banner}/ not seen")
    elif not uart_text:
        m.note(WARN, "uart", "no UART0 output (pass --banner REGEX to require one)")
    rows, pm_fail, pm_warn = pinmux.diff(m.bus, names if names.available else None)
    for s in pm_fail:
        m.note(FAIL, "pinmux", s)
    for s in pm_warn:
        m.note(WARN, "pinmux", s)
    missing = [r for r in rows if r[4] == "missing"]
    if missing:
        msg = f"{len(missing)} expected pad(s) never muxed: " + ", ".join(f"{r[0]} {r[1]}" for r in missing)
        m.note(FAIL if strict_pinmux else WARN, "pinmux", msg)
    driven = collections.OrderedDict()
    for cyc, ctrl, pad, what, val in m.gpio_log:
        if what == "oe" and val:
            driven.setdefault(pad, ctrl)
    for pad, ctrl in driven.items():
        m.note(WARN, "gpio", f"{pad} driven as GPIO output ({ctrl}); not a known output of this board")
    for addr, info in sorted(m.bus.polled.items()):
        name = names.decode(addr) if names.available else f"{addr:#x}"
        m.note(WARN, "poll", f"poll loop broken on {name} ({addr:#010x}) read from {m.sym(info['pc'])} "
               f"(value {info['value']:#x} never changed)")
    want_jump = hook_target is not None or expect_bootloader
    if want_jump and m.bootloader_jump is None:
        what = (f"{hook_target[1]} forced at {enter_bootloader_ms} ms" if hook_target
                else f"reset flags {m.reset_flags:#x}")
        m.note(FAIL, "loader", f"{what}: no jump to 0x80020000")
    if m.bootloader_jump is not None:
        j = m.bootloader_jump
        lvl = FAIL if want_jump else WARN
        if not want_jump:
            m.note(WARN, "loader", f"firmware jumped to the download loader at {j['at_ms']:.3f} ms")
        if j["mcache_ctl"] & 3:
            m.note(lvl, "loader", f"jumped to the loader with L1 caches on (mcache_ctl {j['mcache_ctl']:#x})")
        if j["watchdogs"]:
            m.note(lvl, "loader", f"jumped to the loader with {', '.join(j['watchdogs'])} running")
        if j["mstatus_mie"]:
            m.note(WARN, "loader", "jumped to the loader with mstatus.MIE set")
        if j["plic_enabled"]:
            m.note(WARN, "loader", f"jumped to the loader with PLIC sources enabled: {j['plic_enabled']}")
        if j["usb_run"]:
            m.note(WARN, "loader", "jumped to the loader with USB0 still running (usbcmd.rs)")

    # ---- sections
    p("")
    p("== checks")
    kinds_failed = {f.kind for f in m.findings if f.level == FAIL}

    def chk(label, kinds, extra=""):
        bad = kinds_failed & set(kinds)
        p(f"  [{'FAIL' if bad else ' ok '}] {label}{(' - ' + extra) if extra else ''}")

    w0 = m.uc.mem_read(0x80040000, 4)
    chk("boot signature 0x0B4D5475 at 0x80040000, entry 0x80040004", ["boot", "image"],
        f"word0 {int.from_bytes(w0, 'little'):#010x}")
    chk("reaches idle (WFI)", ["idle"], f"first WFI at {m.ms(m.first_wfi):.3f} ms in "
        f"{m.sym(m.first_wfi_pc)}, {m.wfi_count} WFIs" if m.first_wfi is not None else
        ("left for the loader first" if m.bootloader_jump else "never"))
    chk("no traps / panics / illegal instructions / hangs", ["trap", "panic", "illegal", "exception",
                                                              "csr", "hang", "emu", "fetch"])
    chk("no unmapped or unaligned accesses", ["unmapped", "unaligned", "mmio"])
    chk("no flash erase/program below 0x80070000 (ROM API gate)", ["flash-gate", "flash", "otp", "rom"],
        f"{len(m.flash_ops)} flash op(s)")
    chk("no plain stores to XIP flash / ROM", ["xip-store", "rom-store", "boot-region"])
    if m.stack:
        lo, hi = m.stack
        used = (hi - m.sp_min) if m.sp_min is not None else 0
        chk("stack stays inside its region", ["stack"],
            f"{lo:#x}..{hi:#x}, peak use {used} bytes")
    first = next((ln for ln in uart_text.splitlines() if ln.strip()), "")
    chk("UART0 banner", ["uart"], repr(first[:70]) if uart_text else "no output")
    chk("pin mux matches the board table", ["pinmux"],
        f"{sum(r[4] == 'ok' for r in rows)} ok, {len(missing)} missing, "
        f"{sum(r[4] == 'WRONG' for r in rows)} wrong, {sum(r[4] == 'extra' for r in rows)} extra")
    if want_jump:
        label = f"forced {hook_target[1]}" if hook_target else f"reset flags {m.reset_flags:#x}"
        chk(f"{label}: clean jump to the loader 0x80020000", ["hook", "loader"],
            f"at {m.bootloader_jump['at_ms']:.3f} ms" if m.bootloader_jump else "no jump")
    wd = [x for x in m.bus.models if type(x).__name__ == "Ewdg" and x.feeds]
    if wd or "watchdog" in kinds_failed:
        chk("watchdog never expires", ["watchdog"], ", ".join(
            f"{x.name}: {x.feeds} feeds, longest gap {x.max_gap * 1000 / CPU_HZ:.1f} ms" for x in wd))

    p("")
    p("== findings")
    if not m.findings:
        p("  (none)")
    for f in m.findings:
        p(f"  {f.level} [{f.kind}] @{m.ms(f.cycle):.3f}ms: {f.msg}")

    if uart_text:
        p("")
        p("== UART0")
        for line in uart_text.splitlines()[:200]:
            p(f"  | {line}")

    p("")
    p("== pin mux (IOC/PIOC FUNC_CTL)")
    for ctrl, pad, want, have, status, meaning in rows:
        w = f"{want:#06x}" if want is not None else "   -  "
        h = f"{have:#06x}" if have is not None else "   -  "
        p(f"  {status:8s} {ctrl:4s} {pad}  want {w}  have {h}  {meaning}")

    if m.gpio_log:
        p("")
        p("== GPIO outputs")
        per = collections.defaultdict(list)
        for cyc, ctrl, pad, what, val in m.gpio_log:
            if what == "level":
                per[(ctrl, pad)].append(cyc)
        for (ctrl, pad), ts in per.items():
            if len(ts) > 1:
                gaps = [(b - a) * 1000 / CPU_HZ for a, b in zip(ts, ts[1:])]
                p(f"  {ctrl} {pad}: {len(ts)} level changes, mean interval {sum(gaps) / len(gaps):.3f} ms")
            else:
                p(f"  {ctrl} {pad}: {len(ts)} level change(s)")

    if m.irq_count:
        p("")
        p("== interrupts taken")
        for k, v in m.irq_count.most_common():
            p(f"  {k}: {v}")

    if m.flash_ops or m.rom_calls:
        p("")
        p("== ROM API")
        for (drv, slot), n in sorted(m.rom_calls.items()):
            p(f"  {drv}+{slot:#04x}: {n} call(s)")
        for cyc, what, lo, length, st in m.flash_ops:
            p(f"  @{m.ms(cyc):.3f}ms {what} {lo:#x}+{length:#x} {st}")

    if show_mmio:
        p("")
        p("== MMIO registers written (address, writes, last value, register)")
        for addr in sorted(m.bus.write_count):
            name = names.decode(addr, m.bus.last_write.get(addr)) if names.available else ""
            p(f"  {addr:#010x} {m.bus.write_count[addr]:6d}  {m.bus.last_write.get(addr, 0):#010x}  {name}")

    if any(f.level == FAIL for f in m.findings) and m.blocks:
        p("")
        p("== last basic blocks")
        for a in list(m.blocks)[-12:]:
            p(f"  {a:#010x} {m.sym(a)}")

    nf = sum(f.level == FAIL for f in m.findings)
    nw = sum(f.level == WARN for f in m.findings)
    p("")
    p(f"RESULT: {'FAIL' if nf else 'PASS'} ({nf} failure(s), {nw} warning(s))")
    return out


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("image", nargs="?", default=DEFAULT_ELF, help="firmware ELF or UF2 (default: release ELF)")
    ap.add_argument("--backup", help="tools/dump_all.py backup dir to preload the config banks from")
    ap.add_argument("--ms", type=float, default=1200.0, help="virtual run time in ms (default 1200)")
    ap.add_argument("--trace", nargs="?", const="-", help="instruction trace to FILE (or stdout)")
    ap.add_argument("--banner", help="regex that UART0 output must match (hard failure otherwise)")
    ap.add_argument("--strict-pinmux", action="store_true", help="expected pads never muxed = failure")
    ap.add_argument("--no-align-check", action="store_true", help="skip the (slow) alignment hooks")
    ap.add_argument("--enter-bootloader", type=float, metavar="MS",
                    help="at MS, force a call to the firmware's enter-bootloader function and "
                         "require a jump to 0x80020000")
    ap.add_argument("--reset-flags", type=lambda x: int(x, 0), default=0, metavar="VALUE",
                    help="PPOR reset_flag at boot (e.g. 0x10000 = previous reset by EWDG0)")
    ap.add_argument("--expect-bootloader", action="store_true",
                    help="require a clean jump to the loader (use with --reset-flags)")
    ap.add_argument("--no-mmio", action="store_true", help="omit the MMIO write list")
    ap.add_argument("-v", "--verbose", action="store_true", help="print events and UART live")
    a = ap.parse_args(argv)
    if not os.path.exists(a.image):
        print(f"no such image: {a.image} (build with firmware/package-uf2.sh)", file=sys.stderr)
        return 2
    trace = None
    if a.trace:
        trace = sys.stdout if a.trace == "-" else open(a.trace, "w")
    try:
        res = gate(a.image, ms=a.ms, backup=a.backup, banner=a.banner, strict_pinmux=a.strict_pinmux,
                   align_check=not a.no_align_check, trace=trace, verbose=a.verbose,
                   enter_bootloader_ms=a.enter_bootloader, show_mmio=not a.no_mmio,
                   reset_flags=a.reset_flags, expect_bootloader=a.expect_bootloader)
    except (OSError, ValueError) as e:
        print(f"cannot load {a.image}: {e}", file=sys.stderr)
        return 2
    finally:
        if trace not in (None, sys.stdout):
            trace.close()
    print(res.text)
    return 0 if res.ok else 1


if __name__ == "__main__":
    sys.exit(main())
