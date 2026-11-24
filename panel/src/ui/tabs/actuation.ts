import { KEY_MODE_NORMAL, KEY_MODE_RAPID_TRIGGER } from "../../protocol/keyboard";
import { SWITCH_TYPES, switchType } from "../../protocol/keycodes";
import { KEY_LAYOUT } from "../../protocol/layout";
import type { App } from "../app";
import { confirmDialog, h, mm } from "../dom";
import type { TabDef } from "../tabs";
import { numberField, quickSelect, section } from "../widgets";

interface Draft { mode: number; actuation: number; release: number; rtPress: number; rtRelease: number; topDz: number; bottomDz: number; switchType: number; [k: string]: number }

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
  const shortest = Math.min(...keys.map((k) => switchType(types[k] ?? 0).travelUm));
  const errors: string[] = [];
  if (um(d.actuation) >= shortest) errors.push(`Actuation point must be below the switch travel (${mm(shortest, 3)} mm).`);
  if (um(d.release) > um(d.actuation)) errors.push("Release point must not be deeper than the actuation point.");
  if (um(d.actuation) < 100) errors.push("Actuation point below 0.10 mm is not accepted by the vendor driver.");
  if (d.mode === KEY_MODE_RAPID_TRIGGER) {
    if (um(d.rtPress) < 10 || um(d.rtRelease) < 10) errors.push("Rapid-trigger sensitivity must be at least 0.01 mm.");
    if (um(d.rtPress) >= shortest) errors.push("Rapid-trigger press sensitivity exceeds the switch travel.");
    if (um(d.topDz) + um(d.bottomDz) >= shortest) errors.push("The dead zones cover the whole travel.");
  }
  if (errors.length) return app.toast("error", errors.join(" "));
  void app.edit("Writing actuation…", (c) => {
    for (const k of keys) {
      c.keyMode[k] = d.mode;
      c.actuationUm[k] = um(d.actuation);
      c.releaseUm[k] = um(d.release);
      if (d.mode === KEY_MODE_RAPID_TRIGGER) {
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
  if (new Set(next).size > 4) return app.toast("error", "The firmware supports at most 4 different switch types at once.");
  const ok = await confirmDialog({
    title: "Change switch type",
    body: "The switch type selects the travel curve the firmware uses. After changing it the affected keys must be recalibrated (Calibration tab). Continue?",
    confirmLabel: "Change",
  });
  if (!ok) return;
  await app.editCalibration("Writing switch type…", (c) => {
    c.switchType = next;
  });
}

export const actuationTab: TabDef = {
  id: "actuation",
  title: "Actuation",
  hint: "Click keys to select them (several at once), set the values below, then Apply.",
  visual(app, i) {
    const c = app.config!;
    const rt = c.keyMode[i] === KEY_MODE_RAPID_TRIGGER;
    return {
      label: KEY_LAYOUT[i].name,
      sub: rt ? `±${mm(c.rtPressUm[i])}` : mm(c.actuationUm[i]),
      badge: rt ? "RT" : undefined,
      state: rt ? "accent" : null,
      title: `${KEY_LAYOUT[i].name}: ${rt ? "rapid trigger" : "normal"}, actuation ${mm(c.actuationUm[i])} mm, release ${mm(c.releaseUm[i])} mm` +
        (rt ? `, RT ${mm(c.rtPressUm[i], 3)}/${mm(c.rtReleaseUm[i], 3)} mm, dead zones ${mm(c.topDeadzoneUm[i], 3)}/${mm(c.bottomDeadzoneUm[i], 3)} mm` : ""),
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
    const rt = d.mode === KEY_MODE_RAPID_TRIGGER;
    const setMode = (m: number) => {
      d.mode = m;
      app.render();
    };
    const none = app.selection.size === 0;
    const maxTravel = 4;
    return h("div", null,
      quickSelect(app, () => {
        const first = [...app.selection][0];
        if (first !== undefined) loadDraft(app, first);
      }),
      h("div", { class: "cols" },
        h("div", { class: "col" },
          section("Trigger mode",
            h("div", { class: "segmented" },
              h("button", { class: !rt ? "seg active" : "seg", onclick: () => setMode(KEY_MODE_NORMAL) }, "Normal"),
              h("button", { class: rt ? "seg active" : "seg", onclick: () => setMode(KEY_MODE_RAPID_TRIGGER) }, "Rapid trigger"),
            ),
            h("p", { class: "help" }, rt
              ? "The key re-triggers as soon as it moves down by the press sensitivity and releases when it moves up by the release sensitivity, anywhere between the dead zones."
              : "The key actuates at a fixed depth and releases when it comes back above the release point, like a mechanical switch."),
          ),
          section("Fixed points",
            numberField("Actuation point", d, "actuation", { min: 0.1, max: maxTravel, step: 0.01, unit: "mm" }),
            numberField("Release point", d, "release", { min: 0.1, max: maxTravel, step: 0.01, unit: "mm", help: "must not be deeper than the actuation point" }),
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
                SWITCH_TYPES.map((s) => h("option", { value: String(s.id) }, `${s.name} · ${mm(s.travelUm, 3)} mm`)),
              );
              select.value = String(d.switchType);
              return select;
            })(),
            h("p", { class: "help" }, "Tells the firmware which magnetic switch sits under the selected keys. Stored per board, not per profile; recalibrate afterwards."),
            h("button", { class: "btn", disabled: none || !!app.busy, onclick: () => void applySwitchType(app) }, "Set switch type"),
          ),
        ),
      ),
    );
  },
};
