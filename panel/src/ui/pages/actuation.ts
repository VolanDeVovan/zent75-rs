/**
 * Actuation: global first, per key second. With no key selected the controls edit every key
 * that follows the global values; with a selection they edit those keys. Sliders write on
 * release; only the field that moved is written.
 */
import { switchType } from "../../protocol/keycodes";
import { KEY_LAYOUT } from "../../protocol/layout";
import type { App, Page } from "../app";
import { card, cardWith, disclosure, help, linkButton, segmented, selectionBar, slider, travelGauge } from "../components";
import { FACTORY_FEEL } from "../defaults";
import { h, mm, plural } from "../dom";
import {
  MODES, customisedKeys, feelOf, globalFeel, globalKeys, patchFeel, sameFeel, setFeel, usesFixed, usesRt, validateFeel, type KeyFeel,
} from "../model";

export function travelOfKey(app: App, key: number): number {
  return switchType(app.calibration.switchType[key] ?? 0, app.customSwitches).travelUm;
}

/** Apply `patch` to `keys` if every key stays valid; otherwise say why and write nothing. */
export function applyFeel(app: App, keys: readonly number[], patch: Partial<KeyFeel>): void {
  const c = app.config;
  for (const k of keys) {
    const errors = validateFeel(patchFeel(feelOf(c, k), patch), travelOfKey(app, k));
    if (errors.length) {
      app.toast("error", `${keys.length > 1 ? `${KEY_LAYOUT[k].name}: ` : ""}${errors[0]}`);
      app.render();
      return;
    }
  }
  void app.edit("actuation", (d) => {
    for (const k of keys) setFeel(d, k, patchFeel(feelOf(d, k), patch));
  });
}

export function keyNames(keys: readonly number[], max = 6): string {
  const names = keys.slice(0, max).map((k) => KEY_LAYOUT[k].name);
  return keys.length > max ? `${names.join(", ")} and ${keys.length - max} more` : names.join(", ");
}

const um = (v: number) => Math.round(v * 1000);

export const actuationPage: Page = {
  id: "actuation",
  title: "Actuation",
  group: 0,
  render(app) {
    const c = app.config;
    const global = globalFeel(c);
    const custom = customisedKeys(c);
    const customSet = new Set(custom);
    const sel = [...app.selection];
    const targets = sel.length ? sel : globalKeys(c);
    const shown = sel.length ? feelOf(c, sel[0]) : global;
    const mixed = (field: keyof KeyFeel) => sel.some((k) => feelOf(c, k)[field] !== shown[field]);
    const travel = Math.min(...targets.map((k) => travelOfKey(app, k)));
    const rt = usesRt(shown.mode);
    const fixed = usesFixed(shown.mode);
    const gauge = travelGauge(shown, travel);
    const preview = (patch: Partial<KeyFeel>) => gauge.update(patchFeel(shown, patch));
    const set = (patch: Partial<KeyFeel>) => applyFeel(app, targets, patch);

    const field = (fid: string, label: string, key: Exclude<keyof KeyFeel, "mode">, o: { min: number; max: number; step: number; fine: number; digits: number; help?: string }) =>
      slider({
        fid, label, help: o.help, value: shown[key] / 1000, min: o.min, max: o.max, step: o.step, fineStep: o.fine, digits: o.digits, unit: "mm",
        factory: FACTORY_FEEL[key] / 1000, mixed: mixed(key),
        onInput: (v) => preview({ [key]: um(v) }),
        onCommit: (v) => set({ [key]: um(v) }),
      });
    const maxPoint = Math.floor((travel - 10) / 10) / 100;

    const selCustom = sel.filter((k) => customSet.has(k));
    const allFactory = c.keyMode.every((_, k) => sameFeel(feelOf(c, k), FACTORY_FEEL));

    return h("div", { class: "page" },
      app.stage({
        select: "multi",
        hint: sel.length ? null : "Click or drag across keys to give them their own values (marked with a dot). With nothing selected you are setting all keys.",
        visual: (i) => {
          const f = feelOf(c, i);
          const mode = MODES.find((m) => m.mode === f.mode);
          const isRt = usesRt(f.mode);
          return {
            label: KEY_LAYOUT[i].name,
            sub: usesFixed(f.mode) ? mm(f.actuationUm) : `±${mm(f.rtPressUm)}`,
            badge: mode?.short || undefined,
            state: isRt ? "accent" : null,
            dot: customSet.has(i) ? "custom" : null,
            title: `${KEY_LAYOUT[i].name}: ${mode?.label ?? `mode ${f.mode}`}, press ${mm(f.actuationUm)} mm, release ${mm(f.releaseUm)} mm` +
              (isRt ? `, rapid trigger ${mm(f.rtPressUm, 3)} / ${mm(f.rtReleaseUm, 3)} mm, dead zones ${mm(f.topDeadzoneUm, 3)} / ${mm(f.bottomDeadzoneUm, 3)} mm` : "") +
              (customSet.has(i) ? " (customised)" : ""),
          };
        },
      }),
      selectionBar(app, { noneText: custom.length ? `Setting all keys except ${plural(custom.length, "customised key")}` : "Setting all keys", customised: true }),
      h("div", { class: "grid two" },
        cardWith(sel.length ? keyNames(sel) : "All keys",
          selCustom.length
            ? linkButton("Use the global values", () => void app.edit("actuation", (d) => {
                const g = globalFeel(d);
                for (const k of sel) setFeel(d, k, g);
              }), "Give the selected keys the same values as all other keys")
            : null,
          segmented(MODES.map((m) => ({ value: m.mode, label: m.label, title: m.help })), shown.mode, (mode) => set({ mode })),
          mixed("mode") ? help("The selected keys use different modes; this shows the first one.") : help(MODES.find((m) => m.mode === shown.mode)?.help ?? `Unknown mode ${shown.mode}.`),
          gauge.element,
        ),
        card(null,
          fixed ? field("act", "Actuation point", "actuationUm", { min: 0.1, max: maxPoint, step: 0.05, fine: 0.01, digits: 2, help: "how deep the key goes before it presses" }) : null,
          rt ? field("rtp", "Press sensitivity", "rtPressUm", { min: 0.01, max: 1, step: 0.01, fine: 0.005, digits: 3, help: "movement down that presses again" }) : null,
          rt ? field("rtr", "Release sensitivity", "rtReleaseUm", { min: 0.01, max: 1, step: 0.01, fine: 0.005, digits: 3, help: "movement up that releases" }) : null,
          rt ? help("Below 0.10 mm keys can chatter; raise the values if you see double presses.") : null,
          disclosure(app, "act-fine", "Fine tuning",
            field("rel", "Release point", "releaseUm", { min: 0.1, max: maxPoint, step: 0.05, fine: 0.01, digits: 2,
              help: fixed ? "never deeper than the actuation point" : "kept for when the key goes back to Normal" }),
            !fixed ? field("act", "Actuation point", "actuationUm", { min: 0.1, max: maxPoint, step: 0.05, fine: 0.01, digits: 2, help: "kept for when the key goes back to Normal" }) : null,
            rt ? field("dzt", "Top dead zone", "topDeadzoneUm", { min: 0, max: 1, step: 0.01, fine: 0.005, digits: 3, help: "ignored travel at the top" }) : null,
            rt ? field("dzb", "Bottom dead zone", "bottomDeadzoneUm", { min: 0, max: 1, step: 0.01, fine: 0.005, digits: 3, help: "ignored travel at the bottom" }) : null,
          ),
        ),
      ),
      h("div", { class: "page-foot" },
        h("button", {
          type: "button", class: "btn", disabled: allFactory,
          title: "Normal mode, 1.60 mm press, 0.80 mm release on every key, customised ones too. Undoable.",
          onclick: () => void app.edit("actuation reset", (d) => {
            for (let k = 0; k < d.keyMode.length; k++) setFeel(d, k, { ...FACTORY_FEEL });
          }),
        }, "Reset actuation to factory values"),
        h("span", { class: "help" }, "Different switch under some keys? Set it in ", h("a", { href: "#/sensors" }, "Sensors"), "."),
      ),
    );
  },
};
