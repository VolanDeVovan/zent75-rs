/**
 * Sensors: what the hall sensors see and how the firmware turns it into travel. Four views:
 * live travel, manual calibration, switch type per key, custom switch curves. The last two
 * write board-global data that the firmware stores at once, so both ask first.
 */
import { CURVE_LAST, CURVE_MAX_POINTS, KEY_MODE_RAPID_TRIGGER, type CustomSwitch, type LiveSample } from "../../protocol/keyboard";
import { SWITCH_TYPES, switchType } from "../../protocol/keycodes";
import { KEY_LAYOUT } from "../../protocol/layout";
import {
  DEFAULT_CURVE, TRAVEL_MAX_UM, TRAVEL_MIN_UM, customSwitchCode, defaultCustomSwitch, parseCurve, pointTravelUm,
  pointsForTravel, resampleCurve, validateCustomSwitch, withCurve,
} from "../../protocol/switches";
import type { App, Page } from "../app";
import { card, cardWith, help, row, segmented, selectionBar, toggle } from "../components";
import { confirmDialog, h, hexToRgb, mm, plural, rgbToHex } from "../dom";

type View = "live" | "calibrate" | "types" | "curves";
const viewOf = (app: App): View => (app.ui.sensorsView ??= "live");
const typeName = (app: App, id: number) => switchType(id, app.customSwitches).name;

// ------------------------------------------------------------ live travel

function ensureStream(app: App): void {
  if (app.ui.liveStop) return;
  let frames = 0;
  let since = performance.now();
  let pending: LiveSample | null = null;
  const peaks: number[] = (app.ui.livePeaks ??= new Array(KEY_LAYOUT.length).fill(0));
  const paint = () => {
    const s = pending;
    pending = null;
    const view = app.keyboardView;
    if (!s || !view || app.page !== "sensors" || viewOf(app) !== "live" || !app.store.connected) return;
    const cfg = app.config;
    const cal = app.calibration;
    const raw = !!app.ui.liveRaw;
    for (let k = 0; k < s.raw.length; k++) {
      const full = switchType(cal.switchType[k] ?? 0, app.customSwitches).travelUm;
      const um = s.travelUm ? s.travelUm[k] : 0;
      if (um > peaks[k]) peaks[k] = um;
      let fill = um / full;
      if (!s.travelUm) fill = (cal.top[k] - s.raw[k]) / Math.max(1, cal.top[k] - cal.bottom[k]);
      // approximation from travel alone (the rapid-trigger state is not streamed)
      const actuated = cfg.keyMode[k] === KEY_MODE_RAPID_TRIGGER ? um > cfg.topDeadzoneUm[k] + cfg.rtPressUm[k] : um >= cfg.actuationUm[k];
      view.setFill(k, fill, raw || !s.travelUm ? String(Math.round(s.raw[k])) : um > 0 ? mm(um) : "", actuated ? "ok" : null);
    }
    const now = performance.now();
    if (now - since >= 1000) {
      const rate = document.getElementById("live-rate");
      if (rate) rate.textContent = `${Math.round((frames * 1000) / (now - since))} frames per second`;
      const pk = document.getElementById("live-peaks");
      if (pk) {
        const top = peaks.map((um, k) => ({ um, k })).filter((p) => p.um >= 100).sort((a, b) => b.um - a.um).slice(0, 12);
        pk.textContent = top.length ? top.map((p) => `${KEY_LAYOUT[p.k].name} ${mm(p.um)}`).join("   ") : "Nothing pressed yet.";
      }
      frames = 0;
      since = now;
    }
  };
  app.ui.liveStop = app.store.subscribeLive((s) => {
    frames++;
    if (!pending) requestAnimationFrame(paint);
    pending = s;
  });
}

function liveView(app: App): HTMLElement[] {
  if (app.ui.liveWanted !== false) ensureStream(app);
  const running = !!app.ui.liveStop;
  return [
    app.stage({
      select: "none",
      hint: "Press keys: each cap fills with its travel and turns green once it is past its actuation point.",
      visual: (i) => ({ label: KEY_LAYOUT[i].name, sub: "", fill: 0 }),
    }),
    h("div", { class: "grid two" },
      card("Stream",
        h("div", { class: "actions" },
          h("button", {
            type: "button", class: "btn",
            onclick: () => {
              if (app.ui.liveStop) {
                app.ui.liveStop();
                app.ui.liveStop = null;
                app.ui.liveWanted = false;
              } else app.ui.liveWanted = true;
              app.render();
            },
          }, running ? "Pause" : "Resume"),
          h("span", { id: "live-rate", class: "help" }, running ? "Waiting for frames…" : "Paused"),
        ),
        row("Show raw sensor counts", toggle("live-raw", !!app.ui.liveRaw, (v) => { app.ui.liveRaw = v; }, "Show raw sensor counts"), "instead of millimetres"),
        help("The keyboard streams travel only while this view is open."),
      ),
      cardWith("Deepest presses", h("button", { type: "button", class: "link", onclick: () => (app.ui.livePeaks as number[] | undefined)?.fill(0) }, "Reset"),
        h("p", { id: "live-peaks", class: "peaks" }, "Nothing pressed yet."),
      ),
    ),
  ];
}

// ------------------------------------------------------------ calibration

function startCalibration(app: App): void {
  let scheduled = false;
  void app.store.startCalibration((session) => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      const view = app.keyboardView;
      if (!view || app.store.calibrationFlow?.session !== session) return;
      for (let k = 0; k < session.keyCount; k++) {
        const done = session.accepted[k];
        view.setFill(k, session.span(k) / session.requiredSpan(k), done ? "ok" : String(session.span(k)), done ? "ok" : null);
      }
      const counter = document.getElementById("cal-count");
      if (counter) counter.textContent = `${session.acceptedCount} of ${session.keyCount} keys done`;
    });
  });
}

function finishCalibration(app: App): void {
  void app.store.finishCalibration().then((keys) => {
    if (!keys) return;
    if (keys.length === 0) app.toast("info", "No key was pressed all the way; nothing was written.");
    else app.toast("ok", `Calibrated and saved ${plural(keys.length, "key")}${keys.length <= 8 ? `: ${keys.map((k) => KEY_LAYOUT[k].name).join(", ")}` : ""}.`);
  });
}

function calibrateView(app: App): HTMLElement[] {
  const s = app.store;
  const session = s.calibrationFlow?.session ?? null;
  const running = !!session;
  const cal = app.calibration;
  const weak = cal.top.filter((top, k) => top - cal.bottom[k] < switchType(cal.switchType[k]).calibSpan).length;
  return [
    app.stage({
      select: "none",
      hint: running ? "Keys turn green once their full travel has been seen. Keys you do not press keep their stored values." : null,
      visual: (i) => {
        const name = KEY_LAYOUT[i].name;
        if (session) {
          const done = session.accepted[i];
          return { label: name, sub: done ? "ok" : String(session.span(i)), fill: session.span(i) / session.requiredSpan(i), state: done ? "ok" : null };
        }
        const span = cal.top[i] - cal.bottom[i];
        return {
          label: name, sub: String(span),
          state: span < switchType(cal.switchType[i]).calibSpan ? "warn" : null,
          title: `${name}: rest ${cal.top[i]}, bottom ${cal.bottom[i]} (span ${span}), ${typeName(app, cal.switchType[i])}`,
        };
      },
    }),
    h("div", { class: "grid two" },
      card("Manual calibration",
        running
          ? h("ol", { class: "steps" },
              h("li", null, "Hands off for a few seconds: the rest level is recorded first."),
              h("li", null, "Press each key slowly all the way down, then let go."),
              h("li", null, "Finish stores the green keys. The keyboard sends no keys to the computer while calibrating."),
            )
          : help(weak
              ? `${plural(weak, "key")} (orange) ${weak === 1 ? "has" : "have"} too small a range between rest and bottom. Calibrate ${weak === 1 ? "it" : "them"}.`
              : "The caps show each sensor's range between rest and bottom. Calibrate after changing switches, or when a key triggers at the wrong depth."),
        h("div", { class: "actions" },
          running
            ? [
                h("button", { type: "button", class: "btn primary", disabled: app.busy, onclick: () => finishCalibration(app) }, "Finish and save"),
                h("button", { type: "button", class: "btn", disabled: app.busy, onclick: () => void s.cancelCalibration().then(() => app.toast("info", "Calibration abandoned; stored values are unchanged.")) }, "Abandon"),
                h("span", { id: "cal-count", class: "help" }, `${session!.acceptedCount} of ${session!.keyCount} keys done`),
              ]
            : h("button", { type: "button", class: "btn primary", disabled: app.busy, onclick: () => startCalibration(app) }, "Start calibration"),
        ),
        running && s.dirty ? help("Finish also saves your unsaved profile changes.") : null,
      ),
      card("Automatic",
        row("Automatic calibration",
          toggle("auto-cal", app.config.autoCalibration, (v) => void app.edit("automatic calibration", (d) => { d.autoCalibration = v; }), "Automatic calibration"),
          "the firmware follows sensor drift during use"),
        help("Part of the profile: save to keep it. A manual calibration is still needed once per switch."),
      ),
    ),
  ];
}

// ------------------------------------------------------------ switch types

async function setSwitchType(app: App, id: number): Promise<void> {
  const keys = [...app.selection];
  if (!keys.length) return app.toast("info", "Select keys on the keyboard first.");
  const next = [...app.calibration.switchType];
  for (const k of keys) next[k] = id;
  if (new Set(next).size > 4) return app.toast("error", "The firmware supports at most 4 different switch types at once.");
  const ok = await confirmDialog({
    title: `Set ${plural(keys.length, "key")} to ${typeName(app, id)}`,
    body: "The switch type selects the travel curve the firmware uses. It applies to all profiles and the keyboard stores it at once (no Save, no Undo). Recalibrate the affected keys afterwards.",
    confirmLabel: "Set switch type",
  });
  if (!ok) return;
  await app.store.editCalibration("switch type", (c) => { c.switchType = next; });
}

function typesView(app: App): HTMLElement[] {
  const types = app.calibration.switchType;
  const sel = [...app.selection];
  const used = [...new Set(types)];
  const select = h("select", { class: "input", "aria-label": "Switch type", dataset: { fid: "type-select" }, onchange: () => { app.ui.typeChoice = Number(select.value); } },
    SWITCH_TYPES.map((t) => switchType(t.id, app.customSwitches)).map((s) => h("option", { value: String(s.id) }, `${s.name}, ${mm(s.travelUm)} mm`)),
  );
  select.value = String(app.ui.typeChoice ?? types[sel[0] ?? 0] ?? 0);
  return [
    app.stage({
      select: "multi",
      hint: sel.length ? null : "Select the keys that have a different switch under them.",
      visual: (i) => ({
        label: KEY_LAYOUT[i].name,
        sub: types[i] > 3 ? `C${4096 - types[i]}` : `T${types[i]}`,
        state: used.length > 1 && types[i] !== used[0] ? "accent" : null,
        title: `${KEY_LAYOUT[i].name}: ${typeName(app, types[i])}`,
      }),
    }),
    selectionBar(app, { noneText: used.length === 1 ? `All keys: ${typeName(app, used[0])}` : `${used.length} switch types in use` }),
    card("Switch type",
      h("div", { class: "actions" },
        select,
        h("button", { type: "button", class: "btn primary", disabled: !sel.length || app.busy, onclick: () => void setSwitchType(app, Number(select.value)) },
          sel.length ? `Set for ${plural(sel.length, "key")}…` : "Select keys first"),
      ),
      help("Tells the firmware which magnetic switch sits under a key, and so how sensor readings become millimetres. Stored for the whole keyboard, not per profile. “Custom switch” types use the curves in the next view."),
    ),
  ];
}

// ------------------------------------------------------------ custom switch curves

interface CurveDraft {
  index: number;
  sw: CustomSwitch;
  text: string;
  parseError: string | null;
  /** travel the curve points were made for (what "Fit to travel" reshapes from) */
  curveTravelUm: number;
}

function loadCurveDraft(app: App, index: number): CurveDraft {
  const sw = structuredClone(app.customSwitches?.[index] ?? defaultCustomSwitch(index));
  return (app.ui.curve = { index, sw, text: sw.curve.join(", "), parseError: null, curveTravelUm: sw.travelUm } satisfies CurveDraft);
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

async function writeCurve(app: App, d: CurveDraft): Promise<void> {
  const errors = d.parseError ? [d.parseError] : validateCustomSwitch(d.sw);
  if (errors.length) return app.toast("error", errors.join(" "));
  const code = customSwitchCode(d.index);
  const users = app.calibration.switchType.filter((t) => t === code).length;
  const ok = await confirmDialog({
    title: `Write custom switch ${d.index + 1}`,
    body: h("div", null,
      h("p", null, "Custom switch records belong to the whole keyboard. The firmware stores this write in flash immediately: no Save needed, no Undo, not affected by switching profiles."),
      h("p", null, users
        ? `${plural(users, "key")} ${users === 1 ? "uses" : "use"} this switch type; their travel and actuation change at once. Check their actuation points against the new travel (${mm(d.sw.travelUm, 3)} mm).`
        : "No key uses this switch type yet; assign it under Switch types afterwards."),
      h("p", null, "Calibration (rest / bottom values) is not touched."),
    ),
    confirmLabel: "Write to keyboard",
  });
  if (!ok) return;
  await app.store.editCustomSwitches("custom switch", (list) => { list[d.index] = d.sw; });
  if (app.customSwitches) loadCurveDraft(app, d.index);
  app.render();
}

function curvesView(app: App): HTMLElement[] {
  const d = (app.ui.curve as CurveDraft | undefined) ?? loadCurveDraft(app, 0);
  const sw = d.sw;
  const code = customSwitchCode(d.index);
  const users = app.calibration.switchType.filter((t) => t === code).length;
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
  const name = h("input", { type: "text", class: "input", value: sw.name, maxLength: 63, "aria-label": "Name", dataset: { fid: "curve-name" }, onchange: () => { sw.name = name.value; app.render(); } });
  const travel = h("input", {
    type: "number", class: "input num", min: String(TRAVEL_MIN_UM), max: String(TRAVEL_MAX_UM), step: "10", value: String(sw.travelUm), "aria-label": "Full travel", dataset: { fid: "curve-travel" },
    onchange: () => { sw.travelUm = Math.round(Number(travel.value)); app.render(); },
  });
  const color = h("input", { type: "color", class: "color", value: rgbToHex(sw.color), "aria-label": "Colour", onchange: () => { sw.color = hexToRgb(color.value); app.render(); } });
  const text = h("textarea", {
    class: "input curve-text", rows: 6, spellcheck: false, value: d.text, "aria-label": "Curve points", dataset: { fid: "curve-text" },
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
  return [
    h("div", { class: "toolbar" },
      segmented([0, 1, 2, 3].map((i) => ({ value: i, label: `Custom ${i + 1}` })), d.index, (i) => { loadCurveDraft(app, i); app.render(); }),
      h("span", { class: "help" }, `Used by ${plural(users, "key")}.`),
    ),
    h("div", { class: "grid two" },
      card(null,
        row("Name", name),
        row("Full travel", [travel, h("span", { class: "unit" }, "µm")], `${TRAVEL_MIN_UM} to ${TRAVEL_MAX_UM}`),
        row("Colour", color, "shown by the vendor driver only"),
        h("label", { class: "stack" }, `Curve, ${plural(sw.curve.length, "point")} (${pointsForTravel(sw.travelUm)} fit ${mm(sw.travelUm, 3)} mm)`, text),
        h("div", { class: "actions" },
          h("button", { type: "button", class: "btn small", disabled: sw.curve.length < 2, title: "Reshape the curve to the point count that fits the travel", onclick: () => setCurve(resampleCurve(sw.curve, d.curveTravelUm, sw.travelUm)) }, "Fit to travel"),
          h("button", { type: "button", class: "btn small", onclick: () => setCurve(resampleCurve(DEFAULT_CURVE, 3420, sw.travelUm)) }, "Factory shape"),
          h("button", { type: "button", class: "btn small", disabled: !changed, onclick: () => { loadCurveDraft(app, d.index); app.render(); } }, "Revert"),
        ),
      ),
      card(null,
        curvePlot(sw),
        help("Solid: this record. Dashed: the factory curve (3.42 mm)."),
        errors.length
          ? h("ul", { class: "warn-text" }, errors.map((e) => h("li", null, e)))
          : help(changed ? "Valid, not written yet." : "Matches the keyboard."),
        h("div", { class: "actions end" },
          h("button", { type: "button", class: "btn primary", disabled: app.busy || !changed || errors.length > 0, onclick: () => void writeCurve(app, d) }, "Write to keyboard…"),
        ),
        help(`Point i is the normalised sensor reading (0 at rest, ${CURVE_LAST} at the calibrated bottom) at (i − 1) × 0.05 mm of travel. The last point is the full travel and must be ${CURVE_LAST}; points must not decrease; at most ${CURVE_MAX_POINTS} points.`),
      ),
    ),
  ];
}

// ------------------------------------------------------------ page

export const sensorsPage: Page = {
  id: "sensors",
  title: "Sensors",
  group: 2,
  render(app) {
    const view = viewOf(app);
    const setView = async (v: View) => {
      app.ui.liveStop?.();
      app.ui.liveStop = null;
      await app.store.leaveLiveModes();
      app.ui.sensorsView = v;
      app.ui.liveWanted = true;
      app.selection.clear();
      app.render();
    };
    return h("div", { class: "page" },
      h("div", { class: "toolbar" },
        segmented<View>([
          { value: "live", label: "Live travel" },
          { value: "calibrate", label: "Calibrate" },
          { value: "types", label: "Switch types" },
          { value: "curves", label: "Custom curves" },
        ], view, (v) => void setView(v)),
      ),
      view === "live" ? liveView(app) : view === "calibrate" ? calibrateView(app) : view === "types" ? typesView(app) : curvesView(app),
    );
  },
};
