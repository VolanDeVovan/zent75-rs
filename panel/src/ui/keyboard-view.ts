import { KEY_LAYOUT, LAYOUT_HEIGHT, LAYOUT_WIDTH } from "../protocol/layout";
import { h } from "./dom";

export interface KeyVisual {
  /** main legend */
  label: string;
  /** small second line */
  sub?: string;
  /** tooltip */
  title?: string;
  /** CSS colour of the little swatch / underglow */
  color?: string | null;
  /** 0..1 fill from the top (travel, progress) */
  fill?: number;
  /** semantic state used for styling */
  state?: "ok" | "warn" | "dim" | "accent" | null;
  /** tiny badge in the corner */
  badge?: string;
}

export interface KeyboardViewOptions {
  visual: (index: number) => KeyVisual;
  selected: ReadonlySet<number>;
  onClick?: (index: number, event: MouseEvent) => void;
  onContextMenu?: (index: number, event: MouseEvent) => void;
}

/** Handle for cheap per-frame updates (live travel) without rebuilding the DOM. */
export interface KeyboardView {
  element: HTMLElement;
  setFill(index: number, fill: number, sub?: string, state?: KeyVisual["state"]): void;
}

export function renderKeyboard(options: KeyboardViewOptions): KeyboardView {
  const fills: HTMLElement[] = [];
  const subs: HTMLElement[] = [];
  const caps: HTMLElement[] = [];
  const board = h("div", {
    class: "kb",
    style: { aspectRatio: `${LAYOUT_WIDTH} / ${LAYOUT_HEIGHT}` },
    role: "group",
    "aria-label": "Keyboard layout",
  });
  for (const k of KEY_LAYOUT) {
    const v = options.visual(k.index);
    const fill = h("span", { class: "kb-fill", style: { height: `${Math.round((v.fill ?? 0) * 100)}%` } });
    const sub = h("span", { class: "kb-sub" }, v.sub ?? "");
    const cap = h("button", {
      type: "button",
      class: ["kb-key", options.selected.has(k.index) ? "selected" : "", v.state ? `is-${v.state}` : "", v.label.length > 5 ? "long" : ""].join(" "),
      title: v.title ?? `${k.name} (key ${k.index})`,
      "aria-pressed": options.selected.has(k.index) ? "true" : "false",
      style: {
        left: `${(k.x / LAYOUT_WIDTH) * 100}%`,
        top: `${(k.y / LAYOUT_HEIGHT) * 100}%`,
        width: `${(k.w / LAYOUT_WIDTH) * 100}%`,
        height: `${(1 / LAYOUT_HEIGHT) * 100}%`,
      },
      onclick: (e: MouseEvent) => options.onClick?.(k.index, e),
      oncontextmenu: (e: MouseEvent) => {
        if (options.onContextMenu) {
          e.preventDefault();
          options.onContextMenu(k.index, e);
        }
      },
    },
      h("span", { class: "kb-cap" },
        fill,
        v.color ? h("span", { class: "kb-swatch", style: { background: v.color } }) : null,
        v.badge ? h("span", { class: "kb-badge" }, v.badge) : null,
        h("span", { class: "kb-label" }, v.label),
        sub,
      ),
    );
    fills.push(fill);
    subs.push(sub);
    caps.push(cap);
    board.appendChild(cap);
  }
  return {
    element: h("div", { class: "kb-wrap" }, board),
    setFill(index, fill, sub, state) {
      fills[index].style.height = `${Math.round(Math.min(1, Math.max(0, fill)) * 100)}%`;
      if (sub !== undefined && subs[index].textContent !== sub) subs[index].textContent = sub;
      if (state !== undefined) {
        caps[index].classList.toggle("is-ok", state === "ok");
        caps[index].classList.toggle("is-warn", state === "warn");
        caps[index].classList.toggle("is-accent", state === "accent");
      }
    },
  };
}
