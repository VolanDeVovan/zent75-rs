/**
 * Application state and shell. The app talks to the keyboard only through the
 * KeyboardProtocol interface; which protocol / transport is behind it is decided in connect*().
 */
import { CalibrationFlow } from "../protocol/calibration";
import {
  cloneConfig, type Calibration, type CustomSwitch, type DeviceInfo, type KeyboardProtocol, type LiveSample, type ProfileConfig,
} from "../protocol/keyboard";
import type { Transport } from "../protocol/transport";
import { ZentStockProtocol } from "../protocol/zent/protocol";
import { MockDevice } from "../transport/mock";
import { WebHidTransport, webHidSupported } from "../transport/webhid";
import { clear, confirmDialog, h } from "./dom";
import { renderKeyboard, type KeyVisual, type KeyboardView } from "./keyboard-view";
import { TABS, type TabDef, type TabId } from "./tabs";

export interface Toast { id: number; kind: "info" | "ok" | "error"; text: string }

/** Chooses the protocol for a transport. A second protocol (our own firmware) plugs in here. */
export function createProtocol(transport: Transport): KeyboardProtocol {
  return new ZentStockProtocol(transport);
}

export class App {
  kb: KeyboardProtocol | null = null;
  info: DeviceInfo | null = null;
  backend = "";
  config: ProfileConfig | null = null;
  calibration: Calibration | null = null;
  /** board-global custom switch records 0..3 */
  customSwitches: CustomSwitch[] | null = null;
  profileNames: string[] = [];
  activeProfile = 0;
  /** RAM differs from flash (edits not saved yet) */
  dirty = false;
  busy: string | null = null;
  tab: TabId = "keymap";
  selection = new Set<number>();
  /** per-tab UI state that must survive re-renders */
  ui: Record<string, any> = {};
  live: LiveSample | null = null;
  calibrationFlow: CalibrationFlow | null = null;
  keyboardView: KeyboardView | null = null;

  private toasts: Toast[] = [];
  private toastSeq = 0;
  private queue: Promise<unknown> = Promise.resolve();
  private stopLive: (() => void) | null = null;
  private liveListeners = new Set<(s: LiveSample) => void>();
  private unsub: (() => void)[] = [];

  constructor(private readonly root: HTMLElement) {}

  // ------------------------------------------------------------ connection

  async connectWebHid(): Promise<void> {
    try {
      const transport = await WebHidTransport.request();
      if (!transport) return;
      await this.attach(transport);
    } catch (e) {
      this.toast("error", `Could not connect: ${(e as Error).message}`);
    }
  }

  async connectGranted(): Promise<boolean> {
    try {
      const [transport] = await WebHidTransport.granted();
      if (!transport) return false;
      await this.attach(transport);
      return true;
    } catch {
      return false;
    }
  }

  async connectMock(version: [number, number, number] = [0, 0, 18]): Promise<void> {
    await this.attach(new MockDevice({ demo: true, version }));
  }

  private async attach(transport: Transport): Promise<void> {
    await this.run("Connecting…", async () => {
      const kb = createProtocol(transport);
      this.info = await kb.connect();
      this.kb = kb;
      this.backend = transport.info.backend;
      this.unsub.push(kb.on("disconnect", () => this.handleDisconnect()));
      this.unsub.push(kb.on("profile", (p) => this.handleProfileNotice(p)));
      await this.loadAll();
    });
    if (!this.kb) this.render();
  }

  async disconnect(): Promise<void> {
    await this.leaveLiveModes();
    const kb = this.kb;
    this.reset();
    await kb?.disconnect();
    this.render();
  }

  private reset(): void {
    for (const u of this.unsub.splice(0)) u();
    this.kb = null;
    this.info = null;
    this.config = null;
    this.calibration = null;
    this.customSwitches = null;
    this.dirty = false;
    this.selection.clear();
    this.stopLive = null;
    this.calibrationFlow = null;
    this.live = null;
  }

  private handleDisconnect(): void {
    this.reset();
    this.toast("error", "The keyboard disconnected.");
    this.render();
  }

  private handleProfileNotice(index: number): void {
    if (index === this.activeProfile || this.busy) return;
    this.toast("info", `The keyboard switched to profile ${index + 1}.`);
    void this.run("Loading profile…", async () => {
      this.activeProfile = index;
      this.dirty = false;
      this.config = await this.kb!.readConfig();
    });
  }

  private async loadAll(): Promise<void> {
    const kb = this.kb!;
    this.activeProfile = await kb.getActiveProfile();
    this.profileNames = await kb.readProfileNames();
    this.config = await kb.readConfig();
    this.calibration = await kb.readCalibration();
    this.customSwitches = await kb.readCustomSwitches();
    this.dirty = false;
  }

  // ------------------------------------------------------------ operations

  /** Serialise device operations and show a busy state while they run. */
  run<T>(label: string, fn: () => Promise<T>): Promise<T | undefined> {
    const job = this.queue.then(async () => {
      this.busy = label;
      this.renderChrome();
      try {
        return await fn();
      } catch (e) {
        console.error(e);
        this.toast("error", `${label.replace(/…$/, "")} failed: ${(e as Error).message}`);
        return undefined;
      } finally {
        this.busy = null;
        this.render();
      }
    });
    this.queue = job.catch(() => undefined);
    return job;
  }

  /** Change the active profile's config: applied to the keyboard's RAM immediately, like the vendor driver. */
  edit(label: string, mutate: (draft: ProfileConfig) => void): Promise<unknown> {
    if (!this.kb || !this.config) return Promise.resolve();
    const next = cloneConfig(this.config);
    mutate(next);
    return this.run(label, async () => {
      try {
        const written = await this.kb!.writeConfig(next);
        this.config = next;
        if (written > 0) this.dirty = true;
      } catch (e) {
        // the device may hold a half-applied change: re-read the truth
        this.config = await this.kb!.readConfig().catch(() => this.config);
        throw e;
      }
    });
  }

  editCalibration(label: string, mutate: (draft: Calibration) => void): Promise<unknown> {
    if (!this.kb || !this.calibration) return Promise.resolve();
    const next = cloneConfig(this.calibration);
    mutate(next);
    return this.run(label, async () => {
      const written = await this.kb!.writeCalibration(next);
      this.calibration = next;
      if (written > 0) this.dirty = true;
    });
  }

  /** Custom switch records live in bank 1: the firmware saves them to flash on every write. */
  editCustomSwitches(label: string, mutate: (draft: CustomSwitch[]) => void): Promise<unknown> {
    if (!this.kb || !this.customSwitches) return Promise.resolve();
    const next = cloneConfig(this.customSwitches);
    mutate(next);
    return this.run(label, async () => {
      try {
        await this.kb!.writeCustomSwitches(next);
        this.customSwitches = next;
      } catch (e) {
        this.customSwitches = await this.kb!.readCustomSwitches().catch(() => this.customSwitches);
        throw e;
      }
    });
  }

  save(): Promise<unknown> {
    return this.run("Saving to flash…", async () => {
      await this.kb!.save();
      this.dirty = false;
      this.toast("ok", "Saved to flash.");
    });
  }

  async switchProfile(index: number): Promise<void> {
    if (index === this.activeProfile || !this.kb) return;
    if (this.dirty) {
      const ok = await confirmDialog({
        title: "Unsaved changes",
        body: "Switching profiles discards changes that were not saved to flash. Switch anyway?",
        confirmLabel: "Discard and switch", danger: true,
      });
      if (!ok) return;
    }
    await this.leaveLiveModes();
    await this.run("Switching profile…", async () => {
      await this.kb!.setActiveProfile(index);
      this.activeProfile = index;
      this.config = await this.kb!.readConfig();
      this.dirty = false;
    });
  }

  reload(): Promise<unknown> {
    return this.run("Reading configuration…", () => this.loadAll());
  }

  // ------------------------------------------------------------ live stream

  /** Subscribe to live samples; the stream runs while there is at least one subscriber. */
  subscribeLive(listener: (s: LiveSample) => void): () => void {
    this.liveListeners.add(listener);
    if (!this.stopLive && this.kb) {
      this.stopLive = this.kb.startStream((s) => {
        this.live = s;
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

  /** Stop streaming / calibration when leaving a tab or the device. */
  async leaveLiveModes(): Promise<void> {
    this.ui.liveStop?.();
    this.ui.liveStop = null;
    if (this.calibrationFlow?.running) await this.calibrationFlow.cancel().catch(() => undefined);
    this.calibrationFlow = null;
  }

  async setTab(tab: TabId): Promise<void> {
    if (tab === this.tab) return;
    await this.leaveLiveModes();
    this.tab = tab;
    this.selection.clear();
    this.render();
  }

  // ------------------------------------------------------------ toasts

  toast(kind: Toast["kind"], text: string): void {
    const t = { id: ++this.toastSeq, kind, text };
    this.toasts.push(t);
    this.renderToasts();
    setTimeout(() => {
      this.toasts = this.toasts.filter((x) => x !== t);
      this.renderToasts();
    }, kind === "error" ? 9000 : 4000);
  }

  // ------------------------------------------------------------ rendering

  private elHeader!: HTMLElement;
  private elMain!: HTMLElement;
  private elToasts!: HTMLElement;

  mount(): void {
    this.elHeader = h("header", { class: "topbar" });
    this.elMain = h("main", { class: "main" });
    this.elToasts = h("div", { class: "toasts", role: "status", "aria-live": "polite" });
    clear(this.root);
    this.root.append(this.elHeader, this.elMain, this.elToasts,
      h("footer", { class: "footer" },
        "WebHID works only in Chromium-based browsers (Chrome, Edge, Brave, Opera). Firefox and Safari cannot talk to the keyboard. ",
        "Changes are applied to the keyboard's RAM at once and survive a replug only after “Save to flash”.",
      ),
    );
    this.render();
  }

  render(): void {
    this.renderChrome();
    clear(this.elMain);
    if (!this.kb || !this.config || !this.info) {
      this.elMain.appendChild(this.renderConnect());
      return;
    }
    const tab: TabDef = TABS.find((t) => t.id === this.tab) ?? TABS[0];
    const visual = (i: number): KeyVisual => tab.visual(this, i);
    this.keyboardView = renderKeyboard({
      visual,
      selected: this.selection,
      onClick: tab.onKeyClick ? (i, e) => tab.onKeyClick!(this, i, e) : undefined,
      onContextMenu: tab.onKeyContext ? (i, e) => tab.onKeyContext!(this, i, e) : undefined,
    });
    this.elMain.append(
      h("nav", { class: "tabs", role: "tablist" },
        TABS.map((t) => h("button", {
          class: t.id === this.tab ? "tab active" : "tab", role: "tab", "aria-selected": String(t.id === this.tab),
          onclick: () => void this.setTab(t.id),
        }, t.title)),
      ),
      h("section", { class: "board" },
        tab.hint ? h("p", { class: "hint" }, tab.hint) : null,
        this.keyboardView.element,
      ),
      h("section", { class: "panel" }, tab.render(this)),
    );
  }

  /** Header only (cheap; used while busy). */
  renderChrome(): void {
    clear(this.elHeader);
    const connected = !!this.kb && !!this.info;
    this.elHeader.append(
      h("div", { class: "brand" }, h("span", { class: "brand-mark" }, "Z75"), h("span", null, "ZENT75-HE panel")),
      connected
        ? h("div", { class: "profiles", role: "group", "aria-label": "Profiles" },
            this.profileNames.map((name, i) => h("button", {
              class: i === this.activeProfile ? "pill active" : "pill", disabled: !!this.busy,
              title: `Switch to profile ${i + 1}`, onclick: () => void this.switchProfile(i),
            }, h("span", { class: "pill-n" }, String(i + 1)), name)),
          )
        : h("div", { class: "profiles" }),
      h("div", { class: "status" },
        this.busy ? h("span", { class: "busy" }, this.busy) : null,
        connected ? h("span", { class: "chip" }, `fw ${this.info!.firmwareVersion}`, this.backend === "mock" ? " · mock" : "") : null,
        connected
          ? h("button", {
              class: this.dirty ? "btn primary attention" : "btn", disabled: !!this.busy,
              title: "Persist the keyboard's current RAM configuration to flash (BE 02)",
              onclick: () => void this.save(),
            }, this.dirty ? "Save to flash •" : "Save to flash")
          : null,
        connected ? h("button", { class: "btn ghost", onclick: () => void this.disconnect() }, "Disconnect") : null,
      ),
    );
  }

  private renderToasts(): void {
    clear(this.elToasts);
    for (const t of this.toasts) this.elToasts.appendChild(h("div", { class: `toast ${t.kind}` }, t.text));
  }

  private renderConnect(): HTMLElement {
    const supported = webHidSupported();
    return h("section", { class: "connect" },
      h("h1", null, "ZENT75-HE configurator"),
      h("p", { class: "lede" },
        "Keymap, actuation, rapid trigger, SOCD, DKS, lighting, profiles, calibration and live key travel for the HOVEY WORKS ZENT75-HE, straight from the browser over WebHID.",
      ),
      supported
        ? h("p", { class: "note" }, "WebHID works only in Chromium-based browsers (Chrome, Edge, Brave, Opera). You are on one of them.")
        : h("div", { class: "warning", role: "alert" },
            h("strong", null, "This browser has no WebHID."),
            " navigator.hid is missing, so the page cannot reach the keyboard. Open it in a Chromium-based browser (Chrome, Edge, Brave, Opera) over https or localhost. ",
            "You can still explore everything with the mock device.",
          ),
      h("div", { class: "connect-actions" },
        h("button", { class: "btn primary big", disabled: !supported || !!this.busy, onclick: () => void this.connectWebHid() }, "Connect keyboard"),
        h("button", { class: "btn big", disabled: !!this.busy, onclick: () => void this.connectMock() }, "Mock device"),
        h("button", { class: "btn ghost", disabled: !!this.busy, title: "Mock of the older firmware without key groups / DKS", onclick: () => void this.connectMock([0, 0, 9]) }, "Mock fw 0.0.9"),
      ),
      h("ul", { class: "facts" },
        h("li", null, "USB 36b0:357e, vendor raw-HID interface (usage page 0xFF60, usage 0x61)."),
        h("li", null, "On Linux the hidraw node must be accessible to your user (udev rule)."),
        h("li", null, "Close the vendor web driver first: only one page can hold the device."),
      ),
    );
  }
}
