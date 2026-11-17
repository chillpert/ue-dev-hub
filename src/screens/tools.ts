// Tools: everyday helpers for the project, one click each.
import { api, type ToolsInfo } from "../api";
import { jobView, startJob } from "../job";
import { busy as anythingRunning, busyLabel, current, hasTool, rerender, store } from "../store";
import { confirmDialog, errorBanner, errorText, h, icon, ideNames, modal, plural, showInFolderLabel, toast } from "../ui";
import { forgetIdeChoice } from "./launch";
import { bakeMapPicker, changedMaps, type BakeChoice } from "./bake-maps";

const tl = { info: null as ToolsInfo | null, error: null as string | null };

export async function loadTools() {
  const gen = store.gen;
  try {
    const info = await api.toolsInfo();
    if (!current(gen)) return;
    tl.info = info;
    tl.error = null;
  } catch (e) {
    if (!current(gen)) return;
    tl.error = errorText(e);
  }
  rerender();
}

export function resetTools() {
  tl.info = null;
  tl.error = null;
}

function row(ic: string, title: string, text: Node | string, ...actions: (Node | false | null)[]) {
  return h(
    "div",
    { class: "tool-row" },
    h("span", { class: "accent-icon" }, icon(ic, 20)),
    h("div", { class: "col grow min0" }, h("span", { class: "strong" }, title), h("span", { class: "muted small" }, text)),
    h("div", { class: "row gap-xs" }, ...actions),
  );
}

function chooser(kind: "ide" | "terminal", list: { id: string; name: string }[], current: string | null) {
  if (list.length < 2) return null;
  return h(
    "select",
    {
      class: "select",
      "aria-label": kind === "ide" ? "Choose an IDE" : "Choose a terminal",
      onchange: async (e: Event) => {
        const id = (e.target as HTMLSelectElement).value;
        try {
          await api.setAppChoice(kind, id);
        } catch (err) {
          toast(errorText(err), "error");
          return;
        }
        if (store.prefs) store.prefs[kind] = id;
        if (kind === "ide") forgetIdeChoice();
        await loadTools();
      },
    },
    ...list.map((a) => h("option", { value: a.id, selected: a.id === (current ?? list[0].id) ? true : undefined }, a.name)),
  );
}

const run = (fn: () => Promise<unknown>) => async () => {
  try {
    await fn();
  } catch (e) {
    toast(errorText(e), "error");
  }
};

async function chooseBakeMaps() {
  const info = await api.packageInfo().catch((e) => {
    toast(errorText(e), "error");
    return null;
  });
  if (!info) return;
  const choice: BakeChoice = { selected: changedMaps(info.maps), quality: store.project!.lightingQuality };
  const result = await modal<BakeChoice | null>((close) => {
    const body = h("div", { class: "col gap" });
    const { selected } = choice;
    const draw = () =>
      body.replaceChildren(
        ...bakeMapPicker(info.maps, info.qualities, choice, draw),
        info.editorRunning ? h("p", { class: "error-text small" }, "Close Unreal Editor first.") : "",
        h(
          "div",
          { class: "actions end" },
          h("button", { class: "btn", onclick: () => close(null) }, "Cancel"),
          h("button", { class: "btn primary", disabled: selected.size === 0 || info.editorRunning, onclick: () => close(choice) }, selected.size ? `Bake ${plural(selected.size, "map")}` : "Bake"),
        ),
      );
    draw();
    return h("div", { class: "card wide" }, h("h2", {}, "Bake lighting"), h("p", { class: "muted small" }, "Without packaging. The baked maps stay modified in your working copy."), body);
  }, null);
  if (result) await startJob(() => api.startBake([...result.selected], result.quality));
}

export function toolsScreen() {
  const job = store.job;
  if (job && ["lighting", "soundbanks", "dependencies", "regenerate", "repairGit"].includes(job.snapshot.kind)) return jobView([], loadTools);

  const p = store.project!;
  const info = tl.info;
  const busy = anythingRunning();
  const terminal = info?.terminals.find((t) => t.id === info.terminal) ?? info?.terminals[0];
  const ide = info?.ides.find((t) => t.id === info.ide) ?? info?.ides[0];
  const deps = info?.dependencies ?? [];
  const outdated = deps.filter((d) => !d.upToDate).length;

  const open = [
    hasTool("terminal") &&
      row(
        "terminal",
        "Terminal",
        "Opens in the project folder.",
        h("button", { class: "btn", disabled: !terminal, onclick: run(() => api.openTerminal(terminal?.id ?? null)) }, "Open"),
        info && chooser("terminal", info.terminals, info.terminal),
      ),
    row("folder", "Project folder", h("span", { class: "mono" }, p.root), h("button", { class: "btn", onclick: run(() => api.openPath(p.root)) }, showInFolderLabel())),
    hasTool("ide") &&
      row(
        "code",
        "Code",
        ide ? `Opens the project in your IDE. ${info!.ides.map((i) => i.name).join(", ")} found.` : info ? `No IDE found. Install ${ideNames()}.` : "Looking for IDEs...",
        h("button", { class: "btn", disabled: !ide, onclick: run(() => api.openIde(ide?.id ?? null)) }, "Open"),
        info && chooser("ide", info.ides, info.ide),
      ),
  ];

  const content = [
    hasTool("bake_lighting") &&
      row("sun", "Bake lighting", info ? `Without packaging. ${info.changedMaps ? `${plural(info.changedMaps, "map")} changed since your last bake.` : "No map changed since your last bake."}` : "Without packaging.", h("button", { class: "btn", disabled: busy, onclick: chooseBakeMaps }, "Choose maps")),
    hasTool("soundbanks") &&
      p.wwiseVersion &&
      row(
        "sound",
        "Wwise soundbanks",
        info && !info.wwiseFound ? (info.wwiseUnsupported ?? `Wwise ${p.wwiseVersion} isn't installed.`) : `Regenerate for Windows with Wwise ${p.wwiseVersion}.`,
        h("button", { class: "btn", disabled: busy || (info ? !info.wwiseFound : true), onclick: () => startJob(() => api.startTool("soundbanks")) }, "Generate"),
      ),
    hasTool("dependencies") &&
      p.hasDependencies &&
      row(
        "download",
        "Dependencies",
        `Downloads defined in ue-dev-hub.toml. ${outdated ? `${plural(outdated, "download")} missing or outdated.` : "All up to date."}`,
        h("button", { class: outdated ? "btn primary" : "btn", disabled: busy, onclick: () => startJob(() => api.startTool("dependencies", outdated === 0)) }, outdated ? "Download" : "Download again"),
      ),
  ].filter(Boolean);

  const fix = [
    hasTool("clean") &&
      row(
        "trash",
        "Clean build files",
        "Deletes Binaries, Intermediate and the .sln. Compiling the next time takes a while, because everything is built from scratch.",
        h(
          "button",
          {
            class: "btn",
            disabled: busy,
            onclick: run(async () => {
              if (!(await confirmDialog("Clean build files?", "This deletes Binaries, Intermediate and the .sln in the project folder. Compiling the next time builds everything again.", "Clean", true))) return;
              const removed = await api.cleanBuildFiles();
              toast(removed.length ? `Deleted ${removed.join(", ")}` : "Nothing to clean");
              loadTools();
            }),
          },
          "Clean",
        ),
      ),
    hasTool("regenerate") && row("refresh", "Regenerate project files", "Use it when your IDE doesn't show new C++ files.", h("button", { class: "btn", disabled: busy, onclick: () => startJob(() => api.startTool("regenerate")) }, "Regenerate")),
    hasTool("repair_git") && row("git", "Repair Git setup", "Reinstalls hooks and LFS, fetches submodules again.", h("button", { class: "btn", disabled: busy, onclick: () => startJob(() => api.startTool("repairGit")) }, "Repair")),
  ].filter(Boolean);

  return h(
    "main",
    { class: "main narrow" },
    h("header", { class: "page-head" }, h("div", { class: "col gap-xs" }, h("h1", {}, "Tools"), h("p", { class: "muted" }, "Everyday helpers for the project, one click each."))),
    busy &&
      h(
        "div",
        { class: "banner warn", role: "status" },
        icon("alert", 18),
        h("span", {}, h("strong", {}, `${busyLabel()} is running.`), " Some tools wait until it's done."),
      ),
    tl.error && errorBanner(tl.error, loadTools),
    h("section", { class: "card", "aria-labelledby": "open" }, h("h2", { id: "open" }, "Open"), ...open),
    content.length > 0 && h("section", { class: "card", "aria-labelledby": "build" }, h("h2", { id: "build" }, "Content and builds"), ...content),
    fix.length > 0 && h("section", { class: "card", "aria-labelledby": "fix" }, h("h2", { id: "fix" }, "When something is broken"), ...fix),
  );
}
