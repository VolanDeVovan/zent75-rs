/** Tiny DOM helpers; the panel has no UI framework. */
export type Child = Node | string | number | null | undefined | false | Child[];
type Attrs = Record<string, unknown>;

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs?: Attrs | null, ...children: Child[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (v === null || v === undefined || v === false) continue;
      if (k === "class") el.className = String(v);
      else if (k === "style" && typeof v === "object") {
        for (const [p, val] of Object.entries(v as Record<string, string>)) {
          if (p.startsWith("--")) el.style.setProperty(p, val);
          else (el.style as unknown as Record<string, string>)[p] = val;
        }
      } else if (k === "dataset" && typeof v === "object") Object.assign(el.dataset, v);
      else if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2).toLowerCase(), v as EventListener);
      else if (k in el && k !== "list" && k !== "type") (el as unknown as Record<string, unknown>)[k] = v;
      else el.setAttribute(k, v === true ? "" : String(v));
    }
  }
  append(el, children);
  return el;
}

function append(el: Node, children: Child[]): void {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    if (Array.isArray(c)) append(el, c);
    else el.appendChild(c instanceof Node ? c : document.createTextNode(String(c)));
  }
}

export function clear(el: Element): void {
  el.replaceChildren();
}

export const mm = (um: number, digits = 2): string => (um / 1000).toFixed(digits);
export const plural = (n: number, one: string, many = `${one}s`): string => `${n} ${n === 1 ? one : many}`;

export function rgbToHex(c: { r: number; g: number; b: number }): string {
  return "#" + [c.r, c.g, c.b].map((v) => v.toString(16).padStart(2, "0")).join("");
}

export function hexToRgb(hex: string): { r: number; g: number; b: number } {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
  if (!m) return { r: 0, g: 0, b: 0 };
  return { r: parseInt(m[1], 16), g: parseInt(m[2], 16), b: parseInt(m[3], 16) };
}

/** A modal built from `content`; `close(value)` resolves the promise. Escape resolves null. */
export function openDialog<T>(options: { title: string; wide?: boolean }, content: (close: (value: T | null) => void) => Child): Promise<T | null> {
  return new Promise((resolve) => {
    let result: T | null = null;
    const dialog = h("dialog", { class: options.wide ? "modal wide" : "modal" });
    const close = (value: T | null) => {
      result = value;
      dialog.close();
    };
    dialog.append(h("h3", null, options.title), h("div", { class: "modal-content" }, content(close)));
    dialog.addEventListener("close", () => {
      dialog.remove();
      resolve(result);
    });
    // a click on the backdrop closes
    dialog.addEventListener("mousedown", (e) => {
      if (e.target === dialog) close(null);
    });
    document.body.appendChild(dialog);
    dialog.showModal();
  });
}

export interface Choice { id: string; label: string; kind?: "primary" | "danger" }

/** Several ways out of a question; resolves the chosen id, or null for Cancel / Escape. */
export function choiceDialog(options: { title: string; body: Child; choices: Choice[]; cancelLabel?: string; typeToConfirm?: string }): Promise<string | null> {
  return openDialog<string>({ title: options.title }, (close) => {
    const buttons = options.choices.map((c) => h("button", {
      class: c.kind === "danger" ? "btn danger" : c.kind === "primary" ? "btn primary" : "btn", type: "button", onclick: () => close(c.id),
    }, c.label));
    let field: HTMLElement | null = null;
    if (options.typeToConfirm) {
      const word = options.typeToConfirm.toLowerCase();
      for (const b of buttons) b.disabled = true;
      const input = h("input", {
        type: "text", class: "input", placeholder: options.typeToConfirm, autocomplete: "off",
        oninput: () => {
          for (const b of buttons) b.disabled = input.value.trim().toLowerCase() !== word;
        },
      });
      field = h("label", { class: "stack" }, `Type “${options.typeToConfirm}” to continue`, input);
    }
    return [
      h("div", { class: "modal-body" }, options.body),
      field,
      h("div", { class: "modal-actions" },
        h("button", { class: "btn ghost", type: "button", onclick: () => close(null) }, options.cancelLabel ?? "Cancel"),
        buttons,
      ),
    ];
  });
}

/** Yes / no. `danger` styles the confirm button; `typeToConfirm` asks for a word first. */
export async function confirmDialog(options: { title: string; body: Child; confirmLabel?: string; danger?: boolean; typeToConfirm?: string }): Promise<boolean> {
  const id = await choiceDialog({
    title: options.title, body: options.body, typeToConfirm: options.typeToConfirm,
    choices: [{ id: "ok", label: options.confirmLabel ?? "Confirm", kind: options.danger ? "danger" : "primary" }],
  });
  return id === "ok";
}

/** Ask for one line of text; null when cancelled. */
export function promptDialog(options: { title: string; label: string; value: string; maxLength?: number; confirmLabel?: string }): Promise<string | null> {
  return openDialog<string>({ title: options.title }, (close) => {
    const input = h("input", { type: "text", class: "input", value: options.value, maxLength: options.maxLength ?? 64, autocomplete: "off" });
    queueMicrotask(() => input.select());
    return h("form", { onsubmit: (e: Event) => { e.preventDefault(); close(input.value); } },
      h("label", { class: "stack" }, options.label, input),
      h("div", { class: "modal-actions" },
        h("button", { class: "btn ghost", type: "button", onclick: () => close(null) }, "Cancel"),
        h("button", { class: "btn primary", type: "submit" }, options.confirmLabel ?? "OK"),
      ),
    );
  });
}

export interface MenuItem { label: string; onSelect: () => void; danger?: boolean; disabled?: boolean }

/** A small menu under `anchor`; closes on choice, outside click or Escape. */
export function popupMenu(anchor: HTMLElement, items: MenuItem[]): void {
  document.querySelector(".menu")?.remove();
  const r = anchor.getBoundingClientRect();
  const menu = h("div", { class: "menu", role: "menu", style: { left: `${Math.max(8, r.left)}px`, top: `${r.bottom + 6}px` } },
    items.map((item) => h("button", {
      type: "button", role: "menuitem", class: item.danger ? "menu-item danger" : "menu-item", disabled: item.disabled,
      onclick: () => { close(); item.onSelect(); },
    }, item.label)),
  );
  const onDown = (e: Event) => {
    if (!menu.contains(e.target as Node)) close();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") close();
  };
  function close(): void {
    menu.remove();
    document.removeEventListener("mousedown", onDown, true);
    document.removeEventListener("keydown", onKey, true);
  }
  document.body.appendChild(menu);
  setTimeout(() => {
    document.addEventListener("mousedown", onDown, true);
    document.addEventListener("keydown", onKey, true);
  });
  menu.querySelector("button")?.focus();
}
