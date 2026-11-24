/** Tiny DOM helpers; the panel has no UI framework. */
type Child = Node | string | number | null | undefined | false | Child[];
type Attrs = Record<string, unknown>;

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs?: Attrs | null, ...children: Child[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (v === null || v === undefined || v === false) continue;
      if (k === "class") el.className = String(v);
      else if (k === "style" && typeof v === "object") Object.assign(el.style, v);
      else if (k === "dataset" && typeof v === "object") Object.assign(el.dataset, v);
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
  while (el.firstChild) el.removeChild(el.firstChild);
}

export const mm = (um: number, digits = 2): string => (um / 1000).toFixed(digits);

export function rgbToHex(c: { r: number; g: number; b: number }): string {
  return "#" + [c.r, c.g, c.b].map((v) => v.toString(16).padStart(2, "0")).join("");
}

export function hexToRgb(hex: string): { r: number; g: number; b: number } {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
  if (!m) return { r: 0, g: 0, b: 0 };
  return { r: parseInt(m[1], 16), g: parseInt(m[2], 16), b: parseInt(m[3], 16) };
}

/** Promise-based confirm dialog. `danger` styles the confirm button; `typeToConfirm` asks for a word. */
export function confirmDialog(options: {
  title: string; body: Child; confirmLabel?: string; danger?: boolean; typeToConfirm?: string;
}): Promise<boolean> {
  return new Promise((resolve) => {
    let input: HTMLInputElement | null = null;
    const ok = h("button", { class: options.danger ? "btn danger" : "btn primary", type: "submit", value: "ok" }, options.confirmLabel ?? "Confirm");
    if (options.typeToConfirm) {
      ok.disabled = true;
      input = h("input", {
        type: "text", class: "input", placeholder: options.typeToConfirm, autocomplete: "off",
        oninput: () => (ok.disabled = input!.value.trim().toLowerCase() !== options.typeToConfirm!.toLowerCase()),
      });
    }
    const dialog = h("dialog", { class: "modal" },
      h("form", { method: "dialog" },
        h("h3", null, options.title),
        h("div", { class: "modal-body" }, options.body),
        input ? h("label", { class: "field" }, `Type “${options.typeToConfirm}” to continue`, input) : null,
        h("div", { class: "modal-actions" },
          h("button", { class: "btn", type: "submit", value: "cancel" }, "Cancel"),
          ok,
        ),
      ),
    );
    dialog.addEventListener("close", () => {
      const yes = dialog.returnValue === "ok";
      dialog.remove();
      resolve(yes);
    });
    document.body.appendChild(dialog);
    dialog.showModal();
  });
}
