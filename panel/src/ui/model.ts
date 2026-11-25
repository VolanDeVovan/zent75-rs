/**
 * Values the UI derives from a ProfileConfig. Pure functions, no DOM.
 *
 * "Global" actuation: the firmware stores every threshold per key and has no global value. The
 * panel treats the most common combination of mode and thresholds as the global one; keys that
 * differ from it are "customised".
 */
import {
  KEYCODE_EMPTY, KEY_MODE_NORMAL, KEY_MODE_RAPID_TRIGGER, KEY_MODE_RT_BELOW, type ProfileConfig,
} from "../protocol/keyboard";
import { KEY_DEFS, keyLabel, type KeyDef, type KeyGroup } from "../protocol/keycodes";

// ------------------------------------------------------------ actuation ("feel")

export interface KeyFeel {
  mode: number;
  actuationUm: number;
  releaseUm: number;
  rtPressUm: number;
  rtReleaseUm: number;
  topDeadzoneUm: number;
  bottomDeadzoneUm: number;
}

export const usesRt = (mode: number): boolean => mode === KEY_MODE_RAPID_TRIGGER || mode === KEY_MODE_RT_BELOW;
export const usesFixed = (mode: number): boolean => mode !== KEY_MODE_RAPID_TRIGGER;

export function feelOf(c: ProfileConfig, key: number): KeyFeel {
  return {
    mode: c.keyMode[key],
    actuationUm: c.actuationUm[key],
    releaseUm: c.releaseUm[key],
    rtPressUm: c.rtPressUm[key],
    rtReleaseUm: c.rtReleaseUm[key],
    topDeadzoneUm: c.topDeadzoneUm[key],
    bottomDeadzoneUm: c.bottomDeadzoneUm[key],
  };
}

export function setFeel(c: ProfileConfig, key: number, f: KeyFeel): void {
  c.keyMode[key] = f.mode;
  c.actuationUm[key] = f.actuationUm;
  c.releaseUm[key] = f.releaseUm;
  c.rtPressUm[key] = f.rtPressUm;
  c.rtReleaseUm[key] = f.rtReleaseUm;
  c.topDeadzoneUm[key] = f.topDeadzoneUm;
  c.bottomDeadzoneUm[key] = f.bottomDeadzoneUm;
}

/** Identity of a feel as the key behaves: values the mode does not use are left out. */
const feelId = (f: KeyFeel): string => [
  f.mode,
  ...(usesFixed(f.mode) ? [f.actuationUm, f.releaseUm] : []),
  ...(usesRt(f.mode) ? [f.rtPressUm, f.rtReleaseUm, f.topDeadzoneUm, f.bottomDeadzoneUm] : []),
].join("/");

/** Two keys behave the same (stored values their mode ignores may differ). */
export const sameFeel = (a: KeyFeel, b: KeyFeel): boolean => feelId(a) === feelId(b);

/** The most common feel; ties go to the lowest key index. */
export function globalFeel(c: ProfileConfig): KeyFeel {
  const count = new Map<string, { n: number; key: number }>();
  for (let k = 0; k < c.keyMode.length; k++) {
    const id = feelId(feelOf(c, k));
    const e = count.get(id);
    if (e) e.n++;
    else count.set(id, { n: 1, key: k });
  }
  let best = { n: 0, key: 0 };
  for (const e of count.values()) if (e.n > best.n) best = e;
  return feelOf(c, best.key);
}

/** Keys whose feel differs from the global one. */
export function customisedKeys(c: ProfileConfig): number[] {
  const g = globalFeel(c);
  const out: number[] = [];
  for (let k = 0; k < c.keyMode.length; k++) if (!sameFeel(feelOf(c, k), g)) out.push(k);
  return out;
}

/** Keys that follow the global feel (what an "all keys" edit changes). */
export function globalKeys(c: ProfileConfig): number[] {
  const custom = new Set(customisedKeys(c));
  return c.keyMode.map((_, k) => k).filter((k) => !custom.has(k));
}

/** `f` with `patch` applied; the release point follows the actuation point down. */
export function patchFeel(f: KeyFeel, patch: Partial<KeyFeel>): KeyFeel {
  const next = { ...f, ...patch };
  if (patch.actuationUm !== undefined && patch.releaseUm === undefined && next.releaseUm > next.actuationUm) {
    next.releaseUm = next.actuationUm;
  }
  return next;
}

const mmText = (um: number): string => (um / 1000).toFixed(2);

/** Why the keyboard (or the vendor driver) would not accept this feel on a switch with `travelUm` of travel. */
export function validateFeel(f: KeyFeel, travelUm: number): string[] {
  const e: string[] = [];
  // the fixed points are stored in every mode (rapid trigger keeps them for a later mode change)
  if (f.actuationUm >= travelUm) e.push(`The actuation point must be above the bottom of the switch (${mmText(travelUm)} mm).`);
  if (f.releaseUm > f.actuationUm) e.push("The release point must not be deeper than the actuation point.");
  if (f.actuationUm < 100) e.push("The actuation point cannot be less than 0.10 mm.");
  if (usesRt(f.mode)) {
    if (f.rtPressUm < 10 || f.rtReleaseUm < 10) e.push("Rapid-trigger sensitivity must be at least 0.01 mm.");
    if (f.rtPressUm >= travelUm) e.push("The rapid-trigger press sensitivity exceeds the switch travel.");
    if (f.topDeadzoneUm + f.bottomDeadzoneUm >= travelUm) e.push("The dead zones cover the whole travel.");
  }
  if (f.mode === KEY_MODE_RT_BELOW && f.actuationUm + f.bottomDeadzoneUm >= travelUm) {
    e.push("The actuation point lies inside the bottom dead zone: no room for rapid trigger below it.");
  }
  return e;
}

export const MODES: readonly { mode: number; label: string; short: string; help: string }[] = [
  { mode: KEY_MODE_NORMAL, label: "Normal", short: "",
    help: "Presses at a fixed depth and releases above the release point, like a mechanical switch." },
  { mode: KEY_MODE_RAPID_TRIGGER, label: "Rapid trigger", short: "RT",
    help: "Presses as soon as the key moves down and releases as soon as it moves up, anywhere between the dead zones. The fixed points are not used." },
  { mode: KEY_MODE_RT_BELOW, label: "Rapid trigger below actuation", short: "RT↓",
    help: "First press at the actuation point. Below it the key works as rapid trigger; coming back above the release point resets it." },
];

// ------------------------------------------------------------ SOCD

/** Keys a and b cancel each other (by their main-layer codes). */
export function isSocdPair(c: ProfileConfig, a: number, b: number): boolean {
  return a !== b && c.keymap.main[a] !== KEYCODE_EMPTY && c.keymap.main[b] !== KEYCODE_EMPTY &&
    c.socd[a] === c.keymap.main[b] && c.socd[b] === c.keymap.main[a];
}

export function setSocdPair(c: ProfileConfig, a: number, b: number, on: boolean): void {
  c.socd[a] = on ? c.keymap.main[b] : KEYCODE_EMPTY;
  c.socd[b] = on ? c.keymap.main[a] : KEYCODE_EMPTY;
}

export interface SocdRules {
  /** mutual pairs, a < b */
  pairs: { a: number; b: number }[];
  /** keys that cancel a code without being cancelled back */
  oneWay: { key: number; code: number }[];
}

export function socdRules(c: ProfileConfig): SocdRules {
  const pairs: SocdRules["pairs"] = [];
  const paired = new Set<number>();
  const n = c.socd.length;
  for (let a = 0; a < n; a++) {
    if (c.socd[a] === KEYCODE_EMPTY || paired.has(a)) continue;
    for (let b = a + 1; b < n; b++) {
      if (!paired.has(b) && isSocdPair(c, a, b)) {
        pairs.push({ a, b });
        paired.add(a).add(b);
        break;
      }
    }
  }
  const oneWay: SocdRules["oneWay"] = [];
  for (let k = 0; k < n; k++) if (c.socd[k] !== KEYCODE_EMPTY && !paired.has(k)) oneWay.push({ key: k, code: c.socd[k] });
  return { pairs, oneWay };
}

// ------------------------------------------------------------ what is unsaved

const eq = (a: unknown, b: unknown): boolean => JSON.stringify(a) === JSON.stringify(b);

/** Which parts of two profiles differ, in the words of the navigation. */
export function changedAreas(a: ProfileConfig, b: ProfileConfig): string[] {
  const out: string[] = [];
  const feel = (c: ProfileConfig) => [c.keyMode, c.actuationUm, c.releaseUm, c.rtPressUm, c.rtReleaseUm, c.topDeadzoneUm, c.bottomDeadzoneUm];
  if (!eq(feel(a), feel(b))) out.push("actuation");
  if (!eq(a.lighting, b.lighting)) out.push("lighting");
  if (!eq(a.socd, b.socd)) out.push("SOCD");
  if (!eq(a.keymap, b.keymap) || !eq(a.holdCode, b.holdCode) || !eq(a.modMask, b.modMask)) out.push("keymap");
  if (!eq(a.advanced, b.advanced)) out.push("advanced keys");
  if (a.sleepSeconds !== b.sleepSeconds || a.winLock !== b.winLock || a.autoCalibration !== b.autoCalibration) out.push("settings");
  return out;
}

// ------------------------------------------------------------ key code picker

export interface PickerCategory { id: string; title: string; groups: readonly KeyGroup[] }

/** The 16 key-code groups folded into eight categories; one is visible at a time. */
export const PICKER_CATEGORIES: readonly PickerCategory[] = [
  { id: "basic", title: "Basic", groups: ["letters", "digits", "punctuation", "editing"] },
  { id: "mods", title: "Modifiers, layers", groups: ["modifiers", "layers"] },
  { id: "nav", title: "Navigation, F-keys", groups: ["navigation", "function"] },
  { id: "numpad", title: "Numpad", groups: ["numpad"] },
  { id: "media", title: "Media, apps", groups: ["media", "browser", "system"] },
  { id: "mouse", title: "Mouse", groups: ["mouse"] },
  { id: "board", title: "Keyboard functions", groups: ["board", "lighting"] },
  { id: "other", title: "Other", groups: ["special"] },
];

export function categoryOfCode(code: number | null): string {
  // an empty slot opens on the letters, not on the "special" codes
  const def = code === null || code === KEYCODE_EMPTY ? undefined : KEY_DEFS.find((d) => d.code === code);
  return PICKER_CATEGORIES.find((c) => def && c.groups.includes(def.group))?.id ?? "basic";
}

export function codesOfCategory(id: string, maxCode = Infinity): KeyDef[] {
  const cat = PICKER_CATEGORIES.find((c) => c.id === id) ?? PICKER_CATEGORIES[0];
  return cat.groups.flatMap((g) => KEY_DEFS.filter((d) => d.group === g && d.code <= maxCode));
}

export interface PickerSearch {
  matches: KeyDef[];
  /** the query is a whole number that may be assigned as a raw code */
  raw: number | null;
}

/** Search by label, description or number. `maxCode` caps what may be offered (tap-hold: 1000). */
export function searchCodes(query: string, maxCode = 32767): PickerSearch {
  const q = query.trim().toLowerCase();
  if (!q) return { matches: [], raw: null };
  const starts: KeyDef[] = [];
  const contains: KeyDef[] = [];
  for (const d of KEY_DEFS) {
    if (d.code > maxCode) continue;
    const label = d.label.toLowerCase();
    const title = d.title.toLowerCase();
    if (label === q || label.startsWith(q) || String(d.code) === q) starts.push(d);
    else if (label.includes(q) || title.includes(q)) contains.push(d);
  }
  const n = /^-?\d+$/.test(q) ? Number(q) : null;
  const raw = n !== null && n >= -32768 && n <= maxCode ? n : null;
  return { matches: [...starts, ...contains], raw };
}

/** Cap text for a key code: "—" for empty, "#123" for unknown codes. */
export function codeText(code: number): string {
  return code === KEYCODE_EMPTY ? "—" : keyLabel(code) || `#${code}`;
}
