import { describe, expect, it } from "vitest";
import { crc16Modbus } from "../src/protocol/crc";
import {
  Cmd, MAX_DATA_PER_FRAME, REPORT_ID, REPORT_SIZE, STREAM_LEN_RAW6, STREAM_LEN_TRAVEL,
  decodeDeviceFrame, decodeHostRequest, encodeBootloader, encodeCalibration, encodeHeartbeat, encodeProfileNotice,
  encodeReadFlash, encodeReadRam, encodeReadReply, encodeReboot, encodeResetConfig, encodeSave, encodeSerialReply,
  encodeSerialRequest, encodeStreamRaw6, encodeStreamTravel, encodeSwitchProfile, encodeWriteAck, encodeWriteRam,
  normalizeReport, padReport,
} from "../src/protocol/zent/frames";

/** first n bytes as a plain array */
const head = (r: Uint8Array, n: number) => Array.from(r.subarray(0, n));
const restIsZero = (r: Uint8Array, from: number) => r.subarray(from).every((b) => b === 0);

describe("framing", () => {
  it("uses report id 0 and 1024-byte reports", () => {
    expect(REPORT_ID).toBe(0);
    expect(REPORT_SIZE).toBe(1024);
    expect(MAX_DATA_PER_FRAME).toBe(1011);
  });

  it("pads every frame with zeros to 1024 bytes", () => {
    const p = padReport([0xbe, 0x00, 0x05, 0x00, 0xed]);
    expect(p.length).toBe(1024);
    expect(head(p, 5)).toEqual([0xbe, 0x00, 0x05, 0x00, 0xed]);
    expect(restIsZero(p, 5)).toBe(true);
  });

  it("rejects frames longer than a report", () => {
    expect(() => padReport(new Uint8Array(1025))).toThrow(RangeError);
  });

  it("every encoder returns a full report", () => {
    const all = [
      encodeHeartbeat(), encodeReadFlash(0x80070210, 4), encodeSave(), encodeWriteRam(0x80080160, [1, 2]),
      encodeReadRam(0x80070210, 4), encodeSerialRequest(), encodeSwitchProfile(2), encodeResetConfig(),
      encodeBootloader(), encodeCalibration(true), encodeReboot(), encodeWriteAck(), encodeProfileNotice(1),
      encodeSerialReply(new Uint8Array(16)), encodeReadReply(Cmd.ReadRam, 0x80070210, [1]),
    ];
    for (const r of all) expect(r.length).toBe(1024);
  });

  it("strips a leading report-id byte from 1025-byte hidraw reads", () => {
    const withId = new Uint8Array(1025);
    withId.set(encodeWriteAck(), 1);
    expect(normalizeReport(withId).length).toBe(1024);
    expect(decodeDeviceFrame(withId)).toEqual({ type: "writeAck" });
  });
});

describe("host -> device commands", () => {
  const simple: [string, () => Uint8Array, number][] = [
    ["heartbeat BE 00", encodeHeartbeat, 0x00],
    ["save BE 02", encodeSave, 0x02],
    ["serial BE 1D", encodeSerialRequest, 0x1d],
    ["reset config BE EE", encodeResetConfig, 0xee],
    ["bootloader BE FC", encodeBootloader, 0xfc],
    ["reboot BE FF", encodeReboot, 0xff],
  ];
  for (const [name, enc, cmd] of simple) {
    it(name, () => {
      const r = enc();
      expect(head(r, 5)).toEqual([0xbe, cmd, 0x05, 0x00, 0xed]);
      expect(restIsZero(r, 5)).toBe(true);
    });
  }

  it("read RAM BE 04: address LE32, length BE16", () => {
    const r = encodeReadRam(0x80080160, 166);
    expect(head(r, 11)).toEqual([0xbe, 0x04, 0x0b, 0x00, 0x60, 0x01, 0x08, 0x80, 0x00, 0xa6, 0xed]);
    expect(restIsZero(r, 11)).toBe(true);
    // length above 255 shows the byte order
    expect(head(encodeReadRam(0x80070290, 256), 11)).toEqual([0xbe, 0x04, 0x0b, 0x00, 0x90, 0x02, 0x07, 0x80, 0x01, 0x00, 0xed]);
  });

  it("read flash BE 01 has the same shape", () => {
    expect(head(encodeReadFlash(0x80070210, 4), 11)).toEqual([0xbe, 0x01, 0x0b, 0x00, 0x10, 0x02, 0x07, 0x80, 0x00, 0x04, 0xed]);
  });

  it("read length is limited to what fits a reply", () => {
    expect(() => encodeReadRam(0x80070000, 0)).toThrow(RangeError);
    expect(() => encodeReadRam(0x80070000, 1012)).toThrow(RangeError);
    expect(() => encodeReadRam(0x80070000, 1011)).not.toThrow();
    expect(() => encodeReadRam(-1, 4)).toThrow(RangeError);
  });

  it("write RAM BE 03: len = 13 + n, data, CRC LE, ED", () => {
    // what the vendor driver sends to map key 0 of the main layer to code 41 (Esc)
    const r = encodeWriteRam(0x80080160, [41, 0]);
    const crc = crc16Modbus([0xbe, 0x03, 0x0f, 0x00, 0x60, 0x01, 0x08, 0x80, 0x00, 0x02, 0x29, 0x00]);
    expect(head(r, 15)).toEqual([0xbe, 0x03, 0x0f, 0x00, 0x60, 0x01, 0x08, 0x80, 0x00, 0x02, 0x29, 0x00, crc & 0xff, crc >> 8, 0xed]);
    expect(restIsZero(r, 15)).toBe(true);
  });

  it("write RAM with a long block: LE16 frame length, BE16 data length", () => {
    const data = Uint8Array.from({ length: 300 }, (_, i) => i & 0xff);
    const r = encodeWriteRam(0x80080b00, data);
    expect(r[2] | (r[3] << 8)).toBe(313);
    expect([r[8], r[9]]).toEqual([0x01, 0x2c]);
    expect(Array.from(r.subarray(10, 310))).toEqual(Array.from(data));
    expect(r[310] | (r[311] << 8)).toBe(crc16Modbus(r, 310));
    expect(r[312]).toBe(0xed);
  });

  it("write RAM size limits", () => {
    expect(() => encodeWriteRam(0x80080160, [])).toThrow(RangeError);
    expect(() => encodeWriteRam(0x80080160, new Uint8Array(1012))).toThrow(RangeError);
    const max = encodeWriteRam(0x80080160, new Uint8Array(1011));
    expect(max[1023]).toBe(0xed);
  });

  it("switch profile BE C0", () => {
    for (let i = 0; i < 4; i++) expect(head(encodeSwitchProfile(i), 6)).toEqual([0xbe, 0xc0, 0x06, 0x00, i, 0xed]);
    expect(() => encodeSwitchProfile(4)).toThrow(RangeError);
    expect(() => encodeSwitchProfile(-1)).toThrow(RangeError);
  });

  it("calibration BE FE on / off", () => {
    expect(head(encodeCalibration(true), 6)).toEqual([0xbe, 0xfe, 0x06, 0x00, 0x01, 0xed]);
    expect(head(encodeCalibration(false), 6)).toEqual([0xbe, 0xfe, 0x06, 0x00, 0x00, 0xed]);
  });
});

describe("decoding host requests (device side)", () => {
  it("round-trips every command", () => {
    expect(decodeHostRequest(encodeHeartbeat())).toEqual({ type: "heartbeat" });
    expect(decodeHostRequest(encodeSave())).toEqual({ type: "save" });
    expect(decodeHostRequest(encodeSerialRequest())).toEqual({ type: "serial" });
    expect(decodeHostRequest(encodeResetConfig())).toEqual({ type: "resetConfig" });
    expect(decodeHostRequest(encodeBootloader())).toEqual({ type: "bootloader" });
    expect(decodeHostRequest(encodeReboot())).toEqual({ type: "reboot" });
    expect(decodeHostRequest(encodeReadRam(0x80080160, 166))).toEqual({ type: "read", source: "ram", address: 0x80080160, length: 166 });
    expect(decodeHostRequest(encodeReadFlash(0x80070210, 4))).toEqual({ type: "read", source: "flash", address: 0x80070210, length: 4 });
    expect(decodeHostRequest(encodeSwitchProfile(3))).toEqual({ type: "switchProfile", index: 3 });
    expect(decodeHostRequest(encodeCalibration(true))).toEqual({ type: "calibration", on: true });
    expect(decodeHostRequest(encodeCalibration(false))).toEqual({ type: "calibration", on: false });
    const w = decodeHostRequest(encodeWriteRam(0x80080c60, [5, 188, 255, 10, 50]));
    expect(w).toEqual({ type: "write", address: 0x80080c60, data: Uint8Array.of(5, 188, 255, 10, 50), crcOk: true });
  });

  it("flags a corrupted write", () => {
    const r = encodeWriteRam(0x80080160, [41, 0]);
    r[10] ^= 1;
    const w = decodeHostRequest(r);
    expect(w && w.type === "write" && w.crcOk).toBe(false);
  });

  it("ignores garbage", () => {
    expect(decodeHostRequest(new Uint8Array(1024))).toBeNull();
    expect(decodeHostRequest(padReport([0xbe, 0x00, 0x05, 0x00, 0x00]))).toBeNull(); // no ED
    expect(decodeHostRequest(padReport([0xbe, 0x77, 0x05, 0x00, 0xed]))).toEqual({ type: "unknown", cmd: 0x77 });
  });
});

describe("device -> host frames", () => {
  it("read RAM reply: data at [10, n-3), CRC at [n-3, n-1), echo of address and length", () => {
    const r = encodeReadReply(Cmd.ReadRam, 0x80070210, [18, 0, 0, 0]);
    expect(head(r, 10)).toEqual([0xbe, 0x04, 17, 0x00, 0x10, 0x02, 0x07, 0x80, 0x00, 0x04]);
    expect(r[16]).toBe(0xed);
    expect(decodeDeviceFrame(r)).toEqual({
      type: "read", source: "ram", address: 0x80070210, length: 4, data: Uint8Array.of(18, 0, 0, 0),
    });
  });

  it("read flash reply", () => {
    const f = decodeDeviceFrame(encodeReadReply(Cmd.ReadFlash, 0x80080160, [1, 2, 3]));
    expect(f).toMatchObject({ type: "read", source: "flash", address: 0x80080160, length: 3 });
  });

  it("rejects a read reply with a bad CRC or a missing end marker", () => {
    const r = encodeReadReply(Cmd.ReadRam, 0x80070210, [18, 0, 0, 0]);
    const bad = r.slice();
    bad[11] ^= 0x40;
    expect(decodeDeviceFrame(bad)).toMatchObject({ type: "bad", reason: "crc mismatch" });
    const noEnd = r.slice();
    noEnd[16] = 0;
    expect(decodeDeviceFrame(noEnd)).toMatchObject({ type: "bad", reason: "missing end marker" });
  });

  it("write ack BE 03 05 00 ED", () => {
    expect(head(encodeWriteAck(), 5)).toEqual([0xbe, 0x03, 0x05, 0x00, 0xed]);
    expect(decodeDeviceFrame(encodeWriteAck())).toEqual({ type: "writeAck" });
  });

  it("serial number BE 1D 15 00 + 16 bytes", () => {
    const serial = Uint8Array.from({ length: 16 }, (_, i) => i * 17);
    const r = encodeSerialReply(serial);
    expect(head(r, 4)).toEqual([0xbe, 0x1d, 0x15, 0x00]);
    expect(r[20]).toBe(0xed);
    const f = decodeDeviceFrame(r);
    expect(f).toEqual({ type: "serial", bytes: serial, hex: "00112233445566778899aabbccddeeff" });
    expect(() => encodeSerialReply(new Uint8Array(8))).toThrow(RangeError);
  });

  it("profile notice BE C0 06 00 n", () => {
    expect(decodeDeviceFrame(encodeProfileNotice(2))).toEqual({ type: "profile", index: 2 });
  });

  it("non-frames and unknown commands", () => {
    expect(decodeDeviceFrame(new Uint8Array(1024))).toBeNull();
    expect(decodeDeviceFrame(padReport([0xbe, 0x55, 0x06, 0x00, 0x09, 0xed]))).toMatchObject({ type: "unknown", cmd: 0x55, length: 6 });
    expect(decodeDeviceFrame(padReport([0xbe, 0x04, 0xff, 0xff]))).toMatchObject({ type: "bad" });
  });
});

describe("BE FD stream", () => {
  const raw = Array.from({ length: 83 }, (_, i) => 30000 - i * 10);
  const travel = Array.from({ length: 83 }, (_, i) => i * 40);

  it("travel variant (len 1017): 83 raw words then 83 travel words in µm", () => {
    const r = encodeStreamTravel(raw, travel, [7, 8, 9]);
    expect(r[1]).toBe(0xfd);
    expect(r[2] | (r[3] << 8)).toBe(STREAM_LEN_TRAVEL);
    expect(r[1016]).toBe(0xed);
    expect(r[1014] | (r[1015] << 8)).toBe(crc16Modbus(r, 1014));
    // first raw word sits at offset 4, first travel word at 4 + 166
    expect(r[4] | (r[5] << 8)).toBe(30000);
    expect(r[4 + 166 + 2] | (r[4 + 166 + 3] << 8)).toBe(40);
    const f = decodeDeviceFrame(r);
    if (!f || f.type !== "stream" || f.variant !== "travel") throw new Error("wrong frame type");
    expect(f.raw).toEqual(raw);
    expect(f.travelUm).toEqual(travel);
    // 505 payload words in total, 166 of them known
    expect(f.extra.length).toBe(505 - 166);
    expect(f.extra.slice(0, 4)).toEqual([7, 8, 9, 0]);
  });

  it("raw6 variant (len 1003): 6 scans, averaged per key", () => {
    const scans = [0, 1, 2, 3, 4, 5].map((s) => raw.map((v) => v + s * 6));
    const r = encodeStreamRaw6(scans);
    expect(r[2] | (r[3] << 8)).toBe(STREAM_LEN_RAW6);
    expect(r[1002]).toBe(0xed);
    expect(4 + 6 * 83 * 2 + 3).toBe(1003);
    const f = decodeDeviceFrame(r);
    if (!f || f.type !== "stream" || f.variant !== "raw6") throw new Error("wrong frame type");
    expect(f.samples).toEqual(scans);
    expect(f.raw[0]).toBe(30000 + 15);
    expect(f.raw[82]).toBe(raw[82] + 15);
  });

  it("drops stream frames with a bad CRC", () => {
    const r = encodeStreamTravel(raw, travel);
    r[100] ^= 1;
    expect(decodeDeviceFrame(r)).toMatchObject({ type: "bad", reason: "crc mismatch" });
    const r6 = encodeStreamRaw6([raw, raw, raw, raw, raw, raw]);
    r6[1001] ^= 1;
    expect(decodeDeviceFrame(r6)).toMatchObject({ type: "bad", reason: "crc mismatch" });
  });

  it("rejects other stream lengths and wrong block counts", () => {
    const r = encodeStreamTravel(raw, travel);
    r[2] = 0x10; r[3] = 0x00;
    expect(decodeDeviceFrame(r)).toMatchObject({ type: "bad" });
    expect(() => encodeStreamRaw6([raw, raw])).toThrow(RangeError);
    expect(() => encodeStreamTravel(raw, travel.slice(1))).toThrow(RangeError);
  });
});
