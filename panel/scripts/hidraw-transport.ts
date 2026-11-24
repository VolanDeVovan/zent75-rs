/**
 * Transport over a Linux /dev/hidrawN node, for running the shared codec from Node.
 *
 * write: one report-id byte (0x00) followed by the 1024-byte report
 * read:  one input report per read(); the kernel delivers 1024 bytes (no report-id byte,
 *        because the interface uses unnumbered reports); a 1025-byte form is accepted too.
 */
import { closeSync, existsSync, openSync, read, readFileSync, readdirSync, realpathSync, writeSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { Emitter, type DisconnectListener, type ReportListener, type Transport, type TransportInfo } from "../src/protocol/transport";
import { REPORT_ID, REPORT_SIZE } from "../src/protocol/zent/frames";

export const ZENT_VID = 0x36b0;
export const ZENT_PID = 0x357e;
export const VENDOR_INTERFACE = 2;

export class DeviceUnavailableError extends Error {}

export interface HidrawCandidate {
  path: string;
  vendorId: number;
  productId: number;
  interfaceNumber: number | null;
  name: string;
  /** report descriptor starts with usage page 0xFF60, usage 0x61 */
  vendorUsage: boolean;
}

/** Enumerate hidraw nodes through sysfs (no ioctl needed). */
export function listHidraw(sysRoot = "/sys/class/hidraw"): HidrawCandidate[] {
  if (!existsSync(sysRoot)) return [];
  const out: HidrawCandidate[] = [];
  for (const node of readdirSync(sysRoot).sort()) {
    try {
      const dev = realpathSync(join(sysRoot, node, "device"));
      const uevent = readFileSync(join(dev, "uevent"), "utf8");
      const id = /HID_ID=([0-9A-Fa-f]+):([0-9A-Fa-f]+):([0-9A-Fa-f]+)/.exec(uevent);
      if (!id) continue;
      const name = /HID_NAME=(.*)/.exec(uevent)?.[1] ?? "";
      let interfaceNumber: number | null = null;
      const ifFile = join(dirname(dev), "bInterfaceNumber");
      if (existsSync(ifFile)) interfaceNumber = parseInt(readFileSync(ifFile, "utf8").trim(), 16);
      let vendorUsage = false;
      const descFile = join(dev, "report_descriptor");
      if (existsSync(descFile)) {
        const d = readFileSync(descFile);
        vendorUsage = d.length >= 5 && d[0] === 0x06 && d[1] === 0x60 && d[2] === 0xff && d[3] === 0x09 && d[4] === 0x61;
      }
      out.push({
        path: `/dev/${basename(node)}`,
        vendorId: parseInt(id[2], 16),
        productId: parseInt(id[3], 16),
        interfaceNumber,
        name,
        vendorUsage,
      });
    } catch {
      // node vanished while we were looking at it (re-enumeration); skip
    }
  }
  return out;
}

/** The vendor raw-HID interface of the ZENT75-HE: 36b0:357e, interface 02. */
export function findZentHidraw(sysRoot?: string): HidrawCandidate | null {
  const mine = listHidraw(sysRoot).filter((c) => c.vendorId === ZENT_VID && c.productId === ZENT_PID);
  return mine.find((c) => c.interfaceNumber === VENDOR_INTERFACE) ?? mine.find((c) => c.vendorUsage) ?? null;
}

const GONE = new Set(["ENODEV", "ETIMEDOUT", "EPIPE", "EIO", "ENOENT", "ENXIO", "EPROTO", "ESHUTDOWN"]);

function unavailable(e: unknown, what: string): Error {
  const code = (e as NodeJS.ErrnoException)?.code ?? "";
  if (GONE.has(code)) return new DeviceUnavailableError(`${what}: ${code} (keyboard unplugged, re-enumerating, hung or in its bootloader)`);
  if (code === "EACCES") return new DeviceUnavailableError(`${what}: EACCES (no permission; check the udev rule for 36b0 hidraw nodes)`);
  return e instanceof Error ? e : new Error(String(e));
}

export class HidrawTransport implements Transport {
  readonly info: TransportInfo;
  isOpen = false;
  private fd = -1;
  private reports = new Emitter<Uint8Array>();
  private disconnects = new Emitter<unknown>();

  constructor(readonly path: string, productName = "ZENT75-HE") {
    this.info = { productName, vendorId: ZENT_VID, productId: ZENT_PID, backend: `hidraw:${path}` };
  }

  async open(): Promise<void> {
    try {
      this.fd = openSync(this.path, "r+");
    } catch (e) {
      throw unavailable(e, `open ${this.path}`);
    }
    this.isOpen = true;
    this.readLoop();
  }

  private readLoop(): void {
    const fd = this.fd;
    const buf = Buffer.alloc(REPORT_SIZE + 1);
    const next = () => {
      if (!this.isOpen || this.fd !== fd) return;
      read(fd, buf, 0, buf.length, null, (err, n) => {
        if (!this.isOpen || this.fd !== fd) return;
        if (err) {
          this.isOpen = false;
          this.disconnects.emit(unavailable(err, `read ${this.path}`));
          return;
        }
        if (n > 0) this.reports.emit(Uint8Array.from(buf.subarray(0, n)));
        next();
      });
    };
    next();
  }

  async close(): Promise<void> {
    if (!this.isOpen) return;
    this.isOpen = false;
    const fd = this.fd;
    this.fd = -1;
    // A read() may still be parked in the thread pool; closing the fd is fine on Linux,
    // the caller ends the process afterwards.
    try {
      closeSync(fd);
    } catch {
      /* already gone */
    }
  }

  async send(report: Uint8Array): Promise<void> {
    if (!this.isOpen) throw new DeviceUnavailableError("hidraw node is not open");
    if (report.length !== REPORT_SIZE) throw new RangeError(`report must be ${REPORT_SIZE} bytes`);
    const out = Buffer.alloc(REPORT_SIZE + 1);
    out[0] = REPORT_ID;
    out.set(report, 1);
    try {
      writeSync(this.fd, out);
    } catch (e) {
      throw unavailable(e, `write ${this.path}`);
    }
  }

  onReport(listener: ReportListener): () => void {
    return this.reports.on(listener);
  }

  onDisconnect(listener: DisconnectListener): () => void {
    return this.disconnects.on(listener);
  }
}
