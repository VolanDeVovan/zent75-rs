/**
 * In-memory ZENT75-HE: a Transport that behaves like the stock firmware, for tests and for
 * trying the panel without a keyboard (`?mock=1`).
 *
 * Emulated: global + per-profile memory with a separate "flash" copy, BE 03 CRC check and
 * ack, BE 04 / BE 01 reads, BE C0 profile switch with the pushed notice, BE 02 save,
 * BE EE reset, BE FF reboot, BE FC bootloader, BE FE calibration mode, BE 1D serial and the
 * BE FD stream (both variants) that runs while heartbeats keep arriving.
 */
import { DEFAULT_KEYMAP, KEY_COUNT, LED_COUNT } from "../protocol/layout";
import { Emitter, type DisconnectListener, type ReportListener, type Transport, type TransportInfo } from "../protocol/transport";
import {
  Cmd, REPORT_SIZE, decodeHostRequest, encodeProfileNotice, encodeReadReply, encodeSerialReply, encodeStreamRaw6,
  encodeStreamTravel, encodeWriteAck,
} from "../protocol/zent/frames";
import {
  ADVANCED_KEYS_MIN_VERSION, BIND_RECORD_SIZE, REGIONS, emptyAdvancedKey, encodeAdvancedKey, encodeVersion, packI16, packU16,
  u16le, i16le,
} from "../protocol/zent/memory";
import { switchType } from "../protocol/keycodes";

const GLOBAL_BASE = 0x80070000;
const GLOBAL_SIZE = 0x400;
const PROFILE_BASE = 0x80080000;
const PROFILE_SIZE = 0x2000;

export interface MockOptions {
  version: [number, number, number];
  serial: Uint8Array;
  productName: string;
  /** ms between stream frames */
  streamIntervalMs: number;
  /** the stream stops this long after the last heartbeat */
  streamHoldMs: number;
  /** delay before the profile notice is pushed */
  switchDelayMs: number;
  /** animate key presses so the live view has something to show */
  demo: boolean;
  /** deterministic noise seed */
  seed: number;
}

const DEFAULTS: MockOptions = {
  version: [0, 0, 18],
  serial: Uint8Array.from({ length: 16 }, (_, i) => 0xa0 + i),
  productName: "ZENT  75 HE (mock)",
  streamIntervalMs: 16,
  streamHoldMs: 2500,
  switchDelayMs: 30,
  demo: false,
  seed: 1,
};

function put(mem: Uint8Array, base: number, address: number, data: ArrayLike<number>): void {
  mem.set(data as ArrayLike<number>, address - base);
}

export class MockDevice implements Transport {
  readonly info: TransportInfo;
  isOpen = false;

  /** counters and switches for tests */
  readonly log = { requests: 0, writes: 0, crcErrors: 0, saves: 0, heartbeats: 0, unknown: 0, bytesWritten: 0 };
  /** drop this many upcoming replies (to exercise retries) */
  dropReplies = 0;
  /** corrupt the CRC of this many upcoming read replies */
  corruptReplies = 0;
  state: "app" | "bootloader" = "app";
  calibrationMode = false;

  private readonly opts: MockOptions;
  private reports = new Emitter<Uint8Array>();
  private disconnects = new Emitter<unknown>();
  private global = new Uint8Array(GLOBAL_SIZE);
  private profile = new Uint8Array(PROFILE_SIZE);
  private flashGlobal = new Uint8Array(GLOBAL_SIZE);
  private flashProfiles: Uint8Array[] = [];
  private streamTimer: ReturnType<typeof setInterval> | null = null;
  private lastHeartbeat = -Infinity;
  private clock = 0;
  private rng: number;
  /** physical state of each key: 0 = released, 1 = bottomed out */
  readonly pressed = new Float32Array(KEY_COUNT);
  /** what the sensors really output at rest / at the bottom (differs a bit from the stored calibration) */
  readonly sensorTop = new Uint16Array(KEY_COUNT);
  readonly sensorBottom = new Uint16Array(KEY_COUNT);

  constructor(options: Partial<MockOptions> = {}) {
    this.opts = { ...DEFAULTS, ...options };
    this.rng = this.opts.seed >>> 0 || 1;
    this.info = { productName: this.opts.productName, vendorId: 0x36b0, productId: 0x357e, backend: "mock" };
    for (let k = 0; k < KEY_COUNT; k++) {
      this.sensorTop[k] = 30000 + Math.floor(this.random() * 800) - 400;
      this.sensorBottom[k] = 18000 + Math.floor(this.random() * 800) - 400;
    }
    this.factoryGlobal();
    for (let p = 0; p < 4; p++) this.flashProfiles.push(this.factoryProfile());
    this.flashGlobal.set(this.global);
    this.profile.set(this.flashProfiles[0]);
  }

  // ------------------------------------------------------------ Transport

  async open(): Promise<void> {
    if (this.state === "bootloader") throw new Error("mock device is in its bootloader (call replug())");
    this.isOpen = true;
  }

  async close(): Promise<void> {
    this.isOpen = false;
    this.stopStream();
  }

  async send(report: Uint8Array): Promise<void> {
    if (!this.isOpen) throw new Error("mock device is not open");
    if (report.length !== REPORT_SIZE) throw new Error(`report must be ${REPORT_SIZE} bytes, got ${report.length}`);
    const copy = report.slice();
    queueMicrotask(() => this.handle(copy));
  }

  onReport(listener: ReportListener): () => void {
    return this.reports.on(listener);
  }

  onDisconnect(listener: DisconnectListener): () => void {
    return this.disconnects.on(listener);
  }

  // ------------------------------------------------------------ test / demo controls

  /** Simulate pulling and re-inserting the cable: back to the application, RAM reloaded from flash. */
  replug(): void {
    this.state = "app";
    this.loadFromFlash();
  }

  /** Set how far a key is pressed, 0..1. */
  press(key: number, amount = 1): void {
    this.pressed[key] = Math.min(1, Math.max(0, amount));
  }

  get activeProfile(): number {
    return this.global[REGIONS.cur_param_num.address - GLOBAL_BASE];
  }

  /** Direct view of emulated RAM, for assertions. */
  peek(address: number, length: number): Uint8Array {
    const m = this.resolve(address, length);
    if (!m) throw new RangeError("address outside the mock memory");
    return m.mem.slice(m.offset, m.offset + length);
  }

  /** Direct view of the emulated flash copy of profile `p` (or the global block). */
  peekFlash(address: number, length: number, profile = this.activeProfile): Uint8Array {
    if (address >= PROFILE_BASE) return this.flashProfiles[profile].slice(address - PROFILE_BASE, address - PROFILE_BASE + length);
    return this.flashGlobal.slice(address - GLOBAL_BASE, address - GLOBAL_BASE + length);
  }

  /** Push the notice the board sends when the user switches profile with an Fn shortcut. */
  userSwitchesProfile(index: number): void {
    this.switchProfile(index);
  }

  /** Produce one stream frame right now (tests do not want to wait for timers). */
  emitStreamFrame(): void {
    this.clock += this.opts.streamIntervalMs;
    if (this.opts.demo) this.animate();
    const scan = () => {
      const raw = new Array<number>(KEY_COUNT);
      for (let k = 0; k < KEY_COUNT; k++) {
        const v = this.sensorTop[k] - (this.sensorTop[k] - this.sensorBottom[k]) * this.pressed[k];
        raw[k] = Math.round(v + (this.random() - 0.5) * 12);
      }
      return raw;
    };
    if (this.calibrationMode) {
      this.emit(encodeStreamRaw6([scan(), scan(), scan(), scan(), scan(), scan()]));
      return;
    }
    const raw = scan();
    const top = u16le(this.peek(REGIONS.top_value.address, REGIONS.top_value.length));
    const bot = u16le(this.peek(REGIONS.bot_value.address, REGIONS.bot_value.length));
    const types = i16le(this.peek(REGIONS.keytype_code.address, REGIONS.keytype_code.length));
    const travel = raw.map((v, k) => {
      const span = top[k] - bot[k];
      if (span <= 0) return 0;
      const f = Math.min(1, Math.max(0, (top[k] - v) / span));
      const um = Math.round(f * switchType(types[k]).travelUm);
      return um < 20 ? 0 : um;
    });
    this.emit(encodeStreamTravel(raw, travel));
  }

  // ------------------------------------------------------------ firmware behaviour

  private random(): number {
    // xorshift32
    let x = this.rng;
    x ^= x << 13; x >>>= 0;
    x ^= x >>> 17;
    x ^= x << 5; x >>>= 0;
    this.rng = x;
    return x / 0x100000000;
  }

  private factoryGlobal(): void {
    const g = this.global;
    g.fill(0);
    put(g, GLOBAL_BASE, REGIONS.top_value.address, packU16(new Array(KEY_COUNT).fill(31000)));
    put(g, GLOBAL_BASE, REGIONS.bot_value.address, packU16(new Array(KEY_COUNT).fill(17000)));
    put(g, GLOBAL_BASE, REGIONS.keytype_code.address, packI16(new Array(KEY_COUNT).fill(2)));
    put(g, GLOBAL_BASE, REGIONS.soft_version.address, encodeVersion(...this.opts.version));
  }

  private factoryProfile(): Uint8Array {
    const p = new Uint8Array(PROFILE_SIZE);
    const all = (v: number) => new Array<number>(KEY_COUNT).fill(v);
    const fn = all(1000);
    fn[0] = 319; fn[28] = 315; fn[43] = 313; fn[71] = 311; fn[81] = 312; fn[82] = 317; fn[80] = 318; fn[74] = 316;
    put(p, PROFILE_BASE, REGIONS.key_map.address, packI16(DEFAULT_KEYMAP));
    put(p, PROFILE_BASE, REGIONS.key_map_fn.address, packI16(fn));
    put(p, PROFILE_BASE, REGIONS.press_th.address, packU16(all(300)));
    put(p, PROFILE_BASE, REGIONS.return_th.address, packU16(all(300)));
    put(p, PROFILE_BASE, REGIONS.press_deadzone.address, packU16(all(100)));
    put(p, PROFILE_BASE, REGIONS.return_deadzone.address, packU16(all(100)));
    put(p, PROFILE_BASE, REGIONS.norm_press_th.address, packU16(all(1600)));
    put(p, PROFILE_BASE, REGIONS.norm_return_th.address, packU16(all(800)));
    put(p, PROFILE_BASE, REGIONS.key_mode.address, new Array(KEY_COUNT).fill(0));
    put(p, PROFILE_BASE, REGIONS.key_cancel_code.address, packI16(all(1000)));
    const rgb = new Uint8Array(3 * LED_COUNT);
    for (let i = 0; i < LED_COUNT; i++) rgb.set([0, 200, 255], 3 * i);
    put(p, PROFILE_BASE, REGIONS.rgb.address, rgb);
    put(p, PROFILE_BASE, REGIONS.rgb_config.address, [1 + 4 * 3, 188, 255, 10, 50]);
    put(p, PROFILE_BASE, REGIONS.rgb_enable.address, [1]);
    put(p, PROFILE_BASE, REGIONS.cap_enable.address, [1]);
    put(p, PROFILE_BASE, REGIONS.rgb_fn_enable.address, [1]);
    put(p, PROFILE_BASE, REGIONS.sleep_second_cnt.address, [600 & 0xff, 600 >> 8, 0, 0]);
    put(p, PROFILE_BASE, REGIONS.auto_calib_enable.address, [1]);
    if (this.hasAdvanced) {
      const rec = encodeAdvancedKey(emptyAdvancedKey());
      for (let k = 0; k < KEY_COUNT; k++) put(p, PROFILE_BASE, REGIONS.bind_key_setting.address + k * BIND_RECORD_SIZE, rec);
    }
    return p;
  }

  private get hasAdvanced(): boolean {
    const [major, minor, patch] = this.opts.version;
    return (major << 16) + (minor << 8) + patch >= ADVANCED_KEYS_MIN_VERSION;
  }

  private loadFromFlash(): void {
    this.global.set(this.flashGlobal);
    this.profile.set(this.flashProfiles[this.activeProfile]);
    this.calibrationMode = false;
    this.stopStream();
  }

  private resolve(address: number, length: number): { mem: Uint8Array; offset: number } | null {
    if (length < 1) return null;
    if (address >= GLOBAL_BASE && address + length <= GLOBAL_BASE + GLOBAL_SIZE) return { mem: this.global, offset: address - GLOBAL_BASE };
    if (address >= PROFILE_BASE && address + length <= PROFILE_BASE + PROFILE_SIZE) {
      // firmware older than 0.0.18 has nothing behind the bind_key regions
      if (!this.hasAdvanced && address + length > REGIONS.bind_key_setting.address) return null;
      return { mem: this.profile, offset: address - PROFILE_BASE };
    }
    return null;
  }

  private emit(report: Uint8Array): void {
    if (!this.isOpen) return;
    this.reports.emit(report);
  }

  private reply(report: Uint8Array): void {
    if (this.dropReplies > 0) {
      this.dropReplies--;
      return;
    }
    this.emit(report);
  }

  private handle(report: Uint8Array): void {
    if (!this.isOpen || this.state !== "app") return;
    const req = decodeHostRequest(report);
    if (!req) return;
    this.log.requests++;
    switch (req.type) {
      case "heartbeat":
        this.log.heartbeats++;
        this.lastHeartbeat = Date.now();
        this.startStream();
        break;
      case "read": {
        const m = this.resolve(req.address, req.length);
        if (!m) break; // the real firmware does not answer (or worse); the host times out
        let src = m.mem;
        if (req.source === "flash") src = m.mem === this.global ? this.flashGlobal : this.flashProfiles[this.activeProfile];
        const out = encodeReadReply(req.source === "ram" ? Cmd.ReadRam : Cmd.ReadFlash, req.address, src.subarray(m.offset, m.offset + req.length));
        if (this.corruptReplies > 0) {
          this.corruptReplies--;
          out[10] ^= 0xff;
        }
        this.reply(out);
        break;
      }
      case "write": {
        if (!req.crcOk) {
          this.log.crcErrors++;
          break; // no ack
        }
        const m = this.resolve(req.address, req.data.length);
        if (!m) break;
        const readOnly = [REGIONS.soft_version, REGIONS.cur_param_num].some(
          (r) => req.address < r.address + r.length && req.address + req.data.length > r.address,
        );
        if (!readOnly) m.mem.set(req.data, m.offset);
        this.log.writes++;
        this.log.bytesWritten += req.data.length;
        this.reply(encodeWriteAck());
        break;
      }
      case "serial":
        this.reply(encodeSerialReply(this.opts.serial));
        break;
      case "switchProfile":
        if (req.index < 4) setTimeout(() => this.switchProfile(req.index), this.opts.switchDelayMs);
        break;
      case "save":
        this.log.saves++;
        this.flashGlobal.set(this.global);
        this.flashProfiles[this.activeProfile].set(this.profile);
        break;
      case "resetConfig":
        this.profile.set(this.factoryProfile());
        break;
      case "calibration":
        this.calibrationMode = req.on;
        break;
      case "reboot":
        this.loadFromFlash();
        this.dropConnection("reboot");
        break;
      case "bootloader":
        this.state = "bootloader";
        this.dropConnection("bootloader");
        break;
      default:
        this.log.unknown++;
    }
  }

  private dropConnection(reason: string): void {
    this.stopStream();
    this.isOpen = false;
    this.disconnects.emit(reason);
  }

  private switchProfile(index: number): void {
    // unsaved RAM edits of the old profile are discarded, like on the real board
    const o = REGIONS.cur_param_num.address - GLOBAL_BASE;
    this.global[o] = index;
    this.flashGlobal[o] = index;
    this.profile.set(this.flashProfiles[index]);
    this.emit(encodeProfileNotice(index));
  }

  private startStream(): void {
    if (this.streamTimer || this.opts.streamIntervalMs <= 0) return;
    this.streamTimer = setInterval(() => {
      if (Date.now() - this.lastHeartbeat > this.opts.streamHoldMs) {
        this.stopStream();
        return;
      }
      this.emitStreamFrame();
    }, this.opts.streamIntervalMs);
  }

  private stopStream(): void {
    if (this.streamTimer) clearInterval(this.streamTimer);
    this.streamTimer = null;
  }

  /** Demo animation: a slow wave of key presses plus the occasional full keystroke. */
  private animate(): void {
    const t = this.clock / 1000;
    for (let k = 0; k < KEY_COUNT; k++) {
      const phase = t * 1.3 - k * 0.19;
      const wave = Math.max(0, Math.sin(phase)) ** 6;
      this.pressed[k] = wave;
    }
  }
}
