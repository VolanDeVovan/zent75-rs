/**
 * The shell: top bar (profiles, save state), navigation rail, the current page, toasts.
 * State and device operations live in Store; this file renders and asks the questions
 * (confirmations) that belong in front of them. Design: docs/ux.md.
 */
import type { Calibration, CustomSwitch, DeviceInfo, ProfileConfig } from "../protocol/keyboard";
import { MockDevice } from "../transport/mock";
import { WebHidTransport, webHidSupported } from "../transport/webhid";
import { choiceDialog, clear, confirmDialog, h, popupMenu, promptDialog, type Child } from "./dom";
import { renderKeyboard, type KeyboardView, type KeyboardViewOptions } from "./keyboard-view";
import { actuationPage } from "./pages/actuation";
import { advancedPage } from "./pages/advanced";
import { devicePage, exportProfiles } from "./pages/device";
import { homePage } from "./pages/home";
import { keymapPage } from "./pages/keymap";
import { lightingPage } from "./pages/lighting";
import { sensorsPage } from "./pages/sensors";
import { socdPage } from "./pages/socd";
import { Store, type ToastKind } from "./store";

export type PageId = "home" | "actuation" | "lighting" | "socd" | "keymap" | "advanced" | "sensors" | "device";

export interface Page {
  id: PageId;
  title: string;
  /** rail group: 0 everyday, 1 less often, 2 the keyboard itself */
  group: 0 | 1 | 2;
  render(app: App): HTMLElement;
}

const PAGES: readonly Page[] = [homePage, actuationPage, lightingPage, socdPage, keymapPage, advancedPage, sensorsPage, devicePage];
const GROUP_TITLES = [null, "Less often", "Keyboard"] as const;
const THEMES = ["auto", "light", "dark"] as const;

interface Toast { id: number; kind: ToastKind; text: string }

export class App {
  readonly store = new Store();
  page: PageId = "home";
  /** keys selected on the keyboard drawing; cleared when the page changes */
  selection = new Set<number>();
  /** per-page UI state that must survive re-renders */
  ui: Record<string, any> = {};
  keyboardView: KeyboardView | null = null;

  private toasts: Toast[] = [];
  private toastSeq = 0;
  private elProfiles!: HTMLElement;
  private elSave!: HTMLElement;
  private elRail!: HTMLElement;
  private elMain!: HTMLElement;
  private elToasts!: HTMLElement;

  constructor(private readonly root: HTMLElement) {}

  // ------------------------------------------------------------ shortcuts for the pages

  get config(): ProfileConfig { return this.store.config!; }
  get calibration(): Calibration { return this.store.calibration!; }
  get customSwitches(): CustomSwitch[] | null { return this.store.customSwitches; }
  get info(): DeviceInfo { return this.store.info!; }
  get busy(): boolean { return !!this.store.busy; }

  edit(label: string, mutate: (draft: ProfileConfig) => void): Promise<unknown> {
    return this.store.edit(label, mutate);
  }

  toast(kind: ToastKind, text: string): void {
    const t = { id: ++this.toastSeq, kind, text };
    this.toasts.push(t);
    if (this.toasts.length > 4) this.toasts.shift();
    this.renderToasts();
    setTimeout(() => {
      this.toasts = this.toasts.filter((x) => x !== t);
      this.renderToasts();
    }, kind === "error" ? 9000 : 4000);
  }

  /** The keyboard drawing of a page, with an optional line of guidance under it. */
  stage(options: Omit<KeyboardViewOptions, "selected" | "onSelect"> & { hint?: Child; onSelect?: () => void }): HTMLElement {
    this.keyboardView = renderKeyboard({ ...options, selected: this.selection, onSelect: options.onSelect ?? (() => this.render()) });
    return h("section", { class: "stage" }, this.keyboardView.element, options.hint ? h("p", { class: "stage-hint" }, options.hint) : null);
  }

  // ------------------------------------------------------------ connection

  async connectWebHid(): Promise<void> {
    try {
      const transport = await WebHidTransport.request();
      if (transport) await this.store.attach(transport);
    } catch (e) {
      this.toast("error", `Could not connect: ${(e as Error).message}`);
    }
  }

  async connectGranted(): Promise<boolean> {
    try {
      const [transport] = await WebHidTransport.granted();
      if (!transport) return false;
      await this.store.attach(transport);
      return true;
    } catch {
      return false;
    }
  }

  async connectMock(version: [number, number, number] = [0, 0, 18]): Promise<void> {
    await this.store.attach(new MockDevice({ demo: true, version }));
  }

  async disconnect(): Promise<void> {
    this.stopPageStreams();
    await this.store.disconnect();
  }

  // ------------------------------------------------------------ navigation

  async go(page: PageId): Promise<void> {
    if (!PAGES.some((p) => p.id === page)) page = "home";
    if (location.hash !== `#/${page}`) history.replaceState(null, "", `#/${page}`);
    if (page === this.page) return;
    this.stopPageStreams();
    await this.store.leaveLiveModes();
    this.page = page;
    this.selection.clear();
    this.render();
    window.scrollTo(0, 0);
  }

  private stopPageStreams(): void {
    this.ui.liveStop?.();
    this.ui.liveStop = null;
  }

  // ------------------------------------------------------------ confirmed actions

  async switchProfile(index: number): Promise<void> {
    const s = this.store;
    if (index === s.activeProfile || !s.kb) return;
    if (s.dirty) {
      const choice = await choiceDialog({
        title: "Unsaved changes",
        body: `Profile ${s.activeProfile + 1} has unsaved changes (${s.unsavedAreas.join(", ")}). The keyboard drops them when it switches profile.`,
        cancelLabel: "Stay",
        choices: [
          { id: "discard", label: "Discard and switch", kind: "danger" },
          { id: "save", label: "Save and switch", kind: "primary" },
        ],
      });
      if (!choice) return;
      if (choice === "save") await s.save();
    }
    this.stopPageStreams();
    await s.switchProfile(index);
  }

  async renameProfile(index: number): Promise<void> {
    const s = this.store;
    const name = await promptDialog({
      title: `Rename profile ${index + 1}`, label: "Name (the vendor driver shows at most 10 characters)", value: s.profileNames[index] ?? "", maxLength: 20, confirmLabel: "Rename",
    });
    if (name === null) return;
    if (!name.trim()) return this.toast("error", "The profile name must not be empty.");
    await s.renameProfile(index, name.trim());
  }

  async resetProfile(): Promise<void> {
    const s = this.store;
    const n = s.activeProfile + 1;
    const ok = await confirmDialog({
      title: `Reset profile ${n} to factory defaults`,
      body: h("div", null,
        h("p", null, `Everything in profile ${n} (“${s.profileNames[s.activeProfile] ?? ""}”) goes back to factory values: keymap, actuation, SOCD, advanced keys, lighting, settings. The keyboard stores the reset at once.`),
        h("p", null, "Calibration, switch types, profile names and the other profiles are not touched. While this page stays open, Undo and then Save brings the old profile back."),
      ),
      confirmLabel: "Reset profile", danger: true,
    });
    if (ok) await s.resetProfile();
  }

  // ------------------------------------------------------------ rendering

  mount(): void {
    this.applyTheme();
    this.elProfiles = h("div", { class: "profiles", role: "group", "aria-label": "Profiles" });
    this.elSave = h("div", { class: "savebar" });
    this.elRail = h("nav", { class: "rail", "aria-label": "Sections" });
    this.elMain = h("main", { class: "content" });
    this.elToasts = h("div", { class: "toasts", role: "status", "aria-live": "polite" });
    clear(this.root);
    this.root.append(
      h("header", { class: "topbar" },
        h("a", { class: "brand", href: "#/home", "aria-label": "ZENT75-HE, home" }, h("span", { class: "brand-mark" }), "ZENT75", h("span", { class: "brand-sub" }, "HE")),
        this.elProfiles, this.elSave),
      h("div", { class: "body" }, this.elRail, this.elMain),
      this.elToasts,
    );
    this.store.onChange(() => this.render());
    this.store.onBusy(() => this.renderSaveBar());
    this.store.onToast((kind, text) => this.toast(kind, text));

    const fromHash = () => void this.go((location.hash.replace(/^#\/?/, "") || "home") as PageId);
    window.addEventListener("hashchange", fromHash);
    const initial = location.hash.replace(/^#\/?/, "") as PageId;
    if (PAGES.some((p) => p.id === initial)) this.page = initial;

    window.addEventListener("keydown", (e) => this.onKey(e));
    window.addEventListener("beforeunload", (e) => {
      if (this.store.dirty) e.preventDefault();
    });
    this.render();
  }

  private onKey(e: KeyboardEvent): void {
    if (!this.store.connected || document.querySelector("dialog[open]")) return;
    const mod = e.ctrlKey || e.metaKey;
    const typing = e.target instanceof HTMLElement && e.target.matches("input:not([type=range]):not([type=checkbox]), textarea, select");
    if (mod && e.key.toLowerCase() === "s") {
      e.preventDefault();
      if (this.store.dirty) void this.store.save();
    } else if (mod && !e.shiftKey && e.key.toLowerCase() === "z" && !typing) {
      e.preventDefault();
      void this.store.undo();
    } else if (e.key === "Escape" && this.selection.size && !typing) {
      this.selection.clear();
      this.render();
    }
  }

  render(): void {
    const connected = this.store.connected;
    this.root.classList.toggle("is-connected", connected);
    this.renderProfiles();
    this.renderSaveBar();
    this.renderRail();
    const focus = (document.activeElement as HTMLElement | null)?.dataset?.fid;
    this.keyboardView = null;
    const content = connected ? (PAGES.find((p) => p.id === this.page) ?? PAGES[0]).render(this) : this.renderConnect();
    this.elMain.replaceChildren(content);
    if (focus) this.elMain.querySelector<HTMLElement>(`[data-fid="${focus}"]`)?.focus({ preventScroll: true });
  }

  private renderProfiles(): void {
    const s = this.store;
    clear(this.elProfiles);
    if (!s.connected) return;
    // as many profiles as the keyboard reports
    s.profileNames.slice(0, s.profileCount).forEach((name, i) => {
      const active = i === s.activeProfile;
      const pill = h("button", {
        type: "button", class: active ? "pill active" : "pill", "aria-pressed": String(active),
        title: active ? `Profile ${i + 1} is active. Click for rename, reset, export.` : `Switch to profile ${i + 1}`,
        onclick: () => {
          if (!active) return void this.switchProfile(i);
          popupMenu(pill, [
            { label: "Rename…", onSelect: () => void this.renameProfile(i) },
            { label: "Export this profile", onSelect: () => exportProfiles(this, false, false) },
            { label: "Reset to factory defaults…", danger: true, onSelect: () => void this.resetProfile() },
          ]);
        },
      }, h("span", { class: "pill-n" }, String(i + 1)), h("span", { class: "pill-name" }, name || `Profile ${i + 1}`), active ? h("span", { class: "pill-caret", "aria-hidden": "true" }, "▾") : null);
      this.elProfiles.appendChild(pill);
    });
  }

  /** Save state and its three actions. Repainted alone while an operation runs. */
  private renderSaveBar(): void {
    const s = this.store;
    clear(this.elSave);
    if (!s.connected) {
      if (s.busy) this.elSave.appendChild(h("span", { class: "state busy" }, `${s.busy}…`));
      return;
    }
    const areas = s.unsavedAreas;
    const state = s.busy
      ? h("span", { class: "state busy" }, `${s.busy}…`)
      : s.dirty
        ? h("span", { class: "state unsaved", title: "These changes are active on the keyboard now, but a replug drops them until you save." }, `Unsaved: ${areas.join(", ") || "changes"}`)
        : h("span", { class: "state saved", title: "The keyboard's flash holds what you see." }, "Saved");
    this.elSave.append(
      state,
      h("button", {
        type: "button", class: "btn ghost", disabled: !s.canUndo || !!s.busy, dataset: { fid: "undo" },
        title: s.canUndo ? `Undo: ${s.undoLabel} (Ctrl+Z)` : "Nothing to undo", onclick: () => void s.undo(),
      }, "Undo"),
      h("button", {
        type: "button", class: "btn ghost", disabled: !s.dirty || !!s.busy, dataset: { fid: "discard" },
        title: "Put back what was last saved. Undo brings the changes back.", onclick: () => void s.discard(),
      }, "Discard"),
      h("button", {
        type: "button", class: s.dirty ? "btn primary" : "btn", disabled: !s.dirty || !!s.busy, dataset: { fid: "save" },
        title: "Store the current settings in the keyboard so they survive a replug (Ctrl+S)", onclick: () => void s.save(),
      }, "Save to keyboard"),
    );
  }

  private renderRail(): void {
    clear(this.elRail);
    if (!this.store.connected) return;
    const s = this.store;
    let group = 0;
    for (const p of PAGES) {
      if (p.group !== group) {
        group = p.group;
        this.elRail.appendChild(h("div", { class: "rail-group" }, GROUP_TITLES[group]));
      }
      this.elRail.appendChild(h("a", {
        class: p.id === this.page ? "rail-item active" : "rail-item", href: `#/${p.id}`, "aria-current": p.id === this.page ? "page" : null,
      }, p.title));
    }
    const theme = (localStorage.getItem("zent-theme") ?? "auto") as (typeof THEMES)[number];
    this.elRail.appendChild(h("div", { class: "rail-foot" },
      h("span", { class: "rail-device", title: s.info!.productName }, `Firmware ${s.info!.firmwareVersion}${s.backend === "mock" ? ", mock" : ""}`),
      h("button", {
        type: "button", class: "link", title: "Colour theme: follow the system, light, or dark",
        onclick: () => {
          localStorage.setItem("zent-theme", THEMES[(THEMES.indexOf(theme) + 1) % THEMES.length]);
          this.applyTheme();
          this.renderRail();
        },
      }, `Theme: ${theme}`),
      h("button", { type: "button", class: "link", onclick: () => void this.disconnect() }, "Disconnect"),
    ));
  }

  private applyTheme(): void {
    const theme = localStorage.getItem("zent-theme");
    if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;
    else delete document.documentElement.dataset.theme;
  }

  private renderToasts(): void {
    clear(this.elToasts);
    for (const t of this.toasts) this.elToasts.appendChild(h("div", { class: `toast ${t.kind}` }, t.text));
  }

  private renderConnect(): HTMLElement {
    const supported = webHidSupported();
    const busy = this.busy;
    return h("section", { class: "connect" },
      h("h1", null, "Set up your ZENT75-HE"),
      h("p", { class: "lede" }, "Actuation, rapid trigger, lighting, SOCD, keymap and calibration, straight from the browser. Nothing to install."),
      supported
        ? null
        : h("div", { class: "notice warn", role: "alert" },
            h("strong", null, "This browser cannot reach the keyboard. "),
            "It has no WebHID. Open this page in Chrome, Edge, Brave or Opera, over https or localhost. You can still look around with the demo keyboard.",
          ),
      h("div", { class: "connect-actions" },
        h("button", { class: "btn primary big", disabled: !supported || busy, onclick: () => void this.connectWebHid() }, "Connect keyboard"),
        h("button", { class: "btn big", disabled: busy, onclick: () => void this.connectMock() }, "Try the demo keyboard"),
      ),
      h("details", { class: "disclosure" },
        h("summary", null, "It does not connect"),
        h("ul", { class: "facts" },
          h("li", null, "WebHID works only in Chromium-based browsers (Chrome, Edge, Brave, Opera); Firefox and Safari do not have it."),
          h("li", null, "Close the vendor web driver first: both pages would read each other's replies."),
          h("li", null, "On Linux your user needs access to the keyboard's hidraw node (a udev rule for USB vendor 36b0)."),
          h("li", null, "The keyboard is USB 36b0:357e; the panel uses its vendor interface (usage page 0xFF60)."),
          h("li", null, h("button", { class: "link", disabled: busy, onclick: () => void this.connectMock([0, 0, 9]) }, "Demo keyboard with firmware 0.0.9"), " (no key groups / DKS)."),
        ),
      ),
    );
  }
}
