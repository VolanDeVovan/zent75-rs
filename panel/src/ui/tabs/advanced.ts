import {
  ADV_COMBO, ADV_DKS, ADV_NONE, DKS_HOLD, DKS_NONE, DKS_TAP, KEYCODE_EMPTY, cloneConfig, type AdvancedKey,
} from "../../protocol/keyboard";
import { KEY_LAYOUT } from "../../protocol/layout";
import type { App } from "../app";
import { h, mm } from "../dom";
import type { TabDef } from "../tabs";
import { codeText, keySelect, section, toggle } from "../widgets";

type Sub = "socd" | "combo" | "dks";
const subOf = (app: App): Sub => (app.ui.advSub ??= "socd");
const selected = (app: App): number | null => [...app.selection][0] ?? null;

const PHASES = ["Press past top point", "Press past bottom point", "Release above bottom point", "Release above top point"];

function loadDraft(app: App, key: number): void {
  const a = app.config!.advanced?.[key];
  if (!a) return;
  const d = cloneConfig(a);
  // records that were never used may carry garbage; present sane editing defaults
  if (d.mode === ADV_NONE) {
    d.actions = d.actions.map(() => [0, 0, 0, 0]);
    d.reserved = new Array(13).fill(0xff);
    if (d.dksTopUm < 100 || d.dksTopUm > 3200) d.dksTopUm = 1000;
    if (d.dksBottomUm <= d.dksTopUm || d.dksBottomUm > 3300) d.dksBottomUm = 3000;
    if (d.tapTicks < 8 || d.tapTicks > 400) d.tapTicks = 8;
    if (d.delayTicks > 80) d.delayTicks = 0;
    if (d.randomTicks > 80) d.randomTicks = 0;
    if (d.suppressOriginal > 1) d.suppressOriginal = 1;
  }
  app.ui.advDraft = d;
}

function writeAdvanced(app: App, key: number, record: AdvancedKey, label: string): void {
  void app.edit(label, (c) => {
    if (c.advanced) c.advanced[key] = record;
  }).then(() => loadDraft(app, key));
}

function clearAdvanced(app: App, key: number): void {
  const cur = app.config!.advanced?.[key];
  if (!cur) return;
  // what the vendor driver does on right-click: mode 0 and the four key slots emptied
  writeAdvanced(app, key, { ...cloneConfig(cur), mode: ADV_NONE, keys: [KEYCODE_EMPTY, KEYCODE_EMPTY, KEYCODE_EMPTY, KEYCODE_EMPTY] }, "Clearing binding…");
}

function renderSocd(app: App, key: number | null): HTMLElement {
  const cfg = app.config!;
  if (key === null) return h("p", { class: "help" }, "Select a key to set which key it cancels.");
  const partner = h("select", { class: "input" },
    KEY_LAYOUT.filter((k) => k.index !== key).map((k) => h("option", { value: String(k.index) }, `${k.name} (${codeText(cfg.keymap.main[k.index])})`)),
  );
  return h("div", { class: "cols" },
    h("div", { class: "col" },
      section(`${KEY_LAYOUT[key].name} cancels`,
        keySelect(cfg.socd[key], (code) => void app.edit("Writing SOCD…", (c) => {
          c.socd[key] = code;
        }), !!app.busy),
        h("p", { class: "help" }, "While this key is held, the chosen key code is released (last input wins). Applies to both layers. Board functions cannot be cancelled."),
        h("button", { class: "btn small", disabled: !!app.busy || cfg.socd[key] === KEYCODE_EMPTY, onclick: () => void app.edit("Clearing SOCD…", (c) => {
          c.socd[key] = KEYCODE_EMPTY;
        }) }, "Clear"),
      ),
    ),
    h("div", { class: "col" },
      section("Make a mutual pair",
        h("div", { class: "row" }, partner,
          h("button", {
            class: "btn", disabled: !!app.busy,
            onclick: () => {
              const other = Number(partner.value);
              void app.edit("Writing SOCD pair…", (c) => {
                c.socd[key] = c.keymap.main[other];
                c.socd[other] = c.keymap.main[key];
              });
            },
          }, "Pair both ways"),
        ),
        h("p", { class: "help" }, "Typical use: A ↔ D and W ↔ S, so pressing the opposite direction overrides instead of cancelling movement. Check a game's rules first; some forbid SOCD."),
      ),
    ),
  );
}

function renderCombo(app: App, key: number | null, d: AdvancedKey | null): HTMLElement {
  if (key === null || !d) return h("p", { class: "help" }, "Select a key to bind a group of up to four keys to it.");
  return h("div", { class: "cols" },
    h("div", { class: "col" },
      section(`Keys sent by ${KEY_LAYOUT[key].name}`,
        h("div", { class: "slots" }, d.keys.map((code, i) => h("label", { class: "field" },
          h("span", { class: "field-label" }, `Key ${i + 1}`),
          keySelect(code, (v) => (d.keys[i] = v), !!app.busy),
        ))),
        toggle("Also send the key's own keymap code", d.suppressOriginal === 0, (v) => (d.suppressOriginal = v ? 0 : 1),
          "off = the group replaces the key's normal function"),
        h("div", { class: "row end" },
          h("button", { class: "btn", disabled: !!app.busy, onclick: () => clearAdvanced(app, key) }, "Remove binding"),
          h("button", {
            class: "btn primary", disabled: !!app.busy,
            onclick: () => {
              if (d.keys.every((k) => k === KEYCODE_EMPTY)) return app.toast("error", "Set at least one key for the group.");
              writeAdvanced(app, key, { ...cloneConfig(d), mode: ADV_COMBO }, "Writing key group…");
            },
          }, "Apply group"),
        ),
      ),
    ),
  );
}

function renderDks(app: App, key: number | null, d: AdvancedKey | null): HTMLElement {
  if (key === null || !d) return h("p", { class: "help" }, "Select a key to give it up to four actions along its travel (dynamic keystroke).");
  const actionSelect = (slot: number, phase: number) => {
    const s = h("select", { class: "input tight", onchange: () => (d.actions[slot][phase] = Number(s.value)) },
      h("option", { value: String(DKS_NONE) }, "—"),
      h("option", { value: String(DKS_TAP) }, "tap"),
      // the last phase has nothing after it to hold until
      phase < 3 ? h("option", { value: String(DKS_HOLD) }, "hold →") : null,
    );
    s.value = String(d.actions[slot][phase]);
    if (s.value === "") s.appendChild(h("option", { value: String(d.actions[slot][phase]), selected: true }, `raw ${d.actions[slot][phase]}`));
    return s;
  };
  const num = (label: string, get: () => number, set: (v: number) => void, min: number, max: number, step: number, unit: string) => {
    const i = h("input", { type: "number", class: "input num", min: String(min), max: String(max), step: String(step), value: String(get()), oninput: () => {
      const v = Number(i.value);
      if (Number.isFinite(v)) set(v);
    } });
    return h("label", { class: "field inline" }, h("span", { class: "field-label" }, label), h("span", { class: "field-row" }, i, h("span", { class: "unit" }, unit)));
  };
  return h("div", null,
    h("table", { class: "dks" },
      h("thead", null, h("tr", null, h("th", null, "Slot"), h("th", null, "Key"), PHASES.map((p) => h("th", null, p)))),
      h("tbody", null, d.keys.map((code, slot) => h("tr", null,
        h("td", null, String(slot + 1)),
        h("td", null, keySelect(code, (v) => (d.keys[slot] = v), !!app.busy)),
        [0, 1, 2, 3].map((phase) => h("td", null, actionSelect(slot, phase))),
      ))),
    ),
    h("div", { class: "cols" },
      h("div", { class: "col" },
        section("Trigger points",
          num("Top point", () => d.dksTopUm / 1000, (v) => (d.dksTopUm = Math.round(v * 1000)), 0.1, 3.2, 0.1, "mm"),
          num("Bottom point", () => d.dksBottomUm / 1000, (v) => (d.dksBottomUm = Math.round(v * 1000)), 0.2, 3.3, 0.1, "mm"),
        ),
      ),
      h("div", { class: "col" },
        section("Timing",
          num("Tap length", () => d.tapTicks / 8, (v) => (d.tapTicks = Math.round(v * 8)), 1, 50, 1, "ms"),
          num("Delay", () => d.delayTicks / 8, (v) => (d.delayTicks = Math.round(v * 8)), 0, 10, 1, "ms"),
          num("Random extra delay", () => d.randomTicks / 8, (v) => (d.randomTicks = Math.round(v * 8)), 0, 10, 1, "ms"),
          h("p", { class: "help" }, "Stored as scan ticks (value × 8)."),
        ),
      ),
      h("div", { class: "col" },
        section("Original key",
          toggle("Also send the key's own keymap code", d.suppressOriginal === 0, (v) => (d.suppressOriginal = v ? 0 : 1)),
          h("div", { class: "row end" },
            h("button", { class: "btn", disabled: !!app.busy, onclick: () => clearAdvanced(app, key) }, "Remove binding"),
            h("button", {
              class: "btn primary", disabled: !!app.busy,
              onclick: () => {
                const used = d.keys.map((k, i) => ({ k, any: d.actions[i].some((a) => a !== DKS_NONE) })).filter((s) => s.k !== KEYCODE_EMPTY);
                if (used.length === 0) return app.toast("error", "Set at least one DKS key.");
                if (!used.some((s) => s.any)) return app.toast("error", "Give at least one DKS key an action.");
                if (d.dksBottomUm <= d.dksTopUm) return app.toast("error", "The bottom point must be deeper than the top point.");
                if (d.dksTopUm < 100 || d.dksBottomUm > 3300) return app.toast("error", "Trigger points must be within 0.1 … 3.3 mm.");
                writeAdvanced(app, key, { ...cloneConfig(d), mode: ADV_DKS, reserved: new Array(13).fill(0xff) }, "Writing DKS…");
              },
            }, "Apply DKS"),
          ),
        ),
      ),
    ),
  );
}

export const advancedTab: TabDef = {
  id: "advanced",
  title: "SOCD / DKS",
  hint: "Click a key to edit its advanced behaviour. Right-click clears it.",
  visual(app, i) {
    const cfg = app.config!;
    const sub = subOf(app);
    const name = KEY_LAYOUT[i].name;
    const a = cfg.advanced?.[i];
    if (sub === "socd") {
      const c = cfg.socd[i];
      return c === KEYCODE_EMPTY
        ? { label: name, state: "dim" }
        : { label: name, sub: `⊘ ${codeText(c)}`, state: "accent", title: `${name} cancels ${codeText(c)}` };
    }
    if (!a) return { label: name, state: "dim" };
    if (sub === "combo" && a.mode === ADV_COMBO) {
      return { label: name, sub: a.keys.filter((k) => k !== KEYCODE_EMPTY).map(codeText).join("+"), state: "accent", badge: "GRP" };
    }
    if (sub === "dks" && a.mode === ADV_DKS) {
      return { label: name, sub: `${mm(a.dksTopUm, 1)}/${mm(a.dksBottomUm, 1)}`, state: "accent", badge: "DKS" };
    }
    return { label: name, state: "dim", badge: a.mode === ADV_COMBO ? "GRP" : a.mode === ADV_DKS ? "DKS" : undefined };
  },
  onKeyClick(app, i) {
    const was = app.selection.has(i);
    app.selection.clear();
    if (!was) {
      app.selection.add(i);
      loadDraft(app, i);
      const mode = app.config!.advanced?.[i]?.mode;
      if (mode === ADV_COMBO) app.ui.advSub = "combo";
      if (mode === ADV_DKS) app.ui.advSub = "dks";
    }
    app.render();
  },
  onKeyContext(app, i) {
    if (subOf(app) === "socd") {
      void app.edit("Clearing SOCD…", (c) => {
        c.socd[i] = KEYCODE_EMPTY;
      });
    } else clearAdvanced(app, i);
  },
  render(app) {
    const sub = subOf(app);
    const key = selected(app);
    const supported = !!app.info?.features.advancedKeys;
    if (key !== null && !app.ui.advDraft) loadDraft(app, key);
    const draft = (key !== null ? app.ui.advDraft : null) as AdvancedKey | null;
    const setSub = (s: Sub) => {
      app.ui.advSub = s;
      app.render();
    };
    const tabs = h("div", { class: "segmented" },
      h("button", { class: sub === "socd" ? "seg active" : "seg", onclick: () => setSub("socd") }, "SOCD (last input wins)"),
      h("button", { class: sub === "combo" ? "seg active" : "seg", onclick: () => setSub("combo") }, "Key group"),
      h("button", { class: sub === "dks" ? "seg active" : "seg", onclick: () => setSub("dks") }, "DKS"),
    );
    let body: HTMLElement;
    if (sub === "socd") body = renderSocd(app, key);
    else if (!supported) {
      body = h("div", { class: "warning" }, `Key groups and DKS need firmware 0.0.18 or newer; this keyboard runs ${app.info?.firmwareVersion}.`);
    } else body = sub === "combo" ? renderCombo(app, key, draft) : renderDks(app, key, draft);
    const current = key !== null ? app.config!.advanced?.[key] : null;
    return h("div", null,
      h("div", { class: "row" }, tabs,
        current && current.mode !== ADV_NONE && sub !== "socd"
          ? h("span", { class: "chip" }, `${KEY_LAYOUT[key!].name} is currently a ${current.mode === ADV_COMBO ? "key group" : current.mode === ADV_DKS ? "DKS key" : `mode ${current.mode}`}`)
          : null,
      ),
      body,
    );
  },
};
