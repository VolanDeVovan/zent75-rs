"""Firmware image loading: ELF (PT_LOAD at the load address, like the UF2 the bootloader
writes) or UF2 (via tools/uf2.py), symbols, and config-bank preload from a tools/dump_all.py
backup directory."""
from __future__ import annotations

import bisect
import json
import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
TOOLS = os.path.join(ROOT, "tools")

APP_BASE = 0x80040000
APP_END = 0x80070000
FLASH_BASE = 0x80000000
FLASH_SIZE = 0x100000
SIGNATURE = 0x0B4D5475
BANK1 = 0x80070000
PROFILE_BASE = 0x80080000
PROFILE_STRIDE = 0x2000


class Image:
    """Flash content (addr -> bytes chunks) plus optional ELF metadata."""

    def __init__(self) -> None:
        self.chunks: list[tuple[int, bytes]] = []  # flash (load) addresses
        self.symbols: list[tuple[int, int, str]] = []  # (addr, size, name), sorted
        self._addrs: list[int] = []
        self.sections: dict[str, tuple[int, int, int]] = {}  # name -> (vma, lma, size)
        self.exec_ranges: list[tuple[int, bytes]] = []  # (vma, bytes) of executable sections
        self.sym_by_name: dict[str, int] = {}
        self.path = ""

    # ---- loaders ---------------------------------------------------------------------------
    @classmethod
    def from_elf(cls, path: str) -> "Image":
        from elftools.elf.elffile import ELFFile

        img = cls()
        img.path = path
        with open(path, "rb") as fh:
            elf = ELFFile(fh)
            for seg in elf.iter_segments():
                if seg["p_type"] != "PT_LOAD" or seg["p_filesz"] == 0:
                    continue
                img.chunks.append((seg["p_paddr"], seg.data()[: seg["p_filesz"]]))
            for sec in elf.iter_sections():
                if not sec.name or sec["sh_addr"] == 0 and sec.name.startswith(".debug"):
                    continue
                img.sections[sec.name] = (sec["sh_addr"], sec["sh_addr"], sec["sh_size"])
                if sec["sh_flags"] & 0x4 and sec["sh_type"] == "SHT_PROGBITS" and sec["sh_size"]:
                    img.exec_ranges.append((sec["sh_addr"], sec.data()))
            symtab = elf.get_section_by_name(".symtab")
            raw = []
            if symtab is not None:
                for s in symtab.iter_symbols():
                    if s.name and s["st_info"]["type"] in ("STT_FUNC", "STT_OBJECT", "STT_NOTYPE") \
                            and s["st_shndx"] != "SHN_UNDEF":
                        img.sym_by_name.setdefault(s.name, s["st_value"])
                        # absolute symbols are linker constants (`_hart_stack_size = 2K`), not
                        # addresses: they must not name the code that happens to live there
                        if s["st_shndx"] != "SHN_ABS":
                            raw.append((s["st_value"], s["st_size"], s.name))
        names = demangle([n for _, _, n in raw])
        img.symbols = sorted((a, sz, names.get(n, n)) for a, sz, n in raw
                             if not n.startswith((".L", "$")))
        img._addrs = [a for a, _, _ in img.symbols]
        return img

    @classmethod
    def from_uf2(cls, path: str) -> "Image":
        sys.path.insert(0, TOOLS)
        import uf2  # tools/uf2.py

        img = cls()
        img.path = path
        blocks = uf2.parse_bytes(open(path, "rb").read(), path)
        for b in blocks:
            if b.flags & uf2.FLAG_NOT_MAIN_FLASH:
                continue
            img.chunks.append((b.addr, bytes(b.data)))
        return img

    @classmethod
    def load(cls, path: str) -> "Image":
        with open(path, "rb") as fh:
            head = fh.read(4)
        return cls.from_elf(path) if head == b"\x7fELF" else cls.from_uf2(path)

    # ---- queries ---------------------------------------------------------------------------
    def word(self, addr: int) -> int | None:
        for a, d in self.chunks:
            if a <= addr and addr + 4 <= a + len(d):
                return int.from_bytes(d[addr - a: addr - a + 4], "little")
        return None

    def sym(self, addr: int) -> str:
        i = bisect.bisect_right(self._addrs, addr) - 1
        if i < 0:
            return f"{addr:#010x}"
        a, size, name = self.symbols[i]
        if addr - a > max(size, 0x4000):
            return f"{addr:#010x}"
        return f"{name}+{addr - a:#x}" if addr != a else name

    def find(self, *needles: str) -> list[tuple[int, str]]:
        """Symbols whose (demangled) name contains any needle."""
        out = []
        for a, _, n in self.symbols:
            if any(nd in n for nd in needles):
                out.append((a, n))
        return out

    def section(self, name: str):
        return self.sections.get(name)


def demangle(names: list[str]) -> dict[str, str]:
    """Rust v0 / legacy demangling through c++filt when available (binutils understands v0)."""
    tool = shutil.which("c++filt") or shutil.which("llvm-cxxfilt")
    mangled = [n for n in names if n.startswith(("_R", "_ZN"))]
    if not tool or not mangled:
        return {}
    try:
        out = subprocess.run([tool], input="\n".join(mangled), capture_output=True, text=True,
                             timeout=30).stdout.splitlines()
    except (OSError, subprocess.SubprocessError):
        return {}
    import re
    res = {}
    for m, d in zip(mangled, out):
        res[m] = re.sub(r"\[[0-9a-f]{16}\]", "", d)
    return res


#: Factory images of the stock firmware (what it writes on blank NOR); the core's config tests
#: compare against the same files.
FACTORY_DIR = os.path.join(ROOT, "firmware", "core", "tests", "fixtures", "config")


def config_banks_from_backup(path: str) -> list[tuple[int, bytes]]:
    """(flash address, bytes) writes reconstructing the config banks from a dump_all.py backup.

    The protocol addresses are flash-layout addresses: 0x8007xxxx is bank 1 (board-global, 1:1
    image), 0x8008xxxx is the active profile, stored for profile k at 0x80080000 + k*0x2000.

    A backup holds only the regions of tools/rammap.py. Every bank therefore starts from the
    stock factory image and the dumped regions are laid over it, which is what the bank looks
    like on the real board (stock initialises the other fields on first boot and never changes
    them). Leaving them erased would e.g. arm tap-hold with code 0xFFFF on every key."""
    d = json.load(open(os.path.join(path, "dump.json")))
    active = int(d.get("active_profile", 0))
    profiles = d.get("profiles", {})

    def factory(name: str) -> bytearray:
        with open(os.path.join(FACTORY_DIR, name), "rb") as f:
            return bytearray(f.read())

    bank1 = factory("bank1.bin")
    banks = {int(k): factory(f"profile{int(k)}.bin") for k in profiles}
    for key, regions in profiles.items():
        k = int(key)
        for r in regions.values():
            addr, data = int(r["addr"]), bytes.fromhex(r["data"])
            if BANK1 <= addr < PROFILE_BASE:
                bank, off = (bank1 if k == active else None), addr - BANK1
            elif PROFILE_BASE <= addr < PROFILE_BASE + PROFILE_STRIDE:
                bank, off = banks[k], addr - PROFILE_BASE
            else:
                continue
            if bank is not None and off + len(data) <= len(bank):
                bank[off:off + len(data)] = data
    writes = [(BANK1, bytes(bank1))]
    writes += [(PROFILE_BASE + k * PROFILE_STRIDE, bytes(b)) for k, b in sorted(banks.items())]
    return writes
