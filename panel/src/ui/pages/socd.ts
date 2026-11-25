/**
 * SOCD ("last input wins"): while a key is held it cancels another key's code. Usually set up
 * as mutual pairs (A ⇄ D): switches for the common ones, or click two keys and pair them.
 */
import { KEYCODE_EMPTY } from "../../protocol/keyboard";
import { KEY_LAYOUT } from "../../protocol/layout";
import type { Page } from "../app";
import { card, cardWith, help, keyButton, linkButton, row, toggle } from "../components";
import { h } from "../dom";
import { codeText, isSocdPair, setSocdPair, socdRules } from "../model";

/** Physical keys of the usual pairs (factory layout positions). */
export const SOCD_PRESETS: readonly { a: number; b: number; label: string }[] = [
  { a: 46, b: 48, label: "A ⇄ D" },
  { a: 32, b: 47, label: "W ⇄ S" },
  { a: 80, b: 82, label: "← ⇄ →" },
  { a: 71, b: 81, label: "↑ ⇄ ↓" },
];

const PAIR_TINTS = ["#4aa8ff", "#f2a33d", "#59c98b", "#c77dff", "#ff6b81", "#3fc1b0"];

export const socdPage: Page = {
  id: "socd",
  title: "SOCD",
  group: 0,
  render(app) {
    const c = app.config;
    const rules = socdRules(c);
    const sel = [...app.selection];
    const name = (k: number) => KEY_LAYOUT[k].name;
    const pairOf = (k: number) => rules.pairs.findIndex((p) => p.a === k || p.b === k);
    const clearKey = (k: number) => void app.edit("SOCD", (d) => {
      // removing one side of a pair removes the pair
      const p = socdRules(d).pairs.find((x) => x.a === k || x.b === k);
      if (p) setSocdPair(d, p.a, p.b, false);
      else d.socd[k] = KEYCODE_EMPTY;
    });

    let panel: HTMLElement;
    if (sel.length === 2) {
      const [a, b] = sel;
      const paired = isSocdPair(c, a, b);
      const unassigned = c.keymap.main[a] === KEYCODE_EMPTY || c.keymap.main[b] === KEYCODE_EMPTY;
      panel = card(`${name(a)} and ${name(b)}`,
        h("div", { class: "actions" },
          h("button", {
            type: "button", class: paired ? "btn" : "btn primary", disabled: unassigned,
            onclick: () => void app.edit("SOCD pair", (d) => {
              // each key can be in one rule only
              for (const k of [a, b]) {
                const old = socdRules(d).pairs.find((x) => x.a === k || x.b === k);
                if (old) setSocdPair(d, old.a, old.b, false);
              }
              setSocdPair(d, a, b, !paired);
            }),
          }, paired ? "Unpair" : `Pair ${name(a)} ⇄ ${name(b)}`),
        ),
        help(unassigned ? "Both keys need a key code on the main layer first." : paired
          ? "Paired: pressing one while the other is held releases the held one."
          : "Pressing one while the other is held releases the held one, so the later key wins."),
      );
    } else if (sel.length === 1) {
      const k = sel[0];
      const pi = pairOf(k);
      const partner = pi >= 0 ? (rules.pairs[pi].a === k ? rules.pairs[pi].b : rules.pairs[pi].a) : null;
      panel = cardWith(name(k), c.socd[k] !== KEYCODE_EMPTY ? linkButton("Clear", () => clearKey(k)) : null,
        partner !== null ? help(`Paired with ${name(partner)}.`) : help("Click a second key to pair the two."),
        row("While held, cancels",
          keyButton({ code: c.socd[k], title: `${name(k)} cancels`, emptyText: "Nothing", onPick: (code) => void app.edit("SOCD", (d) => { d.socd[k] = code; }) }),
          "one-way rule, any key code"),
      );
    } else {
      panel = card("Custom pair", help("Click two keys on the keyboard to pair them. Right-click a key to remove its rule."));
    }

    return h("div", { class: "page" },
      app.stage({
        select: "multi",
        maxSelected: 2,
        onContextMenu: clearKey,
        visual: (i) => {
          const pi = pairOf(i);
          if (pi >= 0) {
            const p = rules.pairs[pi];
            const other = p.a === i ? p.b : p.a;
            return { label: name(i), sub: `⇄ ${name(other)}`, tint: PAIR_TINTS[pi % PAIR_TINTS.length], title: `${name(i)} and ${name(other)} cancel each other` };
          }
          if (c.socd[i] !== KEYCODE_EMPTY) return { label: name(i), sub: `✕ ${codeText(c.socd[i])}`, state: "accent", title: `${name(i)} cancels ${codeText(c.socd[i])}` };
          return { label: name(i) };
        },
      }),
      h("div", { class: "grid two" },
        card("Usual pairs",
          SOCD_PRESETS.map((p) => row(p.label,
            toggle(`socd-${p.a}`, isSocdPair(c, p.a, p.b), (v) => void app.edit("SOCD", (d) => setSocdPair(d, p.a, p.b, v)), `SOCD ${p.label}`))),
          help("The later key wins: holding A and pressing D moves right instead of stopping. Some games forbid SOCD; check their rules."),
        ),
        h("div", { class: "stack-cards" },
          panel,
          rules.pairs.length + rules.oneWay.length > 0
            ? cardWith("Active rules", linkButton("Clear all", () => void app.edit("SOCD reset", (d) => { d.socd.fill(KEYCODE_EMPTY); }), "Remove every SOCD rule. Undoable."),
                h("div", { class: "rules" },
                  rules.pairs.map((p) => h("span", { class: "rule" }, `${name(p.a)} ⇄ ${name(p.b)}`,
                    h("button", { type: "button", class: "rule-x", "aria-label": `Remove ${name(p.a)} and ${name(p.b)}`, onclick: () => clearKey(p.a) }, "×"))),
                  rules.oneWay.map((r) => h("span", { class: "rule" }, `${name(r.key)} cancels ${codeText(r.code)}`,
                    h("button", { type: "button", class: "rule-x", "aria-label": `Remove the rule of ${name(r.key)}`, onclick: () => clearKey(r.key) }, "×"))),
                ),
              )
            : null,
        ),
      ),
    );
  },
};
