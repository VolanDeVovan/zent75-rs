"""Simulator opt-in (ZENT_SIM / ZENT_HIDRAW) against a fake sysfs tree; no device I/O."""
import os
import subprocess
import sys

import pytest

import zentlib as zl

TOOLS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
VENDOR_RD = bytes([0x06, 0x60, 0xFF, 0x09, 0x61, 0xA1, 0x01])
KBD_RD = bytes([0x05, 0x01, 0x09, 0x06, 0xA1, 0x01])


def add_node(root, n, hid_id, parent, rd=VENDOR_RD):
    """sysfs/hidrawN/device -> devices/<parent>/<hid dir>, like the kernel lays it out."""
    dev = root / "devices" / parent / f"{hid_id}.{n:04X}"
    dev.mkdir(parents=True)
    (dev / "uevent").write_text(f"DRIVER=hid-generic\nHID_ID=0003:{hid_id.split(':', 1)[1]}\n")
    (dev / "report_descriptor").write_bytes(rd)
    node = root / "class" / f"hidraw{n}"
    node.mkdir(parents=True)
    (node / "device").symlink_to(dev)


@pytest.fixture
def sysfs(tmp_path, monkeypatch):
    monkeypatch.delenv(zl.ENV_SIM, raising=False)
    monkeypatch.delenv(zl.ENV_HIDRAW, raising=False)
    add_node(tmp_path, 2, "0003:000036B0:0000357E", "usb1/1-2/1-2:1.0")  # real board if00
    add_node(tmp_path, 8, "0003:000036B0:0000357E", "usb1/1-2/1-2:1.2")  # real board if02
    add_node(tmp_path, 9, "0003:000036B0:00005A11", "virtual/misc/uhid")  # sim vendor
    add_node(tmp_path, 10, "0003:000036B0:00005A11", "virtual/misc/uhid", KBD_RD)  # sim keyboard
    monkeypatch.setattr(zl, "SYSFS_HIDRAW", str(tmp_path / "class"))
    return tmp_path


def test_default_finds_only_the_real_board(sysfs):
    assert not zl.sim_selected()
    assert zl.find_hidraw() == "/dev/hidraw8"


def test_zent_sim_finds_only_the_sim_vendor_node(sysfs, monkeypatch):
    monkeypatch.setenv(zl.ENV_SIM, "1")
    assert zl.sim_selected()
    assert zl.find_hidraw() == "/dev/hidraw9"


def test_zent_sim_zero_is_off(sysfs, monkeypatch):
    monkeypatch.setenv(zl.ENV_SIM, "0")
    assert not zl.sim_selected()
    assert zl.find_hidraw() == "/dev/hidraw8"


def test_zent_sim_without_sim_finds_nothing(tmp_path, monkeypatch):
    add_node(tmp_path, 8, "0003:000036B0:0000357E", "usb1/1-2/1-2:1.2")
    monkeypatch.setattr(zl, "SYSFS_HIDRAW", str(tmp_path / "class"))
    monkeypatch.setenv(zl.ENV_SIM, "1")
    assert zl.find_hidraw() is None  # never falls back to the real board


def test_zent_hidraw_must_be_the_sim(sysfs, monkeypatch):
    monkeypatch.setenv(zl.ENV_HIDRAW, "/dev/hidraw9")
    assert zl.find_hidraw() == "/dev/hidraw9"
    monkeypatch.setenv(zl.ENV_HIDRAW, "/dev/hidraw8")
    with pytest.raises(zl.SimTargetError):
        zl.find_hidraw()
    monkeypatch.setenv(zl.ENV_HIDRAW, "/dev/hidraw10")  # sim, but the keyboard device
    with pytest.raises(zl.SimTargetError):
        zl.find_hidraw()
    monkeypatch.setenv(zl.ENV_HIDRAW, "/dev/hidraw42")
    assert zl.find_hidraw() is None


def _run(args, **env):
    e = {k: v for k, v in os.environ.items() if k not in (zl.ENV_SIM, zl.ENV_HIDRAW)}
    e.update(env)
    return subprocess.run(args, cwd=TOOLS, env=e, capture_output=True, text=True, timeout=30)


def test_flash_refuses_in_sim_mode(tmp_path):
    img = tmp_path / "x.uf2"
    img.write_bytes(b"")  # empty: even without the guard check_uf2 refuses it (rc 7) before any I/O
    for env in ({zl.ENV_SIM: "1"}, {zl.ENV_HIDRAW: "/dev/hidraw9"}):
        r = _run([sys.executable, "flash_uf2.py", str(img)], **env)
        assert r.returncode == 8, r.stdout + r.stderr
        assert "REFUSED" in r.stdout


def test_rollback_refuses_in_sim_mode(tmp_path):
    # Nonexistent image and backup: even without the guard this could not reach the board.
    r = _run(["bash", "rollback.sh", str(tmp_path / "none.uf2"), str(tmp_path / "none")], **{zl.ENV_SIM: "1"})
    assert r.returncode == 8 and "REFUSED" in r.stderr


def test_dump_all_requires_out_in_sim_mode():
    r = _run([sys.executable, "dump_all.py"], **{zl.ENV_SIM: "1"})
    assert r.returncode == 2 and "--out" in r.stderr
