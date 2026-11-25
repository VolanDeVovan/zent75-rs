/**
 * Application state and every operation on the keyboard. No DOM in here: the shell subscribes
 * and renders, the tests drive it against the mock. The keyboard is reached only through
 * KeyboardProtocol.
 *
 * Save model: the keyboard applies profile edits to RAM at once and keeps them across a replug
 * only after save(). `saved` is the profile as it was when last read after connect, save or a
 * profile switch; `dirty` is a comparison against it. Board-global data (profile names, switch
 * types, calibration, custom switch curves) is stored by the firmware on every write and is not
 * part of that state.
 */
import { CalibrationFlow } from "../protocol/calibration";
import {
  cloneConfig, type Calibration, type CustomSwitch, type DeviceInfo, type KeyboardProtocol, type LiveSample, type ProfileConfig,
} from "../protocol/keyboard";
import type { Transport } from "../protocol/transport";
import { ZentStockProtocol } from "../protocol/zent/protocol";
import { changedAreas } from "./model";

export type ToastKind = "info" | "ok" | "error";

/** Chooses the protocol for a transport. A second protocol (our own firmware) plugs in here. */
export function createProtocol(transport: Transport): KeyboardProtocol {
  return new ZentStockProtocol(transport);
}

interface HistoryEntry { label: string; config: ProfileConfig }

const HISTORY_LIMIT = 50;
const same = (a: unknown, b: unknown): boolean => JSON.stringify(a) === JSON.stringify(b);

export class Store {
  kb: KeyboardProtocol | null = null;
  info: DeviceInfo | null = null;
  backend = "";
  /** the active profile as the keyboard's RAM holds it */
  config: ProfileConfig | null = null;
  /** the active profile as flash holds it, as far as the panel knows (see the file comment) */
  saved: ProfileConfig | null = null;
  calibration: Calibration | null = null;
  /** board-global custom switch records */
  customSwitches: CustomSwitch[] | null = null;
  profileNames: string[] = [];
  activeProfile = 0;
  /** label of the running device operation */
  busy: string | null = null;
  calibrationFlow: CalibrationFlow | null = null;

  private history: HistoryEntry[] = [];
  private queue: Promise<unknown> = Promise.resolve();
  private stopLive: (() => void) | null = null;
  private liveListeners = new Set<(s: LiveSample) => void>();
  private unsub: (() => void)[] = [];
  private changeListeners = new Set<() => void>();
  private busyListeners = new Set<() => void>();
  private toastListeners = new Set<(kind: ToastKind, text: string) => void>();

  /** `protocolFor` is replaceable so that tests can shorten the protocol's delays. */
  constructor(private readonly protocolFor: (transport: Transport) => KeyboardProtocol = createProtocol) {}

  // ------------------------------------------------------------ events

  /** State changed: render everything. */
  onChange(listener: () => void): () => void {
    this.changeListeners.add(listener);
    return () => this.changeListeners.delete(listener);
  }

  /** An operation started: only the status needs repainting. */
  onBusy(listener: () => void): () => void {
    this.busyListeners.add(listener);
    return () => this.busyListeners.delete(listener);
  }

  onToast(listener: (kind: ToastKind, text: string) => void): () => void {
    this.toastListeners.add(listener);
    return () => this.toastListeners.delete(listener);
  }

  toast(kind: ToastKind, text: string): void {
    for (const l of [...this.toastListeners]) l(kind, text);
  }

  changed(): void {
    for (const l of [...this.changeListeners]) l();
  }

  // ------------------------------------------------------------ derived state

  get connected(): boolean {
    return !!this.kb && !!this.info && !!this.config;
  }

  /** RAM differs from the saved profile. */
  get dirty(): boolean {
    return !!this.config && !!this.saved && !same(this.config, this.saved);
  }

  /** Human names of what differs from the saved profile, e.g. ["actuation", "lighting"]. */
  get unsavedAreas(): string[] {
    return this.config && this.saved ? changedAreas(this.config, this.saved) : [];
  }

  get canUndo(): boolean {
    return this.history.length > 0;
  }

  /** What the next undo takes back. */
  get undoLabel(): string | null {
    return this.history.at(-1)?.label ?? null;
  }

  get profileCount(): number {
    return this.info?.profileCount ?? this.profileNames.length;
  }

  // ------------------------------------------------------------ connection

  async attach(transport: Transport): Promise<void> {
    await this.run("Connecting", async () => {
      const kb = this.protocolFor(transport);
      this.info = await kb.connect();
      this.kb = kb;
      this.backend = transport.info.backend;
      this.unsub.push(kb.on("disconnect", () => this.handleDisconnect()));
      this.unsub.push(kb.on("profile", (p) => this.handleProfileNotice(p)));
      await this.loadAll();
    });
  }

  async disconnect(): Promise<void> {
    await this.leaveLiveModes();
    const kb = this.kb;
    this.reset();
    await kb?.disconnect();
    this.changed();
  }

  private reset(): void {
    for (const u of this.unsub.splice(0)) u();
    this.kb = null;
    this.info = null;
    this.config = null;
    this.saved = null;
    this.calibration = null;
    this.customSwitches = null;
    this.history = [];
    this.stopLive = null;
    this.liveListeners.clear();
    this.calibrationFlow = null;
  }

  private handleDisconnect(): void {
    this.reset();
    this.toast("error", "The keyboard disconnected.");
    this.changed();
  }

  private handleProfileNotice(index: number): void {
    if (index === this.activeProfile || this.busy) return;
    this.toast("info", `The keyboard switched to profile ${index + 1}.`);
    void this.run("Loading profile", async () => {
      this.activeProfile = index;
      await this.loadProfile();
    });
  }

  private async loadProfile(): Promise<void> {
    this.config = await this.kb!.readConfig();
    this.saved = cloneConfig(this.config);
    this.history = [];
  }

  private async loadAll(): Promise<void> {
    const kb = this.kb!;
    this.activeProfile = await kb.getActiveProfile();
    this.profileNames = await kb.readProfileNames();
    this.calibration = await kb.readCalibration();
    this.customSwitches = await kb.readCustomSwitches();
    await this.loadProfile();
  }

  /** Read everything again. RAM is then taken as the saved state. */
  reload(): Promise<unknown> {
    return this.run("Reading the keyboard", () => this.loadAll());
  }

  // ------------------------------------------------------------ operations

  /** Serialise device operations; failures become a toast and resolve to undefined. */
  run<T>(label: string, fn: () => Promise<T>): Promise<T | undefined> {
    const job = this.queue.then(async () => {
      this.busy = label;
      for (const l of [...this.busyListeners]) l();
      try {
        return await fn();
      } catch (e) {
        console.error(e);
        this.toast("error", `${label} failed: ${(e as Error).message}`);
        return undefined;
      } finally {
        this.busy = null;
        this.changed();
      }
    });
    this.queue = job.catch(() => undefined);
    return job;
  }

  private remember(label: string, config: ProfileConfig): void {
    this.history.push({ label, config });
    if (this.history.length > HISTORY_LIMIT) this.history.shift();
  }

  /** Write `next` to RAM; on failure re-read what the keyboard really holds. */
  private async write(next: ProfileConfig): Promise<number> {
    try {
      const written = await this.kb!.writeConfig(next);
      this.config = next;
      return written;
    } catch (e) {
      this.config = await this.kb!.readConfig().catch(() => this.config);
      throw e;
    }
  }

  /**
   * Change the active profile. Applied to the keyboard's RAM at once; `label` names the edit
   * for Undo ("actuation", "lighting effect"). The mutation runs when its turn in the queue
   * comes, on the then-current config, so quick successive edits do not overwrite each other.
   */
  edit(label: string, mutate: (draft: ProfileConfig) => void): Promise<unknown> {
    if (!this.kb || !this.config) return Promise.resolve();
    return this.run(`Writing ${label}`, async () => {
      const before = this.config!;
      const next = cloneConfig(before);
      mutate(next);
      if (same(next, before)) return;
      await this.write(next);
      this.remember(label, before);
    });
  }

  /** Take back the last edit (it stays unsaved until save()). */
  undo(): Promise<unknown> {
    if (!this.canUndo) return Promise.resolve();
    return this.run("Undoing", async () => {
      const entry = this.history.pop();
      if (!entry) return;
      try {
        await this.write(entry.config);
      } catch (e) {
        this.history.push(entry);
        throw e;
      }
      this.toast("info", `Undid: ${entry.label}.`);
    });
  }

  /** Put the saved profile back into RAM. Undoable. */
  discard(): Promise<unknown> {
    if (!this.dirty) return Promise.resolve();
    return this.run("Discarding changes", async () => {
      const before = this.config!;
      await this.write(cloneConfig(this.saved!));
      this.remember("discard", before);
      this.toast("info", "Unsaved changes discarded.");
    });
  }

  save(): Promise<unknown> {
    if (!this.kb) return Promise.resolve();
    return this.run("Saving", async () => {
      await this.kb!.save();
      this.saved = cloneConfig(this.config!);
      this.toast("ok", "Saved to the keyboard.");
    });
  }

  /** Switch profile. The keyboard drops unsaved RAM edits; ask the user before calling this. */
  async switchProfile(index: number): Promise<void> {
    if (index === this.activeProfile || !this.kb) return;
    await this.leaveLiveModes();
    await this.run("Switching profile", async () => {
      await this.kb!.setActiveProfile(index);
      this.activeProfile = index;
      await this.loadProfile();
    });
  }

  /**
   * Factory defaults for the active profile. The real firmware stores the reset at once (the
   * mock only resets RAM), so the panel saves explicitly and both end up the same. The previous
   * profile stays in the undo history.
   */
  resetProfile(): Promise<unknown> {
    if (!this.kb) return Promise.resolve();
    return this.run("Resetting profile", async () => {
      const before = this.config!;
      await this.kb!.resetConfig();
      this.config = await this.kb!.readConfig();
      await this.kb!.save();
      this.saved = cloneConfig(this.config);
      this.remember("profile reset", before);
      this.toast("ok", `Profile ${this.activeProfile + 1} is back to factory defaults. Undo, then Save, restores the old one.`);
    });
  }

  /** Board-global, stored by the firmware at once. */
  renameProfile(index: number, name: string): Promise<unknown> {
    if (!this.kb) return Promise.resolve();
    return this.run("Renaming profile", async () => {
      await this.kb!.writeProfileName(index, name);
      this.profileNames = await this.kb!.readProfileNames();
    });
  }

  /** Switch types (and calibration values): board-global, stored by the firmware at once. */
  editCalibration(label: string, mutate: (draft: Calibration) => void): Promise<unknown> {
    if (!this.kb || !this.calibration) return Promise.resolve();
    return this.run(`Writing ${label}`, async () => {
      const next = cloneConfig(this.calibration!);
      mutate(next);
      await this.kb!.writeCalibration(next);
      this.calibration = next;
    });
  }

  /** Custom switch records live in bank 1: the firmware saves them to flash on every write. */
  editCustomSwitches(label: string, mutate: (draft: CustomSwitch[]) => void): Promise<unknown> {
    if (!this.kb || !this.customSwitches) return Promise.resolve();
    return this.run(`Writing ${label}`, async () => {
      const next = cloneConfig(this.customSwitches!);
      mutate(next);
      try {
        await this.kb!.writeCustomSwitches(next);
        this.customSwitches = next;
      } catch (e) {
        this.customSwitches = await this.kb!.readCustomSwitches().catch(() => this.customSwitches);
        throw e;
      }
    });
  }

  /**
   * After something outside edit() rewrote the keyboard (import): read everything again.
   * `savedToFlash` tells whether RAM is now also what flash holds.
   */
  async refreshAfterBulkWrite(label: string, savedToFlash: boolean): Promise<void> {
    const before = this.config;
    const kb = this.kb!;
    this.activeProfile = await kb.getActiveProfile();
    this.profileNames = await kb.readProfileNames();
    this.calibration = await kb.readCalibration();
    this.customSwitches = await kb.readCustomSwitches();
    this.config = await kb.readConfig();
    if (savedToFlash || !this.saved) this.saved = cloneConfig(this.config);
    if (before && !same(before, this.config)) this.remember(label, before);
  }

  // ------------------------------------------------------------ calibration

  startCalibration(onProgress: Parameters<CalibrationFlow["start"]>[0]): Promise<unknown> {
    return this.run("Starting calibration", async () => {
      const flow = new CalibrationFlow(this.kb!);
      await flow.start(onProgress);
      this.calibrationFlow = flow;
    });
  }

  /** Writes the accepted keys and saves (which also stores unsaved profile edits). Returns the calibrated key indices. */
  finishCalibration(): Promise<number[] | undefined> {
    const flow = this.calibrationFlow;
    if (!flow) return Promise.resolve(undefined);
    return this.run("Writing calibration", async () => {
      const keys = await flow.finish();
      this.calibrationFlow = null;
      this.calibration = await this.kb!.readCalibration();
      if (keys.length > 0) this.saved = cloneConfig(this.config!);
      return keys;
    });
  }

  cancelCalibration(): Promise<unknown> {
    const flow = this.calibrationFlow;
    if (!flow) return Promise.resolve();
    return this.run("Leaving calibration", async () => {
      await flow.cancel();
      this.calibrationFlow = null;
    });
  }

  // ------------------------------------------------------------ live stream

  /** Subscribe to live samples; the stream runs while there is at least one subscriber. */
  subscribeLive(listener: (s: LiveSample) => void): () => void {
    this.liveListeners.add(listener);
    if (!this.stopLive && this.kb) {
      this.stopLive = this.kb.startStream((s) => {
        for (const l of [...this.liveListeners]) l(s);
      });
    }
    return () => {
      this.liveListeners.delete(listener);
      if (this.liveListeners.size === 0) {
        this.stopLive?.();
        this.stopLive = null;
      }
    };
  }

  /** Stop streaming and abandon a running calibration (leaving a page, switching profile, disconnecting). */
  async leaveLiveModes(): Promise<void> {
    for (const l of [...this.liveListeners]) this.liveListeners.delete(l);
    this.stopLive?.();
    this.stopLive = null;
    if (this.calibrationFlow?.running) await this.calibrationFlow.cancel().catch(() => undefined);
    this.calibrationFlow = null;
  }
}
