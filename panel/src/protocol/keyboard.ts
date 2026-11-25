/**
 * Protocol-neutral model of the keyboard. The UI only talks to `KeyboardProtocol`
 * and these plain data types; the stock vendor protocol lives in ./zent, and a second
 * implementation (our own firmware) can be added next to it without touching the UI.
 *
 * Units: all distances are micrometres (µm), raw sensor values are ADC counts.
 */

export const KEY_MODE_NORMAL = 0;
export const KEY_MODE_RAPID_TRIGGER = 1;
/**
 * Normal press at the actuation point, rapid trigger only below it: an RT release keeps the key
 * "RT-released" (not sent) until it re-presses by RT or comes back above the release point.
 */
export const KEY_MODE_RT_BELOW = 2;

/**
 * Tap-hold: a key with a hold code sends nothing while pressed; held longer than this it sends
 * the hold code, released earlier it sends its keymap code for one report (a tap). Fixed in the
 * firmware (4000 scans at 8 kHz), not configurable.
 */
export const TAP_HOLD_MS = 500;

/** Modifier-mask bits (= HID boot-report modifier byte). */
export const MOD_BITS: readonly { bit: number; label: string; code: number }[] = [
  { bit: 0x01, label: "LCtrl", code: -8 }, { bit: 0x02, label: "LShift", code: -7 },
  { bit: 0x04, label: "LAlt", code: -6 }, { bit: 0x08, label: "LWin", code: -5 },
  { bit: 0x10, label: "RCtrl", code: -4 }, { bit: 0x20, label: "RShift", code: -3 },
  { bit: 0x40, label: "RAlt", code: -2 }, { bit: 0x80, label: "RWin", code: -1 },
];

/** bind_key_mode values */
export const ADV_NONE = 0;
export const ADV_COMBO = 1;
export const ADV_DKS = 2;

/** DKS action per travel phase */
export const DKS_NONE = 0;
/** press at this phase and keep holding until the next phase */
export const DKS_HOLD = 1;
/** a single tap at this phase */
export const DKS_TAP = 2;

export const KEYCODE_EMPTY = 1000;

export interface Rgb { r: number; g: number; b: number }

/** One 48-byte "bind key" record (key group / combo or DKS). Decoded losslessly. */
export interface AdvancedKey {
  /** ADV_NONE | ADV_COMBO | ADV_DKS (lives in a separate per-key byte array) */
  mode: number;
  /**
   * DKS: actions[slot][phase], 4 slots x 4 phases
   * (phase 0 = press past the top point, 1 = press past the bottom point,
   *  2 = release above the bottom point, 3 = release above the top point).
   */
  actions: number[][];
  /** 4 key codes: the combo members, or the DKS slot keys. KEYCODE_EMPTY = unused. */
  keys: number[];
  /** DKS top trigger point, µm */
  dksTopUm: number;
  /** DKS bottom trigger point, µm */
  dksBottomUm: number;
  /** DKS tap duration, scan ticks (the vendor UI shows value / 8 as "ms") */
  tapTicks: number;
  /** DKS delay, scan ticks */
  delayTicks: number;
  /** DKS random extra delay, scan ticks */
  randomTicks: number;
  /** 13 bytes the vendor driver always writes as 0xFF */
  reserved: number[];
  /**
   * last byte, as written by the vendor driver's "Trigger the original key" switch:
   * 0 = switch on (the key's own keymap code is sent as well), 1 = switch off (default).
   * Taken from the UI label; the driver's variable is called DKSOriginCover, so treat the
   * exact firmware behaviour as unverified.
   */
  suppressOriginal: number;
}

export interface Lighting {
  /** master switch (rgb_enable) */
  enabled: boolean;
  /** Caps Lock indicator (cap_enable) */
  capsIndicator: boolean;
  /** separate lighting while Fn is held (rgb_fn_enable) */
  fnLayerLighting: boolean;
  /** dynamic effect id (see EFFECTS) */
  effect: number;
  /** low two bits of the first rgb_config byte; the vendor driver always writes 1 */
  effectFlags: number;
  hue: number;
  saturation: number;
  /** 0..30 in the vendor UI; above 10 it warns about USB power */
  brightness: number;
  speed: number;
  /** per-LED colour, LED order (see layout.ts KEY_TO_LED) */
  colors: Rgb[];
  /** per-LED: 1 = static colour from `colors`, 0 = follows the dynamic effect */
  solid: number[];
}

/** A per-key value on both keymap layers. */
export interface Layered<T> { main: T[]; fn: T[] }

/** Everything that belongs to one profile. */
export interface ProfileConfig {
  keymap: Layered<number>;
  /**
   * Tap-hold code per key and layer (see TAP_HOLD_MS); KEYCODE_EMPTY = off. Codes above 1000 are
   * refused: the firmware treats a profile whose first entry is > 1000 as corrupt.
   */
  holdCode: Layered<number>;
  /** Modifier bits (MOD_BITS) ORed into the report while the key is pressed, per layer; 0 = none. */
  modMask: Layered<number>;
  /** KEY_MODE_NORMAL | KEY_MODE_RAPID_TRIGGER | KEY_MODE_RT_BELOW per key */
  keyMode: number[];
  /** normal mode: actuation point, µm (norm_press_th) */
  actuationUm: number[];
  /** normal mode: release point, µm (norm_return_th) */
  releaseUm: number[];
  /** rapid trigger (modes 1 and 2): press sensitivity, µm (press_th) */
  rtPressUm: number[];
  /** rapid trigger (modes 1 and 2): release sensitivity, µm (return_th) */
  rtReleaseUm: number[];
  /** rapid trigger (modes 1 and 2): dead zone at the top of the travel, µm (press_deadzone) */
  topDeadzoneUm: number[];
  /** rapid trigger (modes 1 and 2): dead zone at the bottom of the travel, µm (return_deadzone) */
  bottomDeadzoneUm: number[];
  /** SOCD "last input wins": key code that this key cancels while held; KEYCODE_EMPTY = off */
  socd: number[];
  /** null when the firmware has no key groups / DKS (stock firmware < 0.0.18) */
  advanced: AdvancedKey[] | null;
  lighting: Lighting;
  /** idle seconds before sleep, 0 = never */
  sleepSeconds: number;
  winLock: boolean;
  autoCalibration: boolean;
}

/** Board-global sensor data (not part of a profile). */
export interface Calibration {
  /** ADC counts with the key at rest (top_value) */
  top: number[];
  /** ADC counts with the key bottomed out (bot_value) */
  bottom: number[];
  /** magnetic switch type per key (keytype_code; 4095..4092 = custom switch record 0..3) */
  switchType: number[];
}

/** Number of user-defined switch records (board-global). */
export const CUSTOM_SWITCH_COUNT = 4;
/** keytype_code of custom switch record i is CUSTOM_SWITCH_CODE_BASE - i (4095..4092). */
export const CUSTOM_SWITCH_CODE_BASE = 4095;
/** Most curve points a record can hold. */
export const CURVE_MAX_POINTS = 88;
/** Required value of the last curve point (normalised sensor position at full travel). */
export const CURVE_LAST = 2636;
/** Distance between curve points, µm of travel; point 0 sits this far above rest. */
export const CURVE_STEP_UM = 50;

/**
 * A user-defined switch (board-global, bank 1): its travel curve maps the normalised sensor
 * position f (0 at rest … 2636 at full swing) to key travel. Point i is the f the sensor reads
 * at travel (i − 1) · 50 µm; the last point is the full travel and must be 2636.
 */
export interface CustomSwitch {
  name: string;
  /** full travel, µm (1500..4200) */
  travelUm: number;
  /** curve points, non-decreasing, 2..88 entries, last = CURVE_LAST */
  curve: number[];
  /** display colour of the vendor driver */
  color: Rgb;
  /** u32 at +0xF8 (default 6000, meaning unknown; 0xFFFF marks the record invalid) */
  wordF8: number;
  /** u8 at +0xFC (default 0, meaning unknown; 0xFF marks the record invalid) */
  byteFC: number;
  /** raw bytes of the curve area behind the last point (kept so that a round trip is lossless) */
  spare: number[];
}

export interface DeviceInfo {
  protocol: string;
  productName: string;
  firmwareVersion: string;
  /** (major << 16) | (minor << 8) | patch */
  versionNumber: number;
  serial: string;
  keyCount: number;
  ledCount: number;
  profileCount: number;
  features: {
    /** key groups (combo) and DKS */
    advancedKeys: boolean;
  };
}

export interface LiveSample {
  /** "travel": normal frame, "raw": calibration-mode frame (no travel available) */
  kind: "travel" | "raw";
  raw: number[];
  /** µm per key; null in calibration mode */
  travelUm: number[] | null;
}

export interface KeyboardEvents {
  /** the board switched profile on its own (Fn shortcut) or after a request */
  profile: number;
  /** the transport went away */
  disconnect: unknown;
}

export interface KeyboardProtocol {
  /** short id of the protocol implementation, e.g. "zent-stock" */
  readonly id: string;

  connect(): Promise<DeviceInfo>;
  disconnect(): Promise<void>;
  readonly info: DeviceInfo | null;

  on<K extends keyof KeyboardEvents>(event: K, listener: (value: KeyboardEvents[K]) => void): () => void;

  // ---- profiles
  getActiveProfile(): Promise<number>;
  /** Switch and wait until the board reports the new profile. Unsaved RAM edits of the old profile are lost. */
  setActiveProfile(index: number): Promise<void>;
  readProfileNames(): Promise<string[]>;
  writeProfileName(index: number, name: string): Promise<void>;

  // ---- configuration of the active profile (RAM; call save() to persist)
  readConfig(): Promise<ProfileConfig>;
  /**
   * Write a config to RAM. Only bytes that differ from what the device currently holds
   * (as last read or written through this object) are sent. Returns the number of bytes written.
   */
  writeConfig(config: ProfileConfig): Promise<number>;

  // ---- custom switch curves (board-global; the firmware saves them to flash on every write)
  readCustomSwitches(): Promise<CustomSwitch[]>;
  /**
   * Write the records that differ from the device, one at a time. Validate first
   * (protocol/switches.ts): an invalid record is replaced by the default on the next boot.
   */
  writeCustomSwitches(switches: CustomSwitch[]): Promise<number>;

  // ---- calibration
  readCalibration(): Promise<Calibration>;
  writeCalibration(calibration: Calibration): Promise<number>;
  /** Enter / leave calibration mode (the live stream then carries raw scans only). */
  setCalibrationMode(on: boolean): Promise<void>;

  // ---- live data
  /** Start the live stream (keeps it alive with heartbeats). Returns a stop function. */
  startStream(listener: (sample: LiveSample) => void): () => void;

  // ---- device actions
  /** Persist RAM to flash. */
  save(): Promise<void>;
  /** Reset the active profile to factory defaults (in RAM). */
  resetConfig(): Promise<void>;
  reboot(): Promise<void>;
  /** Reboot into the UF2 bootloader. The device disappears afterwards. */
  enterBootloader(): Promise<void>;
}

export function cloneConfig<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}
