import { KEYCODE_EMPTY, MOD_BITS, TAP_HOLD_MS } from "../../protocol/keyboard";
import { keyLabel, keyTitle } from "../../protocol/keycodes";
import { DEFAULT_KEYMAP, FN_FIXED_KEYS, KEY_LAYOUT } from "../../protocol/layout";
import type { App } from "../app";
import { h } from "../dom";
import type { TabDef } from "../tabs";
import { keySelect, keyPicker, section } from "../widgets";

type Layer = "main" | "fn";
const layerOf = (app: App): Layer => (app.ui.layer ??= "main");

function assign(app: App, key: number, code: number): void {
  const layer = layerOf(app);
  void app.edit("Writing key…", (c) => {
    c.keymap[layer][key] = code;
  });
}

function setHold(app: App, key: number, code: number): void {
  const layer = layerOf(app);
  void app.edit("Writing hold code…", (c) => {
    c.holdCode[layer][key] = code;
  });
}

function setMask(app: App, key: number, mask: number): void {
  const layer = layerOf(app);
  void app.edit("Writing modifier mask…", (c) => {
    c.modMask[layer][key] = mask;
  });
}

const maskText = (mask: number) => MOD_BITS.filter((m) => mask & m.bit).map((m) => m.label).join("+");

/** Tap-hold code and modifier mask of one key on the current layer. */
function tapHoldSection(app: App, key: number): HTMLElement {
  const layer = layerOf(app);
  const cfg = app.config!;
  const hold = cfg.holdCode[layer][key];
  const mask = cfg.modMask[layer][key];
  const busy = !!app.busy;
  const tap = cfg.keymap[layer][key];
  return section(`Tap-hold and modifiers (${layer === "fn" ? "Fn" : "main"} layer)`,
    h("label", { class: "field" },
      h("span", { class: "field-label" }, "Hold code", h("small", null, "sent while the key is held longer than 0.5 s")),
      keySelect(hold, (code) => setHold(app, key, code), busy),
    ),
    hold !== KEYCODE_EMPTY
      ? h("p", { class: "help" },
          `Tap → ${keyLabel(tap) || "nothing"}, hold → ${keyLabel(hold)}. While pressed the key sends nothing; released within ${TAP_HOLD_MS} ms it sends `,
          h("code", null, keyLabel(tap) || "nothing"),
          ` once (a tap), held longer it sends `, h("code", null, keyLabel(hold)), ` until released. The ${TAP_HOLD_MS} ms are fixed in the firmware. The tap therefore arrives on release, not on press.`)
      : h("p", { class: "help" }, `Off (“empty”): the key sends its code on press as usual. Pick a code to make it a tap-hold key: tap = the code above, hold (> ${TAP_HOLD_MS} ms) = this code.`),
    hold !== KEYCODE_EMPTY ? h("button", { class: "btn small", disabled: busy, onclick: () => setHold(app, key, KEYCODE_EMPTY) }, "Tap-hold off") : null,
    h("div", { class: "field" },
      h("span", { class: "field-label" }, "Modifier mask", h("small", null, "added to whatever the key sends, while it is pressed")),
      h("div", { class: "row wrap" },
        MOD_BITS.map((m) => {
          const input = h("input", {
            type: "checkbox", checked: (mask & m.bit) !== 0, disabled: busy,
            onchange: () => setMask(app, key, input.checked ? mask | m.bit : mask & ~m.bit),
          });
          return h("label", { class: "toggle inline" }, input, h("span", null, m.label));
        }),
      ),
    ),
    h("p", { class: "help" }, mask
      ? `While pressed the key also holds ${maskText(mask)} (e.g. a key sending “1” with LShift gives “!”). Applies during the tap-hold wait as well. Win lock still blocks the Win bits.`
      : "None. Set bits to send modifiers together with this key's code."),
  );
}

export const keymapTab: TabDef = {
  id: "keymap",
  title: "Keymap",
  hint: "Click a key, then pick what it should send. Right-click a key to clear it.",
  visual(app, i) {
    const layer = layerOf(app);
    const code = app.config!.keymap[layer][i];
    const name = KEY_LAYOUT[i].name;
    if (layer === "fn" && code === KEYCODE_EMPTY) {
      return { label: "", sub: name, state: "dim", title: `${name}: nothing on the Fn layer` };
    }
    const label = keyLabel(code);
    const hold = app.config!.holdCode[layer][i];
    const mask = app.config!.modMask[layer][i];
    const extra = [
      hold !== KEYCODE_EMPTY ? `hold ${keyTitle(hold)}` : "",
      mask ? `+ ${maskText(mask)}` : "",
    ].filter(Boolean).join(", ");
    return {
      label,
      sub: hold !== KEYCODE_EMPTY ? `hold ${keyLabel(hold)}` : mask ? `+${maskText(mask)}` : layer === "fn" ? name : undefined,
      badge: hold !== KEYCODE_EMPTY ? (mask ? "TH+M" : "TH") : mask ? "M" : undefined,
      state: layer === "fn" ? "accent" : code === KEYCODE_EMPTY ? "dim" : null,
      title: `${name} (key ${i}) → ${keyTitle(code)}${extra ? ` (${extra})` : ""}`,
    };
  },
  onKeyClick(app, i) {
    const was = app.selection.has(i);
    app.selection.clear();
    if (!was) app.selection.add(i);
    app.render();
  },
  onKeyContext(app, i) {
    app.selection.clear();
    app.selection.add(i);
    assign(app, i, KEYCODE_EMPTY);
  },
  render(app) {
    const layer = layerOf(app);
    const key = [...app.selection][0] ?? null;
    const cfg = app.config!;
    const current = key === null ? null : cfg.keymap[layer][key];
    const setLayer = (l: Layer) => {
      app.ui.layer = l;
      app.render();
    };
    const custom = h("input", { type: "number", class: "input num", min: "-8", max: "32767", step: "1", placeholder: "code", value: current ?? "" });
    return h("div", { class: "cols" },
      h("div", { class: "col narrow" },
        section("Layer",
          h("div", { class: "segmented" },
            h("button", { class: layer === "main" ? "seg active" : "seg", onclick: () => setLayer("main") }, "Main"),
            h("button", { class: layer === "fn" ? "seg active" : "seg", onclick: () => setLayer("fn") }, "Fn"),
          ),
          h("p", { class: "help" }, layer === "fn"
            ? "Active while a key mapped to Fn is held. Empty slots send nothing."
            : "The base layer. Put “Fn” on a key to reach the Fn layer."),
        ),
        section("Selected key",
          key === null
            ? h("p", { class: "help" }, "No key selected.")
            : h("div", null,
                h("p", { class: "kv" }, h("b", null, KEY_LAYOUT[key].name), ` · index ${key}`),
                h("p", { class: "kv" }, "Main: ", h("code", null, keyLabel(cfg.keymap.main[key]) || "empty"), "  Fn: ", h("code", null, keyLabel(cfg.keymap.fn[key]) || "empty")),
                layer === "fn" && FN_FIXED_KEYS.includes(key)
                  ? h("p", { class: "warn-text" }, "The vendor driver treats this key's Fn binding as fixed (built-in shortcut). Changing it works, but you lose that shortcut.")
                  : null,
                h("div", { class: "row" },
                  h("button", { class: "btn small", disabled: !!app.busy, onclick: () => assign(app, key, KEYCODE_EMPTY) }, "Clear"),
                  layer === "main"
                    ? h("button", { class: "btn small", disabled: !!app.busy, onclick: () => assign(app, key, DEFAULT_KEYMAP[key]) }, "Factory default")
                    : null,
                ),
                h("div", { class: "row" },
                  custom,
                  h("button", {
                    class: "btn small", disabled: !!app.busy,
                    title: "Assign a raw key code (signed 16-bit)",
                    onclick: () => {
                      const v = Number(custom.value);
                      if (Number.isInteger(v) && v >= -32768 && v <= 32767) assign(app, key, v);
                      else app.toast("error", "Key code must be an integer between -32768 and 32767.");
                    },
                  }, "Set raw code"),
                ),
              ),
        ),
        key === null ? null : tapHoldSection(app, key),
      ),
      h("div", { class: "col" },
        keyPicker(current, (code) => {
          if (key === null) app.toast("info", "Select a key on the keyboard first.");
          else assign(app, key, code);
        }, !!app.busy),
      ),
    );
  },
};
