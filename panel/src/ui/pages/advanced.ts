/**
 * Advanced keys: a key that sends a group of up to four keys, or a DKS key with up to four
 * actions along its travel. Both are one 48-byte record per key, so they are edited as a draft
 * and written with Apply. Needs firmware with key groups / DKS (stock ≥ 0.0.18).
 */
import {
  ADV_COMBO, ADV_DKS, ADV_NONE, DKS_HOLD, DKS_NONE, DKS_TAP, KEYCODE_EMPTY, cloneConfig, type AdvancedKey,
} from "../../protocol/keyboard";
import { KEY_LAYOUT } from "../../protocol/layout";
import type { App, Page } from "../app";
import { card, cardWith, disclosure, help, keyButton, linkButton, row, segmented, toggle } from "../components";
import { h, mm } from "../dom";
import { codeText } from "../model";

const PHASES = ["Pressed past the top point", "Pressed past the bottom point", "Released above the bottom point", "Released above the top point"];

interface Draft { key: number; mode: number; record: AdvancedKey }

/** The record of `key` for editing; an unused one starts from the vendor driver's editing defaults. */
function loadDraft(app: App, key: number): Draft {
  const d = cloneConfig(app.config.advanced![key]);
  if (d.mode === ADV_NONE) {
    d.actions = d.actions.map(() => [0, 0, 0, 0]);
    d.reserved = new Array(13).fill(0xff);
    d.dksTopUm = 1000;
    d.dksBottomUm = 3000;
    d.tapTicks = 8;
    d.delayTicks = 0;
    d.randomTicks = 0;
    if (d.suppressOriginal > 1) d.suppressOriginal = 1;
  }
  return (app.ui.advDraft = { key, mode: d.mode, record: d });
}

function draftOf(app: App, key: number): Draft {
  const d = app.ui.advDraft as Draft | undefined;
  return d && d.key === key ? d : loadDraft(app, key);
}

function write(app: App, key: number, record: AdvancedKey, label: string): void {
  void app.edit(label, (c) => {
    if (c.advanced) c.advanced[key] = record;
  }).then(() => {
    app.ui.advDraft = null;
    app.render();
  });
}

function remove(app: App, key: number): void {
  const cur = app.config.advanced?.[key];
  if (!cur || cur.mode === ADV_NONE) return;
  // what the vendor driver does on right-click: mode 0 and the four key slots emptied
  write(app, key, { ...cloneConfig(cur), mode: ADV_NONE, keys: [KEYCODE_EMPTY, KEYCODE_EMPTY, KEYCODE_EMPTY, KEYCODE_EMPTY] }, "advanced key removal");
}

function slotButton(app: App, d: Draft, slot: number, what: string): HTMLElement {
  return keyButton({
    code: d.record.keys[slot], title: `${KEY_LAYOUT[d.key].name}: ${what} ${slot + 1}`, emptyText: "Empty",
    onPick: (code) => { d.record.keys[slot] = code; app.render(); },
  });
}

function ownCodeRow(app: App, d: Draft): HTMLElement {
  return row("Also send the key's own code",
    toggle("adv-own", d.record.suppressOriginal === 0, (v) => { d.record.suppressOriginal = v ? 0 : 1; app.render(); }, "Also send the key's own code"),
    "off: this replaces what the key normally sends");
}

function groupEditor(app: App, d: Draft): HTMLElement[] {
  return [
    h("div", { class: "slots" }, d.record.keys.map((_, i) => h("label", { class: "stack" }, `Key ${i + 1}`, slotButton(app, d, i, "group key")))),
    ownCodeRow(app, d),
    h("div", { class: "actions end" },
      h("button", {
        type: "button", class: "btn primary",
        onclick: () => {
          if (d.record.keys.every((k) => k === KEYCODE_EMPTY)) return app.toast("error", "Set at least one key for the group.");
          write(app, d.key, { ...cloneConfig(d.record), mode: ADV_COMBO }, "key group");
        },
      }, "Apply key group"),
    ),
  ];
}

function dksEditor(app: App, d: Draft): HTMLElement[] {
  const r = d.record;
  const cell = (slot: number, phase: number) => {
    const v = r.actions[slot][phase];
    // the last phase has nothing after it to hold until
    const order = phase < 3 ? [DKS_NONE, DKS_TAP, DKS_HOLD] : [DKS_NONE, DKS_TAP];
    const text = v === DKS_NONE ? "" : v === DKS_TAP ? "Tap" : v === DKS_HOLD ? "Hold" : `raw ${v}`;
    return h("button", {
      type: "button", class: v === DKS_NONE ? "dks-cell" : v === DKS_HOLD ? "dks-cell on hold" : "dks-cell on",
      title: v === DKS_HOLD ? "Hold until the next phase. Click to change." : v === DKS_TAP ? "A single tap. Click to change." : "Nothing. Click to change.",
      dataset: { fid: `dks-${slot}-${phase}` },
      onclick: () => { r.actions[slot][phase] = order[(order.indexOf(v) + 1) % order.length] ?? DKS_NONE; app.render(); },
    }, text);
  };
  const num = (fid: string, label: string, value: number, set: (v: number) => void, min: number, max: number, step: number, unit: string) => {
    const input = h("input", {
      type: "number", class: "input num", min: String(min), max: String(max), step: String(step), value: String(value), "aria-label": label, dataset: { fid },
      onchange: () => {
        const v = Number(input.value);
        if (Number.isFinite(v)) set(v);
        app.render();
      },
    });
    return row(label, [input, h("span", { class: "unit" }, unit)]);
  };
  return [
    h("div", { class: "dks-wrap" },
      h("table", { class: "dks" },
        h("thead", null, h("tr", null, h("th", null, "Sends"), PHASES.map((p) => h("th", null, p)))),
        h("tbody", null, r.keys.map((_, slot) => h("tr", null,
          h("td", null, slotButton(app, d, slot, "DKS key")),
          [0, 1, 2, 3].map((phase) => h("td", null, cell(slot, phase))),
        ))),
      ),
    ),
    help("Click a cell to cycle: nothing, tap, hold until the next phase."),
    num("dks-top", "Top point", r.dksTopUm / 1000, (v) => (r.dksTopUm = Math.round(v * 1000)), 0.1, 3.2, 0.1, "mm"),
    num("dks-bot", "Bottom point", r.dksBottomUm / 1000, (v) => (r.dksBottomUm = Math.round(v * 1000)), 0.2, 3.3, 0.1, "mm"),
    ownCodeRow(app, d),
    disclosure(app, "dks-timing", "Timing",
      num("dks-tap", "Tap length", r.tapTicks / 8, (v) => (r.tapTicks = Math.round(v * 8)), 1, 50, 1, "ms"),
      num("dks-delay", "Delay", r.delayTicks / 8, (v) => (r.delayTicks = Math.round(v * 8)), 0, 10, 1, "ms"),
      num("dks-rand", "Random extra delay", r.randomTicks / 8, (v) => (r.randomTicks = Math.round(v * 8)), 0, 10, 1, "ms"),
      help("Stored in scan ticks, eight per millisecond."),
    ),
    h("div", { class: "actions end" },
      h("button", {
        type: "button", class: "btn primary",
        onclick: () => {
          const used = r.keys.map((k, i) => ({ k, any: r.actions[i].some((a) => a !== DKS_NONE) })).filter((s) => s.k !== KEYCODE_EMPTY);
          if (used.length === 0) return app.toast("error", "Set at least one DKS key.");
          if (!used.some((s) => s.any)) return app.toast("error", "Give at least one DKS key an action.");
          if (r.dksBottomUm <= r.dksTopUm) return app.toast("error", "The bottom point must be deeper than the top point.");
          if (r.dksTopUm < 100 || r.dksBottomUm > 3300) return app.toast("error", "Trigger points must be within 0.1 … 3.3 mm.");
          write(app, d.key, { ...cloneConfig(r), mode: ADV_DKS, reserved: new Array(13).fill(0xff) }, "DKS");
        },
      }, "Apply DKS"),
    ),
  ];
}

export const advancedPage: Page = {
  id: "advanced",
  title: "Advanced keys",
  group: 1,
  render(app) {
    const adv = app.config.advanced;
    if (!app.info.features.advancedKeys || !adv) {
      return h("div", { class: "page" },
        h("div", { class: "notice" },
          h("strong", null, "Not available on this firmware. "),
          `Key groups and DKS need firmware 0.0.18 or newer; this keyboard runs ${app.info.firmwareVersion}.`),
      );
    }
    const key = [...app.selection][0] ?? null;
    const used = adv.filter((a) => a.mode !== ADV_NONE).length;
    let editor: HTMLElement | null = null;
    if (key !== null) {
      const d = draftOf(app, key);
      const stored = adv[key].mode;
      const modes = [
        { value: ADV_NONE, label: "Plain key" },
        { value: ADV_COMBO, label: "Key group", title: "One press sends up to four keys together" },
        { value: ADV_DKS, label: "DKS", title: "Dynamic keystroke: different actions at different depths" },
        ...(d.mode > ADV_DKS ? [{ value: d.mode, label: `Mode ${d.mode}` }] : []),
      ];
      editor = cardWith(KEY_LAYOUT[key].name,
        stored !== ADV_NONE ? linkButton("Remove", () => remove(app, key), "Make it a plain key again") : null,
        segmented(modes, d.mode, (m) => { d.mode = m; app.render(); }),
        d.mode === ADV_COMBO ? groupEditor(app, d)
          : d.mode === ADV_DKS ? dksEditor(app, d)
          : help(stored === ADV_NONE ? "A plain key. Choose Key group or DKS to give it more to do." : `Stored as a ${stored === ADV_COMBO ? "key group" : "DKS key"}. Use Remove to make it a plain key.`),
      );
    }
    return h("div", { class: "page" },
      app.stage({
        select: "single",
        hint: key === null ? (used ? "Click a key to edit it. Right-click removes its group or DKS." : "Click a key to make it send a group of keys, or different keys at different depths (DKS).") : null,
        onSelect: () => { app.ui.advDraft = null; app.render(); },
        onContextMenu: (i) => remove(app, i),
        visual: (i) => {
          const a = adv[i];
          const name = KEY_LAYOUT[i].name;
          if (a.mode === ADV_COMBO) return { label: name, sub: a.keys.filter((k) => k !== KEYCODE_EMPTY).map(codeText).join("+"), state: "accent", badge: "GRP" };
          if (a.mode === ADV_DKS) return { label: name, sub: `${mm(a.dksTopUm, 1)}/${mm(a.dksBottomUm, 1)}`, state: "accent", badge: "DKS" };
          return { label: name, state: used ? "dim" : null };
        },
      }),
      editor ?? (used ? null : card(null, help("Nothing here yet. A key group sends several keys with one press (a shortcut on one key). DKS sends up to four keys depending on how deep the key is pressed."))),
    );
  },
};
