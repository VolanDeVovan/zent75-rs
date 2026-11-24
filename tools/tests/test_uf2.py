"""uf2.py on synthetic images (no firmware files, no hardware)."""
import struct

import pytest

import uf2


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


def test_pack_round_trips_through_parse():
    data = bytes(range(256)) * 2 + b"tail"  # two full blocks and a short one
    image = uf2.pack(data)
    blocks = uf2.parse_bytes(image)
    assert len(image) == 3 * uf2.BLOCK_SIZE
    assert [(b.number, b.total, b.size) for b in blocks] == [(0, 3, 256), (1, 3, 256), (2, 3, 4)]
    assert {(b.flags, b.family) for b in blocks} == {(uf2.FLAG_FAMILY_ID, 0x0B4D5475)}
    assert uf2.ranges(blocks) == [(0x80040000, bytearray(data))]
    assert uf2.ranges(uf2.parse_bytes(uf2.pack(b"x", base=0x1000, family=7))) == [(0x1000, bytearray(b"x"))]
    with pytest.raises(ValueError):
        uf2.pack(b"")


def test_cli_pack_then_unpack(tmp_path, monkeypatch, capsys):
    src, image = tmp_path / "app.bin", tmp_path / "app.uf2"
    src.write_bytes(bytes(700))
    monkeypatch.setattr("sys.argv", ["uf2.py", "pack", str(src), "-o", str(image)])
    uf2.main()
    monkeypatch.setattr("sys.argv", ["uf2.py", str(image), str(tmp_path / "out")])
    uf2.main()
    assert (tmp_path / "out_80040000.bin").read_bytes() == bytes(700)
    assert "range 0x80040000..0x800402bc (700 bytes)" in capsys.readouterr().out
