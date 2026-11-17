// App state shared by all screens. Screens read it and call `rerender()` after changing it.
import type { AddonInfo, AfterLaunch, Check, JobState, NextcloudStatus, Prefs, ProjectEntry, ProjectInfo } from "./api";

export type BuiltinScreen = "launch" | "package" | "builds" | "test" | "report" | "tools" | "setup" | "settings";
/// Addon pages are `addon:<addon id>:<page id>`.
export type Screen = BuiltinScreen | `addon:${string}`;

export const store = {
  project: null as ProjectInfo | null,
  projectError: null as string | null,
  projects: [] as ProjectEntry[],
  prefs: null as Prefs | null,
  checks: [] as Check[],
  checking: false,
  screen: "launch" as Screen,
  job: null as JobState | null,
  version: "",
  /// False when running without being installed (the copy Launch.sh downloads).
  installed: true,
  /// Installed with a Linux package: updates come from the package manager, not the hub.
  packageManaged: false,
  /// The addon API version the hub implements; addons see it as `hub.version`.
  addonApi: 1,
  /// Update & Launch starts by itself after a countdown when the hub opens a project.
  autoLaunch: false,
  /// What happens to the hub once the editor it started is running.
  afterLaunch: "keep" as AfterLaunch,
  /// Update & Launch (or Launch, or Update & compile) is running; other work waits for it.
  launching: false,
  /// Which of those, for "X is running" messages.
  flowName: "Update & Launch",
  /// Names of the addon steps running, like "Pushing". The backend runs one at a time.
  steps: [] as string[],
  swapperOpen: false,
  /// Installed addons, and what the project needs that couldn't be installed.
  addons: [] as AddonInfo[],
  addonNotice: null as string | null,
  nextcloud: null as NextcloudStatus | null,
  connecting: false,
  /// Increases with every project load, so answers for a previous project can be dropped.
  gen: 0,
};

/// Whether an answer asked for at `gen` (a value of `store.gen`) is still for the open project.
export function current(gen: number): boolean {
  return gen === store.gen;
}

let renderFn: () => void = () => {};
let frame = 0;

export function setRenderer(fn: () => void) {
  renderFn = fn;
}

// A repaint replaces the page, so one between pressing and releasing the mouse button would
// swallow the click (the button pressed is gone by the release). While a button is held, repaints
// wait for the release, or a few seconds at most if the release happens outside the window.
let held = false;
let heldTimer = 0;
let waiting = false;

function release() {
  held = false;
  clearTimeout(heldTimer);
  if (waiting) {
    waiting = false;
    rerender();
  }
}
window.addEventListener(
  "pointerdown",
  () => {
    held = true;
    clearTimeout(heldTimer);
    heldTimer = window.setTimeout(release, 3000);
  },
  true,
);
window.addEventListener("pointerup", release, true);
window.addEventListener("pointercancel", release, true);
window.addEventListener("blur", release);

/// Repaints at most once per frame.
export function rerender() {
  if (held) {
    waiting = true;
    return;
  }
  if (frame) return;
  frame = requestAnimationFrame(() => {
    frame = 0;
    if (held) waiting = true;
    else renderFn();
  });
}

export function renderNow() {
  if (frame) cancelAnimationFrame(frame);
  frame = 0;
  waiting = false;
  renderFn();
}

export function problemCount(): number {
  return store.checks.filter((c) => c.status === "error" || c.status === "warning").length;
}

export function blockingCount(): number {
  return store.checks.filter((c) => c.status === "error").length;
}

export function hasTool(id: string): boolean {
  return store.project?.tools.includes(id) ?? false;
}

/// Which screen shows a job of this kind.
export function screenForJob(kind: string): Screen {
  if (kind === "package") return "package";
  if (kind === "testMap") return "test";
  if (kind === "upload") return "builds";
  return "tools";
}

export function jobRunning(): boolean {
  return !!store.job && !store.job.snapshot.finished;
}

/// Anything running that other work has to wait for: a job, Update & Launch or an addon step.
export function busy(): boolean {
  return jobRunning() || store.launching || store.steps.length > 0;
}

/// What's running, for "X is running" messages.
export function busyLabel(): string {
  if (jobRunning()) return store.job!.snapshot.title;
  if (store.launching) return store.flowName;
  return store.steps[0] ?? "";
}
