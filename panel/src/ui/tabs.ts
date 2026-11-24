import type { App } from "./app";
import type { KeyVisual } from "./keyboard-view";
import { keymapTab } from "./tabs/keymap";
import { actuationTab } from "./tabs/actuation";
import { advancedTab } from "./tabs/advanced";
import { lightingTab } from "./tabs/lighting";
import { calibrationTab } from "./tabs/calibration";
import { liveTab } from "./tabs/live";
import { deviceTab } from "./tabs/device";

export type TabId = "keymap" | "actuation" | "advanced" | "lighting" | "calibration" | "live" | "device";

export interface TabDef {
  id: TabId;
  title: string;
  /** one line above the keyboard explaining what clicking keys does */
  hint?: string;
  visual(app: App, index: number): KeyVisual;
  onKeyClick?(app: App, index: number, event: MouseEvent): void;
  onKeyContext?(app: App, index: number, event: MouseEvent): void;
  render(app: App): HTMLElement;
}

export const TABS: readonly TabDef[] = [keymapTab, actuationTab, advancedTab, lightingTab, calibrationTab, liveTab, deviceTab];
