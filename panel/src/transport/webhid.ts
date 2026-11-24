/**
 * Transport over WebHID (Chromium-based browsers only: Chrome, Edge, Brave, Opera, ...).
 * Firefox and Safari do not implement navigator.hid.
 */
import { Emitter, type DisconnectListener, type ReportListener, type Transport, type TransportInfo } from "../protocol/transport";
import { REPORT_ID, REPORT_SIZE } from "../protocol/zent/frames";

export const ZENT_VENDOR_ID = 0x36b0;
export const ZENT_PRODUCT_ID = 0x357e;
/** vendor raw-HID collection of the stock firmware */
export const ZENT_USAGE_PAGE = 0xff60;
export const ZENT_USAGE = 0x61;

export function webHidSupported(): boolean {
  return typeof navigator !== "undefined" && "hid" in navigator && !!navigator.hid;
}

function isVendorInterface(d: HIDDevice): boolean {
  return d.vendorId === ZENT_VENDOR_ID && d.collections.some((c) => c.usagePage === ZENT_USAGE_PAGE && c.usage === ZENT_USAGE);
}

export class WebHidTransport implements Transport {
  readonly info: TransportInfo;
  private reports = new Emitter<Uint8Array>();
  private disconnects = new Emitter<unknown>();
  private onInput = (e: HIDInputReportEvent) => {
    this.reports.emit(new Uint8Array(e.data.buffer, e.data.byteOffset, e.data.byteLength));
  };
  private onGone = (e: HIDConnectionEvent) => {
    if (e.device === this.device) this.disconnects.emit("unplugged");
  };

  constructor(private readonly device: HIDDevice) {
    this.info = {
      productName: device.productName || "ZENT75-HE",
      vendorId: device.vendorId,
      productId: device.productId,
      backend: "webhid",
    };
  }

  /** Devices the user already granted to this origin. */
  static async granted(): Promise<WebHidTransport[]> {
    if (!webHidSupported()) return [];
    return (await navigator.hid.getDevices()).filter(isVendorInterface).map((d) => new WebHidTransport(d));
  }

  /** Show the browser's device chooser. Must be called from a user gesture. Returns null when cancelled. */
  static async request(): Promise<WebHidTransport | null> {
    if (!webHidSupported()) throw new Error("WebHID is not available in this browser");
    const devices = await navigator.hid.requestDevice({
      filters: [{ vendorId: ZENT_VENDOR_ID, usagePage: ZENT_USAGE_PAGE, usage: ZENT_USAGE }],
    });
    const d = devices.find(isVendorInterface);
    return d ? new WebHidTransport(d) : null;
  }

  get isOpen(): boolean {
    return this.device.opened;
  }

  async open(): Promise<void> {
    if (!this.device.opened) await this.device.open();
    this.device.addEventListener("inputreport", this.onInput);
    navigator.hid.addEventListener("disconnect", this.onGone);
  }

  async close(): Promise<void> {
    this.device.removeEventListener("inputreport", this.onInput);
    navigator.hid.removeEventListener("disconnect", this.onGone);
    if (this.device.opened) await this.device.close();
  }

  async send(report: Uint8Array): Promise<void> {
    if (report.length !== REPORT_SIZE) throw new RangeError(`report must be ${REPORT_SIZE} bytes`);
    await this.device.sendReport(REPORT_ID, report as Uint8Array<ArrayBuffer>);
  }

  onReport(listener: ReportListener): () => void {
    return this.reports.on(listener);
  }

  onDisconnect(listener: DisconnectListener): () => void {
    return this.disconnects.on(listener);
  }
}
