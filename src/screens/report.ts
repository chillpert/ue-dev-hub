// Report a problem: posts the report, with logs and screenshots, to the team's Discord channel.
import { api, type ReportContext, type ReportInput } from "../api";
import { current, rerender, store } from "../store";
import { errorBanner, errorText, h, icon, isWindows, toast } from "../ui";

const r = {
  ctx: null as ReportContext | null,
  error: null as string | null,
  title: "",
  steps: "",
  happened: "",
  expected: "",
  media: [] as string[],
  attachCommit: true,
  attachEngineLog: true,
  attachBuildLog: false,
  attachHubLog: true,
  attachSetup: true,
  busy: false,
  /// Send was pressed without a title: the field is marked until one is typed.
  titleMissing: false,
  sentNote: null as string | null,
  project: "",
};

export async function loadReport() {
  const gen = store.gen;
  try {
    const ctx = await api.reportContext();
    if (!current(gen)) return;
    r.ctx = ctx;
    r.error = null;
    if (r.project !== store.project!.root) {
      r.project = store.project!.root;
      r.attachEngineLog = r.ctx.engineLog.exists;
      r.attachBuildLog = r.ctx.buildLog.exists && r.ctx.buildLog.failed === true;
    }
  } catch (e) {
    if (!current(gen)) return;
    r.error = errorText(e);
  }
  rerender();
}

export function resetReport() {
  r.ctx = null;
  r.error = null;
}

/// Fills in the form from a problem the hub ran into, for "Report a problem" buttons.
export function prefillReport(title: string, happened: string, buildFailed: boolean) {
  r.title = title;
  r.titleMissing = false;
  r.happened = happened;
  if (buildFailed) r.attachBuildLog = true;
  r.sentNote = null;
}

function setupText(): string {
  return store.checks.map((c) => `- ${c.title}: ${c.status === "ok" ? c.detail : `${c.status.toUpperCase()}, ${c.detail.split("\n")[0]}`}`).join("\n");
}

function input(): ReportInput {
  const c = r.ctx!;
  const logs: string[] = [];
  if (r.attachEngineLog && c.engineLog.exists) logs.push(c.engineLog.path);
  if (r.attachBuildLog && c.buildLog.exists) logs.push(c.buildLog.path);
  if (r.attachHubLog && c.hubLog.exists) logs.push(c.hubLog.path);
  return {
    title: r.title,
    steps: r.steps,
    happened: r.happened,
    expected: r.expected,
    includeCommit: r.attachCommit,
    setup: r.attachSetup ? setupText() : "",
    logs,
    media: r.media,
  };
}

async function send() {
  if (r.busy || !r.ctx) return;
  if (!r.title.trim()) {
    r.titleMissing = true;
    rerender();
    requestAnimationFrame(() => document.querySelector<HTMLInputElement>('input[name="report-title"]')?.focus());
    return;
  }
  r.busy = true;
  r.sentNote = null;
  rerender();
  try {
    const sent = await api.sendReport(input());
    const notes = [
      sent.trimmed.length ? `Only the end of ${sent.trimmed.join(" and ")} was attached, to fit Discord's size limit.` : "",
      sent.skipped.length ? `Too big to attach: ${sent.skipped.join(", ")}. Share those another way.` : "",
    ].filter(Boolean);
    toast("Sent to Discord. Thanks!");
    // Clear the form for the next report, but keep the choices.
    r.title = r.steps = r.happened = r.expected = "";
    r.titleMissing = false;
    r.media = [];
    r.sentNote = notes.length ? notes.join(" ") : null;
  } catch (e) {
    toast(errorText(e), "error");
  }
  r.busy = false;
  rerender();
}

async function copyMarkdown() {
  try {
    await navigator.clipboard.writeText(await api.reportMarkdown(input()));
    toast("Copied as Markdown");
  } catch (e) {
    toast(errorText(e), "error");
  }
}

function field(label: string, key: "title" | "steps" | "happened" | "expected", rows: number, placeholder = "", required = false) {
  const invalid = required && key === "title" && r.titleMissing;
  const hint = h("span", { class: "small error-text", id: `${key}-hint` }, invalid ? "Give it a title." : "");
  const onInput = (e: Event) => {
    const el = e.target as HTMLInputElement | HTMLTextAreaElement;
    r[key] = el.value;
    if (invalid && el.value.trim()) {
      r.titleMissing = false;
      el.classList.remove("invalid");
      el.removeAttribute("aria-invalid");
      hint.textContent = "";
    }
  };
  const attrs = {
    value: r[key],
    placeholder,
    name: `report-${key}`,
    class: invalid ? "invalid" : undefined,
    required: required || undefined,
    "aria-invalid": invalid ? "true" : undefined,
    "aria-describedby": invalid ? `${key}-hint` : undefined,
    oninput: onInput,
  };
  const el = rows === 1 ? h("input", { type: "text", ...attrs }) : h("textarea", { rows, ...attrs });
  return h(
    "label",
    { class: "field" },
    h("span", { class: "strong small" }, label, required && h("span", { class: "required", "aria-hidden": "true" }, "*")),
    el,
    invalid && hint,
  );
}

function attach(label: string, note: string, key: "attachCommit" | "attachEngineLog" | "attachBuildLog" | "attachHubLog" | "attachSetup", disabled = false) {
  return h(
    "label",
    { class: "option" },
    h("input", {
      type: "checkbox",
      checked: r[key] && !disabled,
      disabled,
      onchange: () => {
        r[key] = !r[key];
        rerender();
      },
    }),
    h("span", { class: "col min0" }, h("span", {}, label), h("span", { class: "muted small ellipsis" }, note)),
  );
}

export function reportScreen() {
  const c = r.ctx;
  const p = store.project!;
  const buildNote = !c?.buildLog.exists
    ? "The hub hasn't compiled yet"
    : c.buildLog.failed
      ? `The last compile failed (${c.buildLog.modified})`
      : "Compiled without errors, so it's off by default";
  return h(
    "main",
    { class: "main" },
    h(
      "header",
      { class: "page-head" },
      h("div", { class: "col gap-xs" }, h("h1", {}, "Report a problem"), h("p", { class: "muted" }, "Describe what went wrong. It's posted to the team's Discord channel with your logs and the exact commit.")),
    ),
    !c
      ? r.error
        ? errorBanner(r.error, loadReport)
        : h("p", { class: "muted" }, "Loading...")
      : h(
          "div",
          { class: "split" },
          h(
            "form",
            { class: "card col gap grow-3", onsubmit: (e: Event) => e.preventDefault() },
            field("Title", "title", 1, "What's wrong, in a few words", true),
            field("Steps to reproduce", "steps", 4, "1. Open ...\n2. ...\n3. ..."),
            field("What happened", "happened", 3, "What you saw"),
            field("What should happen", "expected", 3, "What you expected"),
            r.media.length > 0 &&
              h(
                "ul",
                { class: "list" },
                ...r.media.map((m) =>
                  h(
                    "li",
                    { class: "list-row" },
                    icon("image", 16),
                    h("span", { class: "grow ellipsis small" }, m.split(/[\\/]/).pop()),
                    h("button", { type: "button", class: "icon-btn", "aria-label": "Remove", onclick: () => ((r.media = r.media.filter((x) => x !== m)), rerender()) }, icon("x", 14)),
                  ),
                ),
              ),
            h(
              "button",
              {
                type: "button",
                class: "btn dashed",
                onclick: async () => {
                  const files = await api.reportPickMedia().catch((e) => (toast(errorText(e), "error"), []));
                  r.media.push(...files);
                  rerender();
                },
              },
              icon("image", 16),
              "Add a screenshot or video (optional)",
            ),
          ),
          h(
            "aside",
            { class: "card sticky col gap grow-1", "aria-labelledby": "att" },
            h("h2", { id: "att" }, "Attached for you"),
            attach("Branch and commit", `${c.branch} @ ${c.commit}`, "attachCommit"),
            attach("Engine log", c.engineLog.exists ? `${p.name}.log from ${c.engineLog.modified}` : "No engine log yet", "attachEngineLog", !c.engineLog.exists),
            attach("Last build log", buildNote, "attachBuildLog", !c.buildLog.exists),
            attach("Hub log", c.hubLog.exists ? "What the hub did since it started" : "No hub log yet", "attachHubLog", !c.hubLog.exists),
            attach("Your setup", isWindows() ? "Engine, Visual Studio and .NET" : "Engine, Git and .NET", "attachSetup"),
            h(
              "p",
              { class: "muted small" },
              `Logs are zipped. Everything together must stay under ${c.maxUploadMb} MB, so long logs keep only their end and big videos may be left out.`,
            ),
            c.hasWebhook
              ? h("button", { class: "btn primary block", disabled: r.busy, onclick: send }, icon("chat", 16), r.busy ? "Sending..." : "Send to Discord")
              : h(
                  "div",
                  { class: "banner" },
                  h("span", { class: "small" }, "Sending isn't set up for this project yet. It needs a Discord webhook as report.discord_webhook in ue-dev-hub.toml. Until then, copy the report and paste it yourself."),
                ),
            r.sentNote && h("p", { class: "warn-text small" }, r.sentNote),
            h("button", { class: c.hasWebhook ? "btn block" : "btn primary block", onclick: copyMarkdown }, icon("copy", 16), "Copy as text"),
            p.issueUrl && h("a", { href: p.issueUrl, class: "row gap-xs", onclick: (e: Event) => (e.preventDefault(), api.openUrl(p.issueUrl!).catch((err) => toast(errorText(err), "error"))) }, icon("external", 14), "Open the task tracker"),
          ),
        ),
  );
}
