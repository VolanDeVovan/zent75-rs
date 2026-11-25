/**
 * Keymap: what each key sends on the main and the Fn layer, plus the two firmware extras the
 * vendor driver never exposed: tap-hold and modifier masks. Click a key, then search or browse.
 */
import { KEYCODE_EMPTY, MOD_BITS, TAP_HOLD_MS } from "../../protocol/keyboard";
import { keyLabel, keyTitle } from "../../protocol/keycodes";
import { DEFAULT_KEYMAP, FN_FIXED_KEYS, KEY_LAYOUT } from "../../protocol/layout";
import type { App, Page } from "../app";
import { card, cardWith, disclosure, help, keyButton, keyPicker, row, segmented } from "../components";
import { h } from "../dom";
import { codeText } from "../model";

type Layer = "main" | "fn";
const layerOf = (app: App): Layer => (app.ui.layer ??= "main");
const maskText = (mask: number) => MOD_BITS.filter((m) => mask & m.bit).map((m) => m.label).join("+");

function inspector(app: App, key: number, layer: Layer): HTMLElement {
  const c = app.config;
  const saved = app.store.saved!;
  const code = c.keymap[layer][key];
  const hold = c.holdCode[layer][key];
  const mask = c.modMask[layer][key];
  const assign = (v: number) => void app.edit("keymap", (d) => { d.keymap[layer][key] = v; });
  const savedCode = saved.keymap[layer][key];
  const tap = codeText(code);
  return cardWith(`${KEY_LAYOUT[key].name} on the ${layer === "fn" ? "Fn" : "main"} layer`,
    h("span", { class: code === KEYCODE_EMPTY ? "sends empty" : "sends", title: keyTitle(code) }, code === KEYCODE_EMPTY ? "sends nothing" : `sends ${tap}`),
    layer === "fn" && FN_FIXED_KEYS.includes(key)
      ? h("p", { class: "warn-text" }, "This key carries a built-in Fn shortcut (profile, lighting, …). Remapping works, but the shortcut is gone until you put it back.")
      : null,
    h("div", { class: "actions" },
      layer === "main"
        ? h("button", { type: "button", class: "btn small", disabled: code === DEFAULT_KEYMAP[key], onclick: () => assign(DEFAULT_KEYMAP[key]) }, `Factory default (${codeText(DEFAULT_KEYMAP[key])})`)
        : null,
      h("button", { type: "button", class: "btn small", disabled: code === savedCode, onclick: () => assign(savedCode) }, `Revert to saved (${codeText(savedCode)})`),
      h("button", { type: "button", class: "btn small", disabled: code === KEYCODE_EMPTY, onclick: () => assign(KEYCODE_EMPTY) }, "Clear"),
    ),
    disclosure(app, "km-extra", hold !== KEYCODE_EMPTY || mask ? `Tap-hold and modifiers (${[hold !== KEYCODE_EMPTY ? `hold ${codeText(hold)}` : "", mask ? `+${maskText(mask)}` : ""].filter(Boolean).join(", ")})` : "Tap-hold and modifiers",
      row("When held",
        // codes above 1000 are refused: the firmware treats such a profile as corrupt
        keyButton({ code: hold, title: `${KEY_LAYOUT[key].name}: code while held`, maxCode: KEYCODE_EMPTY, emptyText: "Same as tap", onPick: (v) => void app.edit("tap-hold", (d) => { d.holdCode[layer][key] = v; }) }),
        `held longer than ${TAP_HOLD_MS / 1000} s`),
      hold !== KEYCODE_EMPTY
        ? help(`Tap sends ${tap}, hold sends ${keyLabel(hold)}. The tap arrives when you let go, because the keyboard waits ${TAP_HOLD_MS} ms to tell the two apart. The time is fixed in the firmware.`)
        : null,
      h("div", { class: "row" },
        h("div", { class: "row-text" }, h("span", { class: "row-label" }, "Add modifiers"), h("span", { class: "row-help" }, "held together with the key, e.g. 1 + LShift gives !")),
      ),
      h("div", { class: "mods" }, MOD_BITS.map((m) => h("button", {
        type: "button", class: mask & m.bit ? "chip active" : "chip", "aria-pressed": String(!!(mask & m.bit)), dataset: { fid: `mod-${m.bit}` },
        onclick: () => void app.edit("modifier mask", (d) => { d.modMask[layer][key] = mask ^ m.bit; }),
      }, m.label))),
      mask & 0x88 ? help("Win lock still blocks the Win bits.") : null,
    ),
  );
}

export const keymapPage: Page = {
  id: "keymap",
  title: "Keymap",
  group: 1,
  render(app) {
    const layer = layerOf(app);
    const c = app.config;
    const key = [...app.selection][0] ?? null;
    const mainIsFactory = c.keymap.main.every((code, k) => code === DEFAULT_KEYMAP[k]);
    return h("div", { class: "page" },
      h("div", { class: "toolbar" },
        segmented<Layer>([
          { value: "main", label: "Main layer" },
          { value: "fn", label: "Fn layer", title: "Active while a key mapped to Fn is held" },
        ], layer, (l) => { app.ui.layer = l; app.render(); }),
        layer === "main"
          ? h("button", {
              type: "button", class: "btn small", disabled: mainIsFactory, title: "Every key of the main layer back to its factory code. Tap-hold and modifiers stay. Undoable.",
              onclick: () => void app.edit("keymap reset", (d) => { d.keymap.main = [...DEFAULT_KEYMAP]; }),
            }, "Reset main layer")
          : h("span", { class: "help" }, "Empty keys send nothing while Fn is held."),
      ),
      app.stage({
        select: "single",
        hint: key === null ? "Click a key to change what it sends. Right-click clears it." : null,
        onContextMenu: (i) => {
          app.selection.clear();
          app.selection.add(i);
          void app.edit("keymap", (d) => { d.keymap[layer][i] = KEYCODE_EMPTY; });
        },
        visual: (i) => {
          const code = c.keymap[layer][i];
          const name = KEY_LAYOUT[i].name;
          const hold = c.holdCode[layer][i];
          const mask = c.modMask[layer][i];
          const saved = app.store.saved!;
          const changed = code !== saved.keymap[layer][i] || hold !== saved.holdCode[layer][i] || mask !== saved.modMask[layer][i];
          if (code === KEYCODE_EMPTY) return { label: "", sub: layer === "fn" ? name : "empty", state: "dim", title: `${name}: nothing on this layer`, dot: changed ? "unsaved" : null };
          const extra = [hold !== KEYCODE_EMPTY ? `hold ${keyTitle(hold)}` : "", mask ? `+ ${maskText(mask)}` : ""].filter(Boolean).join(", ");
          return {
            label: keyLabel(code),
            sub: hold !== KEYCODE_EMPTY ? `hold ${keyLabel(hold)}` : mask ? `+${maskText(mask)}` : layer === "fn" ? name : undefined,
            badge: hold !== KEYCODE_EMPTY ? "TH" : mask ? "M" : undefined,
            state: layer === "fn" ? "accent" : null,
            dot: changed ? "unsaved" : null,
            title: `${name} sends ${keyTitle(code)}${extra ? ` (${extra})` : ""}${changed ? ", not saved yet" : ""}`,
          };
        },
      }),
      key === null
        ? null
        : h("div", { class: "grid two" },
            inspector(app, key, layer),
            card("Pick what it sends",
              keyPicker({ current: c.keymap[layer][key], state: (app.ui.picker ??= {}), onPick: (code) => void app.edit("keymap", (d) => { d.keymap[layer][key] = code; }) }),
            ),
          ),
    );
  },
};
