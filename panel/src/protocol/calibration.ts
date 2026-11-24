/**
 * Manual calibration, following the vendor driver's rules (see PROTOCOL-NOTES.md):
 *
 *  - the board is put into calibration mode and streams raw scans;
 *  - the first frame decides the sensor polarity of each key (rest value above or below 32768);
 *  - until any key has moved by more than MOVE_THRESHOLD counts, both extremes of every key are
 *    tracked; after that the rest-side extreme is frozen, so the rest value is the one seen
 *    while nobody touched the board, and only the pressed-side extreme keeps following;
 *  - a key counts as calibrated once its span exceeds the minimum for its switch type;
 *  - on finish only calibrated keys get new top/bottom values, the others keep the old ones.
 */
import type { Calibration, KeyboardProtocol, LiveSample } from "./keyboard";
import { switchType } from "./keycodes";

export const MOVE_THRESHOLD = 3000;
export const POLARITY_MIDPOINT = 32768;

export class CalibrationSession {
  readonly keyCount: number;
  readonly min: number[];
  readonly max: number[];
  /** true: value rises when pressed (rest value was above the midpoint in the vendor's terms "direction 0") */
  readonly restIsHigh: boolean[];
  readonly accepted: boolean[];
  frames = 0;
  private restFrozen = false;

  constructor(private readonly before: Calibration) {
    this.keyCount = before.top.length;
    this.min = new Array(this.keyCount).fill(0xffff);
    this.max = new Array(this.keyCount).fill(0);
    this.restIsHigh = new Array(this.keyCount).fill(false);
    this.accepted = new Array(this.keyCount).fill(false);
  }

  /** Minimum raw span a key must cover to be accepted. */
  requiredSpan(key: number): number {
    return switchType(this.before.switchType[key]).calibSpan;
  }

  span(key: number): number {
    return Math.max(0, this.max[key] - this.min[key]);
  }

  get acceptedCount(): number {
    return this.accepted.filter(Boolean).length;
  }

  /** Feed one raw scan (per-key mean of a calibration-mode frame). */
  feed(raw: ArrayLike<number>): void {
    if (raw.length !== this.keyCount) throw new RangeError(`expected ${this.keyCount} raw values`);
    if (this.frames === 0) {
      // "direction 0" in the vendor code: first value above the midpoint
      for (let k = 0; k < this.keyCount; k++) this.restIsHigh[k] = raw[k] > POLARITY_MIDPOINT;
    }
    this.frames++;
    for (let k = 0; k < this.keyCount; k++) {
      if (this.max[k] - this.min[k] > MOVE_THRESHOLD) this.restFrozen = true;
      const v = Math.floor(raw[k]);
      if (this.restIsHigh[k]) {
        // vendor: max always follows, min only until something moved; top := min, bottom := max
        if (v > this.max[k]) this.max[k] = v;
        if (v < this.min[k] && !this.restFrozen) this.min[k] = v;
      } else {
        // the ZENT75-HE case: rest ~30000, bottom ~18000; top := max, bottom := min
        if (v > this.max[k] && !this.restFrozen) this.max[k] = v;
        if (v < this.min[k]) this.min[k] = v;
      }
      if (this.max[k] - this.min[k] > this.requiredSpan(k)) this.accepted[k] = true;
    }
  }

  /** Calibration to write: accepted keys take the measured values, the rest stay as they were. */
  result(): Calibration {
    const top = [...this.before.top];
    const bottom = [...this.before.bottom];
    for (let k = 0; k < this.keyCount; k++) {
      if (!this.accepted[k]) continue;
      if (this.restIsHigh[k]) {
        top[k] = this.min[k];
        bottom[k] = this.max[k];
      } else {
        top[k] = this.max[k];
        bottom[k] = this.min[k];
      }
    }
    return { top, bottom, switchType: [...this.before.switchType] };
  }
}


const wait = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/**
 * The whole manual calibration procedure on top of any KeyboardProtocol:
 * read old values -> calibration mode on -> collect raw scans -> calibration mode off ->
 * write top/bottom -> save to flash.
 */
export class CalibrationFlow {
  session: CalibrationSession | null = null;
  private stopStream: (() => void) | null = null;

  constructor(private readonly kb: KeyboardProtocol, private readonly settleMs = 100) {}

  get running(): boolean {
    return this.session !== null;
  }

  async start(onProgress?: (session: CalibrationSession, sample: LiveSample) => void): Promise<CalibrationSession> {
    if (this.session) throw new Error("calibration is already running");
    const before = await this.kb.readCalibration();
    const session = new CalibrationSession(before);
    this.session = session;
    await this.kb.setCalibrationMode(true);
    this.stopStream = this.kb.startStream((sample) => {
      // travel frames still in flight from before the mode switch are ignored
      if (sample.kind !== "raw" || this.session !== session) return;
      session.feed(sample.raw);
      onProgress?.(session, sample);
    });
    return session;
  }

  /** Leave calibration mode without touching the stored values. */
  async cancel(): Promise<void> {
    const had = this.session;
    this.session = null;
    this.stopStream?.();
    this.stopStream = null;
    if (had) await this.kb.setCalibrationMode(false);
  }

  /** Leave calibration mode, write the accepted keys and save. Returns the indices that were calibrated. */
  async finish(): Promise<number[]> {
    const session = this.session;
    if (!session) throw new Error("calibration is not running");
    const accepted = session.accepted.flatMap((ok, k) => (ok ? [k] : []));
    this.session = null;
    this.stopStream?.();
    this.stopStream = null;
    await this.kb.setCalibrationMode(false);
    if (accepted.length === 0) return [];
    await wait(this.settleMs);
    await this.kb.writeCalibration(session.result());
    await this.kb.save();
    return accepted;
  }
}
