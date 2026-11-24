/**
 * Key codes of the stock firmware keymap (signed 16-bit).
 *   -8..-1   modifiers          4..119   HID keyboard usages
 *   201..225 consumer / system  251..255 mouse buttons
 *   301..320 board functions    401..404 Fn layers       1000 empty
 */
export interface KeyDef {
  code: number;
  /** short label for a key cap */
  label: string;
  /** longer description for tooltips */
  title: string;
  group: KeyGroup;
}

export type KeyGroup =
  | "letters" | "digits" | "punctuation" | "editing" | "modifiers" | "function" | "navigation" | "numpad"
  | "media" | "browser" | "system" | "mouse" | "board" | "lighting" | "layers" | "special";

export const GROUP_TITLES: Record<KeyGroup, string> = {
  letters: "Letters", digits: "Digits", punctuation: "Punctuation", editing: "Editing", modifiers: "Modifiers",
  function: "Function keys", navigation: "Navigation", numpad: "Numpad", media: "Media", browser: "Browser",
  system: "System / macOS", mouse: "Mouse buttons", board: "Board functions", lighting: "Lighting shortcuts",
  layers: "Layers", special: "Special",
};

const defs: KeyDef[] = [];
const add = (group: KeyGroup, code: number, label: string, title = label) => defs.push({ code, label, title, group });

// modifiers
add("modifiers", -8, "LCtrl", "Left Ctrl"); add("modifiers", -7, "LShift", "Left Shift");
add("modifiers", -6, "LAlt", "Left Alt"); add("modifiers", -5, "LWin", "Left Win / Cmd");
add("modifiers", -4, "RCtrl", "Right Ctrl"); add("modifiers", -3, "RShift", "Right Shift");
add("modifiers", -2, "RAlt", "Right Alt"); add("modifiers", -1, "RWin", "Right Win / Cmd");

// HID 0..3 (not offered by the vendor picker, but valid table entries)
add("special", 0, "None", "HID reserved (0): no key");
add("special", 1, "RollOver", "HID ErrorRollOver"); add("special", 2, "PostFail", "HID POSTFail");
add("special", 3, "ErrUndef", "HID ErrorUndefined");

for (let i = 0; i < 26; i++) add("letters", 4 + i, String.fromCharCode(65 + i));
["1 !", "2 @", "3 #", "4 $", "5 %", "6 ^", "7 &", "8 *", "9 (", "0 )"].forEach((l, i) => add("digits", 30 + i, l));
add("editing", 40, "Enter"); add("editing", 41, "Esc", "Escape"); add("editing", 42, "Bksp", "Backspace");
add("editing", 43, "Tab"); add("editing", 44, "Space");
add("punctuation", 45, "- _"); add("punctuation", 46, "= +"); add("punctuation", 47, "[ {"); add("punctuation", 48, "] }");
add("punctuation", 49, "\\ |"); add("punctuation", 50, "# ~", "Non-US # ~ (ISO)"); add("punctuation", 51, "; :");
add("punctuation", 52, "' \""); add("punctuation", 53, "` ~"); add("punctuation", 54, ", <"); add("punctuation", 55, ". >");
add("punctuation", 56, "/ ?");
add("editing", 57, "Caps", "Caps Lock");
for (let i = 0; i < 12; i++) add("function", 58 + i, `F${i + 1}`);
add("navigation", 70, "PrtSc", "Print Screen"); add("navigation", 71, "ScrLk", "Scroll Lock"); add("navigation", 72, "Pause");
add("navigation", 73, "Ins", "Insert"); add("navigation", 74, "Home"); add("navigation", 75, "PgUp", "Page Up");
add("navigation", 76, "Del", "Delete"); add("navigation", 77, "End"); add("navigation", 78, "PgDn", "Page Down");
add("navigation", 79, "→", "Right arrow"); add("navigation", 80, "←", "Left arrow");
add("navigation", 81, "↓", "Down arrow"); add("navigation", 82, "↑", "Up arrow");
add("numpad", 83, "NumLk", "Num Lock"); add("numpad", 84, "KP /"); add("numpad", 85, "KP *"); add("numpad", 86, "KP -");
add("numpad", 87, "KP +"); add("numpad", 88, "KP Ent", "Keypad Enter");
["KP 1", "KP 2", "KP 3", "KP 4", "KP 5", "KP 6", "KP 7", "KP 8", "KP 9", "KP 0"].forEach((l, i) => add("numpad", 89 + i, l));
add("numpad", 99, "KP .", "Keypad . / Del");
add("punctuation", 100, "\\ | ISO", "Non-US \\ | (ISO)");
add("system", 101, "App", "Application (HID 0x65)"); add("system", 102, "Power", "Keyboard Power");
add("numpad", 103, "KP =");
for (let i = 0; i < 12; i++) add("function", 104 + i, `F${13 + i}`);
add("system", 116, "Exec", "Execute"); add("system", 117, "Help"); add("system", 118, "Menu", "Menu (HID 0x76)");
add("system", 119, "Select");

// consumer page
add("media", 201, "Vol+", "Volume up"); add("media", 202, "Vol-", "Volume down"); add("media", 203, "Mute");
add("media", 204, "Play", "Play / pause"); add("media", 205, "Stop"); add("media", 206, "Prev", "Previous track");
add("media", 207, "Next", "Next track");
add("system", 208, "Calc", "Calculator"); add("system", 209, "Mail", "E-mail"); add("system", 210, "Files", "File explorer");
add("browser", 211, "Search", "Browser search"); add("browser", 212, "WWW", "Browser home"); add("browser", 213, "Back", "Browser back");
add("browser", 214, "StopLd", "Browser stop loading"); add("browser", 215, "Fwd", "Browser forward");
add("browser", 216, "Refresh", "Browser refresh"); add("browser", 217, "Bookmk", "Browser bookmarks");
add("system", 218, "CtlPnl", "Control panel"); add("media", 219, "Player", "Media player");
add("system", 220, "Bri+", "Screen brightness up"); add("system", 221, "Bri-", "Screen brightness down");
add("system", 222, "Launchpad", "macOS Launchpad"); add("system", 223, "MissionCtl", "macOS Mission Control");
add("system", 224, "Assistant", "macOS assistant (Siri)"); add("system", 225, "VK .", "VK_PERIOD (225)");

// mouse
add("mouse", 251, "MouseL", "Left mouse button"); add("mouse", 252, "MouseR", "Right mouse button");
add("mouse", 253, "MouseM", "Middle mouse button"); add("mouse", 254, "Mouse4", "Mouse back button");
add("mouse", 255, "Mouse5", "Mouse forward button");

// board functions
add("board", 301, "Boot", "Reboot the keyboard into the UF2 bootloader");
add("board", 302, "Reboot", "Reboot the keyboard");
add("board", 303, "RT off", "Temporarily disable rapid trigger (DISABLE_EX)");
add("board", 304, "RT on", "Temporarily enable rapid trigger (ENABLE_EX)");
add("board", 305, "InitSet", "Reset the configuration (INITSET)");
add("board", 306, "~Esc", "Esc; with Shift it sends ~");
add("board", 307, "Cfg1", "Switch to profile 1 (1 s cool-down)"); add("board", 308, "Cfg2", "Switch to profile 2 (1 s cool-down)");
add("board", 309, "Cfg3", "Switch to profile 3 (1 s cool-down)"); add("board", 310, "Cfg4", "Switch to profile 4 (1 s cool-down)");
add("lighting", 311, "Light+", "Lighting brightness up"); add("lighting", 312, "Light-", "Lighting brightness down");
add("lighting", 313, "NextFx", "Next lighting effect"); add("lighting", 314, "PrevFx", "Previous lighting effect");
add("lighting", 315, "LightSw", "Lighting on / off");
add("board", 316, "WinLock", "Win key lock on / off");
add("lighting", 317, "FxSpd+", "Lighting effect speed up"); add("lighting", 318, "FxSpd-", "Lighting effect speed down");
add("board", 319, "5sReset", "Hold 5 s to reset the keyboard to factory settings");
add("board", 320, "WASD⇄Arr", "Swap WASD with the arrow keys");

// layers
add("layers", 401, "Fn", "Fn layer (hold)"); add("layers", 402, "Fn2"); add("layers", 403, "Fn3"); add("layers", 404, "Fn4");

add("special", 1000, "", "Empty (transparent / unassigned)");

export const KEY_DEFS: readonly KeyDef[] = defs;
const BY_CODE = new Map<number, KeyDef>(defs.map((d) => [d.code, d]));

export function keyDef(code: number): KeyDef | undefined {
  return BY_CODE.get(code);
}

export function keyLabel(code: number): string {
  const d = BY_CODE.get(code);
  return d ? d.label : `#${code}`;
}

export function keyTitle(code: number): string {
  const d = BY_CODE.get(code);
  return d ? `${d.title} (${code})` : `Unknown code ${code}`;
}

export function isKnownKeyCode(code: number): boolean {
  return BY_CODE.has(code);
}

/** Order of groups in the key picker. */
export const PICKER_GROUPS: readonly KeyGroup[] = [
  "letters", "digits", "punctuation", "editing", "modifiers", "function", "navigation", "numpad",
  "media", "browser", "system", "mouse", "layers", "board", "lighting", "special",
];

/** Dynamic lighting effects (rgb_config byte 0 = 1 + 4 * id). Names translated from the vendor driver. */
export const EFFECTS: readonly { id: number; name: string }[] = [
  { id: 1, name: "Ice blue, solid" },
  { id: 3, name: "Rainbow gradient" },
  { id: 5, name: "Ice blue, breathing" },
  { id: 12, name: "Three-colour breathing" },
  { id: 13, name: "RGB scroll, horizontal" },
  { id: 14, name: "RGB scroll, vertical" },
  { id: 16, name: "RGB converge to centre" },
  { id: 18, name: "RGB rotate around centre" },
  { id: 21, name: "RGB marquee" },
  { id: 23, name: "Rainbow breathing" },
  { id: 24, name: "Starlight" },
  { id: 29, name: "Ripples" },
  { id: 30, name: "Piano RGB" },
  { id: 32, name: "Reactive bloom, single colour" },
  { id: 33, name: "Reactive bloom, RGB" },
  { id: 35, name: "Reactive single key, ice blue" },
  { id: 39, name: "Reactive cross" },
  { id: 43, name: "Reactive, whole board RGB" },
  { id: 50, name: "Sparkle, single colour" },
];

/** Magnetic switch types (keytype_code) with their full travel. */
export const SWITCH_TYPES: readonly { id: number; name: string; travelUm: number; minRtUm: number; calibSpan: number }[] = [
  { id: 0, name: "TTC KOM (King of Magnetic)", travelUm: 3420, minRtUm: 10, calibSpan: 7000 },
  { id: 1, name: "TTC KOM (new)", travelUm: 3375, minRtUm: 10, calibSpan: 7000 },
  { id: 2, name: "Gateron Magnetic Jade Pro", travelUm: 3420, minRtUm: 15, calibSpan: 7000 },
  { id: 3, name: "Type 3 (3.78 mm, not offered by the vendor UI)", travelUm: 3780, minRtUm: 30, calibSpan: 8000 },
];

export function switchType(id: number) {
  return SWITCH_TYPES.find((s) => s.id === id) ?? SWITCH_TYPES[0];
}
