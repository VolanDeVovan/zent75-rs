/** Building blocks shared by the pages. */
import { KEYCODE_EMPTY, KEY_MODE_RAPID_TRIGGER, KEY_MODE_RT_BELOW } from "../protocol/keyboard";
import { keyTitle } from "../protocol/keycodes";
import { KEY_COUNT, QUICK_SELECT } from "../protocol/layout";
import type { App } from "./app";
import { h, mm, openDialog, plural, type Child } from "./dom";
import {
  PICKER_CATEGORIES, categoryOfCode, codeText, codesOfCategory, customisedKeys, searchCodes, usesFixed, usesRt, type KeyFeel,
} from "./model";

// ------------------------------------------------------------ layout

export function card(title: string | null, ...children: Child[]): HTMLElement {
  return h("section", { class: "card" }, title ? h("h2", null, title) : null, ...children);
}

/** Card whose title row carries an action on the right (a link, a reset button). */
export function cardWith(title: string, action: Child, ...children: Child[]): HTMLElement {
  return h("section", { class: "card" }, h("div", { class: "card-head" }, h("h2", null, title), action), ...children);
}

/** One setting: label (and help) on the left, the control on the right. */
export function row(label: Child, control: Child, help?: Child): HTMLElement {
  return h("div", { class: "row" },
    h("div", { class: "row-text" }, h("span", { class: "row-label" }, label), help ? h("span", { class: "row-help" }, help) : null),
    h("div", { class: "row-control" }, control),
  );
}

export function help(...children: Child[]): HTMLElement {
  return h("p", { class: "help" }, ...children);
}

export function linkButton(label: string, onClick: () => void, title?: string): HTMLElement {
  return h("button", { type: "button", class: "link", title, onclick: onClick }, label);
}

/** A collapsible part of a page; remembers whether it is open across re-renders. */
export function disclosure(app: App, id: string, title: string, ...children: Child[]): HTMLElement {
  const open = (app.ui.open ??= {}) as Record<string, boolean>;
  const el = h("details", { class: "disclosure", open: !!open[id], ontoggle: () => (open[id] = el.open) },
    h("summary", null, title),
    h("div", { class: "disclosure-body" }, ...children),
  );
  return el;
}

// ------------------------------------------------------------ controls

export function toggle(fid: string, checked: boolean, onChange: (v: boolean) => void, label?: string): HTMLElement {
  const input = h("input", { type: "checkbox", role: "switch", checked, "aria-label": label, dataset: { fid }, onchange: () => onChange(input.checked) });
  return h("label", { class: "switch" }, input, h("span", { class: "switch-track" }));
}

export function segmented<T extends string | number>(options: readonly { value: T; label: string; title?: string }[], value: T, onChange: (v: T) => void): HTMLElement {
  return h("div", { class: "segmented", role: "group" },
    options.map((o) => h("button", {
      type: "button", class: o.value === value ? "seg active" : "seg", title: o.title, "aria-pressed": String(o.value === value),
      dataset: { fid: `seg-${String(o.value)}` },
      onclick: () => { if (o.value !== value) onChange(o.value); },
    }, o.label)),
  );
}

export interface SliderOptions {
  /** stable id, keeps the keyboard focus across re-renders */
  fid: string;
  label: string;
  help?: string;
  value: number;
  min: number;
  max: number;
  /** step of the slider */
  step: number;
  /** step of the exact number (defaults to `step`) */
  fineStep?: number;
  unit?: string;
  /** decimals shown */
  digits?: number;
  /** factory value; a reset button appears when the value differs */
  factory?: number;
  /** class for a special track (the hue rainbow) */
  trackClass?: string;
  /** several keys with different values are being edited */
  mixed?: boolean;
  /** while dragging (no write) */
  onInput?: (value: number) => void;
  /** on release / Enter: write */
  onCommit: (value: number) => void;
}

/** A setting with a slider and an exact number. Writes on release, not while dragging. */
export function slider(o: SliderOptions): HTMLElement {
  const digits = o.digits ?? 0;
  const text = (v: number) => v.toFixed(digits);
  const clamp = (v: number) => Math.min(o.max, Math.max(o.min, v));
  const range = h("input", {
    type: "range", class: o.trackClass ?? "", min: String(o.min), max: String(o.max), step: String(o.step), value: String(o.value),
    "aria-label": o.label, dataset: { fid: `${o.fid}-range` },
    oninput: () => {
      num.value = text(Number(range.value));
      o.onInput?.(Number(range.value));
    },
    onchange: () => o.onCommit(Number(range.value)),
  });
  const num = h("input", {
    type: "number", class: "input num", min: String(o.min), max: String(o.max), step: String(o.fineStep ?? o.step), value: text(o.value),
    "aria-label": `${o.label}, exact value`, dataset: { fid: `${o.fid}-num` },
    onchange: () => {
      const v = Number(num.value);
      if (num.value.trim() === "" || !Number.isFinite(v)) {
        num.value = text(o.value);
        return;
      }
      o.onCommit(clamp(v));
    },
  });
  const changed = o.factory !== undefined && Math.abs(o.factory - o.value) > 1e-9;
  return h("div", { class: "slider" },
    h("div", { class: "slider-head" },
      h("span", { class: "row-label" }, o.label, o.mixed ? h("span", { class: "tag" }, "mixed") : null),
      o.help ? h("span", { class: "row-help" }, o.help) : null,
    ),
    h("div", { class: "slider-body" },
      range, num,
      h("span", { class: "unit" }, o.unit ?? ""),
      h("button", {
        type: "button", class: "icon-btn", disabled: !changed, "aria-label": `Reset ${o.label.toLowerCase()} to default`,
        title: o.factory === undefined ? "" : changed ? `Reset to default (${text(o.factory)}${o.unit ? ` ${o.unit}` : ""})` : "At the default value",
        style: { visibility: o.factory === undefined ? "hidden" : "visible" },
        onclick: () => o.onCommit(o.factory!),
      }, "↺"),
    ),
  );
}

// ------------------------------------------------------------ key selection

const ARROWS = [71, 80, 81, 82];

/** Selection summary and quick sets for pages where several keys are edited at once. */
export function selectionBar(app: App, options: { noneText: string; customised?: boolean } = { noneText: "No keys selected" }): HTMLElement {
  const set = (keys: Iterable<number>) => {
    app.selection.clear();
    for (const k of keys) app.selection.add(k);
    app.render();
  };
  const all = Array.from({ length: KEY_COUNT }, (_, i) => i);
  const n = app.selection.size;
  const custom = options.customised ? customisedKeys(app.config) : [];
  const chip = (label: string, keys: () => Iterable<number>, title?: string) =>
    h("button", { type: "button", class: "chip", title, onclick: () => set(keys()) }, label);
  return h("div", { class: "selbar" },
    h("span", { class: n ? "selbar-count on" : "selbar-count" }, n ? `${plural(n, "key")} selected` : options.noneText),
    h("div", { class: "selbar-sets" },
      chip("All", () => all),
      chip("WASD", () => QUICK_SELECT.wasd),
      chip("Letters", () => QUICK_SELECT.letters),
      chip("Digits", () => QUICK_SELECT.digits),
      chip("Arrows", () => ARROWS),
      custom.length ? chip(`Customised (${custom.length})`, () => custom, "Keys that differ from the global values") : null,
      chip("Invert", () => all.filter((k) => !app.selection.has(k))),
      n ? h("button", { type: "button", class: "chip clear", onclick: () => set([]) }, "Clear selection") : null,
    ),
  );
}

// ------------------------------------------------------------ key code picker

interface PickerState { cat?: string; query?: string }

/**
 * Key codes by search or by category, one category visible at a time. A whole number in the
 * search box can be assigned as a raw code. `maxCode` caps what is offered (tap-hold: 1000).
 */
export function keyPicker(o: { current: number | null; maxCode?: number; state: PickerState; onPick: (code: number) => void; autofocus?: boolean }): HTMLElement {
  const maxCode = o.maxCode ?? 32767;
  const st = o.state;
  st.cat ??= categoryOfCode(o.current);
  const grid = h("div", { class: "picker-grid" });
  const cats = h("div", { class: "picker-cats", role: "tablist" });
  const pick = (code: number, label: string, title: string) => h("button", {
    type: "button", class: code === o.current ? "pick active" : "pick", title, onclick: () => o.onPick(code),
  }, label);
  const paint = () => {
    const q = (st.query ?? "").trim();
    cats.replaceChildren(...PICKER_CATEGORIES.map((c) => h("button", {
      type: "button", role: "tab", class: !q && c.id === st.cat ? "chip active" : "chip", "aria-selected": String(!q && c.id === st.cat),
      onclick: () => { st.cat = c.id; st.query = ""; search.value = ""; paint(); },
    }, c.title)));
    if (!q) {
      grid.replaceChildren(...codesOfCategory(st.cat!, maxCode).map((d) => pick(d.code, d.code === KEYCODE_EMPTY ? "Empty" : d.label, keyTitle(d.code))));
      return;
    }
    const res = searchCodes(q, maxCode);
    const items: Node[] = res.matches.map((d) => pick(d.code, d.code === KEYCODE_EMPTY ? "Empty" : d.label, keyTitle(d.code)));
    if (res.raw !== null && !res.matches.some((d) => d.code === res.raw)) items.push(pick(res.raw, `Raw code ${res.raw}`, "Assign this number as a raw key code"));
    if (items.length === 0) {
      items.push(h("p", { class: "help" }, /^-?\d+$/.test(q) ? `Code ${q} is out of range here (at most ${maxCode}).` : `Nothing matches “${q}”. Try a name like “vol”, “f13”, “mouse”, or a code number.`));
    }
    grid.replaceChildren(...items);
  };
  const search = h("input", {
    type: "search", class: "input picker-search", placeholder: "Search keys: “vol”, “F13”, “mouse”, or a code number", value: st.query ?? "",
    "aria-label": "Search key codes", dataset: { fid: "picker-search" },
    oninput: () => { st.query = search.value; paint(); },
  });
  if (o.autofocus) queueMicrotask(() => search.focus());
  paint();
  return h("div", { class: "picker" }, search, cats, grid);
}

/** The picker as a dialog; resolves the chosen code or null. */
export function pickKeyDialog(title: string, current: number, maxCode?: number): Promise<number | null> {
  return openDialog<number>({ title, wide: true }, (close) => [
    keyPicker({ current, maxCode, state: {}, onPick: (code) => close(code), autofocus: true }),
    h("div", { class: "modal-actions" },
      h("button", { type: "button", class: "btn ghost", onclick: () => close(null) }, "Cancel"),
      current !== KEYCODE_EMPTY ? h("button", { type: "button", class: "btn", onclick: () => close(KEYCODE_EMPTY) }, "Set to empty") : null,
    ),
  ]);
}

/** A button showing a key code; opens the picker dialog. */
export function keyButton(o: { code: number; title: string; maxCode?: number; emptyText?: string; onPick: (code: number) => void }): HTMLElement {
  return h("button", {
    type: "button", class: o.code === KEYCODE_EMPTY ? "keybtn empty" : "keybtn", title: `${keyTitle(o.code)}. Click to change.`,
    onclick: async () => {
      const code = await pickKeyDialog(o.title, o.code, o.maxCode);
      if (code !== null && code !== o.code) o.onPick(code);
    },
  }, o.code === KEYCODE_EMPTY ? (o.emptyText ?? "None") : codeText(o.code));
}

// ------------------------------------------------------------ travel gauge

/**
 * A millimetre ruler of the key travel, rest on the left, bottom on the right, with the points
 * that decide when the key presses and releases. `update` moves them while a slider is dragged.
 */
export function travelGauge(feel: KeyFeel, travelUm: number): { element: HTMLElement; update(f: KeyFeel): void } {
  const pct = (um: number) => `${Math.min(100, Math.max(0, (um / travelUm) * 100))}%`;
  const ticks: Node[] = [];
  for (let um = 0; um <= travelUm; um += 500) {
    ticks.push(h("span", { class: um % 1000 === 0 ? "gauge-tick major" : "gauge-tick", style: { left: pct(um) } },
      um % 1000 === 0 ? h("span", { class: "gauge-num" }, String(um / 1000)) : null));
  }
  const zone = h("span", { class: "gauge-zone" });
  const dzTop = h("span", { class: "gauge-dz" });
  const dzBottom = h("span", { class: "gauge-dz" });
  const act = h("span", { class: "gauge-mark act" }, h("span", { class: "gauge-flag" }));
  const rel = h("span", { class: "gauge-mark rel" }, h("span", { class: "gauge-flag" }));
  const caption = h("p", { class: "gauge-caption" });
  const update = (f: KeyFeel) => {
    const rt = usesRt(f.mode);
    const fixed = usesFixed(f.mode);
    act.hidden = rel.hidden = !fixed;
    dzTop.hidden = dzBottom.hidden = !rt;
    act.style.left = pct(f.actuationUm);
    rel.style.left = pct(f.releaseUm);
    (act.firstChild as HTMLElement).textContent = `press ${mm(f.actuationUm)}`;
    (rel.firstChild as HTMLElement).textContent = `release ${mm(f.releaseUm)}`;
    rel.classList.toggle("flip", f.actuationUm - f.releaseUm < travelUm * 0.3);
    dzTop.style.left = "0";
    dzTop.style.width = pct(f.topDeadzoneUm);
    dzBottom.style.left = pct(travelUm - f.bottomDeadzoneUm);
    dzBottom.style.width = pct(f.bottomDeadzoneUm);
    // the stretch of travel in which the key is (or can be) pressed
    const from = f.mode === KEY_MODE_RAPID_TRIGGER ? f.topDeadzoneUm : f.actuationUm;
    const to = rt ? travelUm - f.bottomDeadzoneUm : travelUm;
    zone.style.left = pct(from);
    zone.style.width = pct(Math.max(0, to - from));
    zone.classList.toggle("rt", rt);
    caption.textContent = f.mode === KEY_MODE_RAPID_TRIGGER
      ? `Anywhere in the blue zone: ${mm(f.rtPressUm)} mm down presses, ${mm(f.rtReleaseUm)} mm up releases.`
      : f.mode === KEY_MODE_RT_BELOW
        ? `Presses at ${mm(f.actuationUm)} mm, then rapid trigger below it (${mm(f.rtPressUm)} down / ${mm(f.rtReleaseUm)} up) until the key rises above ${mm(f.releaseUm)} mm.`
        : `Presses at ${mm(f.actuationUm)} mm on the way down, releases at ${mm(f.releaseUm)} mm on the way up.`;
  };
  update(feel);
  return {
    element: h("div", { class: "gauge", role: "img", "aria-label": "Key travel diagram" },
      h("div", { class: "gauge-ends" }, h("span", null, "rest"), h("span", null, `bottom, ${mm(travelUm)} mm`)),
      h("div", { class: "gauge-track" }, zone, dzTop, dzBottom, ticks, rel, act),
      caption,
    ),
    update,
  };
}
