/**
 * Frame codec of the stock ZENT75-HE firmware (vendor raw HID interface, usage page 0xFF60).
 *
 *   BE <cmd> <len LE16> ... ED      padded with zeros to a 1024-byte report, report id 0
 *
 * `len` counts the whole frame including BE and ED. Frames that carry data put a
 * CRC-16/MODBUS (LE) right before ED, computed over everything in front of it.
 *
 * This module is pure: bytes in, bytes out. Both directions are implemented
 * (host -> device requests and device -> host replies) because the mock device and the
 * tests need the device side too.
 */
import { crc16Modbus } from "../crc";

export const REPORT_SIZE = 1024;
export const REPORT_ID = 0;
export const SOF = 0xbe;
export const EOF = 0xed;

export const Cmd = {
  Heartbeat: 0x00,
  ReadFlash: 0x01,
  Save: 0x02,
  WriteRam: 0x03,
  ReadRam: 0x04,
  Serial: 0x1d,
  SwitchProfile: 0xc0,
  ResetConfig: 0xee,
  Bootloader: 0xfc,
  Stream: 0xfd,
  Calibration: 0xfe,
  Reboot: 0xff,
} as const;

/** Header (4) + address (4) + length (2) + crc (2) + ED (1). */
export const DATA_FRAME_OVERHEAD = 13;
/** Largest data block that fits into one report. */
export const MAX_DATA_PER_FRAME = REPORT_SIZE - DATA_FRAME_OVERHEAD;

export const STREAM_LEN_TRAVEL = 1017;
export const STREAM_LEN_RAW6 = 1003;
export const SERIAL_REPLY_LEN = 21;

// ---------------------------------------------------------------- helpers

/** Zero-pad a frame to one full report. */
export function padReport(frame: ArrayLike<number>): Uint8Array {
  if (frame.length > REPORT_SIZE) throw new RangeError(`frame of ${frame.length} bytes exceeds ${REPORT_SIZE}`);
  const out = new Uint8Array(REPORT_SIZE);
  out.set(frame as ArrayLike<number>);
  return out;
}

/**
 * Normalise an input report: hidraw may hand over 1025 bytes with a leading report-id 0,
 * WebHID hands over exactly the payload.
 */
export function normalizeReport(report: Uint8Array): Uint8Array {
  if (report.length === REPORT_SIZE + 1 && report[0] === REPORT_ID && report[1] === SOF) return report.subarray(1);
  return report;
}

function simple(cmd: number): Uint8Array {
  return padReport([SOF, cmd, 5, 0, EOF]);
}

function withArg(cmd: number, arg: number): Uint8Array {
  return padReport([SOF, cmd, 6, 0, arg & 0xff, EOF]);
}

function checkAddr(address: number): void {
  if (!Number.isInteger(address) || address < 0 || address > 0xffffffff) throw new RangeError(`bad address ${address}`);
}

function readRequest(cmd: number, address: number, length: number): Uint8Array {
  checkAddr(address);
  if (!Number.isInteger(length) || length < 1 || length > MAX_DATA_PER_FRAME) {
    throw new RangeError(`read length ${length} out of range 1..${MAX_DATA_PER_FRAME}`);
  }
  return padReport([
    SOF, cmd, 11, 0,
    address & 0xff, (address >>> 8) & 0xff, (address >>> 16) & 0xff, (address >>> 24) & 0xff,
    (length >> 8) & 0xff, length & 0xff, // length is big-endian, unlike everything else
    EOF,
  ]);
}

/** Build `BE cmd len addr n data crc ED` (used by write requests and by read replies). */
function dataFrame(cmd: number, address: number, data: ArrayLike<number>): Uint8Array {
  checkAddr(address);
  const n = data.length;
  if (n < 1 || n > MAX_DATA_PER_FRAME) throw new RangeError(`data length ${n} out of range 1..${MAX_DATA_PER_FRAME}`);
  const total = DATA_FRAME_OVERHEAD + n;
  const out = new Uint8Array(REPORT_SIZE);
  out[0] = SOF;
  out[1] = cmd;
  out[2] = total & 0xff;
  out[3] = total >> 8;
  out[4] = address & 0xff;
  out[5] = (address >>> 8) & 0xff;
  out[6] = (address >>> 16) & 0xff;
  out[7] = (address >>> 24) & 0xff;
  out[8] = (n >> 8) & 0xff;
  out[9] = n & 0xff;
  for (let i = 0; i < n; i++) out[10 + i] = data[i] & 0xff;
  const crc = crc16Modbus(out, 10 + n);
  out[10 + n] = crc & 0xff;
  out[11 + n] = crc >> 8;
  out[12 + n] = EOF;
  return out;
}

// ---------------------------------------------------------------- host -> device

export const encodeHeartbeat = (): Uint8Array => simple(Cmd.Heartbeat);
export const encodeSave = (): Uint8Array => simple(Cmd.Save);
export const encodeSerialRequest = (): Uint8Array => simple(Cmd.Serial);
export const encodeResetConfig = (): Uint8Array => simple(Cmd.ResetConfig);
export const encodeBootloader = (): Uint8Array => simple(Cmd.Bootloader);
export const encodeReboot = (): Uint8Array => simple(Cmd.Reboot);
export const encodeReadRam = (address: number, length: number): Uint8Array => readRequest(Cmd.ReadRam, address, length);
export const encodeReadFlash = (address: number, length: number): Uint8Array => readRequest(Cmd.ReadFlash, address, length);
export const encodeWriteRam = (address: number, data: ArrayLike<number>): Uint8Array => dataFrame(Cmd.WriteRam, address, data);

export function encodeSwitchProfile(index: number): Uint8Array {
  if (!Number.isInteger(index) || index < 0 || index > 3) throw new RangeError(`profile index ${index} out of range 0..3`);
  return withArg(Cmd.SwitchProfile, index);
}

export function encodeCalibration(on: boolean): Uint8Array {
  return withArg(Cmd.Calibration, on ? 1 : 0);
}

// ---------------------------------------------------------------- device -> host (encoders, for the mock)

export const encodeReadReply = (cmd: typeof Cmd.ReadRam | typeof Cmd.ReadFlash, address: number, data: ArrayLike<number>): Uint8Array =>
  dataFrame(cmd, address, data);

export const encodeWriteAck = (): Uint8Array => simple(Cmd.WriteRam);

export function encodeSerialReply(serial: ArrayLike<number>): Uint8Array {
  if (serial.length !== 16) throw new RangeError("serial number must be 16 bytes");
  const f = new Uint8Array(SERIAL_REPLY_LEN);
  f[0] = SOF;
  f[1] = Cmd.Serial;
  f[2] = SERIAL_REPLY_LEN;
  f[3] = 0;
  f.set(serial as ArrayLike<number>, 4);
  f[20] = EOF;
  return padReport(f);
}

/** The board pushes this on its own when the profile changes (Fn shortcut or after BE C0). */
export const encodeProfileNotice = (index: number): Uint8Array => withArg(Cmd.SwitchProfile, index);

function streamFrame(total: number, words: ArrayLike<number>, tail: ArrayLike<number> = []): Uint8Array {
  const out = new Uint8Array(REPORT_SIZE);
  out[0] = SOF;
  out[1] = Cmd.Stream;
  out[2] = total & 0xff;
  out[3] = total >> 8;
  let o = 4;
  for (let i = 0; i < words.length; i++) {
    out[o++] = words[i] & 0xff;
    out[o++] = (words[i] >> 8) & 0xff;
  }
  for (let i = 0; i < tail.length; i++) out[o++] = tail[i] & 0xff;
  if (o > total - 3) throw new RangeError("stream payload too long");
  const crc = crc16Modbus(out, total - 3);
  out[total - 3] = crc & 0xff;
  out[total - 2] = crc >> 8;
  out[total - 1] = EOF;
  return out;
}

/**
 * Normal live frame (len 1017): `keyCount` raw ADC words, then `keyCount` travel words (µm).
 * The rest of the 1010-byte payload is not interpreted by the vendor driver; `extra` words
 * are placed right behind the two known blocks.
 */
export function encodeStreamTravel(raw: ArrayLike<number>, travelUm: ArrayLike<number>, extra: ArrayLike<number> = []): Uint8Array {
  if (raw.length !== travelUm.length) throw new RangeError("raw and travel must have the same length");
  const words = [...Array.from(raw), ...Array.from(travelUm), ...Array.from(extra)];
  return streamFrame(STREAM_LEN_TRAVEL, words);
}

/** Calibration-mode frame (len 1003): 6 consecutive scans of `keyCount` raw ADC words. */
export function encodeStreamRaw6(samples: ArrayLike<ArrayLike<number>>): Uint8Array {
  if (samples.length !== 6) throw new RangeError("need exactly 6 sample blocks");
  const words: number[] = [];
  for (let s = 0; s < 6; s++) words.push(...Array.from(samples[s]));
  return streamFrame(STREAM_LEN_RAW6, words);
}

// ---------------------------------------------------------------- device -> host (decoder)

export interface ReadReply {
  type: "read";
  source: "ram" | "flash";
  address: number;
  length: number;
  data: Uint8Array;
}
export interface WriteAck { type: "writeAck" }
export interface SerialReply { type: "serial"; bytes: Uint8Array; hex: string }
export interface ProfileNotice { type: "profile"; index: number }
export interface StreamTravelFrame {
  type: "stream";
  variant: "travel";
  /** raw ADC counts, one per key */
  raw: number[];
  /** travel in micrometres, one per key */
  travelUm: number[];
  /** remaining payload words the vendor driver ignores */
  extra: number[];
}
export interface StreamRaw6Frame {
  type: "stream";
  variant: "raw6";
  /** 6 scans x keyCount raw ADC counts */
  samples: number[][];
  /** per-key mean of the 6 scans (what the vendor calibration uses) */
  raw: number[];
}
export type StreamFrame = StreamTravelFrame | StreamRaw6Frame;
export interface BadFrame { type: "bad"; cmd: number; reason: string }
export interface UnknownFrame { type: "unknown"; cmd: number; length: number; body: Uint8Array }

export type DeviceFrame = ReadReply | WriteAck | SerialReply | ProfileNotice | StreamFrame | BadFrame | UnknownFrame;

function frameLen(r: Uint8Array): number {
  return r[2] | (r[3] << 8);
}

function crcOk(r: Uint8Array, total: number): boolean {
  return crc16Modbus(r, total - 3) === (r[total - 3] | (r[total - 2] << 8));
}

function words(r: Uint8Array, offset: number, count: number): number[] {
  const out = new Array<number>(count);
  for (let i = 0; i < count; i++) out[i] = r[offset + 2 * i] | (r[offset + 2 * i + 1] << 8);
  return out;
}

export function toHex(bytes: ArrayLike<number>): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * Decode one input report from the keyboard. Returns null when the report is not a
 * protocol frame at all (does not start with BE).
 */
export function decodeDeviceFrame(report: Uint8Array, keyCount = 83): DeviceFrame | null {
  const r = normalizeReport(report);
  if (r.length < 5 || r[0] !== SOF) return null;
  const cmd = r[1];
  const total = frameLen(r);
  const bad = (reason: string): BadFrame => ({ type: "bad", cmd, reason });
  if (total < 5 || total > r.length) return bad(`length ${total} out of range`);

  switch (cmd) {
    case Cmd.ReadRam:
    case Cmd.ReadFlash: {
      if (total < DATA_FRAME_OVERHEAD + 1) return bad("read reply too short");
      if (r[total - 1] !== EOF) return bad("missing end marker");
      if (!crcOk(r, total)) return bad("crc mismatch");
      const address = (r[4] | (r[5] << 8) | (r[6] << 16) | (r[7] << 24)) >>> 0;
      const length = (r[8] << 8) | r[9];
      const data = r.slice(10, total - 3);
      return { type: "read", source: cmd === Cmd.ReadRam ? "ram" : "flash", address, length, data };
    }
    case Cmd.WriteRam:
      if (total === 5 && r[4] === EOF) return { type: "writeAck" };
      return bad("malformed write ack");
    case Cmd.Serial: {
      if (total !== SERIAL_REPLY_LEN) return bad(`serial reply length ${total}`);
      const bytes = r.slice(4, 20);
      return { type: "serial", bytes, hex: toHex(bytes) };
    }
    case Cmd.SwitchProfile:
      if (total !== 6) return bad(`profile notice length ${total}`);
      return { type: "profile", index: r[4] };
    case Cmd.Stream: {
      if (r[total - 1] !== EOF) return bad("missing end marker");
      if (!crcOk(r, total)) return bad("crc mismatch");
      const payloadWords = (total - 7) >> 1;
      if (total === STREAM_LEN_RAW6 && payloadWords >= 6 * keyCount) {
        const samples: number[][] = [];
        for (let s = 0; s < 6; s++) samples.push(words(r, 4 + 2 * s * keyCount, keyCount));
        const raw = new Array<number>(keyCount);
        for (let k = 0; k < keyCount; k++) {
          let sum = 0;
          for (let s = 0; s < 6; s++) sum += samples[s][k];
          raw[k] = sum / 6;
        }
        return { type: "stream", variant: "raw6", samples, raw };
      }
      if (total === STREAM_LEN_TRAVEL && payloadWords >= 2 * keyCount) {
        return {
          type: "stream",
          variant: "travel",
          raw: words(r, 4, keyCount),
          travelUm: words(r, 4 + 2 * keyCount, keyCount),
          extra: words(r, 4 + 4 * keyCount, payloadWords - 2 * keyCount),
        };
      }
      return bad(`unknown stream length ${total}`);
    }
    default:
      return { type: "unknown", cmd, length: total, body: r.slice(4, Math.max(4, total - 1)) };
  }
}

// ---------------------------------------------------------------- host -> device (decoder, for the mock)

export type HostRequest =
  | { type: "heartbeat" }
  | { type: "save" }
  | { type: "serial" }
  | { type: "resetConfig" }
  | { type: "bootloader" }
  | { type: "reboot" }
  | { type: "read"; source: "ram" | "flash"; address: number; length: number }
  | { type: "write"; address: number; data: Uint8Array; crcOk: boolean }
  | { type: "switchProfile"; index: number }
  | { type: "calibration"; on: boolean }
  | { type: "unknown"; cmd: number };

/** Decode a host request the way the firmware would. Returns null for non-frames. */
export function decodeHostRequest(report: Uint8Array): HostRequest | null {
  const r = normalizeReport(report);
  if (r.length < 5 || r[0] !== SOF) return null;
  const cmd = r[1];
  const total = frameLen(r);
  if (total < 5 || total > r.length || r[total - 1] !== EOF) return null;
  switch (cmd) {
    case Cmd.Heartbeat: return { type: "heartbeat" };
    case Cmd.Save: return { type: "save" };
    case Cmd.Serial: return { type: "serial" };
    case Cmd.ResetConfig: return { type: "resetConfig" };
    case Cmd.Bootloader: return { type: "bootloader" };
    case Cmd.Reboot: return { type: "reboot" };
    case Cmd.ReadRam:
    case Cmd.ReadFlash: {
      if (total !== 11) return null;
      const address = (r[4] | (r[5] << 8) | (r[6] << 16) | (r[7] << 24)) >>> 0;
      return { type: "read", source: cmd === Cmd.ReadRam ? "ram" : "flash", address, length: (r[8] << 8) | r[9] };
    }
    case Cmd.WriteRam: {
      if (total < DATA_FRAME_OVERHEAD + 1) return null;
      const address = (r[4] | (r[5] << 8) | (r[6] << 16) | (r[7] << 24)) >>> 0;
      const n = (r[8] << 8) | r[9];
      if (n !== total - DATA_FRAME_OVERHEAD) return null;
      return { type: "write", address, data: r.slice(10, 10 + n), crcOk: crcOk(r, total) };
    }
    case Cmd.SwitchProfile:
      return total === 6 ? { type: "switchProfile", index: r[4] } : null;
    case Cmd.Calibration:
      return total === 6 ? { type: "calibration", on: r[4] !== 0 } : null;
    default:
      return { type: "unknown", cmd };
  }
}
