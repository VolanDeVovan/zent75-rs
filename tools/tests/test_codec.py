"""Pure codec: CRC vectors, frame layouts, decoders. No I/O."""
import pytest

import rammap
import zentlib as zl


@pytest.mark.parametrize("data, crc", [
    (b"123456789", 0x4B37),  # the CRC-16/MODBUS check value
    (b"", 0xFFFF),
    (b"\x00", 0x40BF),
    (bytes.fromhex("010300000001"), 0x0A84),  # Modbus "read 1 holding register", wire bytes 84 0A
])
def test_crc16_vectors(data, crc):
    assert zl.crc16(data) == crc


def test_simple_frames():
    assert zl.encode_simple(zl.CMD_HEARTBEAT) == bytes.fromhex("be000500ed")
    assert zl.encode_simple(zl.CMD_SAVE) == bytes.fromhex("be020500ed")
    assert zl.encode_simple(zl.CMD_SERIAL) == bytes.fromhex("be1d0500ed")
    assert zl.encode_simple(zl.CMD_SWITCH_PROFILE, 3) == bytes.fromhex("bec0060003ed")
    assert zl.encode_simple(zl.CMD_CALIBRATION, 1) == bytes.fromhex("befe060001ed")
    assert zl.WRITE_ACK == bytes.fromhex("be030500ed")


def test_read_request_layout():
    # BE 04 0B 00 <addr LE32> <len BE16> ED
    assert zl.encode_read(zl.CMD_READ_RAM, 0x80070210, 4) == bytes.fromhex("be040b0010020780" "0004" "ed")
    assert zl.encode_read(zl.CMD_READ_FLASH, 0x80080B00, 0x1FF) == bytes.fromhex("be010b00000b0880" "01ff" "ed")
    for bad in (0, -1, zl.MAX_DATA_PER_FRAME + 1):
        with pytest.raises(ValueError):
            zl.encode_read(zl.CMD_READ_RAM, 0x80070000, bad)
    with pytest.raises(ValueError):
        zl.encode_read(zl.CMD_READ_RAM, 1 << 32, 4)


def test_write_frame_layout():
    # BE 03 <13+n LE16> <addr LE32> <n BE16> <data> <crc LE16> ED
    data = bytes([0x11, 0x22, 0x33])
    f = zl.encode_write(0x80080160, data)
    assert len(f) == 13 + 3
    assert f[:4] == bytes([0xBE, 0x03, 16, 0])
    assert f[4:8] == bytes.fromhex("60010880")
    assert f[8:10] == bytes([0, 3])  # big-endian byte count
    assert f[10:13] == data
    assert int.from_bytes(f[13:15], "little") == zl.crc16(f[:13])  # CRC over everything before it
    assert f[15] == 0xED


def test_write_frame_length_field_is_le16_and_count_is_be16():
    data = bytes(range(256)) + bytes(44)  # 300 bytes: both fields need their high byte
    f = zl.encode_write(0x80080D00, data)
    assert f[2:4] == (313).to_bytes(2, "little")
    assert f[8:10] == (300).to_bytes(2, "big")
    assert f[-1] == 0xED and len(f) == 313


def test_write_frame_limits():
    assert len(zl.encode_write(0x80080000, bytes(zl.MAX_DATA_PER_FRAME))) == zl.REPORT_SIZE
    with pytest.raises(ValueError):
        zl.encode_write(0x80080000, bytes(zl.MAX_DATA_PER_FRAME + 1))
    with pytest.raises(ValueError):
        zl.encode_write(0x80080000, b"")


def test_to_report_adds_report_id_and_padding():
    r = zl.to_report(zl.encode_simple(zl.CMD_HEARTBEAT))
    assert len(r) == 1025 and r[0] == 0 and r[1:6] == bytes.fromhex("be000500ed") and not any(r[6:])
    with pytest.raises(ValueError):
        zl.to_report(bytes(1025))


@pytest.mark.parametrize("n", [1, 2, 83, 166, 255, 256, 1011])
def test_data_frame_round_trip(n):
    data = bytes((7 * i + n) & 0xFF for i in range(n))
    frame = zl.encode_data_frame(zl.CMD_READ_RAM, 0x80080B00, data)
    padded = frame.ljust(zl.REPORT_SIZE, b"\x00")
    f = zl.decode_data_frame(padded)
    assert f == zl.DataFrame(zl.CMD_READ_RAM, 0x80080B00, n, data)
    # vendor-driver slicing: data at [10:L-3], crc at [L-3:L-1]
    L = padded[2] | padded[3] << 8
    assert padded[10:L - 3] == data
    assert int.from_bytes(padded[L - 3:L - 1], "little") == zl.crc16(padded[:L - 3])
    assert zl.match_read_reply(padded, zl.CMD_READ_RAM, 0x80080B00, n) == data
    assert zl.match_read_reply(b"\x00" + padded, zl.CMD_READ_RAM, 0x80080B00, n) == data  # 1025-byte form


def test_read_reply_rejections():
    data = bytes(range(16))
    good = zl.encode_data_frame(zl.CMD_READ_RAM, 0x80070000, data).ljust(1024, b"\x00")
    ok = lambda r, cmd=zl.CMD_READ_RAM, addr=0x80070000, n=16: zl.match_read_reply(r, cmd, addr, n)
    assert ok(good) == data
    assert ok(good, cmd=zl.CMD_READ_FLASH) is None  # other command
    assert ok(good, addr=0x80070010) is None  # stale reply for another address
    assert ok(good, n=15) is None and ok(good, n=17) is None  # wrong amount of data
    for i in (4, 9, 10, 25, 26, 27):  # addr, count, data, data, crc lo, crc hi
        bad = bytearray(good)
        bad[i] ^= 0x01
        assert ok(bytes(bad)) is None, i
    bad = bytearray(good)
    bad[28] = 0x00  # ED missing
    assert ok(bytes(bad)) is None
    bad = bytearray(good)
    bad[2:4] = (2000).to_bytes(2, "little")  # length beyond the report
    assert ok(bytes(bad)) is None
    assert ok(b"") is None and ok(bytes(1024)) is None and ok(good[:20]) is None


def test_write_ack_and_serial_and_profile_notice():
    assert zl.is_write_ack(zl.WRITE_ACK.ljust(1024, b"\x00"))
    assert not zl.is_write_ack(bytes.fromhex("be040500ed").ljust(1024, b"\x00"))
    assert not zl.is_write_ack(b"")
    serial = bytes([0xBE, 0x1D, 0x15, 0x00]) + bytes(range(16)) + b"\xED"
    assert zl.decode_serial(serial.ljust(1024, b"\x00")) == serial
    assert zl.decode_serial(serial[:-1].ljust(1024, b"\x00")) is None
    assert zl.decode_serial(zl.WRITE_ACK.ljust(1024, b"\x00")) is None
    assert zl.decode_profile_notice(bytes.fromhex("bec0060002ed").ljust(1024, b"\x00")) == 2
    assert zl.decode_profile_notice(zl.WRITE_ACK.ljust(1024, b"\x00")) is None


def _stream(length: int, words: list[int]) -> bytes:
    body = bytes([0xBE, 0xFD]) + length.to_bytes(2, "little") + zl.pack_u16(words)
    return body.ljust(1024, b"\x00")


def test_stream_travel_frame():
    raw = [30000 - k for k in range(83)]
    travel = [10 * k for k in range(83)]
    f = zl.decode_stream(_stream(zl.STREAM_LEN_TRAVEL, raw + travel))
    assert f.length == 1017 and f.raw == raw and f.travel == travel


def test_stream_raw6_frame_is_floored_mean():
    scans = [[20000 + 100 * j + k for k in range(83)] for j in range(6)]
    f = zl.decode_stream(_stream(zl.STREAM_LEN_RAW6, [w for s in scans for w in s]))
    assert f.length == 1003 and f.travel is None
    assert f.raw == [sum(s[k] for s in scans) // 6 for k in range(83)]


def test_stream_rejects_other_frames():
    assert zl.decode_stream(_stream(999, [0] * 166)) is None
    assert zl.decode_stream(zl.WRITE_ACK.ljust(1024, b"\x00")) is None
    assert zl.decode_stream(b"") is None


def test_table_helpers():
    assert zl.unpack_u16(bytes.fromhex("18790068")) == [31000, 0x6800]
    assert zl.unpack_i16(bytes.fromhex("f8ffe803")) == [-8, 1000]
    assert zl.pack_u16([31000, 17000]) == bytes.fromhex("18796842")
    assert zl.unpack_u16(zl.pack_u16(list(range(0, 65536, 257)))) == list(range(0, 65536, 257))
    with pytest.raises(ValueError):
        zl.unpack_u16(b"\x00")
    with pytest.raises(OverflowError):
        zl.pack_u16([65536])
    assert zl.format_version(bytes([18, 0, 0, 0])) == "0.0.18"


def test_rammap():
    assert len(rammap.REGIONS) == 29 + 83
    assert rammap.REGIONS["config_name4"] == (0x80070350, 64)
    assert rammap.REGIONS["bind_key_setting_82"] == (0x80080D00 + 48 * 82, 48)
    assert rammap.in_map(0x80070000, 166) and not rammap.in_map(0x80070000, 167)
    assert rammap.in_map(0x80080D00, 48 * 83)  # adjacent records are one span
    assert not rammap.in_map(0x80080D00, 48 * 83 + 1)
    assert not rammap.in_map(0x80000000, 4) and not rammap.in_map(0x80070000, 0)
    assert rammap.overlaps_read_only(0x80070210, 1) and rammap.overlaps_read_only(0x8007021B, 1)
    assert not rammap.overlaps_read_only(0x80070290, 64)
    assert rammap.is_advanced("bind_key_mode") and not rammap.is_advanced("key_mode")
    assert rammap.is_global("top_value") and not rammap.is_global("key_map")
    # no two regions overlap
    spans = sorted((a, a + n) for a, n in rammap.REGIONS.values())
    assert all(a[1] <= b[0] for a, b in zip(spans, spans[1:]))
