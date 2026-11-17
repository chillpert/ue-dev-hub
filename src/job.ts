// The running-job view: steps with progress, live output, and what to do when it's done.
import { api, type JobSnapshot } from "./api";
import { store, rerender } from "./store";
import { LiveOutput } from "./output";
import { errorText, h, icon, toast } from "./ui";

const MAX_LINES = 3000;

// Built once and kept between renders, so thousands of lines aren't rebuilt per frame.
export const jobOutput = new LiveOutput(MAX_LINES);
let shownJobId = -1;

export function appendJobLine(level: string, text: string) {
  jobOutput.append(level, text);
}

/// Refills the output from the job's own buffer, after a restart or switching jobs.
export function syncJobOutput() {
  const job = store.job;
  if (!job || job.snapshot.id === shownJobId) return;
  shownJobId = job.snapshot.id;
  jobOutput.clear();
  for (const [level, text] of job.lines) appendJobLine(level, text);
}

export async function refreshJob() {
  store.job = await api.jobState().catch(() => null);
  syncJobOutput();
  rerender();
}

export function onJobEvent(snap: JobSnapshot) {
  if (!store.job || store.job.snapshot.id !== snap.id) {
    store.job = { snapshot: snap, lines: [] };
    shownJobId = snap.id;
    jobOutput.clear();
  } else {
    store.job.snapshot = snap;
  }
  rerender();
}

export async function startJob(start: () => Promise<JobSnapshot>) {
  try {
    const snap = await start();
    // The job may already have logged lines (and reset the output) before this returns.
    if (shownJobId !== snap.id) {
      jobOutput.clear();
      shownJobId = snap.id;
    }
    if (!store.job || store.job.snapshot.id !== snap.id) store.job = { snapshot: snap, lines: [] };
    rerender();
  } catch (e) {
    toast(errorText(e), "error");
  }
}

/// "45s", "12m 05s" or "1h 02m".
export function duration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

/// A time that counts up while `end` is missing. The ticker below updates it every second
/// without re-rendering the screen.
function timer(start: number | null, end: number | null, prefix = "") {
  if (start == null) return null;
  if (end != null) return h("span", { class: "muted small nums" }, prefix + duration(end - start));
  return h("span", { class: "muted small nums", "data-since": String(start), "data-prefix": prefix }, prefix + duration(Date.now() - start));
}

/// A step's time as a small badge of its own, so it doesn't run into the step's note.
function stepTime(start: number | null, end: number | null) {
  const t = timer(start, end);
  if (!t) return null;
  t.className = "step-time nums";
  return t;
}

window.setInterval(() => {
  for (const el of document.querySelectorAll<HTMLElement>("[data-since]")) {
    el.textContent = (el.dataset.prefix ?? "") + duration(Date.now() - Number(el.dataset.since));
  }
}, 1000);

function stepIcon(state: string, index: number) {
  if (state === "done") return h("span", { class: "step-icon done" }, icon("check", 13, 'stroke-width="3"'));
  if (state === "failed") return h("span", { class: "step-icon failed" }, "!");
  if (state === "skipped") return h("span", { class: "step-icon skipped" }, "–");
  if (state === "running") return h("span", { class: "step-icon running" }, h("span", { class: "spinner" }));
  return h("span", { class: "step-icon pending" }, String(index + 1));
}

export function stepsList(snap: JobSnapshot) {
  return h(
    "ol",
    { class: "job-steps" },
    ...snap.steps.map((s, i) => {
      const running = s.state === "running";
      const pct = snap.progress?.percent;
      return h(
        "li",
        { class: `job-step ${s.state}` },
        stepIcon(s.state, i),
        h(
          "div",
          { class: "grow" },
          h("div", { class: "row between gap" }, h("span", { class: "strong" }, s.title), stepTime(s.startedMs, s.endedMs)),
          s.note && h("div", { class: "muted small ellipsis step-note", title: s.note }, s.note),
          running &&
            h(
              "div",
              { class: `progress ${pct == null ? "indeterminate" : ""}` },
              h("div", { style: pct == null ? "" : `width: ${Math.min(100, pct).toFixed(1)}%` }),
            ),
          running && snap.progress?.label && h("div", { class: "muted small ellipsis" }, snap.progress.label),
        ),
      );
    }),
  );
}

function headline(snap: JobSnapshot): string {
  if (!snap.finished) return `${snap.title}...`;
  if (snap.ok) return `${snap.title}: done`;
  if (snap.cancelled) return `${snap.title}: cancelled`;
  return `${snap.title}: failed`;
}

export function outputCard(snap: JobSnapshot | null) {
  return h(
    "section",
    { class: "card output-card", "aria-labelledby": "out" },
    h(
      "div",
      { class: "row between" },
      h("h2", { id: "out" }, "Output"),
      h(
        "div",
        { class: "row gap-s" },
        h(
          "button",
          {
            class: "btn small",
            onclick: () =>
              navigator.clipboard.writeText(jobOutput.text).then(
                () => toast("Output copied"),
                (e) => toast(errorText(e), "error"),
              ),
          },
          icon("copy", 14),
          "Copy",
        ),
        snap?.logPath && h("button", { class: "btn small", onclick: () => api.openPath(snap.logPath!).catch((e) => toast(errorText(e), "error")) }, icon("file", 14), "Open log file"),
      ),
    ),
    jobOutput.el,
  );
}

/// The full job screen. `aside` adds screen-specific content on the right.
export function jobView(aside: HTMLElement[] = [], onAgain?: () => void) {
  const snap = store.job!.snapshot;
  syncJobOutput();
  const result =
    snap.finished &&
    h(
      "div",
      { class: `banner ${snap.ok ? "ok" : snap.cancelled ? "" : "error"}` },
      h("div", { class: "grow" }, h("strong", {}, snap.ok ? "Done. " : snap.cancelled ? "Cancelled. " : "It didn't work. "), h("span", { class: "pre" }, snap.message ?? "")),
      snap.errors.length > 0 && h("div", { class: "errors" }, snap.errors.join("\n")),
    );
  return h(
    "main",
    { class: "main job-page" },
    h(
      "header",
      { class: "page-head" },
      h(
        "div",
        { class: "col gap-xs" },
        h(
          "div",
          { class: "muted small" },
          [snap.subtitle, `started ${snap.started}`].filter(Boolean).join(" · "),
          " · ",
          timer(snap.startedMs, snap.endedMs, snap.finished ? "took " : ""),
        ),
        h("h1", {}, headline(snap)),
      ),
      h(
        "div",
        { class: "row gap-s" },
        !snap.finished && h("button", { class: "btn", onclick: () => api.cancelJob().catch((e) => toast(errorText(e), "error")) }, "Cancel"),
        snap.finished && snap.ok && snap.kind === "package" && snap.outputPath &&
          h("button", { class: "btn primary", onclick: () => api.runBuild(snap.outputPath!).catch((e) => toast(errorText(e), "error")) }, icon("launch", 16, 'fill="currentColor" stroke="none"'), "Play"),
        snap.finished && snap.outputPath && h("button", { class: "btn", onclick: () => api.revealPath(snap.outputPath!).catch((e) => toast(errorText(e), "error")) }, icon("folder", 16), "Show build"),
        snap.finished &&
          h(
            "button",
            {
              class: "btn",
              onclick: async () => {
                await api.dismissJob().catch(() => {});
                store.job = null;
                onAgain?.();
                rerender();
              },
            },
            "Done",
          ),
      ),
    ),
    h(
      "div",
      { class: aside.length > 0 ? "job-split" : "job-split no-aside" },
      h("div", { class: "job-col" }, result, h("section", { class: "card", "aria-label": "Progress" }, stepsList(snap)), outputCard(snap)),
      aside.length > 0 && h("aside", { class: "col gap" }, ...aside),
    ),
  );
}
