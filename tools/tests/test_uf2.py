"""re/uf2.py on synthetic images (no firmware files, no hardware)."""
import importlib.util
import os
import struct

import pytest

_spec = importlib.util.spec_from_file_location(
    "uf2", os.path.join(os.path.dirname(__file__), "..", "..", "re", "uf2.py"))
uf2 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(uf2)


def block(addr, data, number, total, flags=0x2000, family=0x0B4D5475):
    head = struct.pack("<8I", 0x0A324655, 0x9E5D5157, flags, addr, len(data), number, total, family)
    return head + data.ljust(476, b"\x00") + struct.pack("<I", 0x0AB16F30)


def test_parse_and_join_ranges(tmp_path):
    a, b, c = bytes([1]) * 256, bytes([2]) * 256, bytes([3]) * 100
    p = tmp_path / "x.uf2"
    # out of order on purpose, a gap before the third block, one block not meant for main flash
    p.write_bytes(block(0x80040100, b, 1, 4) + block(0x80040000, a, 0, 4)
                  + block(0x80050000, c, 2, 4) + block(0x80040200, b"\xFF" * 256, 3, 4, flags=0x2001))
    blocks = uf2.parse(str(p))
    assert [x.number for x in blocks] == [1, 0, 2, 3] and blocks[2].data == c
    assert {x.family for x in blocks} == {0x0B4D5475}
    assert uf2.ranges(blocks) == [(0x80040000, bytearray(a + b)), (0x80050000, bytearray(c))]


def test_malformed_files_are_rejected(tmp_path):
    p = tmp_path / "x.uf2"
    good = block(0x80040000, bytes(256), 0, 1)
    for bad in (b"", good[:-1], b"\x00" + good[1:], good[:-4] + bytes(4),
                good[:16] + struct.pack("<I", 477) + good[20:]):
        p.write_bytes(bad)
        with pytest.raises(ValueError):
            uf2.parse(str(p))
    p.write_bytes(block(0x80040000, bytes(256), 0, 2) + block(0x80040080, bytes(256), 1, 2))
    with pytest.raises(ValueError, match="overlaps"):
        uf2.ranges(uf2.parse(str(p)))
