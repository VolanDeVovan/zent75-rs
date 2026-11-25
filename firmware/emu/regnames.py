"""Register and pin names for the HPM5361, read from the hpm-metapac sources in ~/.cargo.

Everything here is optional: if no metapac checkout is found, `RegNames.available` is False and
the emulator prints raw addresses instead.

- `RegNames.decode(addr, value)` -> "IOC.pad[23].func_ctl {alt_select=0x0}" style names.
- `RegNames.fields(addr)` -> [(field, shift, mask)] of the register at addr (used to auto-clear
  `*busy*` status bits).
- `RegNames.pad_function(pad, alt)` -> "UART0_TXD" from the IOC_<PAD>_FUNC_CTL_<FN> constants.
"""
from __future__ import annotations

import functools
import glob
import os
import re

PAC_GLOB = os.path.expanduser("~/.cargo/git/checkouts/hpm-metapac-*/*/src")
CHIP = "hpm5361"


def pad_name(index: int) -> str:
    """Pad index (IOC pad number, 32 per port) -> 'PA00'."""
    port, pin = divmod(index, 32)
    return f"P{PORTS.get(port, '?')}{pin:02d}"


# IOC port letters by pad index / 32: A, B, ... then the special ports X (13) and Y (14), as in
# hpm-metapac (PY00 = 448).
PORTS = {**{i: "ABCDEFGHIJKLM"[i] for i in range(13)}, 13: "X", 14: "Y", 15: "Z"}
_PORT_IDX = {v: k for k, v in PORTS.items()}


def pad_index(name: str) -> int:
    return _PORT_IDX[name[1]] * 32 + int(name[2:])


class RegNames:
    def __init__(self, src: str | None = None) -> None:
        if src is None:
            cands = sorted(glob.glob(PAC_GLOB), key=os.path.getmtime, reverse=True)
            cands = [c for c in cands if os.path.exists(f"{c}/chips/{CHIP}/pac.rs")]
            src = cands[0] if cands else None
        self.src = src
        self.available = src is not None
        self.instances: list[tuple[int, str, str, str]] = []  # (addr, name, module, type)
        self.irqs: dict[int, str] = {}
        self.pad_funcs: dict[tuple[str, int], str] = {}
        self._mods: dict[str, str] = {}
        if not self.available:
            return
        pac = open(f"{src}/chips/{CHIP}/pac.rs").read()
        self._mods = {m: p for p, m in re.findall(
            r'#\[path = "../../peripherals/(\w+)\.rs"\]\s*pub mod (\w+);', pac)}
        for name, mod, typ, addr in re.findall(
                r"pub const (\w+): (\w+)::(\w+) = unsafe \{ \w+::\w+::from_ptr\((0x[0-9a-f_]+)usize", pac):
            self.instances.append((int(addr.replace("_", ""), 16), name, mod, typ))
        self.instances.sort(reverse=True)
        for doc, name, num in re.findall(r'#\[doc = "(\d+) - (\w+)"\]\s*\w+ = (\d+),', pac):
            self.irqs[int(num)] = name
        for pad, fn, alt in re.findall(r"pub const IOC_(P[A-Z]\d\d)_FUNC_CTL_(\w+): u8 = (\d+);", pac):
            self.pad_funcs[(pad, int(alt))] = fn

    # ---- metapac block parsing -------------------------------------------------------------
    @functools.lru_cache(None)
    def _parse(self, mod: str):
        s = open(f"{self.src}/peripherals/{self._mods[mod]}.rs").read()
        blocks: dict[str, list] = {}
        head = s[:s.find("pub mod regs")] if "pub mod regs" in s else s
        for bm in re.finditer(r"impl (\w+) \{(.*?)\n\}", head, re.S):
            items = []
            for fm in re.finditer(
                    r"pub const fn (\w+)\(\s*self(?:,\s*n: usize)?,?\s*\)\s*->\s*([^{]*?)\{(.*?)\n    \}",
                    bm.group(2), re.S):
                fname, ret, body = fm.groups()
                off = re.search(r"wrapping_add\((0x[0-9a-f]+|\d+)usize(?: \+ n \* (\d+)usize)?\)", body)
                if not off:
                    continue
                base = int(off.group(1), 0)
                stride = int(off.group(2)) if off.group(2) else 0
                cnt = re.search(r"assert!\(n < (\d+)usize\)", body)
                cnt = int(cnt.group(1)) if cnt else 1
                rm = re.search(r"Reg<\s*(?:regs::)?(\w+)", ret)
                kind = ("reg", rm.group(1)) if rm else ("blk", ret.strip().split("::")[-1])
                items.append((fname, base, stride, cnt, kind))
            blocks[bm.group(1)] = items
        regs: dict[str, list] = {}
        rs = s[s.find("pub mod regs"):] if "pub mod regs" in s else ""
        for rm in re.finditer(r"impl (\w+) \{(.*?)\n    \}", rs, re.S):
            fields = []
            for f in re.finditer(
                    r"pub const fn (\w+)\(&self(?:, n: usize)?\) -> [^{]*\{\s*(?:assert![^;]*;\s*)?"
                    r"(?:let offs = [^;]*;\s*)?let val = \(self\.0 >> (\d+)usize(?: \+ offs)?\) & (0x[0-9a-f]+);",
                    rm.group(2)):
                try:
                    fields.append((f.group(1), int(f.group(2)), int(f.group(3), 16)))
                except ValueError:
                    pass
            regs[rm.group(1)] = fields
        return blocks, regs

    def _resolve(self, mod: str, blk: str, off: int):
        blocks, _ = self._parse(mod)
        for fname, base, stride, cnt, kind in blocks.get(blk, []):
            for n in range(cnt):
                o = base + n * stride
                suffix = f"[{n}]" if cnt > 1 else ""
                if kind[0] == "reg" and off == o:
                    return fname + suffix, kind[1]
                if kind[0] == "blk" and o <= off < o + (stride or 0x100000):
                    r = self._resolve(mod, kind[1], off - o)
                    if r:
                        return f"{fname}{suffix}.{r[0]}", r[1]
        return None

    def instance(self, addr: int):
        for a, name, mod, typ in self.instances:  # sorted by address, descending
            if a <= addr < a + (0x400000 if name in ("PLIC", "PLICSW") else 0x4000):
                return a, name, mod, typ
        return None

    @functools.lru_cache(4096)
    def lookup(self, addr: int):
        """addr -> (instance, register path, regs type) or None."""
        if not self.available:
            return None
        inst = self.instance(addr)
        if not inst:
            return None
        a, name, mod, typ = inst
        try:
            r = self._resolve(mod, typ, addr - a)
        except (OSError, KeyError):
            r = None
        return name, (r[0] if r else f"+{addr - a:#x}"), (r[1] if r else None), mod

    def fields(self, addr: int) -> list[tuple[str, int, int]]:
        lk = self.lookup(addr)
        if not lk or not lk[2]:
            return []
        _, regs = self._parse(lk[3])
        return regs.get(lk[2], [])

    def decode(self, addr: int, value: int | None = None) -> str:
        lk = self.lookup(addr)
        if not lk:
            return f"{addr:#010x}"
        name, path, rtype, mod = lk
        s = f"{name}.{path}"
        if value is not None and rtype:
            _, regs = self._parse(mod)
            fs = [f"{fn}={(value >> sh) & m:#x}" for fn, sh, m in regs.get(rtype, []) if (value >> sh) & m]
            if fs:
                s += " {" + " ".join(fs) + "}"
        return s

    def pad_function(self, pad: str, alt: int) -> str | None:
        return self.pad_funcs.get((pad, alt))

    def irq_name(self, n: int) -> str:
        return self.irqs.get(n, f"IRQ{n}")


@functools.lru_cache(1)
def default() -> RegNames:
    return RegNames()


if __name__ == "__main__":
    import sys
    rn = default()
    print("metapac:", rn.src)
    for a in sys.argv[1:]:
        addr = int(a, 0)
        print(f"{addr:#010x}", rn.decode(addr), rn.fields(addr))
