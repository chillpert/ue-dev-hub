// Addon pages: loading the addons that are on for the project, and what they may use.
// The types an addon sees are in addon-api/index.d.ts; this file implements them.
import { convertFileSrc } from "@tauri-apps/api/core";
import type * as Contract from "../addon-api/index";
import { api, type AddonInfo, type Commit, type ProjectInfo, type UpdateInfo, type UpdateResult } from "./api";
import { avatar, loadAvatars } from "./avatars";
import { go } from "./nav";
import { busy, busyLabel, current, rerender, store, type Screen } from "./store";
import { confirmDialog, errorText, h, icon, modal, plural, registerIcon, toast } from "./ui";

// The hub's own types must stay usable as the ones addons were promised.
type Assignable<T extends U, U> = T;
export type ContractCheck = [
  Assignable<ProjectInfo, Contract.ProjectInfo>,
  Assignable<Commit, Contract.Commit>,
  Assignable<UpdateInfo, Contract.UpdateInfo>,
  Assignable<UpdateResult, Contract.UpdateResult>,
];

interface Loaded {
  info: AddonInfo;
  pages: Record<string, Contract.AddonPage>;
  style: HTMLLinkElement | null;
  deactivate?: () => void;
}

type LogListener = (level: "info" | "detail", text: string) => void;

const loaded = new Map<string, Loaded>();
/// Who listens to each addon's step log lines.
const logListeners = new Map<string, Set<LogListener>>();
/// Addons that failed to load, with why. Their pages show the error.
const failed = new Map<string, string>();
/// Loads in progress, so overlapping refreshes activate each addon only once.
const loading = new Map<string, Promise<void>>();

export function screenOf(id: string, page: string): Screen {
  return `addon:${id}:${page}`;
}

function parse(screen: Screen): { id: string; page: string } | null {
  if (!screen.startsWith("addon:")) return null;
  const [, id, page] = screen.split(":");
  return id && page ? { id, page } : null;
}

/// Where an addon's file is served. Slashes stay slashes, so the page can find its other files
/// relative to `import.meta.url`.
function url(id: string, file: string) {
  return `${convertFileSrc(id, "addon")}/${file.split("/").map(encodeURIComponent).join("/")}`;
}

function hubFor(id: string): Contract.Hub {
  return {
    // What the backend implements; it refuses addons made for another version.
    version: store.addonApi as Contract.Hub["version"],
    id,
    call: async <T>(method: string, params: Record<string, unknown> = {}, options: Contract.CallOptions = {}) => {
      const step = options.step ?? null;
      if (!step) return api.addonCall<T>(id, method, params, null);
      // The rest of the hub waits while a step runs, like it does for jobs.
      store.steps.push(step);
      rerender();
      try {
        return await api.addonCall<T>(id, method, params, step);
      } finally {
        store.steps.splice(store.steps.indexOf(step), 1);
        rerender();
      }
    },
    onLog: (fn: LogListener) => {
      let set = logListeners.get(id);
      if (!set) logListeners.set(id, (set = new Set()));
      set.add(fn);
      return () => set.delete(fn);
    },
    ui: { h, icon, registerIcon, toast, modal, confirmDialog, errorText, plural },
    avatars: { avatar, loadAvatars },
    app: {
      busy,
      busyLabel,
      rerender,
      // A copy, so a page can't change the hub's own state by accident.
      project: () => (store.project ? Object.freeze(structuredClone(store.project)) : null),
      setBranch: (branch: string) => {
        if (store.project) store.project.branch = branch;
      },
      activePage: () => {
        const s = parse(store.screen);
        return s && s.id === id ? s.page : null;
      },
      go: (page: string) => go(screenOf(id, page)),
      revealPath: (path: string) => api.revealPath(path),
      checkUpdates: () => api.checkUpdates(),
    },
  };
}

async function load(info: AddonInfo): Promise<Loaded> {
  // The version in the URL makes an updated addon load fresh instead of from the module cache.
  const mod = await import(/* @vite-ignore */ `${url(info.id, "index.js")}?v=${encodeURIComponent(info.version)}`);
  const activate = mod.default as Contract.Activate | undefined;
  if (typeof activate !== "function") throw new Error("Its ui/index.js has no activate function as its default export.");
  const ui = activate(hubFor(info.id));
  let style: HTMLLinkElement | null = null;
  if (info.hasStyle) {
    style = h("link", { rel: "stylesheet", href: `${url(info.id, "style.css")}?v=${encodeURIComponent(info.version)}`, "data-addon": info.id });
    document.head.append(style);
  }
  return { info, pages: ui?.pages ?? {}, style, deactivate: typeof ui?.deactivate === "function" ? () => ui.deactivate!() : undefined };
}

/// Lets the addon clean up, then forgets it. `activate` runs again if it's turned on again.
function release(l: Loaded) {
  if (l.deactivate) safely(l.info.id, l.deactivate);
  l.style?.remove();
}

function unload(id: string) {
  const l = loaded.get(id);
  if (l) release(l);
  logListeners.delete(id);
  loaded.delete(id);
  failed.delete(id);
}

/// Reads which addons are installed and on, loads the new ones and unloads the rest. Leaves an
/// addon page that's no longer there for Launch.
export async function refreshAddons() {
  const gen = store.gen;
  const state = await api.addonsList().catch(() => null);
  if (!current(gen)) return;
  store.addons = state?.installed ?? [];
  const on = new Map(store.addons.filter((a) => a.enabled).map((a) => [a.id, a]));
  for (const id of [...loaded.keys(), ...failed.keys()]) {
    const info = on.get(id);
    if (!info || info.version !== (loaded.get(id)?.info.version ?? "")) unload(id);
  }
  await Promise.all([...on.values()].filter((a) => !loaded.has(a.id) && !failed.has(a.id)).map(startLoad));
  const s = parse(store.screen);
  if (s && !on.get(s.id)?.pages.some((p) => p.id === s.page)) store.screen = "launch";
  rerender();
}

/// Drops the log listeners an addon added while loading, unless a loaded copy still uses them,
/// so the next activation doesn't get each line twice.
function forgetLogListeners(id: string) {
  if (!loaded.has(id)) logListeners.delete(id);
}

function startLoad(a: AddonInfo): Promise<void> {
  const running = loading.get(a.id);
  if (running) return running;
  const p = (async () => {
    try {
      const l = await load(a);
      // Turned off or updated while it loaded: let it go again.
      const now = store.addons.find((x) => x.id === a.id);
      if (now?.enabled && now.version === a.version) loaded.set(a.id, l);
      else {
        release(l);
        forgetLogListeners(a.id);
      }
    } catch (e) {
      failed.set(a.id, errorText(e));
      forgetLogListeners(a.id);
    } finally {
      loading.delete(a.id);
    }
  })();
  loading.set(a.id, p);
  return p;
}

/// Sidebar entries for the pages of addons that are on.
export function addonNavItems(): { screen: Screen; icon: string; label: string }[] {
  return store.addons.filter((a) => a.enabled).flatMap((a) => a.pages.map((p) => ({ screen: screenOf(a.id, p.id), icon: p.icon, label: p.label })));
}

function errorPage(title: string, text: string) {
  return h(
    "main",
    { class: "main narrow" },
    h("header", { class: "page-head" }, h("h1", {}, title)),
    h("section", { class: "card" }, h("p", { class: "pre" }, text), h("button", { class: "btn", onclick: () => go("settings") }, "Open Settings")),
  );
}

export function addonScreen(screen: Screen): HTMLElement {
  const s = parse(screen);
  const info = s && store.addons.find((a) => a.id === s.id);
  if (!s || !info) return errorPage("Addon", "This addon isn't installed anymore.");
  const why = failed.get(s.id);
  if (why) return errorPage(info.name, `${info.name} couldn't be loaded. Try updating or reinstalling it in Settings > Addons.\n\n${why}`);
  const l = loaded.get(s.id);
  if (!l) return h("main", { class: "main" }, h("p", { class: "muted" }, `Loading ${info.name}...`));
  const page = l.pages[s.page];
  if (!page) return errorPage(info.name, `${info.name} is missing its page "${s.page}". Try updating or reinstalling it in Settings > Addons.`);
  try {
    return page.render();
  } catch (e) {
    return errorPage(info.name, `${info.name} hit an error while drawing its page.\n\n${errorText(e)}`);
  }
}

function safely(id: string, f: () => void) {
  try {
    f();
  } catch (e) {
    toast(`${store.addons.find((a) => a.id === id)?.name ?? id}: ${errorText(e)}`, "error");
  }
}

export function enterAddon(screen: Screen) {
  const s = parse(screen);
  const page = s && loaded.get(s.id)?.pages[s.page];
  if (s && page?.enter) safely(s.id, () => page.enter!());
}

/// A log line from one of the addon's steps.
export function addonLog(id: string, level: string, text: string) {
  for (const fn of logListeners.get(id) ?? []) safely(id, () => fn(level === "detail" ? "detail" : "info", text));
}

/// Another project was opened.
export function resetAddons() {
  for (const [id, l] of loaded) for (const page of Object.values(l.pages)) if (page.reset) safely(id, () => page.reset!());
}

/// Offers the project's default addons that are missing. Official ones install once the person
/// says yes; a no is remembered for the project. When installing isn't possible, Settings > Addons
/// says so.
export async function installDefaults() {
  store.addonNotice = null;
  try {
    const o = await api.addonDefaultOffers();
    const missing = [...o.missing];
    if (o.offer.length > 0) {
      const names = o.offer.map((a) => a.name);
      const one = names.length === 1;
      const them = one ? "it" : "them";
      const answer = await modal<"install" | "no" | null>(
        (close) =>
          h(
            "div",
            { class: "card" },
            h("h2", {}, one ? `Install ${names[0]}?` : "Install addons?"),
            h(
              "p",
              { class: "muted pre" },
              `This project uses ${one ? `the ${names[0]} addon` : `these addons: ${names.join(", ")}`}. Install ${them}?\n\nAddons can change your project. You can turn ${them} off later in Settings > Addons.`,
            ),
            h(
              "div",
              { class: "actions end" },
              h("button", { class: "btn", onclick: () => close("no") }, "Don't install"),
              h("button", { class: "btn primary", onclick: () => close("install") }, "Install"),
            ),
          ),
        null,
      );
      // Closing the dialog without an answer asks again next time; only "Don't install" is kept.
      if (answer === null) return;
      const r = await api.addonInstallDefaults(o.offer.map((a) => a.id), answer === "install");
      if (r.installed.length > 0) {
        toast(`Installed ${r.installed.join(", ")} for this project. You can turn it off in Settings > Addons.`);
        await refreshAddons();
      }
      missing.push(...r.missing);
    }
    if (missing.length > 0) {
      const [id, why] = missing[0];
      const name = store.addons.find((a) => a.id === id)?.name ?? id;
      const more = missing.length > 1 ? ` (and ${plural(missing.length - 1, "other addon")})` : "";
      store.addonNotice = `This project uses the addon ${name}${more}, but it couldn't be installed. ${why} Install it from a zip below.`;
      rerender();
    }
  } catch (e) {
    store.addonNotice = errorText(e);
  }
}
