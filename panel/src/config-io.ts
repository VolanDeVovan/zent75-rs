/**
 * Export / import of configurations through a KeyboardProtocol (no DOM here, so it is testable).
 */
import { buildConfigFile, type ConfigFile, type ProfileExport } from "./config-file";
import type { KeyboardProtocol, ProfileConfig } from "./protocol/keyboard";

export interface ExportOptions {
  /** walk through all profiles (switches profiles on the board and switches back) */
  allProfiles: boolean;
  /** calibration, switch types and the custom switch curves (all board-global) */
  includeCalibration: boolean;
}

export async function exportConfig(kb: KeyboardProtocol, options: ExportOptions): Promise<ConfigFile> {
  if (!kb.info) throw new Error("not connected");
  const active = await kb.getActiveProfile();
  const names = await kb.readProfileNames();
  const profiles: ProfileExport[] = [];
  if (options.allProfiles) {
    try {
      for (let i = 0; i < kb.info.profileCount; i++) {
        await kb.setActiveProfile(i);
        profiles.push({ index: i, name: names[i], config: await kb.readConfig() });
      }
    } finally {
      await kb.setActiveProfile(active);
    }
  } else {
    profiles.push({ index: active, name: names[active], config: await kb.readConfig() });
  }
  const calibration = options.includeCalibration ? await kb.readCalibration() : undefined;
  const customSwitches = options.includeCalibration ? await kb.readCustomSwitches() : undefined;
  return buildConfigFile(kb.info, active, profiles, calibration, undefined, customSwitches);
}

export interface ImportOptions {
  /**
   * "active": write the first (or the matching) profile of the file into the active profile.
   * "all": write every profile of the file into its own slot (switches profiles, saves each).
   */
  mode: "active" | "all";
  restoreNames: boolean;
  /** calibration, switch types and the custom switch curves of the file */
  restoreCalibration: boolean;
  /** persist with save(); always on for mode "all" because a profile switch drops unsaved RAM */
  save: boolean;
}

export interface ImportResult {
  bytesWritten: number;
  profiles: number[];
  saved: boolean;
  notes: string[];
}

/** Adapt a config from a file to what the connected firmware supports. */
function adapt(config: ProfileConfig, kb: KeyboardProtocol, notes: string[]): ProfileConfig {
  const supports = kb.info!.features.advancedKeys;
  if (!supports && config.advanced) {
    notes.push("Key groups / DKS in the file were skipped: this firmware does not support them.");
    return { ...config, advanced: null };
  }
  return config;
}

export async function importConfig(kb: KeyboardProtocol, file: ConfigFile, options: ImportOptions): Promise<ImportResult> {
  if (!kb.info) throw new Error("not connected");
  const notes: string[] = [];
  const result: ImportResult = { bytesWritten: 0, profiles: [], saved: false, notes };
  const active = await kb.getActiveProfile();

  const apply = async (target: number, source: (typeof file.profiles)[number]) => {
    let config = adapt(source.config, kb, notes);
    if (!config.advanced && kb.info!.features.advancedKeys) {
      // file from an older firmware: keep whatever the board has
      config = { ...config, advanced: (await kb.readConfig()).advanced };
    }
    result.bytesWritten += await kb.writeConfig(config);
    if (options.restoreNames && source.name) await kb.writeProfileName(target, source.name);
    result.profiles.push(target);
  };

  if (options.mode === "all") {
    try {
      for (const p of file.profiles) {
        await kb.setActiveProfile(p.index);
        await apply(p.index, p);
        await kb.save();
      }
    } finally {
      await kb.setActiveProfile(active);
    }
    result.saved = true;
  } else {
    const source = file.profiles.find((p) => p.index === file.activeProfile) ?? file.profiles[0];
    await apply(active, source);
  }

  if (options.restoreCalibration) {
    // the curves first: switch types may refer to them (bank 1, saved by the firmware at once)
    if (file.customSwitches) result.bytesWritten += await kb.writeCustomSwitches(file.customSwitches);
    if (file.calibration) result.bytesWritten += await kb.writeCalibration(file.calibration);
    else notes.push("The file has no calibration data.");
  }
  if (options.save && !result.saved) {
    await kb.save();
    result.saved = true;
  } else if (options.restoreCalibration && file.calibration && result.saved) {
    await kb.save();
  }
  return result;
}
