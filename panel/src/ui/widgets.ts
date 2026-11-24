/** Reusable bits of the tab panels. */
import { GROUP_TITLES, KEY_DEFS, PICKER_GROUPS, keyDef, keyLabel, keyTitle, type KeyGroup } from "../protocol/keycodes";
import { KEY_COUNT, QUICK_SELECT } from "../protocol/layout";
import type { App } from "./app";
import { h } from "./dom";

/** Grid of every assignable key code, grouped. */
export function keyPicker(current: number | null, onPick: (code: number) => void, disabled = false): HTMLElement {
  const byGroup = new Map<KeyGroup, typeof KEY_DEFS[number][]>();
  for (const d of KEY_DEFS) (byGroup.get(d.group) ?? byGroup.set(d.group, []).get(d.group)!).push(d);
  return h("div", { class: "picker" },
    PICKER_GROUPS.map((g) => h("div", { class: "picker-group" },
      h("h4", null, GROUP_TITLES[g]),
      h("div", { class: "picker-keys" },
        (byGroup.get(g) ?? []).map((d) => h("button", {
          type: "button", disabled,
          class: d.code === current ? "pick active" : "pick",
          title: keyTitle(d.code),
          onclick: () => onPick(d.code),
        }, d.label || "∅")),
      ),
    )),
  );
}

/** Dropdown with every key code (compact alternative to the picker). */
export function keySelect(current: number, onChange: (code: number) => void, disabled = false): HTMLSelectElement {
  const select = h("select", { class: "input", disabled, onchange: () => onChange(Number(select.value)) });
  const byGroup = new Map<KeyGroup, typeof KEY_DEFS[number][]>();
  for (const d of KEY_DEFS) (byGroup.get(d.group) ?? byGroup.set(d.group, []).get(d.group)!).push(d);
  if (!keyDef(current)) select.appendChild(h("option", { value: String(current) }, `#${current} (unknown)`));
  for (const g of PICKER_GROUPS) {
    const og = h("optgroup", { label: GROUP_TITLES[g] });
    for (const d of byGroup.get(g) ?? []) og.appendChild(h("option", { value: String(d.code) }, d.code === 1000 ? "— empty —" : `${d.label}  ·  ${d.title}`));
    select.appendChild(og);
  }
  select.value = String(current);
  return select;
}

export function codeText(code: number): string {
  return code === 1000 ? "—" : keyLabel(code);
}

/** Buttons that select groups of keys (multi-select tabs). `after` runs once the selection changed. */
export function quickSelect(app: App, after?: () => void): HTMLElement {
  const set = (keys: Iterable<number>) => {
    app.selection.clear();
    for (const k of keys) app.selection.add(k);
    after?.();
    app.render();
  };
  const all = Array.from({ length: KEY_COUNT }, (_, i) => i);
  return h("div", { class: "quick" },
    h("span", { class: "quick-label" }, `${app.selection.size} selected`),
    h("button", { class: "btn small", onclick: () => set(all) }, "All"),
    h("button", { class: "btn small", onclick: () => set([]) }, "None"),
    h("button", { class: "btn small", onclick: () => set(QUICK_SELECT.wasd) }, "WASD"),
    h("button", { class: "btn small", onclick: () => set(QUICK_SELECT.letters) }, "Letters"),
    h("button", { class: "btn small", onclick: () => set(QUICK_SELECT.digits) }, "Digits"),
    h("button", { class: "btn small", onclick: () => set(all.filter((k) => !app.selection.has(k))) }, "Invert"),
  );
}

/** Labelled number input bound to a draft object (no re-render while typing). */
export function numberField(label: string, draft: Record<string, number>, key: string, opts: {
  min: number; max: number; step: number; unit?: string; disabled?: boolean; help?: string;
}): HTMLElement {
  const range = h("input", {
    type: "range", min: String(opts.min), max: String(opts.max), step: String(opts.step), value: String(draft[key]), disabled: opts.disabled,
    oninput: () => {
      draft[key] = Number(range.value);
      num.value = range.value;
    },
  });
  const num = h("input", {
    type: "number", class: "input num", min: String(opts.min), max: String(opts.max), step: String(opts.step), value: String(draft[key]), disabled: opts.disabled,
    oninput: () => {
      const v = Number(num.value);
      if (Number.isFinite(v)) {
        draft[key] = v;
        range.value = String(v);
      }
    },
  });
  return h("label", { class: "field" },
    h("span", { class: "field-label" }, label, opts.help ? h("small", null, opts.help) : null),
    h("span", { class: "field-row" }, range, num, opts.unit ? h("span", { class: "unit" }, opts.unit) : null),
  );
}

export function toggle(label: string, checked: boolean, onChange: (v: boolean) => void, help?: string, disabled = false): HTMLElement {
  const input = h("input", { type: "checkbox", checked, disabled, onchange: () => onChange(input.checked) });
  return h("label", { class: "toggle" }, input, h("span", null, label, help ? h("small", null, help) : null));
}

export function section(title: string, ...children: (Node | string | null | false | undefined)[]): HTMLElement {
  return h("div", { class: "section" }, h("h3", null, title), ...children);
}
