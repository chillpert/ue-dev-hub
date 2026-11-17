// Moving between screens, and the sidebar with the project swapper.
import { api, type Link } from "./api";
import { problemCount, rerender, store, type Screen, jobRunning, screenForJob } from "./store";
import { errorText, h, hue, icon, toast } from "./ui";

const enterHooks: Partial<Record<Screen, () => void>> = {};

export function onEnter(screen: Screen, fn: () => void) {
  enterHooks[screen] = fn;
}

let leaveLaunch: () => void = () => {};
export function onLeaveLaunch(fn: () => void) {
  leaveLaunch = fn;
}

let enterAddon: (screen: Screen) => void = () => {};
export function onEnterAddon(fn: (screen: Screen) => void) {
  enterAddon = fn;
}

let addonItems: () => { screen: Screen; icon: string; label: string }[] = () => [];
export function setAddonNavItems(fn: typeof addonItems) {
  addonItems = fn;
}

export function go(screen: Screen) {
  if (screen !== "launch") leaveLaunch();
  store.screen = screen;
  store.swapperOpen = false;
  if (screen.startsWith("addon:")) enterAddon(screen);
  else enterHooks[screen]?.();
  rerender();
  document.querySelector(".main")?.scrollTo({ top: 0 });
}

export function linkIcon(l: Link): string {
  if (l.icon && ["docs", "tasks", "drive", "lock", "chat", "link"].includes(l.icon)) return l.icon;
  const t = `${l.label} ${l.url}`.toLowerCase();
  if (t.includes("doc") || t.includes("wiki")) return "docs";
  if (t.includes("lock")) return "lock";
  if (t.includes("drive") || t.includes("dropbox") || t.includes("sharepoint") || t.includes("onedrive")) return "drive";
  if (t.includes("task") || t.includes("issue") || t.includes("board") || t.includes("jira") || t.includes("linear")) return "tasks";
  if (t.includes("discord") || t.includes("slack") || t.includes("chat")) return "chat";
  return "link";
}

let switchProject: (root: string) => Promise<void> = async () => {};
let addProject: () => Promise<void> = async () => {};

export function setProjectHandlers(onSwitch: (root: string) => Promise<void>, onAdd: () => Promise<void>) {
  switchProject = onSwitch;
  addProject = onAdd;
}

function tileColor(color: string | null, name: string) {
  return color ?? `hsl(${hue(name)} 62% 50%)`;
}

function swapper() {
  const p = store.project!;
  const button = h(
    "button",
    {
      class: "swapper",
      "aria-label": "Switch project",
      "aria-expanded": store.swapperOpen ? "true" : "false",
      onclick: async () => {
        store.swapperOpen = !store.swapperOpen;
        rerender();
        if (store.swapperOpen) {
          store.projects = await api.listProjects().catch(() => store.projects);
          rerender();
        }
      },
    },
    h("span", { class: "project-tile", style: `background: ${tileColor(p.color, p.displayName)}` }, p.displayName[0]?.toUpperCase() ?? "?"),
    h(
      "span",
      { class: "col grow min0" },
      h("span", { class: "strong ellipsis" }, p.displayName),
      h("span", { class: "row gap-xs muted small ellipsis" }, icon("branch", 13), `${p.branch ?? "?"} · UE ${p.engineVersion}`),
    ),
    icon("updown", 16),
  );

  const menu =
    store.swapperOpen &&
    h(
      "div",
      { class: "swapper-menu", role: "menu" },
      ...store.projects.map((e) =>
        h(
          "div",
          { class: `swapper-item ${e.selected ? "on" : ""}`, role: "menuitem" },
          h(
            "button",
            {
              class: "swapper-pick",
              disabled: e.missing,
              title: e.root,
              onclick: async () => {
                store.swapperOpen = false;
                if (!e.selected) await switchProject(e.root);
                rerender();
              },
            },
            h("span", { class: "project-tile small", style: `background: ${e.missing ? "#3a4252" : tileColor(e.color, e.displayName)}` }, e.displayName[0]?.toUpperCase() ?? "?"),
            h(
              "span",
              { class: "col grow min0" },
              h("span", { class: "ellipsis" }, e.displayName),
              h("span", { class: "muted small ellipsis" }, e.missing ? "Folder not found" : `${e.branch ?? "?"} · UE ${e.engineVersion}`),
            ),
            e.selected && icon("check", 16),
          ),
          !e.selected &&
            h(
              "button",
              {
                class: "icon-btn",
                title: "Remove from this list (doesn't delete anything)",
                "aria-label": `Remove ${e.displayName} from the list`,
                onclick: async () => {
                  try {
                    await api.removeProject(e.root);
                    store.projects = await api.listProjects();
                  } catch (err) {
                    toast(errorText(err), "error");
                  }
                  rerender();
                },
              },
              icon("x", 14),
            ),
        ),
      ),
      h(
        "button",
        {
          class: "swapper-add",
          onclick: async () => {
            store.swapperOpen = false;
            rerender();
            await addProject();
          },
        },
        icon("plus", 16),
        "Add a project...",
      ),
    );

  return h("div", { class: "swapper-wrap" }, button, menu);
}

function navItem(screen: Screen, ic: string, label: string, badge?: Node | false) {
  const on = store.screen === screen;
  return h(
    "a",
    {
      href: "#",
      class: `nav ${on ? "on" : ""}`,
      "aria-current": on ? "page" : undefined,
      onclick: (e: Event) => {
        e.preventDefault();
        go(screen);
      },
    },
    icon(ic, 18),
    h("span", { class: "grow" }, label),
    badge,
  );
}

export function sidebar() {
  const p = store.project!;
  const running = jobRunning() ? screenForJob(store.job!.snapshot.kind) : null;
  const busy = (s: Screen) => running === s && h("span", { class: "spinner small", title: "Running" });
  const problems = problemCount();
  const setupOn = store.screen === "setup";
  return h(
    "aside",
    { class: "sidebar" },
    swapper(),
    h(
      "nav",
      { class: "col gap-2", "aria-label": "Main" },
      navItem("launch", "launch", "Launch"),
      navItem("package", "package", "Package", busy("package")),
      navItem("builds", "builds", "Builds", busy("builds")),
      navItem("test", "map", "Test a map", busy("test")),
      navItem("report", "bug", "Report a problem"),
      navItem("tools", "tools", "Tools", busy("tools")),
      ...addonItems().map((a) => navItem(a.screen, a.icon, a.label)),
    ),
    p.links.length > 0 &&
      h(
        "div",
        { class: "col gap-2" },
        h("div", { class: "section-label" }, "Project links"),
        ...p.links.map((l) =>
          h(
            "a",
            {
              href: l.url,
              class: "nav link",
              title: l.url,
              onclick: (e: Event) => {
                e.preventDefault();
                api.openUrl(l.url).catch((err) => toast(errorText(err), "error"));
              },
            },
            icon(linkIcon(l), 17),
            h("span", { class: "grow ellipsis" }, l.label),
            icon("external", 13),
          ),
        ),
      ),
    h("div", { class: "grow" }),
    h(
      "div",
      { class: "col gap-2" },
      h(
        "a",
        {
          href: "#",
          class: `nav ${setupOn ? "on" : ""}`,
          onclick: (e: Event) => {
            e.preventDefault();
            go("setup");
          },
        },
        h("span", { class: "dot-wrap" }, h("span", { class: `dot ${store.checking ? "" : problems > 0 ? "error" : "ok"}` })),
        h("span", { class: "grow" }, store.checking ? "Checking setup..." : problems > 0 ? `${problems} setup problem${problems > 1 ? "s" : ""}` : "Setup is complete"),
      ),
      navItem("settings", "settings", "Settings"),
    ),
  );
}
