import { KEY_MODE_RAPID_TRIGGER, type LiveSample } from "../../protocol/keyboard";
import { switchType } from "../../protocol/keycodes";
import { KEY_LAYOUT } from "../../protocol/layout";
import type { App } from "../app";
import { h, mm } from "../dom";
import type { TabDef } from "../tabs";
import { section, toggle } from "../widgets";

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
    if (!s || !view || app.tab !== "live") return;
    const cfg = app.config!;
    const cal = app.calibration;
    const raw = !!app.ui.liveRaw;
    for (let k = 0; k < s.raw.length; k++) {
      const full = switchType(cal?.switchType[k] ?? 0).travelUm;
      const um = s.travelUm ? s.travelUm[k] : 0;
      if (um > peaks[k]) peaks[k] = um;
      let fill = um / full;
      if (!s.travelUm && cal) fill = (cal.top[k] - s.raw[k]) / Math.max(1, cal.top[k] - cal.bottom[k]);
      const actuated = cfg.keyMode[k] === KEY_MODE_RAPID_TRIGGER ? um > cfg.topDeadzoneUm[k] + cfg.rtPressUm[k] : um >= cfg.actuationUm[k];
      view.setFill(k, fill, raw || !s.travelUm ? String(Math.round(s.raw[k])) : um > 0 ? mm(um) : "", actuated ? "ok" : null);
    }
    const now = performance.now();
    if (now - since >= 1000) {
      const el = document.getElementById("live-rate");
      if (el) el.textContent = `${Math.round((frames * 1000) / (now - since))} frames/s · ${s.kind === "travel" ? "travel frames" : "raw calibration frames"}`;
      const pk = document.getElementById("live-peaks");
      if (pk) {
        const top = peaks.map((um, k) => ({ um, k })).filter((p) => p.um >= 100).sort((a, b) => b.um - a.um).slice(0, 12);
        pk.textContent = top.length ? top.map((p) => `${KEY_LAYOUT[p.k].name} ${mm(p.um)}`).join(" · ") : "nothing pressed yet";
      }
      frames = 0;
      since = now;
    }
  };
  app.ui.liveStop = app.subscribeLive((s) => {
    frames++;
    if (!pending) requestAnimationFrame(paint);
    pending = s;
  });
}

export const liveTab: TabDef = {
  id: "live",
  title: "Live travel",
  hint: "Press keys: each cap fills with its measured travel and turns green past its actuation point.",
  visual(_app, i) {
    return { label: KEY_LAYOUT[i].name, sub: "", fill: 0 };
  },
  render(app) {
    if (app.ui.liveWanted !== false) ensureStream(app);
    const running = !!app.ui.liveStop;
    return h("div", { class: "cols" },
      h("div", { class: "col" },
        section("Stream",
          h("div", { class: "row" },
            h("button", {
              id: "live-toggle", class: "btn",
              onclick: () => {
                if (app.ui.liveStop) {
                  app.ui.liveStop();
                  app.ui.liveStop = null;
                  app.ui.liveWanted = false;
                } else {
                  app.ui.liveWanted = true;
                }
                app.render();
              },
            }, running ? "Pause stream" : "Start stream"),
            h("button", { class: "btn", onclick: () => (app.ui.livePeaks as number[] | undefined)?.fill(0) }, "Reset peaks"),
            h("span", { id: "live-rate", class: "chip" }, running ? "waiting for frames…" : "paused"),
          ),
          toggle("Show raw ADC counts instead of millimetres", !!app.ui.liveRaw, (v) => (app.ui.liveRaw = v)),
          h("p", { class: "help" }, "The keyboard sends BE FD frames only while it receives a heartbeat (BE 00) about once a second; the panel sends it for as long as this tab is open."),
        ),
      ),
      h("div", { class: "col" },
        section("Deepest presses", h("p", { id: "live-peaks", class: "kv" }, "nothing pressed yet")),
      ),
    );
  },
};
