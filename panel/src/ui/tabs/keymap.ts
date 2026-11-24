import { KEYCODE_EMPTY } from "../../protocol/keyboard";
import { keyLabel, keyTitle } from "../../protocol/keycodes";
import { DEFAULT_KEYMAP, FN_FIXED_KEYS, KEY_LAYOUT } from "../../protocol/layout";
import type { App } from "../app";
import { h } from "../dom";
import type { TabDef } from "../tabs";
import { keyPicker, section } from "../widgets";

type Layer = "main" | "fn";
const layerOf = (app: App): Layer => (app.ui.layer ??= "main");

function assign(app: App, key: number, code: number): void {
  const layer = layerOf(app);
  void app.edit("Writing key…", (c) => {
    c.keymap[layer][key] = code;
  });
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
    return {
      label,
      sub: layer === "fn" ? name : undefined,
      state: layer === "fn" ? "accent" : code === KEYCODE_EMPTY ? "dim" : null,
      title: `${name} (key ${i}) → ${keyTitle(code)}`,
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
