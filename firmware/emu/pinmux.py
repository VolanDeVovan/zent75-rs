"""Expected pin mux of the ZENT75-HE (docs/re-peripherals.md section 2) and the diff against
what the emulated firmware wrote to IOC / PIOC FUNC_CTL."""
from __future__ import annotations

from regnames import pad_index, pad_name

IOC_BASE = 0xF4040000
PIOC_BASE = 0xF4118000
FUNC_MASK = 0x11F  # alt_select (4:0) + analog (8)
ANALOG = 0x100

# (controller, pad, FUNC_CTL value, meaning)
EXPECTED: list[tuple[str, str, int, str]] = [
    ("IOC", "PA00", 2, "UART0_TXD (debug console)"),
    ("IOC", "PA01", 2, "UART0_RXD"),
    ("IOC", "PA24", ANALOG, "USB0_DM (analog)"),
    ("IOC", "PA25", ANALOG, "USB0_DP (analog)"),
    ("IOC", "PA29", 5, "SPI1_MOSI -> WS2812 chain"),
    ("IOC", "PB00", ANALOG, "hall ADC input (IN6/IN7)"),
    ("IOC", "PB01", ANALOG, "hall ADC input (IN6/IN7)"),
    *[("IOC", f"PB{n:02d}", ANALOG, f"hall ADC input") for n in range(8, 14)],
    *[("PIOC", f"PY{n:02d}", 3, "PY pad handed to the SOC IOC") for n in range(4)],
    *[("IOC", f"PY{n:02d}", 0x11, f"PWM1_P{4 + n} -> analog-mux address bit {n}") for n in range(4)],
]

# Pads whose mux change hurts recovery (BOOT_MODE straps, JTAG): warn when touched.
SENSITIVE = {"PA02": "BOOT_MODE0", "PA03": "BOOT_MODE1", "PA04": "JTAG TDO", "PA05": "JTAG TDI",
             "PA06": "JTAG TCK", "PA07": "JTAG TMS", "PA08": "JTAG TRST"}


def func_name(names, pad: str, value: int) -> str:
    if value & ANALOG:
        return "ANALOG"
    if names is not None and names.available:
        fn = names.pad_function(pad, value & 0x1F)
        if fn:
            return fn
    return f"ALT{value & 0x1F}"


def written_pads(bus) -> dict[tuple[str, str], int]:
    """(controller, pad) -> last FUNC_CTL value written by the firmware."""
    out = {}
    for ctrl, base in (("IOC", IOC_BASE), ("PIOC", PIOC_BASE)):
        for addr, val in bus.last_write.items():
            off = addr - base
            if 0 <= off < 0x1000 and off % 8 == 0:
                out[(ctrl, pad_name(off // 8))] = val
    return out


def diff(bus, names) -> tuple[list, list, list]:
    """-> (rows, failures, warnings). rows: (ctrl, pad, expected, actual|None, status, meaning)."""
    got = written_pads(bus)
    rows, fails, warns = [], [], []
    expected_keys = set()
    for ctrl, pad, want, meaning in EXPECTED:
        expected_keys.add((ctrl, pad))
        have = got.get((ctrl, pad))
        if have is None:
            status = "missing"
        elif (have & FUNC_MASK) == want:
            status = "ok"
        else:
            status = "WRONG"
            fails.append(f"{ctrl} {pad} FUNC_CTL = {have:#x} ({func_name(names, pad, have)}), "
                         f"expected {want:#x} ({meaning})")
        rows.append((ctrl, pad, want, have, status, meaning))
    for (ctrl, pad), have in sorted(got.items(), key=lambda kv: (kv[0][0], pad_index(kv[0][1]))):
        if (ctrl, pad) in expected_keys:
            continue
        fn = func_name(names, pad, have)
        rows.append((ctrl, pad, None, have, "extra", fn))
        if pad in SENSITIVE:
            warns.append(f"{ctrl} {pad} ({SENSITIVE[pad]}) re-muxed to {fn}")
    return rows, fails, warns
