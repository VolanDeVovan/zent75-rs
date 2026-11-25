# Panel UX

Design of the configurator UI (`src/ui/`). Written before the rebuild; keep it in step with the code.

## Who uses it and how often

One owner, one keyboard, used all day. Ordered by how often each job comes up:

| How often | Job | Where |
|---|---|---|
| Weekly | switch profile, lighting on / brightness / effect, actuation point, rapid trigger on or off | Home, top bar |
| Monthly | different actuation on some keys, SOCD pairs for a game, per-key colours | Actuation, Lighting, SOCD |
| A few times a year | calibrate, look at live travel, back up / restore | Sensors, Device |
| Once, maybe never | remap keys, tap-hold, modifier masks, key groups, DKS, switch types, custom switch curves | Keymap, Advanced keys, Sensors |
| Almost never, can hurt | reset profile, reboot, bootloader | Device → Danger zone (reset also in the profile menu) |

The old panel opened on the keymap and gave every job the same weight (seven equal tabs, a wall
of every key code, an Apply button per form, reset three screens away). The new one orders
everything by the table above.

## Information architecture

```
Top bar   profiles (count from the device) · save state · Undo · Discard · Save to keyboard
Rail      Home
          Actuation
          Lighting
          SOCD
          ── less often ──
          Keymap            (layers, tap-hold, modifier masks)
          Advanced keys     (key groups, DKS; needs firmware ≥ 0.0.18)
          ── keyboard ──
          Sensors           (Live travel · Calibrate · Switch types · Custom curves)
          Device            (info, sleep, Win lock, backup, re-read, Danger zone)
```

Eight destinations in three groups; the first four cover the weekly and monthly jobs. Pages are
addressable (`#/actuation`), so a reload stays where it was.

### Home

Answers "what is my keyboard doing right now" and holds the three things changed most:

- **Typing feel**: Normal / Rapid trigger for all keys, one slider (actuation point, or
  rapid-trigger sensitivity), how many keys are customised, link to Actuation.
- **Lighting**: on/off, brightness, effect, link to Lighting.
- **Gaming**: SOCD pair switches (A⇄D, W⇄S), Win lock, link to SOCD.
- A status line: calibration health, firmware, link to Sensors / Device.

Profiles are in the top bar on every page, so Home does not repeat them.

### Global first, per key second

Actuation follows the Wootility model: with **no key selected the sliders edit "all keys"**;
select keys on the keyboard (click, drag across, or the quick sets All / WASD / Letters / Digits
/ Customised) and the same sliders edit only those. The firmware has no notion of "global", so the
panel derives it: the most common combination of mode and thresholds is the global value, keys
that differ are *customised* (marked with a dot). Editing "all keys" changes the keys that follow
the global value and leaves customised keys alone; "Use the global values" on a selection puts
keys back.

Sliders apply on release, no Apply button (Undo is one click away). Only the field that was moved
is written to the selected keys. The release point follows the actuation point down instead of
producing an error. A travel gauge (a millimetre ruler with the actuation point, release point
and dead zones) shows what the numbers mean and moves while dragging. Release point and dead
zones sit under "Fine tuning".

Composite records (key group, DKS, custom switch curve) keep an explicit Apply / Write button:
half-edited records must not reach the keyboard.

### Keys are picked on the keyboard, codes are picked by search

Every per-key page has the keyboard on top; nothing is chosen from a list of 83 keys. Key codes
(remap target, tap-hold code, SOCD target, group members, DKS slots) come from one picker:
a search box (label, description, or a raw number) plus eight categories, one visible at a time.
Outside Keymap the picker opens as a dialog from a button that shows the current code.

SOCD pairs: switches for the usual pairs, or click two keys and press "Pair". One-way rules and
arbitrary codes remain available for a single selected key.

### Hidden on purpose

- Keymap, tap-hold, modifier masks: second rail group; tap-hold and masks under a disclosure of
  the selected key.
- Key groups and DKS: own page; DKS timing under a disclosure. On firmware without them the page
  says so and offers nothing to click.
- Switch types and custom curves: last two segments of Sensors. Both write board-global data, both ask first.
- Reboot and bootloader: Device → Danger zone, collapsed, each confirmed; bootloader needs the
  typed word.

## Save, undo, reset

The keyboard applies every edit to RAM at once (you feel it immediately) and keeps it across a
replug only after a save. The panel makes that state impossible to miss and cheap to back out of.

**State.** The top bar always shows one of: `Saved` · `Unsaved: actuation, lighting` (amber,
lists what differs) · the running operation. The panel remembers the configuration as it was
when last read after connect, save, or profile switch (the *saved state*); "unsaved" is a
comparison against it, so undoing everything returns to `Saved`.

**Save to keyboard** (button, `Ctrl+S`): the single way to persist. Enabled only when there is
something to save. Closing the tab with unsaved edits asks first.

**Undo** (button, `Ctrl+Z`): steps back one profile edit at a time (up to 50), including an
import and a profile reset. Works after a save too (the result is then unsaved again). The
history belongs to the active profile and is dropped on a profile switch.

**Discard** : writes the saved state back to the keyboard. One click, no dialog (Undo brings the
edits back).

**Reset to default**, from small to large:

1. per control: a ↺ next to a slider whose value differs from the factory value;
2. per key: "Factory default", "Revert to saved", "Clear" on the selected key (Keymap), "Use the
   global values" (Actuation), "Back to effect" (Lighting), right-click to clear (Keymap, SOCD,
   Advanced keys);
3. per page: "Reset actuation", "Reset lighting", "Reset main layer", "Clear all SOCD": one click,
   undoable, not saved until Save;
4. whole profile: profile menu in the top bar (▾ on the active profile) and Device → Danger zone.
   Confirmed. The firmware's own reset persists at once on the real board, so the panel resets
   and saves in one step and says so; Undo then Save brings the old profile back while the
   panel is still open.

Factory values come from `firmware/core/src/defaults.rs` (`src/ui/defaults.ts`).

**Outside this model.** Profile names, switch types, calibration and custom switch curves are
board-global and the firmware stores them on every write, so they are never "unsaved" and Undo
does not cover them. Switch types and curves ask first; calibration finish says that it saves
(which also stores unsaved profile edits); a rename is one dialog. Also confirmed: import (it
can switch through and save all profiles), reboot, bootloader. Switching profile with unsaved
edits offers Save and switch / Discard and switch / Stay.

The saved state is what the panel read at connect: if the keyboard already held unsaved RAM
edits from an earlier session, the panel cannot see that (flash cannot be read safely, `BE 01`
hangs the firmware) and shows them as saved.

## Safety behaviour kept from the old panel

- Bank-1 custom switch writes ask before writing, only changed records are written, invalid records are refused.
- Profile switch with unsaved edits asks.
- Bootloader needs the typed word `bootloader`; reboot is confirmed.
- Tap-hold codes above 1000 cannot be picked (the picker caps the range; the codec refuses them too).
- Key groups / DKS are offered only when the firmware reports them.
- At most four different switch types; exporting all profiles requires a saved state.

## Visual design

The subject is a precision instrument: magnets, travel in millimetres, an aluminium case with
ice-blue LEDs. So: cool aluminium greys, one ice-blue accent taken from the board's default
lighting, amber reserved for "unsaved", red for the danger zone only. Numbers are tabular and
carry units. The keyboard drawing and the travel gauge are the only expressive elements;
everything else is plain rows of label, control, value. Follows the system light / dark setting,
with a manual switch at the bottom of the rail. System fonts, no web fonts, no UI framework: the
UI is ~10 small modules over a `h()` helper, the bundle stays static.

Structure: `store.ts` (state, undo, save; no DOM, tested against the mock), `model.ts` (derived
values: global feel, SOCD pairs, change summary), `components.ts`, `keyboard-view.ts`,
`pages/*.ts`, `app.ts` (shell).

## References

Patterns taken from memory of these tools, not copied assets:

- **Wooting Wootility**: global actuation with per-key override, selection on the keyboard with
  quick sets, travel visualisation, profile strip on top, SOCD/DKS as "advanced keys".
- **VIA / Vial**: keyboard on top with the key-code picker in categories underneath, layers as a
  segmented switch.
- **Razer Synapse / SteelSeries GG (Apex Pro)**: one actuation slider with a millimetre scale,
  rapid trigger as a switch rather than a mode table.
- **macOS / GNOME settings**: rail navigation, rows of label + control, destructive actions at
  the bottom and confirmed.
