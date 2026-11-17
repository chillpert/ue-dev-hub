import { listen } from "@tauri-apps/api/event";
import { api, type JobSnapshot, type Progress } from "./api";
import { appendJobLine, onJobEvent, refreshJob } from "./job";
import { go, onEnter, onEnterAddon, onLeaveLaunch, setAddonNavItems, setProjectHandlers, sidebar } from "./nav";
import { addonLog, addonNavItems, addonScreen, enterAddon, installDefaults, refreshAddons, resetAddons } from "./addons";
import { busy, current, jobRunning, renderNow, setRenderer, store, type BuiltinScreen } from "./store";
import { errorText, h, setPlatform, toast } from "./ui";
import { checkForHubUpdate, offerInstall } from "./updater";
import { loadNextcloud } from "./nextcloud";
import { restoreOutputs } from "./output";
import { appendLaunchLine, cancelCountdown, chooseProjectFolder, isLaunching, launchScreen, loadHome, resetLaunch, setLaunchProgress, startCountdown } from "./screens/launch";
import { loadPackage, packageScreen, resetPackage } from "./screens/package";
import { buildsScreen, loadBuilds, resetBuilds } from "./screens/builds";
import { loadReport, reportScreen, resetReport } from "./screens/report";
import { loadSettings, settingsScreen } from "./screens/settings";
import { runChecks, setReloadProject, setupScreen } from "./screens/setup";
import { loadTestMap, resetTestMap, testMapScreen } from "./screens/testmap";
import { loadTools, resetTools, toolsScreen } from "./screens/tools";

const app = document.getElementById("app")!;

function welcome() {
  const others = store.projects.filter((p) => !p.missing);
  return h(
    "main",
    { class: "welcome" },
    h(
      "div",
      { class: "card col gap" },
      h("h1", {}, others.length ? "Pick a project" : "Welcome"),
      store.projectError && h("p", { class: "error-text" }, store.projectError),
      others.length > 0 &&
        h(
          "ul",
          { class: "list" },
          ...others.map((p) =>
            h(
              "li",
              { class: "list-row" },
              h("span", { class: "col grow min0" }, h("span", { class: "strong" }, p.displayName), h("span", { class: "muted small mono ellipsis" }, p.root)),
              h("button", { class: "btn small", onclick: () => switchProject(p.root) }, "Open"),
            ),
          ),
        ),
      h("p", { class: "muted" }, "Or pick the folder of another game project: the folder with the .uproject file."),
      h("button", { class: others.length ? "btn" : "btn hero", onclick: addProject }, "Choose project folder"),
    ),
  );
}

/// Which text field had focus, so a re-render can give it back with the cursor where it was.
function focusedField() {
  const el = document.activeElement;
  if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement)) return null;
  const key = el.getAttribute("aria-label") ?? el.getAttribute("placeholder") ?? el.getAttribute("name");
  if (!key) return null;
  const sel = el instanceof HTMLSelectElement ? null : { start: el.selectionStart, end: el.selectionEnd, scroll: el.scrollTop };
  return { tag: el.tagName, key, sel };
}

function restoreFocus(f: ReturnType<typeof focusedField>) {
  if (!f) return;
  const match = [...document.querySelectorAll<HTMLElement>(f.tag)].find(
    (e) => (e.getAttribute("aria-label") ?? e.getAttribute("placeholder") ?? e.getAttribute("name")) === f.key,
  );
  if (!match) return;
  match.focus();
  if (f.sel && (match instanceof HTMLInputElement || match instanceof HTMLTextAreaElement)) {
    try {
      match.setSelectionRange(f.sel.start, f.sel.end);
    } catch {
      // Some input types (like checkboxes) have no selection.
    }
    match.scrollTop = f.sel.scroll;
  }
}

/// Scroll positions of lists marked with `data-keep-scroll`, so re-renders don't jump them to the top.
function keptScrolls() {
  return [...document.querySelectorAll<HTMLElement>("[data-keep-scroll]")].map((e) => [e.dataset.keepScroll!, e.scrollTop] as const);
}

function restoreScrolls(kept: ReturnType<typeof keptScrolls>) {
  for (const [key, top] of kept) {
    const el = document.querySelector<HTMLElement>(`[data-keep-scroll="${CSS.escape(key)}"]`);
    if (el) el.scrollTop = top;
  }
}

/// Draws a built-in screen. An error shows on that screen instead of blanking the window.
function builtinScreen(draw: (() => HTMLElement) | undefined): HTMLElement {
  try {
    if (!draw) throw new Error(`There's no screen called "${store.screen}".`);
    return draw();
  } catch (e) {
    console.error(e);
    return h(
      "main",
      { class: "main narrow" },
      h("header", { class: "page-head" }, h("h1", {}, "Something went wrong")),
      h(
        "section",
        { class: "card col gap" },
        h("p", { class: "pre" }, `This page hit an error while drawing.\n\n${errorText(e)}`),
        h("div", { class: "actions" }, h("button", { class: "btn", onclick: () => go("launch") }, "Go to Launch")),
      ),
    );
  }
}

function render() {
  const main = document.querySelector(".main");
  const scroll = main?.scrollTop ?? 0;
  const lists = keptScrolls();
  const focus = focusedField();
  if (!store.project) {
    app.replaceChildren(welcome());
    restoreFocus(focus);
    return;
  }
  const screens = {
    launch: launchScreen,
    package: packageScreen,
    builds: buildsScreen,
    test: testMapScreen,
    report: reportScreen,
    tools: toolsScreen,
    setup: setupScreen,
    settings: settingsScreen,
  };
  const view = store.screen.startsWith("addon:") ? addonScreen(store.screen) : builtinScreen(screens[store.screen as BuiltinScreen]);
  app.replaceChildren(h("div", { class: "shell" }, sidebar(), view));
  const next = document.querySelector(".main");
  if (next) next.scrollTop = scroll;
  restoreScrolls(lists);
  restoreOutputs();
  restoreFocus(focus);
}
setRenderer(render);

// Repaints replace spinners and moving progress bars with new ones, which would start their
// animation over each time and stutter. Each new one starts where a never-replaced one would be.
const ANIMATED: [string, number][] = [
  [".spinner", 800],
  [".progress.indeterminate > div", 1400],
];
function syncAnimations(root: Element) {
  const now = performance.now();
  for (const [selector, period] of ANIMATED) {
    const els = root.matches(selector) ? [root] : root.querySelectorAll<HTMLElement>(selector);
    for (const el of els as Iterable<HTMLElement>) el.style.animationDelay = `-${Math.round(now % period)}ms`;
  }
}
new MutationObserver((records) => {
  for (const r of records) for (const n of r.addedNodes) if (n instanceof HTMLElement) syncAnimations(n);
}).observe(document.body, { childList: true, subtree: true });

// ---------- Loading projects ----------

/// Set when the hub was opened on a project: Update & Launch may start once checks are done.
let pendingAuto: { token: number; checks: Promise<unknown> } | null = null;

/// Loads everything for the selected project. `opened`: the hub was just started for it
/// (or Launch.sh opened it), not picked in the switcher.
async function loadProject(opened = false) {
  const token = ++store.gen;
  cancelCountdown();
  pendingAuto = null;
  let project: typeof store.project = null;
  let error: string | null = null;
  try {
    project = await api.getProject();
  } catch (e) {
    error = errorText(e);
  }
  // A newer load took over while this one waited.
  if (!current(token)) return;
  store.project = project;
  store.projectError = error;
  resetLaunch();
  resetPackage();
  resetBuilds();
  resetTestMap();
  resetTools();
  resetReport();
  resetAddons();
  const projects = await api.listProjects().catch(() => []);
  if (!current(token)) return;
  store.projects = projects;
  if (!store.project) {
    renderNow();
    return;
  }
  const prefs = await api.getPrefs().catch(() => null);
  if (!current(token)) return;
  store.prefs = prefs;
  await refreshAddons();
  if (!current(token)) return;
  ask(() => (current(token) ? installDefaults() : Promise.resolve()));
  loadNextcloud();
  renderNow();
  await refreshJob();
  if (!current(token)) return;
  const checks = runChecks();
  loadHome(true);
  enterCurrent();
  if (opened) pendingAuto = { token, checks };
}

/// Questions at startup, like installing the hub or a project's addons, open one at a time
/// in the order they were asked, so they never stack and one Escape can't answer two.
let questions: Promise<unknown> = Promise.resolve();

function ask(question: () => Promise<unknown>) {
  questions = questions.then(question).catch(() => {});
}

/// Starts the Update & Launch countdown if the hub was opened on this project, once every
/// question at startup has been answered.
async function autoLaunchIfWanted() {
  const p = pendingAuto;
  pendingAuto = null;
  if (!p) return;
  await p.checks;
  if (current(p.token)) startCountdown();
}

function enterCurrent() {
  const loaders: Partial<Record<BuiltinScreen, () => void>> = {
    package: loadPackage,
    builds: loadBuilds,
    test: loadTestMap,
    report: loadReport,
    tools: loadTools,
    settings: loadSettings,
    launch: () => loadHome(false),
  };
  if (store.screen.startsWith("addon:")) enterAddon(store.screen);
  else loaders[store.screen as BuiltinScreen]?.();
}

async function switchProject(root: string) {
  if (busy()) {
    toast("Wait for the running task to finish before switching projects.", "error");
    return;
  }
  try {
    await api.selectProject(root);
    store.screen = "launch";
    await loadProject();
  } catch (e) {
    toast(errorText(e), "error");
  }
}

async function addProject() {
  if (busy()) {
    toast("Wait for the running task to finish before adding a project.", "error");
    return;
  }
  const dir = await chooseProjectFolder();
  if (!dir) return;
  try {
    await api.addProject(dir);
    store.screen = "launch";
    await loadProject();
  } catch (e) {
    store.projectError = errorText(e);
    if (store.project) toast(store.projectError, "error");
    renderNow();
  }
}

setProjectHandlers(switchProject, addProject);
onEnterAddon(enterAddon);
setAddonNavItems(addonNavItems);
onLeaveLaunch(cancelCountdown);
setReloadProject(async () => {
  store.project = await api.getProject().catch(() => store.project);
  renderNow();
  await runChecks();
});
onEnter("package", loadPackage);
onEnter("builds", loadBuilds);
onEnter("test", loadTestMap);
onEnter("report", loadReport);
onEnter("tools", loadTools);
onEnter("settings", loadSettings);
onEnter("setup", runChecks);
onEnter("launch", () => loadHome(false));

// Anything that failed without its own handling still says so, instead of doing nothing.
window.addEventListener("unhandledrejection", (e) => {
  e.preventDefault();
  toast(errorText(e.reason), "error");
});

// ---------- Events from Rust ----------

// Log lines come in batches. Addon steps log to their own page, through `hub.onLog`.
listen<{ id?: string; level: string; text: string }[]>("log-lines", (e) => {
  for (const line of e.payload) {
    if (line.id) addonLog(line.id, line.level, line.text);
    else if (isLaunching()) appendLaunchLine(line.level, line.text);
    else if (jobRunning()) appendJobLine(line.level, line.text);
    else appendLaunchLine(line.level, line.text);
  }
});
listen<Progress>("build-progress", (e) => setLaunchProgress(e.payload));
listen<JobSnapshot>("job", (e) => {
  onJobEvent(e.payload);
  // When a job ends, refresh what it may have changed.
  if (e.payload.finished) {
    if (e.payload.kind === "package") {
      loadPackage();
      loadBuilds();
    }
    if (store.screen === "tools") loadTools();
  }
});
listen("project-changed", async () => {
  store.screen = "launch";
  await loadProject(true);
  await autoLaunchIfWanted();
});

document.addEventListener("mousedown", (e) => {
  if (store.swapperOpen && !(e.target as HTMLElement).closest(".swapper-wrap")) {
    store.swapperOpen = false;
    renderNow();
  }
});

// ---------- Startup ----------

(async () => {
  // A reload in the middle of Update & Launch would otherwise leave its flow marked as running.
  await api.setFlow(null).catch(() => {});
  const info = await api.hubInfo().catch(() => null);
  if (info) {
    store.version = info.version;
    store.installed = info.installed;
    store.packageManaged = info.packageManaged;
    store.addonApi = info.addonApi;
    setPlatform(info.platform);
  }
  store.autoLaunch = await api.getAutoLaunch().catch(() => false);
  store.afterLaunch = await api.getAfterLaunch().catch(() => "keep" as const);
  // Asked before the project's questions, which loading the project adds after it.
  if (info?.offerInstall && !import.meta.env.DEV) ask(offerInstall);
  await loadProject(true);
  // Questions first, so nothing starts by itself underneath them.
  await questions;
  await autoLaunchIfWanted();
  checkForHubUpdate();
})();
