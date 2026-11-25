/** Home: the state of the keyboard at a glance and the three things that change most often. */
import { KEY_MODE_NORMAL, KEY_MODE_RAPID_TRIGGER } from "../../protocol/keyboard";
import { EFFECTS, switchType } from "../../protocol/keycodes";
import { KEY_LAYOUT, primaryLedOfKey } from "../../protocol/layout";
import type { Page } from "../app";
import { cardWith, help, row, segmented, slider, toggle } from "../components";
import { FACTORY_FEEL, FACTORY_LIGHTING } from "../defaults";
import { h, mm, plural, rgbToHex } from "../dom";
import { MODES, customisedKeys, feelOf, globalFeel, globalKeys, isSocdPair, setSocdPair, socdRules, usesFixed, usesRt } from "../model";
import { applyFeel, keyNames, travelOfKey } from "./actuation";
import { effectSelect } from "./lighting";
import { SOCD_PRESETS } from "./socd";

const go = (page: string, label: string) => h("a", { class: "link", href: `#/${page}` }, label);

export const homePage: Page = {
  id: "home",
  title: "Home",
  group: 0,
  render(app) {
    const c = app.config;
    const s = app.store;
    const global = globalFeel(c);
    const followers = globalKeys(c);
    const custom = customisedKeys(c);
    const customSet = new Set(custom);
    const travel = Math.min(...followers.map((k) => travelOfKey(app, k)));
    const modes = MODES.filter((m) => m.mode === KEY_MODE_NORMAL || m.mode === KEY_MODE_RAPID_TRIGGER || m.mode === global.mode);
    const l = c.lighting;
    const rules = socdRules(c);
    const presetCount = SOCD_PRESETS.filter((p) => isSocdPair(c, p.a, p.b)).length;
    const otherRules = rules.pairs.length - presetCount + rules.oneWay.length;
    const cal = app.calibration;
    const weak = cal.top.filter((top, k) => top - cal.bottom[k] < switchType(cal.switchType[k]).calibSpan).length;

    return h("div", { class: "page" },
      h("div", { class: "page-head" },
        h("h1", null, s.profileNames[s.activeProfile] || `Profile ${s.activeProfile + 1}`),
        h("p", { class: "page-sub" }, `Profile ${s.activeProfile + 1} of ${s.profileCount}. Changes work on the keyboard at once; save them to keep them after a replug.`),
      ),
      app.stage({
        select: "none",
        visual: (i) => {
          const f = feelOf(c, i);
          const led = primaryLedOfKey(i);
          const pair = rules.pairs.find((p) => p.a === i || p.b === i);
          return {
            label: KEY_LAYOUT[i].name,
            // only what stands out: keys with their own values
            sub: customSet.has(i) ? (usesFixed(f.mode) ? mm(f.actuationUm) : `±${mm(f.rtPressUm)}`) : undefined,
            state: usesRt(f.mode) ? "accent" : null,
            badge: pair ? "⇄" : undefined,
            tint: l.enabled && l.solid[led] === 1 ? rgbToHex(l.colors[led]) : null,
            title: `${KEY_LAYOUT[i].name}: ${MODES.find((m) => m.mode === f.mode)?.label ?? "?"}, ${mm(f.actuationUm)} mm` +
              (pair ? `, SOCD pair with ${KEY_LAYOUT[pair.a === i ? pair.b : pair.a].name}` : ""),
          };
        },
      }),
      h("div", { class: "grid three" },
        cardWith("Typing feel", go("actuation", "Per key"),
          segmented(modes.map((m) => ({ value: m.mode, label: m.mode === KEY_MODE_NORMAL || m.mode === KEY_MODE_RAPID_TRIGGER ? m.label : "RT below", title: m.help })), global.mode,
            (mode) => applyFeel(app, followers, { mode })),
          usesFixed(global.mode)
            ? slider({
                fid: "home-act", label: "Actuation point", value: global.actuationUm / 1000, min: 0.1, max: Math.floor((travel - 10) / 10) / 100, step: 0.05, fineStep: 0.01, digits: 2, unit: "mm",
                factory: FACTORY_FEEL.actuationUm / 1000,
                onCommit: (v) => applyFeel(app, followers, { actuationUm: Math.round(v * 1000) }),
              })
            : slider({
                fid: "home-rt", label: "Sensitivity", help: "movement that presses and releases", value: global.rtPressUm / 1000, min: 0.01, max: 1, step: 0.01, fineStep: 0.005, digits: 3, unit: "mm",
                onCommit: (v) => applyFeel(app, followers, { rtPressUm: Math.round(v * 1000), rtReleaseUm: Math.round(v * 1000) }),
              }),
          help(custom.length
            ? `Sets ${plural(followers.length, "key")}. ${keyNames(custom, 4)} ${custom.length === 1 ? "has its" : "have their"} own values.`
            : "Sets all keys."),
        ),
        cardWith("Lighting", go("lighting", "Colours and more"),
          row("Lighting", toggle("home-light", l.enabled, (v) => void app.edit("lighting", (d) => { d.lighting.enabled = v; }), "Lighting")),
          slider({
            fid: "home-bri", label: "Brightness", value: l.brightness, min: 0, max: 30, step: 1, factory: FACTORY_LIGHTING.brightness,
            onCommit: (v) => void app.edit("brightness", (d) => { d.lighting.brightness = Math.round(v); }),
          }),
          row("Effect", effectSelect(app, "home-effect")),
        ),
        cardWith("Gaming", go("socd", otherRules > 0 ? `${plural(otherRules, "more rule")}` : "More pairs"),
          SOCD_PRESETS.slice(0, 2).map((p) => row(p.label,
            toggle(`home-socd-${p.a}`, isSocdPair(c, p.a, p.b), (v) => void app.edit("SOCD", (d) => setSocdPair(d, p.a, p.b, v)), `SOCD ${p.label}`),
            "the later key wins instead of cancelling")),
          row("Win key lock", toggle("home-winlock", c.winLock, (v) => void app.edit("Win lock", (d) => { d.winLock = v; }), "Win key lock"), "Win keys do nothing"),
        ),
      ),
      h("p", { class: "status-line" },
        weak
          ? h("span", { class: "warn-text" }, `${plural(weak, "key")} ${weak === 1 ? "looks" : "look"} uncalibrated. `, go("sensors", "Calibrate"))
          : h("span", null, `All ${cal.top.length} keys are calibrated. `, go("sensors", "Live travel and calibration")),
        h("span", null, `Effect: ${EFFECTS.find((e) => e.id === l.effect)?.name ?? `#${l.effect}`}. Sleeps after ${c.sleepSeconds ? `${Math.round(c.sleepSeconds / 60)} min` : "never"}. `, go("device", "Device")),
      ),
    );
  },
};
