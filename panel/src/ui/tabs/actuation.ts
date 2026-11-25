import {
  CURVE_LAST, CURVE_MAX_POINTS, KEY_MODE_NORMAL, KEY_MODE_RAPID_TRIGGER, KEY_MODE_RT_BELOW, type CustomSwitch,
} from "../../protocol/keyboard";
import { SWITCH_TYPES, switchType } from "../../protocol/keycodes";
import { KEY_LAYOUT } from "../../protocol/layout";
import {
  DEFAULT_CURVE, TRAVEL_MAX_UM, TRAVEL_MIN_UM, customSwitchCode, defaultCustomSwitch, parseCurve, pointTravelUm,
  pointsForTravel, resampleCurve, validateCustomSwitch, withCurve,
} from "../../protocol/switches";
import type { App } from "../app";
import { confirmDialog, h, hexToRgb, mm, rgbToHex } from "../dom";
import type { TabDef } from "../tabs";
import { numberField, quickSelect, section } from "../widgets";

interface Draft { mode: number; actuation: number; release: number; rtPress: number; rtRelease: number; topDz: number; bottomDz: number; switchType: number; [k: string]: number }

const MODES = [
  { mode: KEY_MODE_NORMAL, label: "Normal", badge: "",
    help: "The key actuates at a fixed depth and releases when it comes back above the release point, like a mechanical switch." },
  { mode: KEY_MODE_RAPID_TRIGGER, label: "Rapid trigger", badge: "RT",
    help: "The key re-triggers as soon as it moves down by the press sensitivity and releases when it moves up by the release sensitivity, anywhere between the dead zones. The fixed points are not used." },
  { mode: KEY_MODE_RT_BELOW, label: "RT below actuation", badge: "RT½",
    help: "First press at the actuation point, like Normal. Below it the key works as rapid trigger: lifting by the release sensitivity releases it, pressing again by the press sensitivity re-presses it (not above the top dead zone). Only coming back above the release point resets it, after which the next press needs the actuation point again." },
] as const;
const usesRt = (mode: number) => mode === KEY_MODE_RAPID_TRIGGER || mode === KEY_MODE_RT_BELOW;
const usesFixed = (mode: number) => mode !== KEY_MODE_RAPID_TRIGGER;

function loadDraft(app: App, key: number): void {
  const c = app.config!;
  app.ui.act = {
    mode: c.keyMode[key],
    actuation: c.actuationUm[key] / 1000,
    release: c.releaseUm[key] / 1000,
    rtPress: c.rtPressUm[key] / 1000,
    rtRelease: c.rtReleaseUm[key] / 1000,
    topDz: c.topDeadzoneUm[key] / 1000,
    bottomDz: c.bottomDeadzoneUm[key] / 1000,
    switchType: app.calibration?.switchType[key] ?? 0,
  } satisfies Draft;
}

function draftOf(app: App): Draft {
  if (!app.ui.act) loadDraft(app, [...app.selection][0] ?? 0);
  return app.ui.act as Draft;
}

const um = (v: number) => Math.round(v * 1000);

function apply(app: App): void {
  const d = draftOf(app);
  const keys = [...app.selection];
  if (keys.length === 0) return app.toast("info", "Select at least one key.");
  const types = app.calibration?.switchType ?? [];
  const shortest = Math.min(...keys.map((k) => switchType(types[k] ?? 0, app.customSwitches).travelUm));
  const errors: string[] = [];
  // the fixed points are written in every mode (rapid trigger keeps them for a later mode change)
  if (um(d.actuation) >= shortest) errors.push(`Actuation point must be below the switch travel (${mm(shortest, 3)} mm).`);
  if (um(d.release) > um(d.actuation)) errors.push("Release point must not be deeper than the actuation point.");
  if (um(d.actuation) < 100) errors.push("Actuation point below 0.10 mm is not accepted by the vendor driver.");
  if (usesRt(d.mode)) {
    if (um(d.rtPress) < 10 || um(d.rtRelease) < 10) errors.push("Rapid-trigger sensitivity must be at least 0.01 mm.");
    if (um(d.rtPress) >= shortest) errors.push("Rapid-trigger press sensitivity exceeds the switch travel.");
    if (um(d.topDz) + um(d.bottomDz) >= shortest) errors.push("The dead zones cover the whole travel.");
  }
  if (d.mode === KEY_MODE_RT_BELOW && um(d.actuation) + um(d.bottomDz) >= shortest) {
    errors.push("The actuation point lies inside the bottom dead zone: there is no room for rapid trigger below it.");
  }
  if (errors.length) return app.toast("error", errors.join(" "));
  void app.edit("Writing actuation…", (c) => {
    for (const k of keys) {
      c.keyMode[k] = d.mode;
      c.actuationUm[k] = um(d.actuation);
      c.releaseUm[k] = um(d.release);
      if (usesRt(d.mode)) {
        c.rtPressUm[k] = um(d.rtPress);
        c.rtReleaseUm[k] = um(d.rtRelease);
        c.topDeadzoneUm[k] = um(d.topDz);
        c.bottomDeadzoneUm[k] = um(d.bottomDz);
      }
    }
  }).then(() => app.toast("ok", `Applied to ${keys.length} key${keys.length === 1 ? "" : "s"}.`));
}

async function applySwitchType(app: App): Promise<void> {
  const d = draftOf(app);
  const keys = [...app.selection];
  if (keys.length === 0) return app.toast("info", "Select at least one key.");
  const next = [...(app.calibration?.switchType ?? [])];
  for (const k of keys) next[k] = d.switchType;
  if (new Set(next).size > 4) return app.toast("error", "The stock firmware supports at most 4 different switch types at once.");
  const ok = await confirmDialog({
    title: "Change switch type",
    body: "The switch type selects the travel curve the firmware uses. It is board-global and the firmware saves it to flash at once. After changing it the affected keys should be recalibrated (Calibration tab). Continue?",
    confirmLabel: "Change",
  });
  if (!ok) return;
  await app.editCalibration("Writing switch type…", (c) => {
    c.switchType = next;
  });
}

// ------------------------------------------------------------ custom switch editor

interface CurveDraft {
  index: number;
  sw: CustomSwitch;
  text: string;
  parseError: string | null;
  /** travel the curve points were made for (what "Fit curve to travel" reshapes from) */
  curveTravelUm: number;
}

function loadCurveDraft(app: App, index: number): CurveDraft {
  const sw = structuredClone(app.customSwitches?.[index] ?? defaultCustomSwitch(index));
  return (app.ui.curve = { index, sw, text: sw.curve.join(", "), parseError: null, curveTravelUm: sw.travelUm } satisfies CurveDraft);
}

function curveDraftOf(app: App): CurveDraft {
  return (app.ui.curve as CurveDraft | undefined) ?? loadCurveDraft(app, 0);
}

/** travel (x, µm) → f (y) of the draft and the factory curve */
function curvePlot(sw: CustomSwitch): SVGSVGElement {
  const W = 320, H = 180, P = 26;
  const ns = "http://www.w3.org/2000/svg";
  const el = <K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number>) => {
    const e = document.createElementNS(ns, tag);
    for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
    return e;
  };
  const svg = el("svg", { viewBox: `0 0 ${W} ${H}`, class: "curve-plot", role: "img", "aria-label": "Switch curve" });
  const maxX = Math.max(sw.travelUm, 3420) + 100;
  const x = (t: number) => P + ((t + 100) / maxX) * (W - P - 8);
  const y = (f: number) => H - P - (Math.max(-100, f) / CURVE_LAST) * (H - P - 8);
  svg.append(
    el("line", { x1: P, y1: H - P, x2: W - 8, y2: H - P, class: "axis" }),
    el("line", { x1: P, y1: 8, x2: P, y2: H - P, class: "axis" }),
  );
  const label = (text: string, ax: number, ay: number, anchor = "middle") => {
    const t = el("text", { x: ax, y: ay, "text-anchor": anchor, class: "axis-label" });
    t.textContent = text;
    svg.append(t);
  };
  label("travel, mm", W / 2, H - 6);
  label("0", x(0), H - P + 12);
  label(mm(sw.travelUm), x(sw.travelUm), H - P + 12, "end");
  label(String(CURVE_LAST), P - 3, y(CURVE_LAST) + 4, "end");
  label("f", P - 3, H / 2, "end");
  const path = (curve: readonly number[], travel: number, cls: string) => {
    const pts = curve.map((f, i) => `${x(pointTravelUm(i, curve.length, travel)).toFixed(1)},${y(f).toFixed(1)}`);
    svg.append(el("polyline", { points: pts.join(" "), class: cls }));
  };
  path(DEFAULT_CURVE, 3420, "ref");
  if (sw.curve.length >= 2) path(sw.curve, sw.travelUm, "cur");
  return svg;
}

async function writeCurve(app: App): Promise<void> {
  const d = curveDraftOf(app);
  const errors = d.parseError ? [d.parseError] : validateCustomSwitch(d.sw);
  if (errors.length) return app.toast("error", errors.join(" "));
  const code = customSwitchCode(d.index);
  const users = (app.calibration?.switchType ?? []).filter((t) => t === code).length;
  const ok = await confirmDialog({
    title: `Write custom switch ${d.index + 1}`,
    body: h("div", null,
      h("p", null, "Custom switch records are board-global. The firmware saves this write to flash immediately (no “Save to flash” needed, and it is not undone by switching profiles)."),
      h("p", null, users
        ? `${users} key${users === 1 ? " uses" : "s use"} this switch type; their travel and actuation change at once. Check their actuation points against the new travel (${mm(d.sw.travelUm, 3)} mm).`
        : "No key uses this switch type yet; select keys and set the switch type afterwards."),
      h("p", { class: "help" }, "Calibration (rest / bottom values) is not touched."),
    ),
    confirmLabel: "Write",
  });
  if (!ok) return;
  await app.editCustomSwitches("Writing custom switch…", (list) => {
    list[d.index] = d.sw;
  });
  if (app.customSwitches) loadCurveDraft(app, d.index);
  app.render();
}

function curveEditor(app: App): HTMLElement {
  const d = curveDraftOf(app);
  const sw = d.sw;
  const busy = !!app.busy;
  const code = customSwitchCode(d.index);
  const users = (app.calibration?.switchType ?? []).filter((t) => t === code).length;
  const saved = app.customSwitches?.[d.index];
  const changed = !saved || JSON.stringify(saved) !== JSON.stringify(sw);
  const errors = d.parseError ? [d.parseError] : validateCustomSwitch(sw);
  const setCurve = (curve: number[]) => {
    d.sw = withCurve(sw, curve);
    d.curveTravelUm = sw.travelUm;
    d.text = curve.join(", ");
    d.parseError = null;
    app.render();
  };
  const name = h("input", {
    type: "text", class: "input", value: sw.name, maxLength: 63,
    onchange: () => { sw.name = name.value; app.render(); },
  });
  const travel = h("input", {
    type: "number", class: "input num", min: String(TRAVEL_MIN_UM), max: String(TRAVEL_MAX_UM), step: "10", value: String(sw.travelUm),
    onchange: () => { sw.travelUm = Math.round(Number(travel.value)); app.render(); },
  });
  const color = h("input", {
    type: "color", value: rgbToHex(sw.color),
    onchange: () => { sw.color = hexToRgb(color.value); app.render(); },
  });
  const text = h("textarea", {
    class: "input curve-text", rows: 6, spellcheck: false, value: d.text,
    onchange: () => {
      d.text = text.value;
      try {
        d.sw = withCurve(sw, parseCurve(text.value));
        d.curveTravelUm = sw.travelUm;
        d.parseError = null;
      } catch (e) {
        d.parseError = `Curve: ${(e as Error).message}.`;
      }
      app.render();
    },
  });
  return section("Custom switch curves",
    h("p", { class: "help" },
      "Four board-global records, selected as switch types “Custom switch 1–4”. Point i of the curve is the normalised sensor reading f (0 at rest … ",
      String(CURVE_LAST), " at the calibrated bottom) at ", h("b", null, "(i − 1) × 0.05 mm"),
      " of travel; the last point is the full travel and must be ", String(CURVE_LAST), ". Points must not decrease. Up to ", String(CURVE_MAX_POINTS), " points."),
    h("div", { class: "segmented" },
      [0, 1, 2, 3].map((i) => h("button", {
        class: i === d.index ? "seg active" : "seg",
        onclick: () => { loadCurveDraft(app, i); app.render(); },
      }, `Custom ${i + 1}`)),
    ),
    h("div", { class: "cols" },
      h("div", { class: "col" },
        h("label", { class: "field" }, h("span", { class: "field-label" }, "Name"), name),
        h("label", { class: "field" }, h("span", { class: "field-label" }, "Full travel", h("small", null, `${TRAVEL_MIN_UM}..${TRAVEL_MAX_UM} µm`)), h("span", { class: "field-row" }, travel, h("span", { class: "unit" }, "µm"))),
        h("label", { class: "field" }, h("span", { class: "field-label" }, "Colour", h("small", null, "shown by the vendor driver only")), color),
        h("label", { class: "field" },
          h("span", { class: "field-label" }, `Curve (${sw.curve.length} points)`, h("small", null, `${pointsForTravel(sw.travelUm)} points fit ${mm(sw.travelUm, 3)} mm at 0.05 mm spacing`)),
          text,
        ),
        h("div", { class: "row" },
          h("button", {
            class: "btn small", disabled: busy || sw.curve.length < 2, title: "Reshape the curve to the point count that fits the travel",
            onclick: () => setCurve(resampleCurve(sw.curve, d.curveTravelUm, sw.travelUm)),
          }, "Fit curve to travel"),
          h("button", { class: "btn small", disabled: busy, onclick: () => setCurve(resampleCurve(DEFAULT_CURVE, 3420, sw.travelUm)) }, "Factory curve shape"),
          h("button", { class: "btn small", disabled: busy || !changed, onclick: () => { loadCurveDraft(app, d.index); app.render(); } }, "Revert"),
        ),
      ),
      h("div", { class: "col" },
        curvePlot(sw),
        h("p", { class: "help" }, "Solid: this record. Dashed: factory curve (3.42 mm)."),
        h("p", { class: "kv" }, `Used by ${users} key${users === 1 ? "" : "s"} (switch type code ${code}).`),
        errors.length
          ? h("ul", { class: "warn-text" }, errors.map((e) => h("li", null, e)))
          : h("p", { class: "help" }, changed ? "Valid. Not written yet." : "Matches the keyboard."),
        h("div", { class: "row end" },
          h("button", { class: "btn primary", disabled: busy || !changed || errors.length > 0, onclick: () => void writeCurve(app) }, "Write to keyboard…"),
        ),
      ),
    ),
  );
}

// ------------------------------------------------------------ tab

export const actuationTab: TabDef = {
  id: "actuation",
  title: "Actuation",
  hint: "Click keys to select them (several at once), set the values below, then Apply.",
  visual(app, i) {
    const c = app.config!;
    const mode = c.keyMode[i];
    const rt = mode === KEY_MODE_RAPID_TRIGGER;
    const below = mode === KEY_MODE_RT_BELOW;
    const custom = (app.calibration?.switchType[i] ?? 0) > 3;
    const badge = MODES.find((m) => m.mode === mode)?.badge || (mode > 2 ? `m${mode}` : "");
    return {
      label: KEY_LAYOUT[i].name,
      sub: rt ? `±${mm(c.rtPressUm[i])}` : below ? `${mm(c.actuationUm[i])}±` : mm(c.actuationUm[i]),
      badge: (badge + (custom ? " C" : "")).trim() || undefined,
      state: rt || below ? "accent" : null,
      title: `${KEY_LAYOUT[i].name}: ${MODES.find((m) => m.mode === mode)?.label ?? `mode ${mode}`}, actuation ${mm(c.actuationUm[i])} mm, release ${mm(c.releaseUm[i])} mm` +
        (rt || below ? `, RT ${mm(c.rtPressUm[i], 3)}/${mm(c.rtReleaseUm[i], 3)} mm, dead zones ${mm(c.topDeadzoneUm[i], 3)}/${mm(c.bottomDeadzoneUm[i], 3)} mm` : "") +
        `, ${switchType(app.calibration?.switchType[i] ?? 0, app.customSwitches).name}`,
    };
  },
  onKeyClick(app, i) {
    if (app.selection.has(i)) app.selection.delete(i);
    else {
      app.selection.add(i);
      if (app.selection.size === 1) loadDraft(app, i);
    }
    app.render();
  },
  render(app) {
    const d = draftOf(app);
    const rt = usesRt(d.mode);
    const fixed = usesFixed(d.mode);
    const setMode = (m: number) => {
      d.mode = m;
      app.render();
    };
    const none = app.selection.size === 0;
    const maxTravel = 4;
    const current = MODES.find((m) => m.mode === d.mode);
    return h("div", null,
      quickSelect(app, () => {
        const first = [...app.selection][0];
        if (first !== undefined) loadDraft(app, first);
      }),
      h("div", { class: "cols" },
        h("div", { class: "col" },
          section("Trigger mode",
            h("div", { class: "segmented" },
              MODES.map((m) => h("button", { class: d.mode === m.mode ? "seg active" : "seg", onclick: () => setMode(m.mode) }, m.label)),
            ),
            h("p", { class: "help" }, current?.help ?? `Unknown mode ${d.mode}; pick one of the above.`),
          ),
          section("Fixed points",
            numberField("Actuation point", d, "actuation", { min: 0.1, max: maxTravel, step: 0.01, unit: "mm", disabled: !fixed }),
            numberField("Release point", d, "release", { min: 0.1, max: maxTravel, step: 0.01, unit: "mm", disabled: !fixed,
              help: d.mode === KEY_MODE_RT_BELOW ? "full reset above this point; not deeper than the actuation point" : "must not be deeper than the actuation point" }),
          ),
        ),
        h("div", { class: "col" },
          section("Rapid trigger",
            numberField("Press sensitivity", d, "rtPress", { min: 0.01, max: 1, step: 0.005, unit: "mm", disabled: !rt }),
            numberField("Release sensitivity", d, "rtRelease", { min: 0.01, max: 1, step: 0.005, unit: "mm", disabled: !rt }),
            numberField("Top dead zone", d, "topDz", { min: 0, max: 1, step: 0.005, unit: "mm", disabled: !rt }),
            numberField("Bottom dead zone", d, "bottomDz", { min: 0, max: 1, step: 0.005, unit: "mm", disabled: !rt }),
            h("p", { class: "help" }, "Values below 0.10 mm make accidental triggers more likely; raise them if keys chatter."),
          ),
          h("div", { class: "row end" },
            h("button", { class: "btn primary", disabled: none || !!app.busy, onclick: () => apply(app) },
              none ? "Select keys to apply" : `Apply to ${app.selection.size} key${app.selection.size === 1 ? "" : "s"}`),
          ),
        ),
        h("div", { class: "col narrow" },
          section("Switch type",
            (() => {
              const select = h("select", { class: "input", onchange: () => (d.switchType = Number(select.value)) },
                SWITCH_TYPES.map((t) => switchType(t.id, app.customSwitches)).map((s) =>
                  h("option", { value: String(s.id) }, `${s.name} · ${mm(s.travelUm, 3)} mm`)),
              );
              select.value = String(d.switchType);
              return select;
            })(),
            h("p", { class: "help" }, "Tells the firmware which magnetic switch sits under the selected keys. Stored per board, not per profile; recalibrate afterwards. Custom types use the curves below."),
            h("button", { class: "btn", disabled: none || !!app.busy, onclick: () => void applySwitchType(app) }, "Set switch type"),
          ),
        ),
      ),
      curveEditor(app),
    );
  },
};
