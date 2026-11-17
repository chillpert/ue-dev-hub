// Small DOM helpers, text that differs per platform, and the icon set.
import type { Platform } from "./api";

type Child = Node | string | number | null | false | undefined;

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Record<string, unknown> = {},
  ...children: (Child | Child[])[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === undefined || value === null || value === false) continue;
    if (key.startsWith("on") && typeof value === "function") {
      el.addEventListener(key.slice(2).toLowerCase(), value as EventListener);
    } else if (key === "class") {
      el.className = String(value);
    } else if (key === "style") {
      el.setAttribute("style", String(value));
    } else if (key === "value" && "value" in el) {
      (el as HTMLInputElement).value = String(value);
    } else if (key === "checked" && el instanceof HTMLInputElement) {
      el.checked = Boolean(value);
    } else {
      el.setAttribute(key, value === true ? "" : String(value));
    }
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    el.append(typeof child === "number" ? String(child) : child);
  }
  return el;
}

// The hub says which platform it runs on; until then, the browser's guess.
let platform: Platform = navigator.userAgent.includes("Windows") ? "windows" : "linux";

export function setPlatform(p: Platform) {
  platform = p;
}

export function isWindows(): boolean {
  return platform === "windows";
}

/// Text for buttons that open a file manager.
export function showInFolderLabel(): string {
  return isWindows() ? "Show in Explorer" : "Show in folder";
}

/// The IDEs the hub can open on this platform, for "Install ..." hints.
export function ideNames(): string {
  return isWindows() ? "Rider, Visual Studio or VS Code" : "Rider, VS Code or Zed";
}

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

export function plural(n: number, one: string, many = one + "s"): string {
  return `${n} ${n === 1 ? one : many}`;
}

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/);
  return ((parts[0]?.[0] ?? "?") + (parts.length > 1 ? parts[parts.length - 1][0] : "")).toUpperCase();
}

/// A stable color per name, for avatars.
export function hue(name: string): number {
  let x = 0;
  for (const c of name) x = (x * 31 + c.charCodeAt(0)) % 360;
  return x;
}

export function formatDate(seconds: number): string {
  const d = new Date(seconds * 1000);
  const month = d.toLocaleString("en", { month: "short" });
  return `${month} ${d.getDate()}, ${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

// Icons, drawn with a 24x24 stroke grid like the mockups.
const ICONS: Record<string, string> = {
  launch: '<polygon points="7 4 19 12 7 20 7 4"/>',
  package: '<path d="M21 8l-9-5-9 5 9 5 9-5z"/><path d="M3 8v8l9 5 9-5V8"/><path d="M12 13v8"/>',
  map: '<path d="M9 4L3 6v14l6-2 6 2 6-2V4l-6 2-6-2z"/><path d="M9 4v14M15 6v14"/>',
  bug: '<rect x="8" y="6" width="8" height="14" rx="4"/><path d="M12 10v6M4 13h4M16 13h4M5 7l3 2M19 7l-3 2M5 19l3-2M19 19l-3-2"/>',
  tools: '<path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2.4-.6-.6-2.4 2.6-2.6z"/>',
  terminal: '<path d="M4 17l6-5-6-5M12 19h8"/>',
  docs: '<path d="M4 5a2 2 0 0 1 2-2h13v16H6a2 2 0 0 0-2 2V5z"/><path d="M4 19a2 2 0 0 1 2-2h13"/>',
  tasks: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M8 8v5M12 8v8M16 8v3"/>',
  drive: '<path d="M4 15l4-10h8l4 10"/><rect x="3" y="15" width="18" height="5" rx="1.5"/><path d="M7 17.5h.01"/>',
  lock: '<rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/><path d="M12 15v2"/>',
  chat: '<path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.6A8 8 0 1 1 21 12z"/>',
  link: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>',
  external: '<path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>',
  branch: '<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="7" r="2"/><path d="M6 7v10M18 9a6 6 0 0 1-6 6H6"/>',
  updown: '<path d="M7 15l5 5 5-5M7 9l5-5 5 5"/>',
  refresh: '<path d="M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7"/>',
  check: '<path d="M5 12l4 4 10-10"/>',
  builds: '<rect x="3" y="13" width="18" height="7" rx="1.5"/><path d="M5 13V9.5A1.5 1.5 0 0 1 6.5 8h11A1.5 1.5 0 0 1 19 9.5V13M7.5 8V5.5A1.5 1.5 0 0 1 9 4h6a1.5 1.5 0 0 1 1.5 1.5V8"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z"/>',
  code: '<path d="M8 8l-4 4 4 4M16 8l4 4-4 4M14 5l-4 14"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>',
  sound: '<path d="M11 5L6 9H3v6h3l5 4V5z"/><path d="M15.5 8.5a5 5 0 0 1 0 7M18.5 5.5a9 9 0 0 1 0 13"/>',
  upload: '<path d="M12 20V9M7 14l5-5 5 5M5 4h14"/>',
  cloud: '<path d="M7 18a5 5 0 0 1-.6-9.96A6 6 0 0 1 18 9a4.5 4.5 0 0 1-.5 9H7z"/>',
  download: '<path d="M12 4v11M7 10l5 5 5-5M5 20h14"/>',
  trash: '<path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/>',
  git: '<circle cx="12" cy="12" r="3"/><path d="M12 3v6M12 15v6"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  x: '<path d="M6 6l12 12M18 6L6 18"/>',
  settings: '<path d="M4 7h10M18 7h2M4 17h4M12 17h8"/><circle cx="16" cy="7" r="2"/><circle cx="10" cy="17" r="2"/>',
  chevronDown: '<path d="M6 9l6 6 6-6"/>',
  up: '<path d="M6 15l6-6 6 6"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a1 1 0 0 1 1-1h10"/>',
  file: '<path d="M14 3H6a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8z"/><path d="M14 3v5h5"/>',
  image: '<rect x="3" y="5" width="18" height="14" rx="2"/><circle cx="9" cy="10" r="2"/><path d="M21 16l-5-5-9 8"/>',
  layout: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 10h18M12 10v10"/>',
  box: '<path d="M3 7l9-4 9 4v10l-9 4-9-4V7z"/><path d="M3 7l9 4 9-4M12 11v10"/>',
  alert: '<path d="M12 9v4M12 17h.01"/><path d="M10.3 3.9L2 18a2 2 0 0 0 1.7 3h16.6a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/>',
};
// Other names addons may already use.
ICONS.wrench = ICONS.tools;
ICONS.down = ICONS.chevronDown;

export function icon(name: string, size = 18, extra = ""): SVGSVGElement {
  const wrap = document.createElement("span");
  wrap.innerHTML = `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" ${extra}>${ICONS[name] ?? ICONS.link}</svg>`;
  return wrap.firstElementChild as SVGSVGElement;
}

/// Adds an icon for addons: the inside of a 24×24 stroked SVG. Built-in names can't be replaced.
export function registerIcon(name: string, svg: string) {
  if (!(name in ICONS)) ICONS[name] = svg;
}

/// An error in place of a screen's content, with a way to try loading it again.
export function errorBanner(text: string, retry: () => void) {
  return h("div", { class: "banner error", role: "alert" }, h("span", { class: "grow pre" }, text), h("button", { class: "btn small", onclick: retry }, "Try again"));
}

export function toast(text: string, kind: "ok" | "error" = "ok") {
  const el = h("div", { class: `toast ${kind}`, role: "status" }, text);
  document.body.append(el);
  setTimeout(() => el.classList.add("hide"), 3500);
  setTimeout(() => el.remove(), 4000);
}

/// Shows a modal. `build` gets a close function; the promise resolves with what close receives.
export function modal<T>(build: (close: (value: T) => void) => HTMLElement, onDismiss: T): Promise<T> {
  return new Promise((resolve) => {
    const backdrop = h("div", { class: "backdrop" });
    const close = (value: T) => {
      backdrop.remove();
      document.removeEventListener("keydown", onKey);
      resolve(value);
    };
    // Escape closes only the topmost dialog, when one opens over another.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && [...document.querySelectorAll(".backdrop")].pop() === backdrop) close(onDismiss);
    };
    document.addEventListener("keydown", onKey);
    backdrop.addEventListener("mousedown", (e) => {
      if (e.target === backdrop) close(onDismiss);
    });
    const dialog = build(close);
    dialog.classList.add("modal");
    dialog.setAttribute("role", "dialog");
    dialog.setAttribute("aria-modal", "true");
    backdrop.append(dialog);
    document.body.append(backdrop);
    // The button that answers the dialog first, so Enter confirms, then anything focusable.
    const target =
      dialog.querySelector<HTMLElement>("button.primary:not(:disabled)") ??
      dialog.querySelector<HTMLElement>("button.danger:not(:disabled)") ??
      dialog.querySelector<HTMLElement>("input, select, textarea, button:not(:disabled)");
    target?.focus();
  });
}

export function confirmDialog(title: string, message: string, ok: string, danger = false): Promise<boolean> {
  return modal<boolean>(
    (close) =>
      h(
        "div",
        { class: "card" },
        h("h2", {}, title),
        h("p", { class: "muted pre" }, message),
        h(
          "div",
          { class: "actions end" },
          h("button", { class: "btn", onclick: () => close(false) }, "Cancel"),
          h("button", { class: danger ? "btn danger" : "btn primary", onclick: () => close(true) }, ok),
        ),
      ),
    false,
  );
}
