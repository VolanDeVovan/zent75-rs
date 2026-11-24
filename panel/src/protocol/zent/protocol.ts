/**
 * KeyboardProtocol implementation for the stock ZENT75-HE firmware.
 * Everything that knows about BE..ED frames and vendor addresses is in this directory.
 */
import type {
  Calibration, DeviceInfo, KeyboardEvents, KeyboardProtocol, LiveSample, ProfileConfig,
} from "../keyboard";
import { KEY_COUNT, LED_COUNT } from "../layout";
import { Emitter, type Transport } from "../transport";
import {
  type DeviceFrame, type ReadReply,
  decodeDeviceFrame, encodeBootloader, encodeCalibration as encodeCalibrationCmd, encodeHeartbeat, encodeReadFlash,
  encodeReadRam, encodeReboot, encodeResetConfig, encodeSave, encodeSerialRequest, encodeSwitchProfile, encodeWriteRam,
} from "./frames";
import {
  type Images, type Region, type RegionName, type WriteRun,
  ADVANCED_KEYS_MIN_VERSION, ADVANCED_REGIONS, CALIBRATION_REGIONS, PROFILE_COUNT, PROFILE_NAME_SIZE, PROFILE_REGIONS, REGIONS,
  decodeCalibration, decodeProfile, decodeProfileName, decodeVersion, diffRegion, encodeCalibration, encodeProfile,
  encodeProfileName, findRegion,
} from "./memory";

export interface ZentOptions {
  /** how long to wait for a reply before retrying */
  timeoutMs: number;
  /** attempts per request */
  attempts: number;
  /** largest read per frame; the vendor driver never reads more than 256 bytes at once */
  maxReadChunk: number;
  /** largest write per frame; the vendor driver never writes more than 255 bytes at once */
  maxWriteChunk: number;
  /** read back every write and compare */
  verifyWrites: boolean;
  /** time the firmware needs to commit RAM to flash (the vendor driver waits 1100 ms, there is no ack) */
  saveDelayMs: number;
  /** settle time after reset-config */
  resetDelayMs: number;
  /** poll interval / attempts while waiting for a profile switch */
  profilePollMs: number;
  profilePollAttempts: number;
  heartbeatMs: number;
  /** pause after the serial-number reply */
  serialSettleMs: number;
  /**
   * Refuse reads and writes outside the known memory map. Out-of-range requests have hung
   * the stock firmware (BE 01 at unmapped addresses), so this stays on by default.
   */
  restrictToMap: boolean;
}

export const DEFAULT_OPTIONS: ZentOptions = {
  timeoutMs: 400,
  attempts: 3,
  maxReadChunk: 240,
  maxWriteChunk: 255,
  verifyWrites: true,
  saveDelayMs: 1100,
  resetDelayMs: 400,
  profilePollMs: 250,
  profilePollAttempts: 14,
  heartbeatMs: 1000,
  serialSettleMs: 60,
  restrictToMap: true,
};

export class ProtocolError extends Error {}
export class TimeoutError extends ProtocolError {}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

interface Waiter {
  match: (frame: DeviceFrame) => boolean;
  resolve: (frame: DeviceFrame) => void;
}

export class ZentStockProtocol implements KeyboardProtocol {
  readonly id = "zent-stock";
  info: DeviceInfo | null = null;

  private readonly opts: ZentOptions;
  private waiters = new Set<Waiter>();
  private lock: Promise<unknown> = Promise.resolve();
  private unsubscribe: (() => void)[] = [];
  private events = { profile: new Emitter<number>(), disconnect: new Emitter<unknown>() };
  private streamListeners = new Set<(sample: LiveSample) => void>();
  private heartbeat: ReturnType<typeof setInterval> | null = null;
  /** last known device contents of the active profile / global regions */
  private cache: Images = {};
  private connected = false;
  /** statistics, handy for the smoke test and debugging */
  readonly stats = { sent: 0, received: 0, badFrames: 0, retries: 0, streamFrames: 0, retried: [] as string[] };

  constructor(private readonly transport: Transport, options: Partial<ZentOptions> = {}) {
    this.opts = { ...DEFAULT_OPTIONS, ...options };
  }

  // ------------------------------------------------------------ lifecycle

  async connect(): Promise<DeviceInfo> {
    if (!this.transport.isOpen) await this.transport.open();
    this.unsubscribe.push(this.transport.onReport((r) => this.handleReport(r)));
    this.unsubscribe.push(this.transport.onDisconnect((reason) => this.handleDisconnect(reason)));
    this.connected = true;
    try {
      const version = decodeVersion(await this.readRegion("soft_version"));
      const serial = await this.readSerial().catch(() => "");
      this.info = {
        protocol: this.id,
        productName: this.transport.info.productName,
        firmwareVersion: version.text,
        versionNumber: version.number,
        serial,
        keyCount: KEY_COUNT,
        ledCount: LED_COUNT,
        profileCount: PROFILE_COUNT,
        features: { advancedKeys: version.number >= ADVANCED_KEYS_MIN_VERSION },
      };
      return this.info;
    } catch (e) {
      await this.disconnect();
      throw e;
    }
  }

  async disconnect(): Promise<void> {
    this.stopHeartbeat();
    this.streamListeners.clear();
    for (const u of this.unsubscribe.splice(0)) u();
    this.connected = false;
    this.cache = {};
    if (this.transport.isOpen) await this.transport.close().catch(() => undefined);
  }

  on<K extends keyof KeyboardEvents>(event: K, listener: (value: KeyboardEvents[K]) => void): () => void {
    return (this.events[event] as Emitter<KeyboardEvents[K]>).on(listener);
  }

  private handleDisconnect(reason: unknown): void {
    if (!this.connected) return;
    this.connected = false;
    this.stopHeartbeat();
    this.events.disconnect.emit(reason);
  }

  // ------------------------------------------------------------ frame plumbing

  private handleReport(report: Uint8Array): void {
    const frame = decodeDeviceFrame(report, KEY_COUNT);
    if (!frame) return;
    this.stats.received++;
    if (frame.type === "bad") {
      this.stats.badFrames++;
      return;
    }
    if (frame.type === "stream") {
      this.stats.streamFrames++;
      if (this.streamListeners.size) {
        const sample: LiveSample =
          frame.variant === "travel"
            ? { kind: "travel", raw: frame.raw, travelUm: frame.travelUm }
            : { kind: "raw", raw: frame.raw, travelUm: null };
        for (const l of [...this.streamListeners]) l(sample);
      }
      return;
    }
    if (frame.type === "profile") {
      this.invalidateProfileCache();
      this.events.profile.emit(frame.index);
    }
    for (const w of [...this.waiters]) {
      if (w.match(frame)) {
        this.waiters.delete(w);
        w.resolve(frame);
      }
    }
  }

  private async sendRaw(report: Uint8Array): Promise<void> {
    if (!this.connected) throw new ProtocolError("not connected");
    this.stats.sent++;
    await this.transport.send(report);
  }

  /** Run `fn` with exclusive use of the request/reply channel. */
  private exclusive<T>(fn: () => Promise<T>): Promise<T> {
    const run = this.lock.then(fn, fn);
    this.lock = run.catch(() => undefined);
    return run;
  }

  /** Send a request and wait for the matching reply, with retries. Must run inside `exclusive`. */
  private async request<T extends DeviceFrame>(report: Uint8Array, match: (f: DeviceFrame) => f is T, what: string): Promise<T> {
    let lastError: unknown;
    for (let attempt = 0; attempt < this.opts.attempts; attempt++) {
      if (attempt > 0) {
        this.stats.retries++;
        if (this.stats.retried.length < 32) this.stats.retried.push(what);
      }
      let waiter!: Waiter;
      let timer!: ReturnType<typeof setTimeout>;
      const reply = new Promise<DeviceFrame>((resolve, reject) => {
        waiter = { match, resolve };
        timer = setTimeout(() => reject(new TimeoutError(`${what}: no reply within ${this.opts.timeoutMs} ms`)), this.opts.timeoutMs);
      });
      this.waiters.add(waiter);
      try {
        await this.sendRaw(report);
        return (await reply) as T;
      } catch (e) {
        lastError = e;
        // swallow the timeout rejection when sendRaw itself failed
        reply.catch(() => undefined);
        if (!(e instanceof TimeoutError)) throw e;
      } finally {
        clearTimeout(timer);
        this.waiters.delete(waiter);
      }
    }
    throw lastError;
  }

  private checkRange(address: number, length: number, write: boolean): void {
    if (!this.opts.restrictToMap) return;
    const r: Region | undefined = findRegion(address, length);
    if (!r) throw new ProtocolError(`0x${address.toString(16)}+${length} is outside the known memory map`);
    if (write && !r.writable) throw new ProtocolError(`region ${r.name} is read-only`);
  }

  // ------------------------------------------------------------ raw memory access

  private async readOnce(source: "ram" | "flash", address: number, length: number): Promise<Uint8Array> {
    const report = source === "ram" ? encodeReadRam(address, length) : encodeReadFlash(address, length);
    const what = `read ${source} 0x${address.toString(16)}+${length}`;
    const reply = await this.request(
      report,
      (f): f is ReadReply => f.type === "read" && f.source === source && f.address === address && f.data.length === length,
      what,
    );
    return reply.data;
  }

  /** BE 04. Long ranges are split into several frames. */
  async readRam(address: number, length: number): Promise<Uint8Array> {
    this.checkRange(address, length, false);
    return this.exclusive(async () => {
      const out = new Uint8Array(length);
      for (let o = 0; o < length; o += this.opts.maxReadChunk) {
        const n = Math.min(this.opts.maxReadChunk, length - o);
        out.set(await this.readOnce("ram", address + o, n), o);
      }
      return out;
    });
  }

  /**
   * BE 01. The vendor driver defines it but never calls it, and reads at unmapped addresses
   * have hung the stock firmware until a replug. Not used by the panel.
   */
  async readFlash(address: number, length: number): Promise<Uint8Array> {
    this.checkRange(address, length, false);
    return this.exclusive(async () => {
      const out = new Uint8Array(length);
      for (let o = 0; o < length; o += this.opts.maxReadChunk) {
        const n = Math.min(this.opts.maxReadChunk, length - o);
        out.set(await this.readOnce("flash", address + o, n), o);
      }
      return out;
    });
  }

  /** BE 03, acknowledged by `BE 03 05 00 ED`, optionally verified by reading back. */
  async writeRam(address: number, data: Uint8Array): Promise<void> {
    this.checkRange(address, data.length, true);
    return this.exclusive(async () => {
      for (let o = 0; o < data.length; o += this.opts.maxWriteChunk) {
        const chunk = data.subarray(o, Math.min(data.length, o + this.opts.maxWriteChunk));
        const a = address + o;
        await this.request(encodeWriteRam(a, chunk), (f): f is DeviceFrame => f.type === "writeAck", `write 0x${a.toString(16)}+${chunk.length}`);
        if (this.opts.verifyWrites) {
          const back = await this.readOnce("ram", a, chunk.length);
          for (let i = 0; i < chunk.length; i++) {
            if (back[i] !== chunk[i]) {
              throw new ProtocolError(`write verify failed at 0x${(a + i).toString(16)}: wrote ${chunk[i]}, read ${back[i]}`);
            }
          }
        }
      }
    });
  }

  async readRegion(name: RegionName): Promise<Uint8Array> {
    const r = REGIONS[name];
    return this.readRam(r.address, r.length);
  }

  private async readRegions(names: readonly RegionName[]): Promise<Images> {
    const images: Images = {};
    for (const n of names) images[n] = await this.readRegion(n);
    return images;
  }

  private async writeRuns(runs: WriteRun[]): Promise<number> {
    let bytes = 0;
    for (const run of runs) {
      await this.writeRam(run.address, run.data);
      const cached = this.cache[run.region];
      if (cached) cached.set(run.data, run.offset);
      bytes += run.data.length;
    }
    return bytes;
  }

  private async writeImages(next: Images): Promise<number> {
    const names = Object.keys(next) as RegionName[];
    // make sure we know what the device holds, so that only real differences are written
    for (const n of names) if (!this.cache[n]) this.cache[n] = await this.readRegion(n);
    const runs: WriteRun[] = [];
    for (const n of names) runs.push(...diffRegion(n, this.cache[n], next[n]!, this.opts.maxWriteChunk));
    return this.writeRuns(runs);
  }

  private invalidateProfileCache(): void {
    for (const n of [...PROFILE_REGIONS, ...ADVANCED_REGIONS]) delete this.cache[n];
  }

  // ------------------------------------------------------------ info

  /** BE 1D: 16 serial-number bytes, shown as hex like the vendor driver does. */
  async readSerial(): Promise<string> {
    return this.exclusive(async () => {
      const f = await this.request(encodeSerialRequest(), (x): x is Extract<DeviceFrame, { type: "serial" }> => x.type === "serial", "serial number");
      // the stock firmware ignores a request that arrives right behind its serial reply
      await sleep(this.opts.serialSettleMs);
      return f.hex;
    });
  }

  // ------------------------------------------------------------ profiles

  async getActiveProfile(): Promise<number> {
    return (await this.readRegion("cur_param_num"))[0];
  }

  async setActiveProfile(index: number): Promise<void> {
    const report = encodeSwitchProfile(index);
    if ((await this.getActiveProfile()) === index) return;
    this.invalidateProfileCache();
    await this.sendRaw(report);
    for (let i = 0; i < this.opts.profilePollAttempts; i++) {
      await sleep(this.opts.profilePollMs);
      const now = await this.getActiveProfile().catch(() => -1);
      if (now === index) {
        this.invalidateProfileCache();
        return;
      }
    }
    throw new ProtocolError(`profile switch to ${index} was not confirmed by the keyboard`);
  }

  async readProfileNames(): Promise<string[]> {
    const all = await this.readRegion("config_name");
    this.cache.config_name = all;
    const names: string[] = [];
    for (let i = 0; i < PROFILE_COUNT; i++) names.push(decodeProfileName(all.subarray(i * PROFILE_NAME_SIZE, (i + 1) * PROFILE_NAME_SIZE), i));
    return names;
  }

  async writeProfileName(index: number, name: string): Promise<void> {
    if (!Number.isInteger(index) || index < 0 || index >= PROFILE_COUNT) throw new RangeError(`profile index ${index}`);
    const bytes = encodeProfileName(name);
    await this.writeRam(REGIONS.config_name.address + index * PROFILE_NAME_SIZE, bytes);
    this.cache.config_name?.set(bytes, index * PROFILE_NAME_SIZE);
  }

  // ------------------------------------------------------------ configuration

  private get hasAdvanced(): boolean {
    return !!this.info?.features.advancedKeys;
  }

  async readConfig(): Promise<ProfileConfig> {
    const names = this.hasAdvanced ? [...PROFILE_REGIONS, ...ADVANCED_REGIONS] : PROFILE_REGIONS;
    const images = await this.readRegions(names);
    Object.assign(this.cache, images);
    return decodeProfile(images);
  }

  async writeConfig(config: ProfileConfig): Promise<number> {
    const images = encodeProfile(config);
    if (!this.hasAdvanced) {
      delete images.bind_key_mode;
      delete images.bind_key_setting;
    }
    return this.writeImages(images);
  }

  // ------------------------------------------------------------ calibration

  async readCalibration(): Promise<Calibration> {
    const images = await this.readRegions(CALIBRATION_REGIONS);
    Object.assign(this.cache, images);
    return decodeCalibration(images);
  }

  async writeCalibration(calibration: Calibration): Promise<number> {
    return this.writeImages(encodeCalibration(calibration));
  }

  async setCalibrationMode(on: boolean): Promise<void> {
    await this.sendRaw(encodeCalibrationCmd(on));
  }

  // ------------------------------------------------------------ live stream

  startStream(listener: (sample: LiveSample) => void): () => void {
    this.streamListeners.add(listener);
    if (!this.heartbeat) {
      const beat = () => void this.sendRaw(encodeHeartbeat()).catch(() => undefined);
      beat();
      this.heartbeat = setInterval(beat, this.opts.heartbeatMs);
    }
    return () => {
      this.streamListeners.delete(listener);
      if (this.streamListeners.size === 0) this.stopHeartbeat();
    };
  }

  private stopHeartbeat(): void {
    if (this.heartbeat) clearInterval(this.heartbeat);
    this.heartbeat = null;
  }

  // ------------------------------------------------------------ device actions

  async save(): Promise<void> {
    await this.exclusive(async () => {
      await this.sendRaw(encodeSave());
      await sleep(this.opts.saveDelayMs);
    });
  }

  async resetConfig(): Promise<void> {
    await this.exclusive(async () => {
      await this.sendRaw(encodeResetConfig());
      this.invalidateProfileCache();
      await sleep(this.opts.resetDelayMs);
    });
  }

  async reboot(): Promise<void> {
    await this.sendAndLeave(encodeReboot());
  }

  async enterBootloader(): Promise<void> {
    await this.sendAndLeave(encodeBootloader());
  }

  /** Send a command after which the device drops off the bus; that disconnect is expected, not an event. */
  private async sendAndLeave(report: Uint8Array): Promise<void> {
    if (!this.connected) throw new ProtocolError("not connected");
    this.connected = false;
    this.stats.sent++;
    try {
      await this.transport.send(report);
    } finally {
      await this.disconnect();
    }
  }
}
