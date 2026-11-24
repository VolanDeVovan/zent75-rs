/**
 * CRC-16/MODBUS: polynomial 0x8005 reflected (0xA001), init 0xFFFF, no final xor.
 * The stock firmware appends it little-endian to every frame that carries data.
 */
const TABLE = /* @__PURE__ */ (() => {
  const t = new Uint16Array(256);
  for (let i = 0; i < 256; i++) {
    let c = i;
    for (let b = 0; b < 8; b++) c = c & 1 ? (c >>> 1) ^ 0xa001 : c >>> 1;
    t[i] = c;
  }
  return t;
})();

export function crc16Modbus(data: ArrayLike<number>, length: number = data.length): number {
  let crc = 0xffff;
  for (let i = 0; i < length; i++) crc = (crc >>> 8) ^ TABLE[(crc ^ data[i]) & 0xff];
  return crc & 0xffff;
}
