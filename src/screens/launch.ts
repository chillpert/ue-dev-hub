// The home screen: Update & Launch, and the widgets each person picks.
import { open } from "@tauri-apps/plugin-dialog";
import { api, type BuildResult, type HomeStatus, type Progress, type UpdateInfo } from "../api";
import { blockingCount, busy as anythingRunning, busyLabel, current, jobRunning, rerender, store } from "../store";
import { errorText, h, icon, ideNames, modal, plural, toast } from "../ui";
import { avatar, avatarsVersion, loadAvatars } from "../avatars";
import { runChecks } from "./setup";
import { LiveOutput } from "../output";
import { go } from "../nav";
import { prefillReport } from "./report";

type StepId = "setup" | "update" | "build" | "launch";
type StepState = "idle" | "running" | "done" | "failed" | "skipped";
/// Update & Launch, Launch without updating, or Update & compile (no editor).
type Mode = "updateLaunch" | "launch" | "update";
const MODE_NAMES: Record<Mode, string> = { updateLaunch: "Update & Launch", launch: "Launch", update: "Update & compile" };

interface Problem {
  title: string;
  message: string;
  errors?: string[];
  logPath?: string | null;
}

const ideState = { ides: [] as { id: string; name: string }[], ide: null as string | null, loaded: "" };

export function forgetIdeChoice() {
  ideState.loaded = "";
}

async function loadIdes() {
  const root = store.project?.root ?? "";
  if (ideState.loaded === root) return;
  ideState.loaded = root;
  const gen = store.gen;
  const r = await api.ideChoices().catch(() => null);
  if (!current(gen)) return;
  ideState.ides = r?.ides ?? [];
  ideState.ide = r?.ide ?? null;
  rerender();
}

const launch = {
  running: false,
  /// What the last run did, so "Try again" repeats the same thing.
  lastMode: "updateLaunch" as Mode,
  /// Cancel was pressed: the flow stops before its next step.
  stopping: false,
  /// The update itself is being applied, which can't be stopped halfway.
  applying: false,
  logOpen: false,
  steps: {} as Record<StepId, { state: StepState; note: string }>,
  progress: null as Progress | null,
  /// Cancel compile was pressed and the compiler is being stopped.
  cancelling: false,
  problem: null as Problem | null,
  status: null as HomeStatus | null,
  statusError: null as string | null,
  fetching: false,
};

function resetSteps() {
  for (const id of ["setup", "update", "build", "launch"] as StepId[]) launch.steps[id] = { state: "idle", note: "" };
  launch.progress = null;
  launch.problem = null;
}
resetSteps();

// ---------- Watching the editor ----------

/// After a launch, the Unreal Editor tile follows the real editor: "Running" once it shows up,
/// and back to normal once it's closed, instead of keeping the note from when it started.
const editorWatch = { seen: false, busy: false, last: null as boolean | null, launched: false };
const editorListeners: ((running: boolean) => void)[] = [];

/// Package and Test a map follow the editor too, so their "close Unreal Editor first" clears.
export function onEditorChange(fn: (running: boolean) => void) {
  editorListeners.push(fn);
}

async function watchEditor() {
  if (editorWatch.busy || !store.project || document.visibilityState !== "visible") return;
  editorWatch.busy = true;
  try {
    const running = await api.editorRunning();
    if (editorWatch.last !== running) {
      editorWatch.last = running;
      for (const fn of editorListeners) fn(running);
    }
    if (launch.status && launch.status.editorRunning !== running) {
      launch.status.editorRunning = running;
      rerender();
    }
    const st = launch.steps.launch;
    if (!launch.running && st.state === "done") {
      if (running && st.note !== "Running" && st.note !== "Already running") {
        editorWatch.seen = true;
        setStep("launch", "done", "Running");
        if (editorWatch.launched) {
          editorWatch.launched = false;
          api.editorStarted().catch(() => {});
        }
      } else if (!running && editorWatch.seen) {
        // The editor was closed: back to the normal state of all tiles.
        editorWatch.seen = false;
        resetSteps();
        rerender();
      }
    }
  } catch {
    // Checking is best effort.
  }
  editorWatch.busy = false;
}
window.setInterval(watchEditor, 4000);

// ---------- Starting by itself ----------

const AUTO_SECONDS = 5;
const countdown = { left: 0, timer: 0, started: 0 };

function stopCountdown() {
  if (countdown.timer) clearInterval(countdown.timer);
  countdown.timer = 0;
  countdown.left = 0;
  countdown.started = 0;
  document.removeEventListener("keydown", onCountdownKey);
}

function onCountdownKey(e: KeyboardEvent) {
  if (e.key === "Escape") cancelCountdown();
}

export function cancelCountdown() {
  if (!countdown.left) return;
  stopCountdown();
  rerender();
}

/// Counts down, then starts Update & Launch, unless the person turned it off, something needs
/// fixing first, or something else is running. Called when the hub opens a project.
export function startCountdown() {
  const dialogOpen = !!document.querySelector(".backdrop");
  if (!store.autoLaunch || dialogOpen || launch.running || anythingRunning() || blockingCount() > 0 || store.screen !== "launch") return;
  stopCountdown();
  countdown.left = AUTO_SECONDS;
  countdown.started = performance.now();
  document.addEventListener("keydown", onCountdownKey);
  countdown.timer = window.setInterval(() => {
    countdown.left -= 1;
    if (countdown.left <= 0) {
      stopCountdown();
      // Re-check: a task may have started, a dialog opened, or the person moved on.
      if (store.screen === "launch" && !anythingRunning() && blockingCount() === 0 && !document.querySelector(".backdrop")) run("updateLaunch");
    }
    rerender();
  }, 1000);
  rerender();
}

export async function setAutoLaunch(enabled: boolean) {
  store.autoLaunch = enabled;
  if (!enabled) cancelCountdown();
  try {
    await api.setAutoLaunch(enabled);
  } catch (e) {
    toast(errorText(e), "error");
  }
  rerender();
}

/// The project names a Git hooks folder and the person hasn't said whether to use it. Setup's
/// check is a warning only then; once answered (or set up elsewhere) it isn't.
function hooksUnanswered() {
  return store.checks.some((c) => c.id === "gitHooks" && c.status === "warning");
}

function cancelCompile() {
  launch.cancelling = true;
  rerender();
  api
    .cancelJob()
    .catch((e) => toast(errorText(e), "error"))
    .finally(() => {
      launch.cancelling = false;
      rerender();
    });
}

export function isLaunching() {
  return launch.running;
}

export function setLaunchProgress(p: Progress) {
  if (!launch.running) return;
  // Kept for later; nothing shows it, so it doesn't repaint (it comes with every compiler line).
  launch.progress = p;
}

// Output of the launch flow, kept between renders.
export const launchOutput = new LiveOutput(3000);
export function appendLaunchLine(level: string, text: string) {
  launchOutput.append(level, text);
}

/// Reloads the home status. `fetch` also asks the server for new changes; `quiet` does that
/// without showing "Checking..." (for the automatic refreshes).
export async function loadHome(fetch: boolean, quiet = false) {
  const gen = store.gen;
  if (!quiet) launch.fetching = fetch;
  if (fetch) lastFetch = Date.now();
  rerender();
  try {
    const status = await api.homeStatus(fetch);
    if (!current(gen)) return;
    launch.status = status;
    launch.statusError = null;
    void loadAvatars(status.incoming.map((c) => c.email));
    if (store.project && store.project.branch !== status.branch) store.project.branch = status.branch;
  } catch (e) {
    if (!current(gen)) return;
    launch.statusError = errorText(e);
  }
  launch.fetching = false;
  rerender();
}

// ---------- Staying up to date ----------

/// Fetch again when coming back to the hub after this long, and in the background this often.
const FETCH_ON_FOCUS_MS = 3 * 60_000;
const FETCH_EVERY_MS = 10 * 60_000;
let lastFetch = 0;
let refreshing = false;

async function refreshHome(fetchAfter: number) {
  if (refreshing || !store.project || launch.running || launch.fetching || jobRunning()) return;
  refreshing = true;
  try {
    await loadHome(Date.now() - lastFetch >= fetchAfter, true);
  } finally {
    refreshing = false;
  }
}

// Coming back to the window picks up commits or branch switches made elsewhere.
window.addEventListener("focus", () => refreshHome(FETCH_ON_FOCUS_MS));
// While the window is open, look for the team's new changes now and then.
window.setInterval(() => {
  if (document.visibilityState === "visible") refreshHome(FETCH_EVERY_MS);
}, 60_000);

export function resetLaunch() {
  launch.status = null;
  incomingCache = null;
  launch.statusError = null;
  launch.fetching = false;
  // The editor being watched belongs to the previous project.
  editorWatch.seen = false;
  editorWatch.launched = false;
  editorWatch.last = null;
  launchOutput.clear();
  resetSteps();
}

function setStep(id: StepId, state: StepState, note = "") {
  launch.steps[id] = { state, note };
  rerender();
}

/// Stops the flow before its next step. Compiling has its own Cancel, and updating can't be
/// stopped halfway, so this is for the checks and the question before updating.
function stopFlow() {
  launch.stopping = true;
  rerender();
}

async function run(mode: Mode) {
  stopCountdown();
  if (launch.running || !store.project) return;
  if (anythingRunning()) {
    toast(`Wait for ${busyLabel()} to finish first.`, "error");
    return;
  }
  const update = mode !== "launch";
  const openEditor = mode !== "update";
  try {
    // Nothing else may start between the steps below, like packaging on a half-updated tree.
    await api.setFlow(MODE_NAMES[mode]);
  } catch (e) {
    toast(errorText(e), "error");
    return;
  }
  launch.running = true;
  launch.stopping = false;
  launch.lastMode = mode;
  store.launching = true;
  store.flowName = MODE_NAMES[mode];
  resetSteps();
  launchOutput.clear();
  rerender();

  /// Marks the steps not reached as cancelled once Cancel was pressed. True if it was.
  const stopped = () => {
    if (!launch.stopping) return false;
    for (const id of ["setup", "update", "build", "launch"] as StepId[]) {
      const st = launch.steps[id].state;
      if (st === "idle" || st === "running") launch.steps[id] = { state: "skipped", note: "Cancelled" };
    }
    return true;
  };

  try {
    setStep("setup", "running", "Checking...");
    const checks = await runChecks();
    if (stopped()) return;
    const blocking = checks.filter((c) => c.status === "error");
    if (blocking.length > 0) {
      setStep("setup", "failed", `${plural(blocking.length, "thing")} to fix`);
      launch.problem = { title: "Your setup needs fixing first", message: blocking.map((c) => `${c.title}: ${c.detail}`).join("\n") };
      return;
    }
    setStep("setup", "done", `All ${checks.filter((c) => c.status !== "optional").length} checks pass`);

    if (update) {
      setStep("update", "running", "Downloading the latest changes...");
      try {
        await api.prepareRepo();
        const info = await api.checkUpdates();
        if (stopped()) return;
        if (info.commits.length === 0) {
          setStep("update", "done", "Up to date");
        } else {
          const choice = await askToUpdate(info, openEditor);
          if (choice === "cancel") launch.stopping = true;
          if (stopped()) return;
          if (choice === "skip") {
            setStep("update", "skipped", "Skipped");
          } else if (choice !== "cancel") {
            setStep("update", "running", choice.push ? "Updating your branch here and on the server..." : "Updating your branch...");
            launch.applying = true;
            let r;
            try {
              r = await api.applyUpdate(choice.push);
            } finally {
              launch.applying = false;
            }
            if (r.warnings.length) toast(r.warnings.join("\n"), "error");
            setStep("update", "done", `Got ${plural(r.commits, "new change")}${r.pushed ? ", pushed" : ""}`);
          }
        }
      } catch (e) {
        setStep("update", "failed", "Couldn't update");
        launch.problem = { title: "Updating didn't work", message: errorText(e) };
        return;
      }
    } else {
      setStep("update", "skipped", "Skipped");
    }
    if (stopped()) return;

    setStep("build", "running", "Can take a while the first time");
    let result: BuildResult;
    try {
      result = await api.buildProject();
    } catch (e) {
      if (errorText(e) === "Cancelled.") {
        setStep("build", "failed", "Cancelled");
        return;
      }
      setStep("build", "failed");
      launch.problem = { title: "Compiling didn't start", message: errorText(e) };
      return;
    }
    launch.progress = null;
    if (!result.ok) {
      setStep("build", "failed", "Didn't compile");
      launch.problem = {
        title: "The game code didn't compile",
        message:
          result.hint ??
          "This usually means someone pushed code that doesn't compile, or something on your computer is missing. Press Report a problem below to send these details.",
        errors: result.errors,
        logPath: result.logPath,
      };
      return;
    }
    setStep("build", result.skipped ? "skipped" : "done", result.skipped ? "No C++ code" : "Compiled, no errors");
    if (stopped()) return;

    if (!openEditor) {
      setStep("launch", "skipped", "Not opened. Ready when you are.");
      return;
    }
    // Don't open a second editor on the same project.
    if (await api.editorRunning().catch(() => false)) {
      setStep("launch", "done", "Already running");
      editorWatch.seen = true;
      return;
    }
    setStep("launch", "running", "Starting...");
    try {
      await api.launchEditor();
      setStep("launch", "done", "Starting. Its window can take a few minutes.");
      editorWatch.seen = false;
      editorWatch.launched = true;
    } catch (e) {
      setStep("launch", "failed");
      launch.problem = { title: "Unreal Editor didn't start", message: errorText(e) };
    }
  } finally {
    await api.setFlow(null).catch(() => {});
    launch.running = false;
    launch.stopping = false;
    store.launching = false;
    rerender();
    loadHome(false);
  }
}

/// Asks before updating. Resolves with whether to push, "skip" to go on without updating, or
/// "cancel" to stop (Escape too). `thenLaunch`: Unreal Editor opens afterwards, so skipping the
/// update still leaves something to do.
function askToUpdate(info: UpdateInfo, thenLaunch: boolean): Promise<{ push: boolean } | "skip" | "cancel"> {
  const n = info.commits.length;
  const base = info.onDefaultBranch ? `origin/${info.defaultBranch}` : info.defaultBranch;
  const merging = info.strategy === "merge";
  const askPush = info.canPush && info.pushPolicy === "ask";
  let push = info.canPush && info.pushPolicy !== "never";
  return modal<{ push: boolean } | "skip" | "cancel">((close) => {
    const confirm = h("button", { class: "btn primary", onclick: () => close({ push }) });
    const label = () => {
      const verb = merging ? "Merge" : "Rebase";
      confirm.textContent = push ? (merging ? "Merge and push" : "Rebase and force push") : info.myCommits > 0 || merging ? verb : "Update";
    };
    label();
    const what = merging
      ? `Merges the latest ${base} into ${info.branch}.`
      : info.myCommits > 0
        ? `Rebases your ${plural(info.myCommits, "commit")} on ${info.branch} onto the latest ${base}.`
        : `Moves ${info.branch} up to the latest ${base}. You have no commits of your own on it.`;
    const pushNote = merging
      ? `Uploads ${info.branch} to the server with the merged changes. Untick to only update this computer.`
      : `Replaces ${info.branch} on the server with the rebased version. Untick to only update this computer; the server keeps the old version until you push.`;
    return h(
      "div",
      { class: "card wide" },
      h("h2", {}, `${plural(n, "new change")} on ${info.defaultBranch}`),
      h("ul", { class: "commits" }, ...info.commits.map((c) => h("li", {}, h("span", { class: "muted" }, `${c.author}: `), c.subject))),
      info.blockedReason
        ? [
            h("div", { class: "banner error" }, h("div", {}, h("strong", {}, "Can't update right now"), h("p", { class: "pre" }, info.blockedReason))),
            h(
              "div",
              { class: "actions end" },
              thenLaunch
                ? [h("button", { class: "btn", onclick: () => close("cancel") }, "Cancel"), h("button", { class: "btn primary", onclick: () => close("skip") }, "Launch without updating")]
                : h("button", { class: "btn primary", onclick: () => close("cancel") }, "Close"),
            ),
          ]
        : [
            h(
              "ul",
              { class: "facts" },
              h("li", {}, what),
              merging && info.serverHasMore > 0 && h("li", {}, `Also brings in ${plural(info.serverHasMore, "commit")} that ${info.branch} has on the server.`),
              info.hasLocalChanges && h("li", {}, "Your uncommitted changes are set aside during the update and put back after."),
              h("li", {}, `A backup of ${info.branch} is saved first, so nothing can get lost.`),
              !askPush && push && h("li", {}, `Then ${merging ? "pushes" : "force pushes"} ${info.branch} to the server.`),
            ),
            askPush &&
              h(
                "label",
                { class: "option push-option" },
                h("input", {
                  type: "checkbox",
                  checked: push,
                  onchange: (e: Event) => {
                    push = (e.target as HTMLInputElement).checked;
                    label();
                  },
                }),
                h("span", { class: "col" }, h("span", { class: "strong" }, `${merging ? "Push" : "Force push"} ${info.branch} to the server afterwards`), h("span", { class: "muted small" }, pushNote)),
              ),
            h(
              "div",
              { class: "actions end" },
              h("button", { class: "btn", onclick: () => close("cancel") }, "Cancel"),
              thenLaunch && h("button", { class: "btn", onclick: () => close("skip") }, "Launch without updating"),
              confirm,
            ),
          ],
    );
  }, "cancel");
}

// ---------- Tiles ----------

function tile(title: string, state: StepState, note: string, badge?: string, tone?: "ok" | "warn" | "error") {
  const iconEl =
    state === "running"
      ? h("span", { class: "step-icon running" }, h("span", { class: "spinner" }))
      : state === "failed" || tone === "error"
        ? h("span", { class: "step-icon failed" }, "!")
        : state === "done" || tone === "ok"
          ? h("span", { class: "step-icon done" }, icon("check", 13, 'stroke-width="3"'))
          : badge
            ? h("span", { class: "step-icon warn" }, badge)
            : state === "skipped"
              ? h("span", { class: "step-icon skipped" }, "–")
              : h("span", { class: "step-icon pending" });
  return h("li", { class: "tile" }, iconEl, h("span", { class: "col" }, h("span", { class: "strong" }, title), h("span", { class: "muted small" }, note)));
}

function tiles() {
  const s = launch.status;
  const st = launch.steps;
  const active = launch.running || Object.values(st).some((x) => x.state !== "idle");
  const problems = blockingCount();

  const setupNote = store.checking ? "Checking..." : problems > 0 ? `${plural(problems, "thing")} to fix` : `All ${store.checks.filter((c) => c.status !== "optional").length} checks pass`;
  const incoming = s?.incoming.length ?? 0;
  const updNote = launch.fetching ? "Checking..." : !s ? "" : incoming > 0 ? `${plural(incoming, "new change")} on ${s.baseBranch}` : `Up to date with ${s.baseBranch}`;
  const codeNote = !s ? "" : s.lastCompileFailed ? "Last compile failed" : s.lastCompile ? `Compiled ${s.lastCompile.replace(/^Today, /, "today ")}` : "Not compiled by the hub yet";
  const editorNote = s?.editorRunning ? "Running" : "Not running";

  const pick = (id: StepId, idleNote: string) => (active && st[id].state !== "idle" ? st[id].note : idleNote);
  return h(
    "ol",
    { class: "tiles" },
    tile("Setup", active ? st.setup.state : "idle", pick("setup", setupNote), undefined, !active ? (problems > 0 ? "error" : store.checks.length ? "ok" : undefined) : undefined),
    tile("Updates", active ? st.update.state : "idle", pick("update", updNote), !active && incoming > 0 ? String(Math.min(incoming, 99)) : undefined, !active && s && incoming === 0 ? "ok" : undefined),
    tile("Code", active ? st.build.state : "idle", pick("build", codeNote), undefined, !active && s ? (s.lastCompileFailed ? "error" : s.lastCompile ? "ok" : undefined) : undefined),
    tile("Unreal Editor", active ? st.launch.state : "idle", pick("launch", editorNote), undefined, !active && s?.editorRunning ? "ok" : undefined),
  );
}

function problemCard(p: Problem) {
  const report = () =>
    [
      `Project: ${store.project?.displayName} (${store.project?.branch ?? "?"})`,
      `Engine: ${store.project?.engineVersion}`,
      "",
      p.title,
      p.message,
      ...(p.errors?.length ? ["", "Errors:", ...p.errors] : []),
      ...(p.logPath ? ["", `Full log: ${p.logPath}`] : []),
    ].join("\n");
  return h(
    "section",
    { class: "card problem", role: "alert" },
    h("h2", {}, p.title),
    h("p", { class: "pre" }, p.message),
    p.errors && p.errors.length > 0 && h("div", { class: "errors" }, p.errors.join("\n")),
    h(
      "div",
      { class: "actions" },
      h(
        "button",
        {
          class: "btn",
          onclick: () =>
            navigator.clipboard.writeText(report()).then(
              () => toast("Details copied"),
              (e) => toast(errorText(e), "error"),
            ),
        },
        "Copy details",
      ),
      p.logPath && h("button", { class: "btn", onclick: () => api.openPath(p.logPath!).catch((e) => toast(errorText(e), "error")) }, "Open log file"),
      h(
        "button",
        {
          class: "btn",
          onclick: () => {
            prefillReport(p.title, [p.message, ...(p.errors ?? [])].join("\n"), !!p.logPath);
            go("report");
          },
        },
        "Report a problem",
      ),
      blockingCount() > 0 && h("button", { class: "btn", onclick: () => go("setup") }, "Open Setup"),
      h("button", { class: "btn primary", onclick: () => run(launch.lastMode), disabled: launch.running }, "Try again"),
    ),
  );
}

// ---------- Widgets ----------

export const WIDGETS: Record<string, { title: string; description: string }> = {
  incoming: { title: "New changes", description: "What the team pushed that you don't have yet" },
  work: { title: "Your work", description: "Your branch, unsaved changes and last compile" },
  ide: {
    title: "Open your IDE",
    get description() {
      return `${ideNames()}, remembered per project`;
    },
  },
  shortcuts: { title: "Shortcuts", description: "Package, test a map, report a problem, terminal" },
};

export function widgets(): string[] {
  // Ignores widgets that no longer exist, like the old recent builds.
  return (store.prefs?.widgets ?? store.project?.defaultWidgets ?? []).filter((w) => w in WIDGETS);
}

/// The list of new commits, kept between repaints: it can have hundreds of rows with pictures,
/// and only changes with the status or when pictures arrive.
let incomingCache: { status: HomeStatus; avatars: number; el: HTMLElement } | null = null;

function incomingList(s: HomeStatus) {
  if (incomingCache?.status === s && incomingCache.avatars === avatarsVersion()) return incomingCache.el;
  const list = s.incoming;
  const total = Math.max(s.incomingTotal, list.length);
  const el = h(
    "ul",
    { class: "list incoming-list", "data-keep-scroll": "incoming" },
    ...list.map((c) =>
      h(
        "li",
        { class: "list-row", title: `${c.hash}  ${c.subject}` },
        avatar(c.author, c.email),
        h("span", { class: "grow ellipsis" }, c.subject),
        h("span", { class: "muted small nowrap" }, `${c.author} · ${c.when}`),
      ),
    ),
    total > list.length && h("li", { class: "list-row muted small" }, `and ${total - list.length} older ones`),
  );
  incomingCache = { status: s, avatars: avatarsVersion(), el };
  return el;
}

function incomingWidget() {
  const s = launch.status;
  const base = s?.baseBranch ?? store.project?.baseBranch ?? "";
  const list = s?.incoming ?? [];
  const total = Math.max(s?.incomingTotal ?? 0, list.length);
  return h(
    "section",
    { class: "card widget grow-3" },
    h(
      "div",
      { class: "row between baseline" },
      h("h2", {}, `New on ${base}`),
      h("span", { class: "muted small" }, list.length ? `${plural(total, "commit")} you don't have yet` : ""),
    ),
    !s
      ? h("p", { class: "muted" }, launch.statusError ?? "Loading...")
      : list.length === 0
        ? h("p", { class: "muted" }, s.fetchError ? `Couldn't check for changes: ${s.fetchError}` : "You have everything.")
        : incomingList(s),
  );
}

function ideWidget() {
  loadIdes();
  const current = ideState.ides.find((i) => i.id === ideState.ide) ?? ideState.ides[0];
  return h(
    "section",
    { class: "card widget ide-widget" },
    h("div", { class: "row gap-s" }, h("span", { class: "accent-icon" }, icon("code", 20)), h("h2", { class: "grow" }, WIDGETS.ide.title)),
    ideState.ides.length === 0
      ? h("p", { class: "muted" }, `No IDE found. Install ${ideNames()}.`)
      : h(
          "div",
          { class: "row gap-s" },
          ideState.ides.length > 1 &&
            h(
              "select",
              {
                class: "select grow",
                "aria-label": "IDE",
                onchange: async (e: Event) => {
                  const id = (e.target as HTMLSelectElement).value;
                  ideState.ide = id;
                  if (store.prefs) store.prefs.ide = id;
                  await api.setAppChoice("ide", id).catch((err) => toast(errorText(err), "error"));
                },
              },
              ...ideState.ides.map((i) => h("option", { value: i.id, selected: i.id === current?.id ? true : undefined }, i.name)),
            ),
          h(
            "button",
            { class: "btn primary", onclick: () => api.openIde(current?.id ?? null).catch((e) => toast(errorText(e), "error")) },
            ideState.ides.length > 1 ? "Open" : `Open in ${current?.name}`,
          ),
        ),
  );
}

function workWidget() {
  const s = launch.status;
  const server: Record<string, string> = {
    upToDate: "Up to date",
    ahead: "You have commits to push",
    behind: "Behind the server",
    diverged: "Different from the server",
    notOnServer: "Not on the server yet",
  };
  const row = (k: string, v: Node | string) => h("div", { class: "kv" }, h("span", { class: "muted" }, k), v);
  return h(
    "section",
    { class: "card widget grow-2" },
    h("h2", {}, "Your work"),
    !s
      ? h("p", { class: "muted" }, launch.statusError ?? "Loading...")
      : h(
          "div",
          { class: "col" },
          row("Branch", h("span", { class: "mono" }, s.branch)),
          !s.onBaseBranch && row(`Ahead of ${s.baseBranch}`, plural(s.ahead, "commit")),
          row("On the server", server[s.server] ?? s.server),
          row("Uncommitted changes", s.unsavedFiles === 0 ? "None" : plural(s.unsavedFiles, "file")),
          row("Last compile", s.lastCompile ? (s.lastCompileFailed ? `${s.lastCompile}, failed` : s.lastCompile) : "Never"),
        ),
  );
}

function shortcutsWidget() {
  const card = (ic: string, title: string, sub: string, onclick: () => void) =>
    h("button", { class: "shortcut", onclick }, h("span", { class: "accent-icon" }, icon(ic, 20)), h("span", { class: "strong" }, title), h("span", { class: "muted small" }, sub));
  return h(
    "section",
    { class: "shortcuts", "aria-label": "Shortcuts" },
    card("package", "Package the game", "A build to test or ship", () => go("package")),
    card("map", "Test a map", "Cook one map and run it", () => go("test")),
    card("bug", "Report a problem", "Logs and commit attached", () => go("report")),
    store.project?.tools.includes("terminal")
      ? card("terminal", "Open terminal", "In the project folder", () => api.openTerminal(store.prefs?.terminal ?? null).catch((e) => toast(errorText(e), "error")))
      : card("tools", "Tools", "Everything else", () => go("tools")),
  );
}

function renderWidgets() {
  const list = widgets();
  const out: HTMLElement[] = [];
  // Card widgets pair up side by side; full-width ones get their own row.
  let row: HTMLElement[] = [];
  const flush = () => {
    if (row.length) out.push(h("div", { class: "widget-row" }, ...row));
    row = [];
  };
  for (const id of list) {
    if (id === "incoming") row.push(incomingWidget());
    else if (id === "work") row.push(workWidget());
    else if (id === "ide") row.push(ideWidget());
    else {
      flush();
      out.push(shortcutsWidget());
    }
    if (row.length === 2) flush();
  }
  flush();
  return out;
}

export async function customizeHome() {
  const team = store.project?.defaultWidgets ?? [];
  let chosen = [...widgets()];
  const result = await modal<string[] | null | undefined>((close) => {
    const listEl = h("ul", { class: "widget-picker" });
    const draw = () => {
      const order = [...chosen, ...Object.keys(WIDGETS).filter((w) => !chosen.includes(w))];
      listEl.replaceChildren(
        ...order.map((id) => {
          const on = chosen.includes(id);
          const i = chosen.indexOf(id);
          const move = (d: number) => {
            const j = i + d;
            if (j < 0 || j >= chosen.length) return;
            [chosen[i], chosen[j]] = [chosen[j], chosen[i]];
            draw();
          };
          return h(
            "li",
            { class: `picker-row ${on ? "on" : ""}` },
            h(
              "label",
              { class: "row gap-s grow" },
              h("input", {
                type: "checkbox",
                checked: on,
                onchange: () => {
                  chosen = on ? chosen.filter((c) => c !== id) : [...chosen, id];
                  draw();
                },
              }),
              h("span", { class: "col" }, h("span", { class: "strong" }, WIDGETS[id].title), h("span", { class: "muted small" }, WIDGETS[id].description)),
            ),
            on && h("button", { class: "icon-btn", "aria-label": `Move ${WIDGETS[id].title} up`, disabled: i === 0, onclick: () => move(-1) }, icon("up", 16)),
            on && h("button", { class: "icon-btn", "aria-label": `Move ${WIDGETS[id].title} down`, disabled: i === chosen.length - 1, onclick: () => move(1) }, icon("chevronDown", 16)),
          );
        }),
      );
    };
    draw();
    return h(
      "div",
      { class: "card wide" },
      h("h2", {}, "Customize your home screen"),
      h("p", { class: "muted" }, "Launch always stays at the top. This only changes your hub, not the team's."),
      listEl,
      h(
        "div",
        { class: "actions between" },
        h("button", { class: "btn", onclick: () => close(null), title: `Team default: ${team.map((t) => WIDGETS[t]?.title ?? t).join(", ")}` }, "Use the team default"),
        h("div", { class: "row gap-s" }, h("button", { class: "btn", onclick: () => close(undefined) }, "Cancel"), h("button", { class: "btn primary", onclick: () => close(chosen) }, "Save")),
      ),
    );
  }, undefined);
  if (result === undefined) return;
  try {
    await api.setWidgets(result);
  } catch (e) {
    toast(errorText(e), "error");
    return;
  }
  if (store.prefs) store.prefs.widgets = result;
  loadHome(false);
}

export function launchScreen() {
  const s = launch.status;
  const p = store.project!;
  const sub = s
    ? s.onBaseBranch
      ? `${s.branch}`
      : `${s.branch} · ${s.ahead === 0 ? `even with ${s.baseBranch}` : `${plural(s.ahead, "commit")} ahead of ${s.baseBranch}`}`
    : p.branch ?? "";
  const title = launch.running ? (launch.lastMode === "update" ? "Updating..." : "Getting ready...") : blockingCount() > 0 ? "Setup needs attention" : "Ready to launch";
  const busy = launch.running || anythingRunning();

  return h(
    "main",
    { class: "main" },
    h(
      "header",
      { class: "page-head" },
      h("div", { class: "col gap-xs" }, h("div", { class: "muted small" }, sub), h("h1", {}, title)),
      h(
        "div",
        { class: "row gap-s" },
        h("button", { class: "btn", onclick: customizeHome }, icon("layout", 16), "Customize"),
        h("button", { class: "btn", onclick: () => loadHome(true), disabled: launch.fetching }, icon("refresh", 16), launch.fetching ? "Checking..." : "Check for changes"),
      ),
    ),
    h(
      "section",
      { class: "card launch", "aria-label": "Launch" },
      countdown.left > 0
        ? [
            h(
              "div",
              { class: "launch-buttons" },
              h(
                "button",
                {
                  class: "btn hero counting",
                  // The fill animates over the whole countdown; re-renders pick up where it is.
                  style: `--total: ${AUTO_SECONDS}s; --elapsed: -${((performance.now() - countdown.started) / 1000).toFixed(2)}s`,
                  title: "Start now",
                  onclick: () => run("updateLaunch"),
                },
                icon("launch", 20, 'fill="currentColor" stroke="none"'),
                `Update & Launch in ${countdown.left}...`,
              ),
              h("button", { class: "btn hero-2", onclick: cancelCountdown }, "Cancel"),
            ),
            h(
              "label",
              { class: "row gap-s muted small" },
              h("input", { type: "checkbox", checked: true, onchange: () => setAutoLaunch(false) }),
              "Start Update & Launch when the hub opens (Esc cancels this time)",
            ),
          ]
        : h(
            "div",
            { class: "launch-buttons" },
            h(
              "button",
              { class: "btn hero", onclick: () => run("updateLaunch"), disabled: busy },
              icon("launch", 20, 'fill="currentColor" stroke="none"'),
              launch.running ? "Working..." : "Update & Launch",
            ),
            launchSideButtons(busy),
          ),
      !launch.running && anythingRunning() && h("p", { class: "muted small" }, `${busyLabel()} is running. Launch is available when it's done.`),
      hooksUnanswered() &&
        h(
          "div",
          { class: "banner warn small" },
          h("span", { class: "grow" }, "This project wants to use its Git hooks. Update & Launch leaves them off until you choose on Setup."),
          h("button", { class: "btn small", onclick: () => go("setup") }, "Open Setup"),
        ),
      tiles(),
      (launch.running || !launchOutput.empty) && h("details", { class: "log-details", open: launch.logOpen ? true : undefined, ontoggle: (e: Event) => (launch.logOpen = (e.target as HTMLDetailsElement).open) }, h("summary", {}, "Technical details"), launchOutput.el),
    ),
    launch.problem && problemCard(launch.problem),
    ...renderWidgets(),
  );
}

/// Next to Update & Launch: the other ways to run it, or a way to stop it while it runs.
function launchSideButtons(busy: boolean) {
  if (launch.running) {
    if (launch.steps.build.state === "running")
      return h("button", { class: "btn hero-2", onclick: cancelCompile, disabled: launch.cancelling }, launch.cancelling ? "Cancelling..." : "Cancel compile");
    if (launch.applying) return h("button", { class: "btn hero-2", disabled: true, title: "Updating can't be stopped halfway. It's quick." }, "Updating...");
    if (launch.steps.launch.state === "running") return h("button", { class: "btn hero-2", disabled: true }, "Starting...");
    return h("button", { class: "btn hero-2", onclick: stopFlow, disabled: launch.stopping }, launch.stopping ? "Cancelling..." : "Cancel");
  }
  return h(
    "div",
    { class: "launch-alt" },
    h(
      "button",
      { class: "btn", onclick: () => run("launch"), disabled: busy, title: "Compiles if needed and opens Unreal Editor, without getting the team's changes" },
      icon("launch", 14, 'fill="currentColor" stroke="none"'),
      "Launch without updating",
    ),
    h("button", { class: "btn", onclick: () => run("update"), disabled: busy, title: "Gets the team's changes and compiles, without opening Unreal Editor" }, icon("download", 14), "Update & compile"),
  );
}

export async function chooseProjectFolder() {
  const dir = await open({ directory: true, title: "Pick your project folder (the one with the .uproject file)" });
  if (typeof dir !== "string") return null;
  return dir;
}

