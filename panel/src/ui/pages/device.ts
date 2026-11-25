/** Device: facts, the few board settings, backup, and the dangerous actions at the bottom. */
import { ConfigFileError, parseConfigFile, type ConfigFile } from "../../config-file";
import { exportConfig, importConfig } from "../../config-io";
import type { App, Page } from "../app";
import { card, help, row, toggle } from "../components";
import { confirmDialog, h, plural } from "../dom";

const SLEEP_OPTIONS = [0, 60, 120, 180, 300, 600, 900, 1200, 1500, 1800];

function download(name: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  const a = h("a", { href: url, download: name });
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function exportProfiles(app: App, allProfiles: boolean, includeCalibration: boolean): void {
  const s = app.store;
  if (allProfiles && s.dirty) {
    app.toast("error", "Save first: exporting all profiles switches through them, which drops unsaved changes.");
    return;
  }
  void s.run("Exporting", async () => {
    await s.leaveLiveModes();
    const file = await exportConfig(s.kb!, { allProfiles, includeCalibration });
    const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-");
    // walking through the profiles leaves the protocol's write cache on another profile
    if (allProfiles) s.config = await s.kb!.readConfig();
    download(`zent75-${allProfiles ? "all-profiles" : `profile${file.activeProfile + 1}`}-${stamp}.json`, JSON.stringify(file, null, 1));
    app.toast("ok", `Exported ${plural(file.profiles.length, "profile")}.`);
  });
}

async function importFile(app: App, text: string, name: string): Promise<void> {
  const s = app.store;
  let file: ConfigFile;
  try {
    file = parseConfigFile(text, s.info!.keyCount, s.info!.ledCount);
  } catch (e) {
    app.toast("error", e instanceof ConfigFileError ? `Cannot import ${name}: ${e.message}` : `Cannot import ${name}.`);
    return;
  }
  const opts = { all: false, names: true, calibration: false, save: false };
  const multi = file.profiles.length > 1;
  const option = (label: string, key: keyof typeof opts, hint?: string) => row(label, toggle(`imp-${key}`, opts[key], (v) => { opts[key] = v; }, label), hint);
  const ok = await confirmDialog({
    title: "Import configuration",
    body: h("div", null,
      h("p", null, `${name}: ${plural(file.profiles.length, "profile")} from ${file.device.productName || "a keyboard"} (firmware ${file.device.firmwareVersion || "?"}), exported ${file.exportedAt || "at an unknown time"}.`),
      multi ? option(`Restore all ${file.profiles.length} profiles`, "all", "each into its own slot, saved at once; otherwise only the file's active profile goes into the current one") : null,
      option("Restore profile names", "names"),
      file.calibration || file.customSwitches ? option("Restore calibration, switch types, custom curves", "calibration", "only for this same keyboard; stored at once") : null,
      option("Save to the keyboard afterwards", "save"),
      h("p", null, "The current profile is overwritten. Without the save option you can still Undo or Discard."),
    ),
    confirmLabel: "Import",
  });
  if (!ok) return;
  await s.run("Importing", async () => {
    await s.leaveLiveModes();
    const res = await importConfig(s.kb!, file, {
      mode: opts.all ? "all" : "active", restoreNames: opts.names, restoreCalibration: opts.calibration, save: opts.save,
    });
    await s.refreshAfterBulkWrite("import", res.saved);
    for (const n of res.notes) app.toast("info", n);
    app.toast("ok", `Imported${res.saved ? " and saved" : "; not saved yet"}.`);
  });
}

async function reboot(app: App): Promise<void> {
  const ok = await confirmDialog({
    title: "Reboot the keyboard",
    body: "The keyboard restarts and reconnects after a moment. Unsaved changes are lost.",
    confirmLabel: "Reboot", danger: true,
  });
  if (!ok) return;
  const kb = app.store.kb!;
  await app.store.leaveLiveModes();
  await kb.reboot().catch((e) => app.toast("error", (e as Error).message));
  await app.disconnect();
  app.toast("info", "Reboot sent. Connect again once the keyboard is back.");
}

async function enterBootloader(app: App): Promise<void> {
  const ok = await confirmDialog({
    title: "Enter the bootloader",
    body: h("div", null,
      h("p", null, "The keyboard stops working as a keyboard and shows up as a USB drive (“Zent_DLD”) that expects a .uf2 firmware image."),
      h("p", null, "To leave the bootloader without flashing, unplug and replug the cable. Make sure you have another way to type."),
    ),
    confirmLabel: "Enter bootloader", danger: true, typeToConfirm: "bootloader",
  });
  if (!ok) return;
  const kb = app.store.kb!;
  await app.store.leaveLiveModes();
  await kb.enterBootloader().catch((e) => app.toast("error", (e as Error).message));
  await app.disconnect();
  app.toast("info", "Bootloader command sent.");
}

export const devicePage: Page = {
  id: "device",
  title: "Device",
  group: 2,
  render(app) {
    const s = app.store;
    const info = app.info;
    const cfg = app.config;
    const busy = app.busy;
    const sleep = h("select", {
      class: "input", "aria-label": "Sleep after", dataset: { fid: "sleep" },
      onchange: () => void app.edit("sleep timer", (c) => { c.sleepSeconds = Number(sleep.value); }),
    }, SLEEP_OPTIONS.map((sec) => h("option", { value: String(sec) }, sec === 0 ? "Never" : `${sec / 60} min`)));
    if (!SLEEP_OPTIONS.includes(cfg.sleepSeconds)) sleep.appendChild(h("option", { value: String(cfg.sleepSeconds) }, `${cfg.sleepSeconds} s`));
    sleep.value = String(cfg.sleepSeconds);
    const withCal = h("input", { type: "checkbox", checked: app.ui.exportCal !== false, onchange: () => { app.ui.exportCal = withCal.checked; } });
    const file = h("input", {
      type: "file", accept: "application/json,.json", hidden: true,
      onchange: async () => {
        const f = file.files?.[0];
        file.value = "";
        if (f) await importFile(app, await f.text(), f.name);
      },
    });
    const danger = (label: string, text: string, button: string, onClick: () => void) => h("div", { class: "row" },
      h("div", { class: "row-text" }, h("span", { class: "row-label" }, label), h("span", { class: "row-help" }, text)),
      h("div", { class: "row-control" }, h("button", { type: "button", class: "btn danger", disabled: busy, onclick: onClick }, button)),
    );
    const dz = (app.ui.open ??= {}) as Record<string, boolean>;
    const dangerZone = h("details", { class: "disclosure danger-zone", open: !!dz.danger, ontoggle: () => (dz.danger = dangerZone.open) },
      h("summary", null, "Danger zone"),
      h("div", { class: "disclosure-body" },
        danger(`Reset profile ${s.activeProfile + 1}`, "Factory values for this profile only. Stored at once; Undo is possible while the panel stays open.", "Reset profile…", () => void app.resetProfile()),
        danger("Reboot", "Restarts the keyboard. Unsaved changes are lost.", "Reboot…", () => void reboot(app)),
        danger("Bootloader", "For flashing firmware. The keyboard stops typing until it is replugged.", "Enter bootloader…", () => void enterBootloader(app)),
      ),
    );
    return h("div", { class: "page narrow" },
      h("div", { class: "grid two" },
        card("Settings",
          row("Sleep after", sleep, "lighting off when idle"),
          row("Win key lock", toggle("dev-winlock", cfg.winLock, (v) => void app.edit("Win lock", (c) => { c.winLock = v; }), "Win key lock"), "also toggled by the WinLock key"),
          help(`These belong to profile ${s.activeProfile + 1}. Rename, switch or reset profiles from the top bar. Automatic calibration is under Sensors.`),
        ),
        card("Backup",
          h("div", { class: "actions" },
            h("button", { type: "button", class: "btn", disabled: busy, onclick: () => exportProfiles(app, false, withCal.checked) }, "Export this profile"),
            h("button", { type: "button", class: "btn", disabled: busy, onclick: () => exportProfiles(app, true, withCal.checked) }, `Export all ${s.profileCount} profiles`),
            h("button", { type: "button", class: "btn", disabled: busy, onclick: () => file.click() }, "Import…"),
            file,
          ),
          h("label", { class: "check" }, withCal, "Include calibration, switch types and custom curves"),
          help("A JSON file with everything in a profile: keymap, tap-hold, actuation, SOCD, advanced keys, lighting, settings."),
        ),
      ),
      card("About this keyboard",
        h("dl", { class: "info" },
          h("dt", null, "Product"), h("dd", null, info.productName),
          h("dt", null, "Firmware"), h("dd", null, info.firmwareVersion),
          h("dt", null, "Serial number"), h("dd", null, h("code", null, info.serial || "not reported")),
          h("dt", null, "Keys, LEDs"), h("dd", null, `${info.keyCount}, ${info.ledCount}`),
          h("dt", null, "Connection"), h("dd", null, `${info.protocol} over ${s.backend}`),
          h("dt", null, "Key groups, DKS"), h("dd", null, info.features.advancedKeys ? "supported" : "need firmware 0.0.18 or newer"),
        ),
        h("div", { class: "actions" },
          h("button", { type: "button", class: "btn small", disabled: busy, title: "Use this after changing settings with the keyboard's own Fn shortcuts", onclick: () => void s.reload() }, "Read everything again"),
        ),
      ),
      dangerZone,
    );
  },
};
