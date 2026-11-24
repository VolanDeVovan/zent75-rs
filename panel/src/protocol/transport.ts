/**
 * Lowest layer: moves fixed-size raw HID reports to and from a device.
 * Implementations: WebHID (browser), /dev/hidraw (Node), MockDevice (tests, demo).
 * A Transport knows nothing about the bytes it carries.
 */
export interface TransportInfo {
  /** Human-readable name of the device or backend. */
  productName: string;
  vendorId?: number;
  productId?: number;
  /** e.g. "webhid", "hidraw:/dev/hidraw7", "mock" */
  backend: string;
}

export type ReportListener = (report: Uint8Array) => void;
export type DisconnectListener = (reason?: unknown) => void;

export interface Transport {
  readonly info: TransportInfo;
  readonly isOpen: boolean;
  open(): Promise<void>;
  close(): Promise<void>;
  /** Send one output report (payload only, without the report-id byte). */
  send(report: Uint8Array): Promise<void>;
  /** Subscribe to input reports (payload only). Returns an unsubscribe function. */
  onReport(listener: ReportListener): () => void;
  /** Called when the device goes away (unplug, reboot, bootloader). */
  onDisconnect(listener: DisconnectListener): () => void;
}

/** Small helper shared by the transports. */
export class Emitter<T> {
  private listeners = new Set<(value: T) => void>();
  on(listener: (value: T) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
  emit(value: T): void {
    for (const l of [...this.listeners]) l(value);
  }
  get size(): number {
    return this.listeners.size;
  }
}
