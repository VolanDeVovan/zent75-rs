import { CalibrationFlow } from "../../protocol/calibration";
import { switchType } from "../../protocol/keycodes";
import { KEY_LAYOUT } from "../../protocol/layout";
import type { App } from "../app";
import { h } from "../dom";
import type { TabDef } from "../tabs";
import { section, toggle } from "../widgets";

function start(app: App): void {
  void app.run("Starting calibration…", async () => {
    const flow = new CalibrationFlow(app.kb!);
    let scheduled = false;
    await flow.start((session) => {
      if (scheduled) return;
      scheduled = true;
      requestAnimationFrame(() => {
        scheduled = false;
        const view = app.keyboardView;
        if (!view || app.calibrationFlow !== flow) return;
        for (let k = 0; k < session.keyCount; k++) {
          const done = session.accepted[k];
          view.setFill(k, session.span(k) / session.requiredSpan(k), done ? "ok" : String(session.span(k)), done ? "ok" : null);
        }
        const counter = document.getElementById("cal-count");
        if (counter) counter.textContent = `${session.acceptedCount} of ${session.keyCount} keys calibrated · ${session.frames} scans`;
      });
    });
    app.calibrationFlow = flow;
  });
}

function finish(app: App): void {
  const flow = app.calibrationFlow;
  if (!flow) return;
  void app.run("Writing calibration…", async () => {
    const keys = await flow.finish();
    app.calibrationFlow = null;
    app.calibration = await app.kb!.readCalibration();
    if (keys.length === 0) app.toast("info", "No key was calibrated; nothing was written.");
    else {
      app.dirty = false;
      app.toast("ok", `Calibrated and saved ${keys.length} key${keys.length === 1 ? "" : "s"}${keys.length <= 8 ? `: ${keys.map((k) => KEY_LAYOUT[k].name).join(", ")}` : ""}.`);
    }
  });
}

function cancel(app: App): void {
  const flow = app.calibrationFlow;
  if (!flow) return;
  void app.run("Leaving calibration…", async () => {
    await flow.cancel();
    app.calibrationFlow = null;
    app.toast("info", "Calibration abandoned; stored values are unchanged.");
  });
}

export const calibrationTab: TabDef = {
  id: "calibration",
  title: "Calibration",
  hint: "Keys turn green once their full travel has been seen. Other keys keep their stored values.",
  visual(app, i) {
    const cal = app.calibration;
    const name = KEY_LAYOUT[i].name;
    const session = app.calibrationFlow?.session;
    if (session) {
      const done = session.accepted[i];
      return { label: name, sub: done ? "ok" : String(session.span(i)), fill: session.span(i) / session.requiredSpan(i), state: done ? "ok" : null };
    }
    if (!cal) return { label: name };
    const span = cal.top[i] - cal.bottom[i];
    return {
      label: name,
      sub: `${cal.top[i]}/${cal.bottom[i]}`,
      state: span < switchType(cal.switchType[i]).calibSpan ? "warn" : null,
      title: `${name}: rest ${cal.top[i]}, bottom ${cal.bottom[i]} (span ${span}), ${switchType(cal.switchType[i]).name}`,
    };
  },
  render(app) {
    const running = !!app.calibrationFlow?.running;
    const session = app.calibrationFlow?.session;
    return h("div", { class: "cols" },
      h("div", { class: "col" },
        section("Manual calibration",
          h("ol", { class: "steps" },
            h("li", null, "Check that the switch type of every key is right (Actuation tab)."),
            h("li", null, "Press Start and keep your hands off the keyboard for a few seconds: the rest level is recorded first."),
            h("li", null, "Press each key you want to calibrate slowly all the way down, then release it."),
            h("li", null, "Finish writes the new rest / bottom values of the green keys and saves them to flash. Abandon leaves everything as it was."),
          ),
          h("div", { class: "row" },
            h("button", { class: "btn primary", disabled: running || !!app.busy, onclick: () => start(app) }, "Start"),
            h("button", { class: "btn", disabled: !running || !!app.busy, onclick: () => cancel(app) }, "Abandon"),
            h("button", { class: "btn ok", disabled: !running || !!app.busy, onclick: () => finish(app) }, "Finish and save"),
          ),
          h("p", { id: "cal-count", class: "kv" }, running && session ? `${session.acceptedCount} of ${session.keyCount} keys calibrated` : "Not running. The caps show the stored rest / bottom ADC values."),
          h("p", { class: "help" }, "While calibration runs the keyboard streams raw sensor scans; typing still reaches the computer, so keep a harmless window focused."),
        ),
      ),
      h("div", { class: "col narrow" },
        section("Automatic calibration",
          toggle("Keep refining calibration during use", app.config!.autoCalibration, (v) => {
            // the vendor driver saves right after flipping this switch
            void app.edit("Writing…", (c) => {
              c.autoCalibration = v;
            }).then(() => app.save());
          }, "the firmware tracks sensor drift; a manual calibration is still needed once", running || !!app.busy),
        ),
      ),
    );
  },
};
