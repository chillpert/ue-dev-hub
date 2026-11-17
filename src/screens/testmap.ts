// Test a map: cook one map and run it as a standalone game.
import { api, type MapInfo, type MapStatus, type TestMapInfo, type TestRun } from "../api";
import { jobView, startJob } from "../job";
import { busy, busyLabel, current, rerender, store } from "../store";
import { onEditorChange } from "./launch";
import { confirmDialog, errorText, h, icon, modal, toast } from "../ui";

const t = {
  info: null as TestMapInfo | null,
  error: null as string | null,
  query: "",
  mainOnly: true,
  selected: null as string | null,
  buildType: "Development",
  name: "",
  bake: false,
  quality: "Medium",
  light: null as MapStatus | null,
  args: null as string | null,
  optionsOpen: false,
  project: "",
};

let resultsEl: HTMLElement | null = null;

onEditorChange((running) => {
  if (t.info && t.info.editorRunning !== running) {
    t.info.editorRunning = running;
    rerender();
  }
});

export async function loadTestMap() {
  const gen = store.gen;
  try {
    const info = await api.testMapInfo();
    if (!current(gen)) return;
    t.info = info;
    t.error = null;
    if (t.project !== store.project!.root) {
      t.project = store.project!.root;
      t.selected = t.info.recent[0] ?? null;
      // Remember the last build type, unless it was Test and this engine can't build Test.
      const last = t.info.lastBuildType ?? "Development";
      t.buildType = last === "Test" && !t.info.sourceBuild ? "Development" : last;
      t.args = null;
      t.query = "";
      t.bake = false;
      t.quality = store.project!.lightingQuality;
    }
    loadLight();
  } catch (e) {
    if (!current(gen)) return;
    t.error = errorText(e);
  }
  rerender();
}

export function resetTestMap() {
  t.info = null;
  t.project = "";
}

/// Reads whether the selected map changed since its last bake on this computer.
async function loadLight() {
  t.light = null;
  const map = t.selected;
  if (!map) return;
  const gen = store.gen;
  const status = await api.mapLighting(map).catch(() => null);
  if (current(gen) && t.selected === map) {
    t.light = status;
    rerender();
  }
}

function lightNote(): string {
  const l = t.light;
  if (!l) return "Checking...";
  if (l.kind === "unchanged") return "Unchanged since your last bake on this computer.";
  if (l.kind === "changed") return `${l.status} since your last bake: ${l.changedFiles.join(", ")}.`;
  if (l.kind === "missing") return "The map file wasn't found.";
  return `${l.status}.`;
}

// ---------- Saved runs ----------

function size(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  return `${Math.max(1, Math.round(bytes / 1e6))} MB`;
}

function when(ms: number): string {
  const d = new Date(ms);
  const today = new Date().toDateString() === d.toDateString();
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return today ? `today ${time}` : `${d.toLocaleDateString([], { day: "numeric", month: "short" })} ${time}`;
}

async function runAction(what: () => Promise<unknown>, reload = true) {
  try {
    await what();
  } catch (e) {
    toast(errorText(e), "error");
  }
  if (reload) await loadTestMap();
}

function renameRun(run: TestRun) {
  let value = run.name;
  modal<string | null>((close) => {
    const input = h("input", {
      type: "text",
      value,
      "aria-label": "Name",
      oninput: (e: Event) => (value = (e.target as HTMLInputElement).value),
      onkeydown: (e: KeyboardEvent) => e.key === "Enter" && value.trim() && close(value),
    });
    setTimeout(() => (input.focus(), input.select()), 0);
    return h(
      "div",
      { class: "card" },
      h("h2", {}, "Rename test run"),
      input,
      h("div", { class: "actions end" }, h("button", { class: "btn", onclick: () => close(null) }, "Cancel"), h("button", { class: "btn primary", onclick: () => value.trim() && close(value) }, "Rename")),
    );
  }, null).then((name) => {
    if (name) runAction(() => api.renameTestRun(run.id, name));
  });
}

async function deleteRun(run: TestRun) {
  if (!(await confirmDialog(`Delete "${run.name}"?`, `This deletes the build from this computer and frees ${size(run.size)}.`, "Delete", true))) return;
  runAction(() => api.deleteTestRun(run.id));
}

async function deleteAllRuns(runs: TestRun[]) {
  const total = runs.reduce((n, r) => n + r.size, 0);
  if (!(await confirmDialog("Delete all test runs?", `This deletes ${runs.length === 1 ? "the saved build" : `all ${runs.length} saved builds`} from this computer and frees ${size(total)}.`, "Delete all", true))) return;
  runAction(() => api.deleteAllTestRuns());
}

function runsCard() {
  const runs = t.info?.runs ?? [];
  if (runs.length === 0) return null;
  const total = runs.reduce((n, r) => n + r.size, 0);
  return h(
    "section",
    { class: "card", "aria-labelledby": "runs" },
    h(
      "div",
      { class: "row between" },
      h("div", { class: "col" }, h("h2", { id: "runs" }, "Your test runs"), h("span", { class: "muted small" }, `${runs.length === 1 ? "1 build" : `${runs.length} builds`}, ${size(total)} on this computer`)),
      h("button", { class: "btn small", onclick: () => deleteAllRuns(runs) }, icon("trash", 14), "Delete all"),
    ),
    h(
      "div",
      { class: "col gap-xs" },
      ...runs.map((r) =>
        h(
          "div",
          { class: "picker-row" },
          h(
            "div",
            { class: "col grow min0" },
            h("span", { class: "strong ellipsis" }, r.name),
            h(
              "span",
              { class: "muted small ellipsis", title: r.launchArgs ? `Launch options: ${r.launchArgs}` : "" },
              [r.map.split("/").pop(), r.buildType, r.bakeQuality ? `${r.bakeQuality} bake` : "no bake", r.commit && `${r.branch} ${r.commit}`, when(r.createdMs), size(r.size)].filter(Boolean).join(" · "),
            ),
          ),
          h("button", { class: "btn primary small", onclick: () => runAction(() => api.playTestRun(r.id), false) }, icon("launch", 14, 'fill="currentColor" stroke="none"'), "Play"),
          h("button", { class: "btn small", onclick: () => renameRun(r) }, "Rename"),
          h("button", { class: "icon-btn", title: "Delete", "aria-label": `Delete ${r.name}`, onclick: () => deleteRun(r) }, icon("trash", 16)),
        ),
      ),
    ),
  );
}

function mapButton(m: MapInfo) {
  const on = t.selected === m.package;
  return h(
    "button",
    {
      class: `map-pick ${on ? "on" : ""}`,
      "aria-pressed": on ? "true" : "false",
      onclick: () => {
        t.selected = m.package;
        rerender();
        loadLight();
      },
    },
    h("span", { class: "col grow min0" }, h("span", { class: "strong" }, m.name), h("span", { class: "muted small mono ellipsis" }, m.package)),
    on && h("span", { class: "pill on" }, "Selected"),
  );
}

function results() {
  const info = t.info!;
  const suffix = store.project!.mainLevelSuffix;
  const q = t.query.trim().toLowerCase();
  const matches = info.maps.filter((m) => (!t.mainOnly || m.name.endsWith(suffix)) && (!q || m.package.toLowerCase().includes(q)));
  const recent = info.recent.map((r) => info.maps.find((m) => m.package === r)).filter((m): m is MapInfo => !!m);
  return h(
    "div",
    { class: "col gap" },
    recent.length > 0 && !q && h("div", { class: "col gap-xs" }, h("div", { class: "section-label" }, "Recently tested"), ...recent.map(mapButton)),
    h(
      "div",
      { class: "col gap-xs" },
      h("div", { class: "section-label" }, q ? "Matches" : "All maps"),
      matches.length === 0 ? h("p", { class: "muted" }, "No maps match.") : matches.slice(0, 150).map(mapButton),
      matches.length > 150 && h("p", { class: "muted small" }, `${matches.length - 150} more. Search to narrow them down.`),
    ),
  );
}

export function testMapScreen() {
  if (store.job?.snapshot.kind === "testMap") return jobView([], loadTestMap);
  const info = t.info;
  const p = store.project!;
  const selectedName = t.selected?.split("/").pop();
  if (info) resultsEl = h("div", {}, results());

  return h(
    "main",
    { class: "main" },
    h("header", { class: "page-head" }, h("div", { class: "col gap-xs" }, h("h1", {}, "Test a map"), h("p", { class: "muted" }, "Cooks one map and runs it as a standalone game. Much faster than packaging everything."))),
    t.error && h("div", { class: "banner error" }, t.error),
    runsCard(),
    !info
      ? h("p", { class: "muted" }, "Looking for maps...")
      : h(
          "div",
          { class: "split" },
          h(
            "section",
            { class: "card grow-3", "aria-labelledby": "pick" },
            h("h2", { id: "pick" }, "Pick a map"),
            h(
              "div",
              { class: "row gap wrap" },
              h(
                "label",
                { class: "search grow" },
                icon("map", 16),
                h("input", {
                  type: "search",
                  placeholder: "Search maps",
                  "aria-label": "Search maps",
                  value: t.query,
                  oninput: (e: Event) => {
                    t.query = (e.target as HTMLInputElement).value;
                    // Only the list changes, so the search box keeps focus.
                    resultsEl?.replaceChildren(results());
                  },
                }),
              ),
              h(
                "label",
                { class: "row gap-s", title: `Maps whose name ends in ${p.mainLevelSuffix}` },
                h("input", {
                  type: "checkbox",
                  checked: t.mainOnly,
                  onchange: () => {
                    t.mainOnly = !t.mainOnly;
                    resultsEl?.replaceChildren(results());
                  },
                }),
                "Only persistent levels",
              ),
            ),
            resultsEl,
          ),
          h(
            "aside",
            { class: "card sticky grow-1", "aria-labelledby": "run" },
            h("h2", { id: "run" }, selectedName ? `Run ${selectedName}` : "Pick a map first"),
            h("span", { class: "muted small" }, "Build type"),
            h(
              "select",
              {
                class: "select block",
                "aria-label": "Build type",
                onchange: (e: Event) => {
                  t.buildType = (e.target as HTMLSelectElement).value;
                  rerender();
                },
              },
              ...["Development", "Test", "Shipping"].map((bt) =>
                h("option", { value: bt, selected: t.buildType === bt, disabled: bt === "Test" && !info.sourceBuild }, bt === "Test" && !info.sourceBuild ? "Test (needs a source build)" : bt),
              ),
            ),
            h("span", { class: "muted small" }, "Lighting"),
            h(
              "label",
              { class: "option" },
              h("input", {
                type: "checkbox",
                checked: t.bake,
                disabled: !t.selected,
                onchange: () => {
                  t.bake = !t.bake;
                  rerender();
                },
              }),
              h("span", { class: "col" }, h("span", {}, "Bake lighting first"), h("span", { class: `small ${t.light?.kind === "changed" || t.light?.kind === "never" ? "warn-text" : "muted"}` }, t.selected ? lightNote() : "Pick a map first.")),
            ),
            t.bake &&
              h(
                "div",
                { class: "col gap-xs indent" },
                h(
                  "select",
                  {
                    class: "select block",
                    "aria-label": "Lighting quality",
                    onchange: (e: Event) => {
                      t.quality = (e.target as HTMLSelectElement).value;
                      rerender();
                    },
                  },
                  ...info.qualities.map((q) => h("option", { value: q, selected: q === t.quality }, `${q} quality`)),
                ),
                h("span", { class: "muted small" }, "Bakes the map and its sublevels. The baked files stay modified in your working copy."),
                info.editorRunning && h("span", { class: "error-text small" }, "Close Unreal Editor first."),
              ),
            h("span", { class: "muted small" }, "Name"),
            h("input", {
              type: "text",
              value: t.name,
              placeholder: selectedName ? `${selectedName}, ${t.buildType}${t.bake ? `, ${t.quality} bake` : ""}` : "",
              "aria-label": "Name this run",
              oninput: (e: Event) => (t.name = (e.target as HTMLInputElement).value),
            }),
            h(
              "details",
              { open: t.optionsOpen ? true : undefined, ontoggle: (e: Event) => (t.optionsOpen = (e.target as HTMLDetailsElement).open) },
              h("summary", {}, "Launch options"),
              h("textarea", {
                class: "mono small",
                rows: 3,
                "aria-label": "Launch options",
                value: t.args ?? p.launchArgs,
                oninput: (e: Event) => (t.args = (e.target as HTMLTextAreaElement).value),
              }),
            ),
            h(
              "button",
              {
                class: "btn primary block",
                disabled: !t.selected || busy() || (t.bake && info.editorRunning),
                onclick: () =>
                  startJob(() =>
                    api.startTestMap({
                      map: t.selected!,
                      buildType: t.buildType,
                      name: t.name,
                      bakeQuality: t.bake ? t.quality : null,
                      launchArgs: t.args ?? p.launchArgs,
                    }),
                  ),
              },
              icon("launch", 16, 'fill="currentColor" stroke="none"'),
              t.bake ? "Bake, cook and run" : "Cook and run",
            ),
            busy() && h("p", { class: "muted small" }, `${busyLabel()} is running.`),
          ),
        ),
  );
}
