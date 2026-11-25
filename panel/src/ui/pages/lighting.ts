/** Lighting: the effect for the whole board, fixed colours for chosen keys. */
import { EFFECTS } from "../../protocol/keycodes";
import { KEY_LAYOUT, ledsOfKey, primaryLedOfKey } from "../../protocol/layout";
import type { App, Page } from "../app";
import { card, cardWith, help, linkButton, row, selectionBar, slider, toggle } from "../components";
import { FACTORY_LIGHTING } from "../defaults";
import { h, hexToRgb, plural, rgbToHex } from "../dom";

const PALETTE = ["#ffffff", "#ff3b30", "#ff9500", "#ffd60a", "#34c759", "#00c8ff", "#3478f6", "#bf5af2", "#ff2d92"];

export function effectSelect(app: App, fid: string): HTMLElement {
  const l = app.config.lighting;
  const select = h("select", {
    class: "input", "aria-label": "Effect", dataset: { fid },
    onchange: () => void app.edit("lighting effect", (d) => {
      d.lighting.effect = Number(select.value);
      // the vendor driver always writes 1 into the low bits
      d.lighting.effectFlags = 1;
    }),
  }, EFFECTS.map((e) => h("option", { value: String(e.id) }, e.name)));
  if (!EFFECTS.some((e) => e.id === l.effect)) select.appendChild(h("option", { value: String(l.effect) }, `Effect ${l.effect} (unlisted)`));
  select.value = String(l.effect);
  return select;
}

export const lightingPage: Page = {
  id: "lighting",
  title: "Lighting",
  group: 0,
  render(app) {
    const l = app.config.lighting;
    const sel = [...app.selection];
    const staticKeys = KEY_LAYOUT.filter((k) => l.solid[primaryLedOfKey(k.index)] === 1).length;
    const light = (fid: string, label: string, key: "brightness" | "speed" | "hue" | "saturation", max: number, extra: { help?: string; trackClass?: string } = {}) =>
      slider({
        fid, label, value: l[key], min: 0, max, step: 1, factory: FACTORY_LIGHTING[key], ...extra,
        onCommit: (v) => void app.edit(label.toLowerCase(), (d) => { d.lighting[key] = Math.round(v); }),
      });
    const paint = (hex: string | null) => {
      if (!sel.length) return app.toast("info", "Select keys on the keyboard first.");
      void app.edit(hex ? "key colour" : "key colour removal", (d) => {
        for (const k of sel) {
          for (const led of ledsOfKey(k)) {
            d.lighting.solid[led] = hex ? 1 : 0;
            if (hex) d.lighting.colors[led] = hexToRgb(hex);
          }
        }
      });
    };
    const custom = h("input", {
      type: "color", class: "color", value: app.ui.lightColor ?? "#00c8ff", "aria-label": "Custom colour", title: "Any colour",
      onchange: () => { app.ui.lightColor = custom.value; paint(custom.value); },
    });
    const isFactory = (Object.keys(FACTORY_LIGHTING) as (keyof typeof FACTORY_LIGHTING)[]).every((k) => l[k] === FACTORY_LIGHTING[k]) && l.solid.every((v) => v === 0);

    return h("div", { class: "page" },
      app.stage({
        select: "multi",
        hint: sel.length ? null : "Select keys to give them a fixed colour. Keys without one follow the effect.",
        visual: (i) => {
          const led = primaryLedOfKey(i);
          const solid = l.solid[led] === 1;
          return {
            label: KEY_LAYOUT[i].name,
            tint: solid ? rgbToHex(l.colors[led]) : null,
            title: `${KEY_LAYOUT[i].name}: ${solid ? `fixed colour ${rgbToHex(l.colors[led])}` : "follows the effect"}`,
          };
        },
      }),
      selectionBar(app, { noneText: staticKeys ? `${plural(staticKeys, "key")} with a fixed colour` : "All keys follow the effect" }),
      h("div", { class: "grid two" },
        cardWith("Effect", toggle("light-on", l.enabled, (v) => void app.edit("lighting", (d) => { d.lighting.enabled = v; }), "Lighting on"),
          row("Effect", effectSelect(app, "light-effect")),
          light("bri", "Brightness", "brightness", 30, { help: l.brightness > 10 ? "above 10 a weak USB port may brown out" : undefined }),
          light("spd", "Speed", "speed", 255),
          light("hue", "Hue", "hue", 255, { trackClass: "hue" }),
          light("sat", "Saturation", "saturation", 255),
          help("Hue and saturation apply to the single-colour effects."),
        ),
        h("div", { class: "stack-cards" },
          cardWith("Key colour",
            sel.length && sel.some((k) => l.solid[primaryLedOfKey(k)] === 1) ? linkButton("Back to effect", () => paint(null), "Remove the fixed colour from the selected keys") : null,
            h("div", { class: "swatches" },
              PALETTE.map((hex) => h("button", {
                type: "button", class: "swatch", style: { background: hex }, title: hex, "aria-label": `Colour ${hex}`, disabled: !sel.length, onclick: () => paint(hex),
              })),
              h("label", { class: "swatch-other" }, custom, "Other"),
            ),
            help(sel.length ? `Click a colour to set it on ${plural(sel.length, "selected key")}.` : "Select keys on the keyboard, then click a colour."),
          ),
          card("Indicators",
            row("Caps Lock indicator", toggle("light-caps", l.capsIndicator, (v) => void app.edit("Caps indicator", (d) => { d.lighting.capsIndicator = v; }), "Caps Lock indicator")),
            row("Fn-layer lighting", toggle("light-fn", l.fnLayerLighting, (v) => void app.edit("Fn-layer lighting", (d) => { d.lighting.fnLayerLighting = v; }), "Fn-layer lighting"),
              "highlights the Fn shortcuts while Fn is held"),
          ),
        ),
      ),
      h("div", { class: "page-foot" },
        h("button", {
          type: "button", class: "btn", disabled: isFactory, title: "Factory effect, brightness, speed and colour; no fixed key colours. Undoable.",
          onclick: () => void app.edit("lighting reset", (d) => {
            Object.assign(d.lighting, FACTORY_LIGHTING);
            d.lighting.solid.fill(0);
            for (const col of d.lighting.colors) col.r = col.g = col.b = 255;
          }),
        }, "Reset lighting to factory values"),
      ),
    );
  },
};
