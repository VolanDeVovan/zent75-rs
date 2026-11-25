"""End to end against the host simulator (firmware/sim) over a real /dev/uhid + hidraw node.

Skipped unless /dev/uhid is writable and the simulator builds. The simulator enumerates as
36B0:5A11 "ZENT75 SIM"; every access here goes to that node only (the real board, 36B0:357E,
is never opened). Its hidraw node is created root-only 0600 by udev; when it is not accessible
the test makes it so with `sudo -n chmod 0666` if passwordless sudo works, and skips otherwise.
"""
from __future__ import annotations

import os
import queue
import re
import shutil
import socket
import subprocess
import sys
import threading
import time

import pytest

import rammap
import zentlib as zl

TOOLS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ROOT = os.path.dirname(TOOLS)
SIM_DIR = os.path.join(ROOT, "firmware", "sim")
SIM_BIN = os.path.join(SIM_DIR, "target", "debug", "zent-sim")
KEY_A = rammap.KEY_NAMES.index("A")


def _build() -> str | None:
    if not os.access("/dev/uhid", os.W_OK):
        return "/dev/uhid is not writable"
    if not shutil.which("cargo"):
        return "cargo not found"
    # cwd matters: rustup picks the toolchain (firmware/rust-toolchain.toml) from it
    r = subprocess.run(["cargo", "build", "--quiet"], cwd=SIM_DIR, capture_output=True, text=True, timeout=900)
    if r.returncode != 0:
        return "simulator does not build:\n" + r.stderr[-2000:]
    return None


def _make_accessible(node: str) -> None:
    if os.access(node, os.R_OK | os.W_OK):
        return
    if subprocess.run(["sudo", "-n", "true"], capture_output=True).returncode != 0:
        pytest.skip(f"{node} is root-only and passwordless sudo is unavailable")
    subprocess.run(["sudo", "-n", "chmod", "0666", node], check=True)
    assert os.access(node, os.R_OK | os.W_OK)


class Sim:
    def __init__(self, tmp) -> None:
        base = os.environ.get("XDG_RUNTIME_DIR") or "/tmp"
        self.sock_path = os.path.join(base, f"zent-e2e-{os.getpid()}.sock")  # sun_path is short
        self.state_dir = tmp / "state"
        self.proc = subprocess.Popen([SIM_BIN, "--state-dir", str(self.state_dir), "--socket", self.sock_path],
                                     stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        self.lines: queue.Queue[str] = queue.Queue()
        self.log: list[str] = []
        threading.Thread(target=self._pump, daemon=True).start()
        self.node = self._wait_line(r"zent-sim: hidraw (/dev/hidraw\d+)", 10)
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.connect(self.sock_path)
        self.sock.settimeout(10)
        self.rf = self.sock.makefile("r")

    def _pump(self) -> None:
        for line in self.proc.stdout:
            self.lines.put(line.rstrip("\n"))

    def _wait_line(self, pattern: str, timeout: float) -> str:
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            try:
                line = self.lines.get(timeout=0.1)
            except queue.Empty:
                continue
            self.log.append(line)
            m = re.search(pattern, line)
            if m:
                return m.group(1)
        raise AssertionError(f"no line matching {pattern!r}; log:\n" + "\n".join(self.log))

    def cmd(self, line: str) -> list[str]:
        self.sock.sendall((line + "\n").encode())
        body = []
        while True:
            r = self.rf.readline().rstrip("\n")
            if r == "ok":
                return body
            assert not r.startswith("err"), f"{line}: {r}"
            assert r, "control socket closed"
            body.append(r)

    def state(self) -> dict[str, str]:
        return dict(l.split(" ", 1) for l in self.cmd("state") if not l.startswith("key "))

    def close(self) -> None:
        try:
            self.cmd("quit")
        except Exception:
            pass
        try:
            self.proc.wait(5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()


@pytest.fixture(scope="module")
def sim(tmp_path_factory):
    why = _build()
    if why:
        pytest.skip(why)
    s = Sim(tmp_path_factory.mktemp("sim"))
    try:
        _make_accessible(s.node)
        yield s
    finally:
        s.close()


@pytest.fixture
def sim_env(monkeypatch):
    monkeypatch.setenv(zl.ENV_SIM, "1")
    monkeypatch.delenv(zl.ENV_HIDRAW, raising=False)


def _stream_ok(r: bytes) -> bool:
    n = zl.STREAM_LEN_TRAVEL
    return (r[:4] == bytes([0xBE, 0xFD, n & 0xFF, n >> 8]) and r[n - 1] == 0xED
            and zl.crc16(r[:n - 3]) == (r[n - 3] | r[n - 2] << 8))


def test_discovery_is_opt_in(sim, monkeypatch):
    monkeypatch.delenv(zl.ENV_SIM, raising=False)
    monkeypatch.delenv(zl.ENV_HIDRAW, raising=False)
    assert zl.find_hidraw() != sim.node  # default: real-board path only
    monkeypatch.setenv(zl.ENV_SIM, "1")
    assert zl.find_hidraw() == sim.node
    assert zl.wait_device(5) == sim.node
    monkeypatch.delenv(zl.ENV_SIM)
    monkeypatch.setenv(zl.ENV_HIDRAW, sim.node)
    assert zl.find_hidraw() == sim.node


def test_protocol_round_trips(sim, sim_env):
    with zl.Zent() as z:
        assert z.dev == sim.node
        assert z.version() == "1.0.18"
        assert z.serial().startswith(b"\xBE\x1D\x15\x00ZENT75-SIM")
        assert z.profile() == 0

        # key_map entry for A: write (with read-back verify), then save
        addr = rammap.KEY_MAP + 2 * KEY_A
        orig = z.read_ram(addr, 2)
        z.write_ram(addr, zl.pack_u16([5]))
        assert z.read_ram(addr, 2) == zl.pack_u16([5])
        z.save(settle=0.2)
        assert os.path.getsize(sim.state_dir / "profile0.bin") > 0

        # profile switch: the board pushes BE C0 06 00 <n> ED
        z.drain()
        z.simple(zl.CMD_SWITCH_PROFILE, 1)
        assert z.wait(zl.decode_profile_notice, 2.0) == 1
        assert z.profile() == 1
        assert sim.state()["profile"] == "1"
        assert z.read_ram(addr, 2) == orig  # profile 1 untouched
        z.switch_profile(0, settle=0.1)
        assert z.read_ram(addr, 2) == zl.pack_u16([5])  # saved edit survived the switch


def test_stream_and_key_press(sim, sim_env):
    end = time.monotonic() + 10
    while sim.state()["engine_ready"] != "1":
        assert time.monotonic() < end, "engine never became ready"
        time.sleep(0.1)
    with zl.Zent() as z:
        z.heartbeat()
        frames = []
        end = time.monotonic() + 2
        while len(frames) < 5 and time.monotonic() < end:
            r = z.recv(0.2)
            if r and r[:2] == b"\xBE\xFD":
                assert len(r) == zl.REPORT_SIZE
                frames.append(r)
        assert len(frames) >= 5, "no BE FD stream after heartbeat"
        assert all(_stream_ok(f) for f in frames)
        f = zl.decode_stream(frames[-1])
        assert f.travel[KEY_A] == 0 and 25000 < f.raw[KEY_A] < 35000

        sim.cmd("press A")
        seen = 0
        end = time.monotonic() + 3
        while time.monotonic() < end and not seen:
            z.heartbeat()
            r = z.recv(0.2)
            f = zl.decode_stream(r) if r else None
            if f and f.travel[KEY_A] > 0:
                seen = f.travel[KEY_A]
        assert seen > 0, "pressed key never showed travel in the stream"
        # key_map[A] was set to 5 (HID usage B) by the previous test
        hid = sim.state()["hid"]
        assert "keys=05" in hid, hid
        sim.cmd("release A")


def test_dump_all_and_restore_against_the_sim(sim, tmp_path):
    env = {k: v for k, v in os.environ.items() if k not in (zl.ENV_SIM, zl.ENV_HIDRAW)}
    env[zl.ENV_HIDRAW] = sim.node
    before = sorted(os.listdir(os.path.join(ROOT, "backups")))
    out = tmp_path / "backups"
    r = subprocess.run([sys.executable, "dump_all.py", "--out", str(out)], cwd=TOOLS, env=env,
                       capture_output=True, text=True, timeout=300)
    assert r.returncode == 0, r.stdout + r.stderr
    (dump,) = out.iterdir()
    assert (dump / "dump.json").exists() and (dump / "p3_key_map.bin").exists()
    assert (dump / "p0_key_map.bin").read_bytes()[2 * KEY_A:2 * KEY_A + 2] == zl.pack_u16([5])

    r = subprocess.run([sys.executable, "restore.py", str(dump)], cwd=TOOLS, env=env,
                       capture_output=True, text=True, timeout=600)
    assert r.returncode == 0, r.stdout + r.stderr
    assert r.stdout.count("OK (byte-identical)") == 4 and r.stdout.rstrip().endswith("OK")
    assert sorted(os.listdir(os.path.join(ROOT, "backups"))) == before  # repo backups/ untouched
    assert sim.state()["profile"] == "0"
