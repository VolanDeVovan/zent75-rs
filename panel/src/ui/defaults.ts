/**
 * Factory values of a profile, for the "reset to default" controls. Source: the stock firmware's
 * field-wise reset as reimplemented in firmware/core/src/defaults.rs (reset_actuation,
 * reset_lighting, reset_keymaps). The whole-profile reset is done by the firmware itself.
 */
import { KEY_MODE_NORMAL } from "../protocol/keyboard";
import type { KeyFeel } from "./model";

export const FACTORY_FEEL: Readonly<KeyFeel> = {
  mode: KEY_MODE_NORMAL,
  actuationUm: 1600,
  releaseUm: 800,
  rtPressUm: 50,
  rtReleaseUm: 30,
  topDeadzoneUm: 55,
  bottomDeadzoneUm: 0,
};

/** rgb_config 5D 90 FF 0A 32: effect 23, hue 144, saturation 255, brightness 10, speed 50; all switches on. */
export const FACTORY_LIGHTING = {
  enabled: true,
  capsIndicator: true,
  fnLayerLighting: true,
  effect: 23,
  effectFlags: 1,
  hue: 144,
  saturation: 255,
  brightness: 10,
  speed: 50,
} as const;
