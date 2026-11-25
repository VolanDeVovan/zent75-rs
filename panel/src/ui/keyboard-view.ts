/**
 * The keyboard drawing. Keys are chosen here, never from a list: click toggles a key, dragging
 * across keys paints the selection, right-click runs the page's "clear" action.
 */
import { KEY_LAYOUT, LAYOUT_HEIGHT, LAYOUT_WIDTH } from "../protocol/layout";
import { h } from "./dom";

export interface KeyVisual {
  /** main legend */
  label: string;
  /** small second line */
  sub?: string;
  /** tooltip */
  title?: string;
  /** CSS colour the cap is tinted with (static LED colour, SOCD pair, ...) */
  tint?: string | null;
  /** 0..1 fill from the top (travel, calibration progress) */
  fill?: number;
  /** semantic state used for styling */
  state?: "ok" | "warn" | "dim" | "accent" | null;
  /** tiny badge in the corner */
  badge?: string;
  /** corner dot: "custom" = differs from the global value, "unsaved" = differs from what is saved */
  dot?: "custom" | "unsaved" | null;
}

export interface KeyboardViewOptions {
  visual: (index: number) => KeyVisual;
  selected: Set<number>;
  /** none: display only; single: one key at a time; multi: toggle and drag-paint */
  select: "none" | "single" | "multi";
  /** multi only: selecting more drops the oldest */
  maxSelected?: number;
  /** the selection changed (after a click, or once at the end of a drag) */
  onSelect?: () => void;
  onContextMenu?: (index: number) => void;
}

/** Handle for cheap per-frame updates (live travel) without rebuilding the DOM. */
export interface KeyboardView {
  element: HTMLElement;
  setFill(index: number, fill: number, sub?: string, state?: KeyVisual["state"]): void;
}

export function renderKeyboard(options: KeyboardViewOptions): KeyboardView {
  const fills: HTMLElement[] = [];
  const subs: HTMLElement[] = [];
  const caps: HTMLButtonElement[] = [];
  const { selected } = options;
  const board = h("div", {
    class: `kb select-${options.select}`,
    style: { aspectRatio: `${LAYOUT_WIDTH} / ${LAYOUT_HEIGHT}` },
    role: "group",
    "aria-label": "Keyboard",
  });

  const paint = (i: number) => {
    caps[i].classList.toggle("selected", selected.has(i));
    caps[i].setAttribute("aria-pressed", String(selected.has(i)));
  };
  const setSelected = (i: number, on: boolean) => {
    if (on === selected.has(i)) return;
    if (!on) selected.delete(i);
    else {
      if (options.select === "single") {
        for (const k of [...selected]) { selected.delete(k); paint(k); }
      } else if (options.maxSelected && selected.size >= options.maxSelected) {
        const oldest = selected.values().next().value as number;
        selected.delete(oldest);
        paint(oldest);
      }
      selected.add(i);
    }
    paint(i);
  };

  // drag-paint: the first key decides whether the drag adds or removes
  let dragAdds: boolean | null = null;
  const endDrag = () => {
    if (dragAdds === null) return;
    dragAdds = null;
    window.removeEventListener("pointerup", endDrag);
    options.onSelect?.();
  };

  for (const k of KEY_LAYOUT) {
    const v = options.visual(k.index);
    const fill = h("span", { class: "kb-fill", style: { height: `${Math.round((v.fill ?? 0) * 100)}%` } });
    const sub = h("span", { class: "kb-sub" }, v.sub ?? "");
    const cap = h("button", {
      type: "button",
      class: ["kb-key", selected.has(k.index) ? "selected" : "", v.state ? `is-${v.state}` : "", v.tint ? "tinted" : "", v.label.length > 4 ? "long" : ""].filter(Boolean).join(" "),
      title: v.title ?? k.name,
      "aria-pressed": String(selected.has(k.index)),
      tabIndex: options.select === "none" ? -1 : 0,
      style: {
        left: `${(k.x / LAYOUT_WIDTH) * 100}%`,
        top: `${(k.y / LAYOUT_HEIGHT) * 100}%`,
        width: `${(k.w / LAYOUT_WIDTH) * 100}%`,
        height: `${(1 / LAYOUT_HEIGHT) * 100}%`,
        ...(v.tint ? { "--tint": v.tint } : {}),
      },
      onpointerdown: (e: PointerEvent) => {
        if (options.select === "none" || e.button !== 0) return;
        if (options.select === "single") {
          setSelected(k.index, !selected.has(k.index));
          options.onSelect?.();
          return;
        }
        dragAdds = !selected.has(k.index);
        setSelected(k.index, dragAdds);
        window.addEventListener("pointerup", endDrag);
      },
      onpointerenter: (e: PointerEvent) => {
        if (dragAdds !== null && e.buttons & 1) setSelected(k.index, dragAdds);
      },
      // keyboard activation (Enter / Space) arrives as a click without a pointer
      onclick: (e: MouseEvent) => {
        if (options.select === "none" || e.detail !== 0) return;
        setSelected(k.index, !selected.has(k.index));
        options.onSelect?.();
      },
      oncontextmenu: (e: MouseEvent) => {
        if (!options.onContextMenu) return;
        e.preventDefault();
        options.onContextMenu(k.index);
      },
    },
      h("span", { class: "kb-cap" },
        fill,
        v.badge ? h("span", { class: "kb-badge" }, v.badge) : null,
        v.dot ? h("span", { class: `kb-dot ${v.dot}` }) : null,
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
