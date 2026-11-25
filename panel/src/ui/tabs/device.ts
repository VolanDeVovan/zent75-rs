import { ConfigFileError, parseConfigFile, type ConfigFile } from "../../config-file";
import { exportConfig, importConfig } from "../../config-io";
import { KEY_LAYOUT } from "../../protocol/layout";
import type { App } from "../app";
import { confirmDialog, h } from "../dom";
import type { TabDef } from "../tabs";
import { section, toggle } from "../widgets";

const SLEEP_OPTIONS = [0, 60, 120, 180, 300, 600, 900, 1200, 1500, 1800];

function download(name: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  const a = h("a", { href: url, download: name });
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function doExport(app: App, allProfiles: boolean, includeCalibration: boolean): void {
  if (allProfiles && app.dirty) {
    app.toast("error", "Save to flash first: exporting all profiles switches profiles, which discards unsaved changes.");
    return;
  }
  void app.run("Exporting…", async () => {
    await app.leaveLiveModes();
    const file = await exportConfig(app.kb!, { allProfiles, includeCalibration });
    const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-");
    download(`zent75-${allProfiles ? "all-profiles" : `profile${file.activeProfile + 1}`}-${stamp}.json`, JSON.stringify(file, null, 1));
    if (allProfiles) app.config = await app.kb!.readConfig();
    app.toast("ok", `Exported ${file.profiles.length} profile${file.profiles.length === 1 ? "" : "s"}.`);
  });
}

async function doImport(app: App, text: string, name: string): Promise<void> {
  let file: ConfigFile;
  try {
    file = parseConfigFile(text, app.info!.keyCount, app.info!.ledCount);
  } catch (e) {
    app.toast("error", e instanceof ConfigFileError ? `Cannot import ${name}: ${e.message}` : `Cannot import ${name}.`);
    return;
  }
  const opts = { all: false, names: true, calibration: false, save: false };
  const multi = file.profiles.length > 1;
  const ok = await confirmDialog({
    title: "Import configuration",
    body: h("div", null,
      h("p", null, `${name}: ${file.profiles.length} profile${multi ? "s" : ""} from ${file.device.productName || "a keyboard"} (firmware ${file.device.firmwareVersion || "?"}), exported ${file.exportedAt || "at an unknown time"}.`),
      multi
        ? toggle(`Restore all ${file.profiles.length} profiles into their slots`, false, (v) => (opts.all = v), "switches profiles and saves each one to flash; otherwise only the file's active profile goes into the current profile")
        : null,
      toggle("Restore profile names", true, (v) => (opts.names = v)),
      file.calibration || file.customSwitches ? toggle("Restore calibration, switch types and custom switch curves", false, (v) => (opts.calibration = v), "only sensible for the same physical keyboard") : null,
      toggle("Save to flash afterwards", false, (v) => (opts.save = v)),
      h("p", { class: "help" }, "The current configuration in RAM is overwritten."),
    ),
    confirmLabel: "Import",
  });
  if (!ok) return;
  await app.run("Importing…", async () => {
    await app.leaveLiveModes();
    const res = await importConfig(app.kb!, file, {
      mode: opts.all ? "all" : "active", restoreNames: opts.names, restoreCalibration: opts.calibration, save: opts.save,
    });
    app.activeProfile = await app.kb!.getActiveProfile();
    app.profileNames = await app.kb!.readProfileNames();
    app.config = await app.kb!.readConfig();
    app.calibration = await app.kb!.readCalibration();
    app.customSwitches = await app.kb!.readCustomSwitches();
    app.dirty = !res.saved && res.bytesWritten > 0;
    for (const n of res.notes) app.toast("info", n);
    app.toast("ok", `Imported: ${res.bytesWritten} bytes changed${res.saved ? ", saved to flash" : ", not saved yet"}.`);
  });
}

export const deviceTab: TabDef = {
  id: "device",
  title: "Device",
  visual(app, i) {
    return { label: KEY_LAYOUT[i].name, sub: "", state: "dim", title: `${KEY_LAYOUT[i].name} (key ${i})` + (app.config ? "" : "") };
  },
  render(app) {
    const info = app.info!;
    const cfg = app.config!;
    const busy = !!app.busy;
    const nameInput = h("input", { type: "text", class: "input", maxlength: "20", value: app.profileNames[app.activeProfile] ?? "" });
    const sleep = h("select", {
      class: "input", disabled: busy,
      onchange: () => void app.edit("Writing sleep timer…", (c) => {
        c.sleepSeconds = Number(sleep.value);
      }),
    }, SLEEP_OPTIONS.map((s) => h("option", { value: String(s) }, s === 0 ? "Never" : `${s / 60} min`)));
    if (!SLEEP_OPTIONS.includes(cfg.sleepSeconds)) sleep.appendChild(h("option", { value: String(cfg.sleepSeconds) }, `${cfg.sleepSeconds} s`));
    sleep.value = String(cfg.sleepSeconds);
    const exportCal = h("input", { type: "checkbox", checked: true });
    const file = h("input", {
      type: "file", accept: "application/json,.json", class: "hidden",
      onchange: async () => {
        const f = file.files?.[0];
        file.value = "";
        if (f) await doImport(app, await f.text(), f.name);
      },
    });

    return h("div", { class: "cols" },
      h("div", { class: "col" },
        section("Keyboard",
          h("dl", { class: "info" },
            h("dt", null, "Product"), h("dd", null, info.productName),
            h("dt", null, "Firmware"), h("dd", null, info.firmwareVersion),
            h("dt", null, "Serial number"), h("dd", null, h("code", null, info.serial || "not reported")),
            h("dt", null, "Keys / LEDs"), h("dd", null, `${info.keyCount} / ${info.ledCount}`),
            h("dt", null, "Protocol"), h("dd", null, `${info.protocol} over ${app.backend}`),
            h("dt", null, "Key groups / DKS"), h("dd", null, info.features.advancedKeys ? "supported" : "needs firmware 0.0.18+"),
          ),
          h("button", { class: "btn small", disabled: busy, onclick: () => void app.reload() }, "Re-read everything from the keyboard"),
        ),
        section(`Profile ${app.activeProfile + 1}`,
          h("div", { class: "row" },
            nameInput,
            h("button", {
              class: "btn", disabled: busy,
              onclick: () => {
                const name = nameInput.value.trim();
                if (!name) return app.toast("error", "The profile name must not be empty.");
                if ([...name].length > 10) app.toast("info", "The vendor driver limits names to 10 characters; longer names may be cut off there.");
                void app.run("Renaming profile…", async () => {
                  await app.kb!.writeProfileName(app.activeProfile, name);
                  app.profileNames = await app.kb!.readProfileNames();
                  app.dirty = true;
                });
              },
            }, "Rename"),
          ),
          h("p", { class: "help" }, "Switch profiles with the numbered buttons in the header. A switch discards changes that are not saved to flash."),
        ),
      ),
      h("div", { class: "col" },
        section("Settings",
          h("label", { class: "field" }, h("span", { class: "field-label" }, "Sleep after"), sleep),
          toggle("Win key lock", cfg.winLock, (v) => void app.edit("Writing…", (c) => {
            c.winLock = v;
          }), "disables the Win keys (also toggled by the WinLock key)", busy),
          toggle("Automatic calibration", cfg.autoCalibration, (v) => void app.edit("Writing…", (c) => {
            c.autoCalibration = v;
          }), undefined, busy),
        ),
        section("Backup",
          h("label", { class: "toggle" }, exportCal, h("span", null, "Include calibration, switch types and custom switch curves")),
          h("div", { class: "row" },
            h("button", { class: "btn", disabled: busy, onclick: () => doExport(app, false, exportCal.checked) }, "Export this profile"),
            h("button", { class: "btn", disabled: busy, onclick: () => doExport(app, true, exportCal.checked) }, "Export all 4 profiles"),
            h("button", { class: "btn", disabled: busy, onclick: () => file.click() }, "Import JSON…"),
            file,
          ),
          h("p", { class: "help" }, "The JSON holds the full configuration: both layers, tap-hold and modifier masks, actuation, rapid trigger, SOCD, key groups / DKS, lighting and settings."),
        ),
      ),
      h("div", { class: "col narrow" },
        section("Maintenance",
          h("button", { class: "btn primary wide", disabled: busy, onclick: () => void app.save() }, "Save to flash"),
          h("button", {
            class: "btn wide", disabled: busy,
            onclick: async () => {
              const ok = await confirmDialog({
                title: "Reset this profile",
                body: `Profile ${app.activeProfile + 1} goes back to factory defaults in RAM. It becomes permanent only when you save to flash.`,
                confirmLabel: "Reset profile", danger: true,
              });
              if (!ok) return;
              await app.run("Resetting profile…", async () => {
                await app.kb!.resetConfig();
                app.config = await app.kb!.readConfig();
                app.dirty = true;
              });
            },
          }, "Reset profile to defaults"),
          h("button", {
            class: "btn wide", disabled: busy,
            onclick: async () => {
              const ok = await confirmDialog({
                title: "Reboot the keyboard",
                body: "The keyboard restarts and reconnects. Changes that were not saved to flash are lost.",
                confirmLabel: "Reboot", danger: true,
              });
              if (!ok) return;
              const kb = app.kb!;
              await app.leaveLiveModes();
              await kb.reboot().catch((e) => app.toast("error", (e as Error).message));
              await app.disconnect();
              app.toast("info", "Reboot sent. Connect again once the keyboard is back.");
            },
          }, "Reboot"),
          h("button", {
            class: "btn danger wide", disabled: busy,
            onclick: async () => {
              const ok = await confirmDialog({
                title: "Enter the bootloader",
                body: h("div", null,
                  h("p", null, "The keyboard stops working as a keyboard and shows up as a USB drive (“Zent_DLD”) that expects a .uf2 firmware image."),
                  h("p", null, "To leave the bootloader without flashing, unplug and replug the cable. Make sure you have another way to type."),
                ),
                confirmLabel: "Enter bootloader", danger: true, typeToConfirm: "bootloader",
              });
              if (!ok) return;
              const kb = app.kb!;
              await app.leaveLiveModes();
              await kb.enterBootloader().catch((e) => app.toast("error", (e as Error).message));
              await app.disconnect();
              app.toast("info", "Bootloader command sent.");
            },
          }, "Enter bootloader…"),
        ),
      ),
    );
  },
};
