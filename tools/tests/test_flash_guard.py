"""flash_uf2.check_uf2: only application-region images for this board may be flashed."""
import os
import struct

import pytest

import flash_uf2 as fu

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def block(addr, size=256, family=fu.FAMILY_ID, flags=fu.UF2_FLAG_FAMILY, no=0, total=1, magic_end=fu.UF2_MAGIC_END):
    hdr = struct.pack("<8I", fu.UF2_MAGIC0, fu.UF2_MAGIC1, flags, addr, size, no, total, family)
    return hdr + b"\0" * 476 + struct.pack("<I", magic_end)


def test_vendor_images_pass():
    for name in ("0.0.18.uf2", "0.0.9.uf2"):
        with open(os.path.join(ROOT, "firmware", "vendor", name), "rb") as f:
            assert fu.check_uf2(f.read()) > 0


def test_app_region_edges_pass():
    assert fu.check_uf2(block(0x80040000) + block(0x80070000 - 256)) == 2


@pytest.mark.parametrize("addr", [0x80000000, 0x80001000, 0x80003000, 0x80020000, 0x8003FF00, 0x8003FFF0])
def test_protected_region_refused(addr):
    with pytest.raises(fu.UnsafeImage, match="protected"):
        fu.check_uf2(block(fu.APP_REGION[0]) + block(addr))


def test_outside_app_region_refused():
    with pytest.raises(fu.UnsafeImage, match="outside"):
        fu.check_uf2(block(0x80070000))
    with pytest.raises(fu.UnsafeImage, match="outside"):
        fu.check_uf2(block(0x80070000 - 128))


@pytest.mark.parametrize("bad", [
    dict(family=0xE48BFF56), dict(flags=0), dict(magic_end=0), dict(size=0), dict(size=477),
])
def test_malformed_refused(bad):
    with pytest.raises(fu.UnsafeImage):
        fu.check_uf2(block(0x80040000, **bad))


def test_bad_size_refused():
    with pytest.raises(fu.UnsafeImage):
        fu.check_uf2(b"")
    with pytest.raises(fu.UnsafeImage):
        fu.check_uf2(block(0x80040000)[:500])


def test_flash_refuses_before_touching_board(tmp_path, monkeypatch):
    p = tmp_path / "evil.uf2"
    p.write_bytes(block(0x80020000))
    monkeypatch.setattr(fu, "enter_bootloader", lambda: pytest.fail("must not touch the board"))
    assert fu.flash(str(p)) == 7
