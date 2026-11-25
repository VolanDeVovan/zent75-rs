"""The config memory map exists three times: tools/rammap.py, the panel
(panel/src/protocol/zent/memory.ts) and the firmware (firmware/core/src/config.rs). They are
kept by hand, so this pins them to each other."""
import os
import re

import rammap

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _read(*parts: str) -> str:
    with open(os.path.join(ROOT, *parts), encoding="utf-8") as f:
        return f.read()


def _rust_offsets(module: str) -> dict[str, int]:
    """`pub const NAME: usize = 0x…;` of `pub mod <module> { … }` in config.rs, lower-cased."""
    src = _read("firmware", "core", "src", "config.rs")
    body = re.search(rf"^pub mod {module} \{{\n(.*?)^\}}", src, re.S | re.M).group(1)
    return {m[1].lower(): int(m[2], 16)
            for m in re.finditer(r"pub const (\w+): usize = (0x[0-9A-Fa-f_]+);", body)}


def _rust_const(name: str) -> int:
    src = _read("firmware", "core", "src", "config.rs")
    return int(re.search(rf"pub const {name}: \w+ = (0x[0-9A-Fa-f_]+);", src).group(1).replace("_", ""), 16)


def test_firmware_offsets_match_rammap():
    bank1, bank2 = _rust_const("BANK1_BASE"), _rust_const("BANK2_BASE")
    b1, b2 = _rust_offsets("b1"), _rust_offsets("b2")
    assert bank1 == 0x80070000 and bank2 == 0x80080000
    seen = 0
    for name, (addr, length) in rammap.REGIONS.items():
        base, offsets, size = (bank1, b1, _rust_const("BANK1_SIZE")) if rammap.is_global(name) \
            else (bank2, b2, _rust_const("BANK2_SIZE"))
        assert addr + length <= base + size, f"{name} ends outside the firmware's bank"
        m = re.fullmatch(r"(config_name)(\d)|(bind_key_setting)_(\d+)", name)
        if m and m[1]:
            want = offsets["config_name"] + rammap.PROFILE_NAME_SIZE * (int(m[2]) - 1)
        elif m:
            want = offsets["bind_key_setting"] + rammap.BIND_RECORD_SIZE * int(m[4])
        else:
            want = offsets[name]
        assert addr - base == want, f"{name}: rammap {addr:#x}, firmware {base + want:#x}"
        seen += 1
    assert seen == len(rammap.REGIONS)


def test_panel_regions_match_rammap():
    src = _read("panel", "src", "protocol", "zent", "memory.ts")
    regions = {m[1]: int(m[2], 16)
               for m in re.finditer(r'(?:whole|region)\("(\w+)", (0x[0-9a-fA-F]+),', src)}
    assert len(regions) == 32
    first = {"config_name": "config_name1", "bind_key_setting": "bind_key_setting_00"}
    for name, addr in regions.items():
        assert rammap.REGIONS[first.get(name, name)][0] == addr, name
    # nothing the tools know is missing from the panel
    panel_names = {first.get(n, n) for n in regions}
    for name in rammap.REGIONS:
        if not re.fullmatch(r"config_name[234]|bind_key_setting_(?!00)\d+", name):
            assert name in panel_names, name
