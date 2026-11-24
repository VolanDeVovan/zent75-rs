import { describe, expect, it } from "vitest";
import { crc16Modbus } from "../src/protocol/crc";

const ascii = (s: string) => Uint8Array.from(s, (c) => c.charCodeAt(0));

describe("CRC-16/MODBUS", () => {
  it("matches the catalogue check value for '123456789'", () => {
    expect(crc16Modbus(ascii("123456789"))).toBe(0x4b37);
  });

  it("returns the init value for empty input", () => {
    expect(crc16Modbus(new Uint8Array(0))).toBe(0xffff);
  });

  it("matches well-known Modbus RTU frames", () => {
    // 01 03 00 00 00 0A -> C5 CD on the wire (low byte first)
    expect(crc16Modbus([0x01, 0x03, 0x00, 0x00, 0x00, 0x0a])).toBe(0xcdc5);
    // 11 03 00 6B 00 03 -> 76 87
    expect(crc16Modbus([0x11, 0x03, 0x00, 0x6b, 0x00, 0x03])).toBe(0x8776);
    // 01 06 00 01 00 03 -> 98 0B
    expect(crc16Modbus([0x01, 0x06, 0x00, 0x01, 0x00, 0x03])).toBe(0x0b98);
  });

  it("single bytes", () => {
    expect(crc16Modbus([0x00])).toBe(0x40bf);
    expect(crc16Modbus([0xff])).toBe(0x00ff);
  });

  it("honours the explicit length argument", () => {
    const buf = new Uint8Array(1024);
    buf.set(ascii("123456789"));
    expect(crc16Modbus(buf, 9)).toBe(0x4b37);
    expect(crc16Modbus(buf)).not.toBe(0x4b37);
  });

  it("a frame followed by its own CRC (low byte first) has residue 0", () => {
    const data = [0xbe, 0x03, 0x0f, 0x00, 0x60, 0x01, 0x08, 0x80, 0x00, 0x02, 0x29, 0x00];
    const crc = crc16Modbus(data);
    expect(crc16Modbus([...data, crc & 0xff, crc >> 8])).toBe(0);
  });

  it("agrees with the bitwise reference implementation", () => {
    const ref = (d: number[]) => {
      let c = 0xffff;
      for (const b of d) {
        c ^= b;
        for (let i = 0; i < 8; i++) c = c & 1 ? (c >> 1) ^ 0xa001 : c >> 1;
      }
      return c;
    };
    let x = 12345;
    for (let n = 0; n < 50; n++) {
      const d: number[] = [];
      for (let i = 0; i < n * 7 + 1; i++) {
        x = (x * 1103515245 + 12345) & 0x7fffffff;
        d.push(x & 0xff);
      }
      expect(crc16Modbus(d)).toBe(ref(d));
    }
  });
});
