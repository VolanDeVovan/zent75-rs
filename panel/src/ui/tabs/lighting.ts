import { EFFECTS } from "../../protocol/keycodes";
import { KEY_LAYOUT, ledsOfKey, primaryLedOfKey } from "../../protocol/layout";
import type { App } from "../app";
import { h, hexToRgb, rgbToHex } from "../dom";
import type { TabDef } from "../tabs";
import { numberField, quickSelect, section, toggle } from "../widgets";

interface Draft { hue: number; saturation: number; brightness: number; speed: number; [k: string]: number }

function draftOf(app: App): Draft {
  const l = app.config!.lighting;
  return (app.ui.light ??= { hue: l.hue, saturation: l.saturation, brightness: l.brightness, speed: l.speed });
}

export const lightingTab: TabDef = {
  id: "lighting",
  title: "Lighting",
  hint: "Select keys to give them a static colour, or return them to the dynamic effect.",
  visual(app, i) {
    const l = app.config!.lighting;
    const led = primaryLedOfKey(i);
    const solid = l.solid[led] === 1;
    return {
      label: KEY_LAYOUT[i].name,
      sub: solid ? "static" : "effect",
      color: solid ? rgbToHex(l.colors[led]) : null,
      state: solid ? null : "dim",
      title: `${KEY_LAYOUT[i].name}: LED ${ledsOfKey(i).join(", ")}, ${solid ? `static ${rgbToHex(l.colors[led])}` : "follows the dynamic effect"}`,
    };
  },
  onKeyClick(app, i) {
    if (app.selection.has(i)) app.selection.delete(i);
    else app.selection.add(i);
    app.render();
  },
  render(app) {
    const l = app.config!.lighting;
    const d = draftOf(app);
    const none = app.selection.size === 0;
    const color = h("input", { type: "color", class: "color", value: app.ui.lightColor ?? "#00c8ff", oninput: () => (app.ui.lightColor = color.value) });
    const effect = h("select", {
      class: "input",
      disabled: !!app.busy,
      onchange: () => void app.edit("Writing effect…", (c) => {
        c.lighting.effect = Number(effect.value);
        // the vendor driver always writes 1 into the low bits
        c.lighting.effectFlags = 1;
      }),
    }, EFFECTS.map((e) => h("option", { value: String(e.id) }, `${e.id} · ${e.name}`)));
    if (!EFFECTS.some((e) => e.id === l.effect)) effect.appendChild(h("option", { value: String(l.effect) }, `${l.effect} · (unlisted)`));
    effect.value = String(l.effect);

    const setSolid = (solid: boolean) => {
      const keys = [...app.selection];
      const rgb = hexToRgb(color.value);
      void app.edit("Writing colours…", (c) => {
        for (const k of keys) {
          for (const led of ledsOfKey(k)) {
            c.lighting.solid[led] = solid ? 1 : 0;
            if (solid) c.lighting.colors[led] = { ...rgb };
          }
        }
      });
    };

    return h("div", null,
      quickSelect(app),
      h("div", { class: "cols" },
        h("div", { class: "col narrow" },
          section("Switches",
            toggle("Lighting on", l.enabled, (v) => void app.edit("Writing…", (c) => {
              c.lighting.enabled = v;
            }), undefined, !!app.busy),
            toggle("Caps Lock indicator", l.capsIndicator, (v) => void app.edit("Writing…", (c) => {
              c.lighting.capsIndicator = v;
            }), undefined, !!app.busy),
            toggle("Separate Fn-layer lighting", l.fnLayerLighting, (v) => void app.edit("Writing…", (c) => {
              c.lighting.fnLayerLighting = v;
            }), "highlights the Fn bindings while Fn is held", !!app.busy),
          ),
        ),
        h("div", { class: "col" },
          section("Dynamic effect",
            h("label", { class: "field" }, h("span", { class: "field-label" }, "Effect"), effect),
            numberField("Brightness", d, "brightness", { min: 0, max: 30, step: 1, help: "above 10 a weak USB port may brown out" }),
            numberField("Speed", d, "speed", { min: 0, max: 255, step: 1 }),
            numberField("Hue", d, "hue", { min: 0, max: 255, step: 1 }),
            numberField("Saturation", d, "saturation", { min: 0, max: 255, step: 1 }),
            h("div", { class: "row end" },
              h("button", {
                class: "btn primary", disabled: !!app.busy,
                onclick: () => void app.edit("Writing lighting…", (c) => {
                  Object.assign(c.lighting, {
                    hue: Math.round(d.hue), saturation: Math.round(d.saturation), brightness: Math.round(d.brightness), speed: Math.round(d.speed),
                  });
                }).then(() => (app.ui.light = null)),
              }, "Apply effect settings"),
            ),
          ),
        ),
        h("div", { class: "col narrow" },
          section("Static colour",
            h("div", { class: "row" }, color, h("span", { class: "help" }, "for the selected keys")),
            h("div", { class: "row" },
              h("button", { class: "btn primary", disabled: none || !!app.busy, onclick: () => setSolid(true) }, "Set static colour"),
              h("button", { class: "btn", disabled: none || !!app.busy, onclick: () => setSolid(false) }, "Back to effect"),
            ),
            h("p", { class: "help" }, "85 LEDs: one per key, three under the space bar."),
          ),
        ),
      ),
    );
  },
};
