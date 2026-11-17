// Setup: what the computer needs, each with a way to fix it.
import { open } from "@tauri-apps/plugin-dialog";
import { api, type Check, type Fix } from "../api";
import { blockingCount, current, rerender, store } from "../store";
import { errorText, h, icon, plural, toast } from "../ui";

export async function runChecks(): Promise<Check[]> {
  const gen = store.gen;
  store.checking = true;
  rerender();
  let checks: Check[] = [];
  try {
    checks = await api.runChecks();
  } catch (e) {
    if (current(gen)) toast(`Couldn't check your setup: ${errorText(e)}`, "error");
  }
  // Checks for a project that's no longer open: the new project runs its own.
  if (!current(gen)) return checks;
  store.checks = checks;
  store.checking = false;
  rerender();
  return store.checks;
}

let reloadProject: () => Promise<void> = async () => {};
export function setReloadProject(fn: () => Promise<void>) {
  reloadProject = fn;
}

async function applyFix(fix: Fix) {
  try {
    if (fix.kind === "openUrl") {
      await api.openUrl(fix.url);
      return;
    }
    if (fix.kind === "browseEngine") {
      const dir = await open({ directory: true, title: "Pick the Unreal Engine folder (for example UE_5.8)" });
      if (typeof dir !== "string") return;
      await api.setEnginePath(dir);
    } else if (fix.kind === "createConfig") {
      await api.createConfig();
      toast("Created ue-dev-hub.toml. Commit it so the team gets it.");
      await reloadProject();
    } else if (fix.kind === "openConfig") {
      await api.openConfig();
      return;
    } else if (fix.kind === "useGitHooks") {
      await api.setGitHooks(fix.yes);
    }
    await runChecks();
  } catch (e) {
    toast(errorText(e), "error");
  }
}

function checkRow(c: Check) {
  const badge =
    c.status === "ok"
      ? h("span", { class: "step-icon done" }, icon("check", 13, 'stroke-width="3"'))
      : c.status === "optional"
        ? h("span", { class: "step-icon pending" })
        : h("span", { class: `step-icon ${c.status === "error" ? "failed" : "warn"}` }, "!");
  const expanded = c.status === "error" || c.status === "warning" || (c.status === "optional" && c.fixes.length > 0 && (c.id === "config" || c.id === "gitHooks"));
  return h(
    "li",
    { class: `check ${c.status} ${expanded ? "expanded" : ""}` },
    badge,
    h(
      "div",
      { class: "col gap-xs grow min0" },
      h("div", { class: "row gap-s wrap" }, h("span", { class: "strong" }, c.title), !expanded && h("span", { class: "muted" }, c.detail)),
      expanded && h("p", { class: "muted pre" }, c.detail),
      expanded &&
        c.fixes.length > 0 &&
        h(
          "div",
          { class: "actions" },
          ...c.fixes.map((f, i) => h("button", { class: i === 0 ? "btn primary" : "btn", onclick: () => applyFix(f) }, f.label)),
          c.fixes.some((f) => f.kind === "openUrl") && c.status === "error" && h("button", { class: "btn", onclick: runChecks }, "I installed it"),
        ),
    ),
  );
}

export function setupScreen() {
  const blocking = blockingCount();
  const order = { error: 0, warning: 1, optional: 3, ok: 2 } as const;
  const checks = [...store.checks].sort((a, b) => order[a.status] - order[b.status]);
  return h(
    "main",
    { class: "main narrow" },
    h(
      "header",
      { class: "page-head" },
      h("div", { class: "col gap-xs" }, h("h1", {}, "Setup"), h("p", { class: "muted" }, `What your computer needs to work on ${store.project?.displayName}.`)),
      h("button", { class: "btn", onclick: runChecks, disabled: store.checking }, icon("refresh", 16), store.checking ? "Checking..." : "Check again"),
    ),
    blocking > 0 &&
      h("div", { class: "banner error" }, h("div", {}, h("strong", {}, `${plural(blocking, "thing")} to fix before you can launch.`), " Install it, then press Check again.")),
    h("section", { class: "card", "aria-label": "Checks" }, h("ul", { class: "checks" }, ...checks.map(checkRow))),
  );
}
