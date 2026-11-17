// Settings: personal choices and the project list.
import { api, type AfterLaunch } from "../api";
import { rerender, store } from "../store";
import { errorText, h, icon, toast } from "../ui";
import { customizeHome, setAutoLaunch, widgets, WIDGETS } from "./launch";
import { checkForHubUpdate, installHub } from "../updater";
import { loadNextcloud, nextcloudCard } from "../nextcloud";
import { addonsCard, loadAddonsCard } from "./addons-card";

async function setAfterLaunch(action: AfterLaunch) {
  store.afterLaunch = action;
  try {
    await api.setAfterLaunch(action);
  } catch (e) {
    toast(errorText(e), "error");
  }
  rerender();
}

export async function loadSettings() {
  store.projects = await api.listProjects().catch(() => store.projects);
  rerender();
  loadNextcloud(true);
  loadAddonsCard();
}

export function settingsScreen() {
  const p = store.project!;
  return h(
    "main",
    { class: "main narrow" },
    h("header", { class: "page-head" }, h("div", { class: "col gap-xs" }, h("h1", {}, "Settings"), h("p", { class: "muted" }, "Your own choices. Team settings live in the project's ue-dev-hub.toml."))),
    h(
      "section",
      { class: "card" },
      h("h2", {}, "Starting"),
      h(
        "label",
        { class: "tool-row" },
        h("span", { class: "accent-icon" }, icon("launch", 20)),
        h(
          "div",
          { class: "col grow" },
          h("span", { class: "strong" }, "Update & Launch when the hub opens"),
          h("span", { class: "muted small" }, "Starts by itself after a 5-second countdown you can cancel. Off: open the hub, then press Update & Launch."),
        ),
        h("input", { type: "checkbox", class: "switch", checked: store.autoLaunch, onchange: (e: Event) => setAutoLaunch((e.target as HTMLInputElement).checked) }),
      ),
      h(
        "label",
        { class: "tool-row" },
        h("span", { class: "accent-icon" }, icon("launch", 20)),
        h(
          "div",
          { class: "col grow" },
          h("span", { class: "strong" }, "After the editor starts"),
          h("span", { class: "muted small" }, "Only after a launch from here, and never while something else is still running."),
        ),
        h(
          "select",
          { class: "select", onchange: (e: Event) => setAfterLaunch((e.target as HTMLSelectElement).value as AfterLaunch) },
          ...(
            [
              ["keep", "Keep the hub open"],
              ["minimize", "Minimize the hub"],
              ["close", "Close the hub"],
            ] as const
          ).map(([value, text]) => h("option", { value, selected: store.afterLaunch === value }, text)),
        ),
      ),
    ),
    addonsCard(),
    nextcloudCard(),
    h(
      "section",
      { class: "card" },
      h("h2", {}, "Home screen"),
      h(
        "div",
        { class: "tool-row" },
        h("span", { class: "accent-icon" }, icon("layout", 20)),
        h("div", { class: "col grow" }, h("span", { class: "strong" }, "Widgets"), h("span", { class: "muted small" }, widgets().map((w) => WIDGETS[w]?.title ?? w).join(", ") || "Only Launch")),
        h("button", { class: "btn", onclick: customizeHome }, "Customize"),
      ),
    ),
    h(
      "section",
      { class: "card" },
      h("h2", {}, `Project settings for ${p.displayName}`),
      h(
        "div",
        { class: "tool-row" },
        h("span", { class: "accent-icon" }, icon("file", 20)),
        h("div", { class: "col grow" }, h("span", { class: "strong" }, "ue-dev-hub.toml"), h("span", { class: "muted small" }, p.hasConfig ? "Links, packaging defaults and tools for everyone. Changes go through a commit." : "Not created yet. Setup can create one.")),
        p.hasConfig && h("button", { class: "btn", onclick: () => api.openConfig().catch((e) => toast(errorText(e), "error")) }, "Open"),
      ),
    ),
    h(
      "section",
      { class: "card" },
      h("h2", {}, "Projects"),
      h(
        "ul",
        { class: "list" },
        ...store.projects.map((e) =>
          h(
            "li",
            { class: "list-row" },
            h("span", { class: "col grow min0" }, h("span", { class: "strong" }, e.displayName), h("span", { class: "muted small mono ellipsis" }, e.root)),
            e.selected
              ? h("span", { class: "pill on" }, "Open")
              : h(
                  "button",
                  {
                    class: "btn small",
                    onclick: async () => {
                      await api.removeProject(e.root).catch((err) => toast(errorText(err), "error"));
                      loadSettings();
                    },
                  },
                  "Remove from list",
                ),
          ),
        ),
      ),
    ),
    h(
      "section",
      { class: "card" },
      h("h2", {}, "About"),
      h(
        "div",
        { class: "tool-row" },
        h("span", { class: "accent-icon" }, icon("download", 20)),
        h(
          "div",
          { class: "col grow" },
          h("span", { class: "strong" }, `UE Dev Hub ${store.version}`),
          h("span", { class: "muted small" }, store.packageManaged ? "Installed as a package. Update it with your package manager." : store.installed ? "Installed. Updates itself when it starts." : "Running from this project's folder, not installed. Updates itself when it starts."),
        ),
        h("button", { class: "btn", onclick: () => checkForHubUpdate(true) }, "Check for updates"),
      ),
      !store.installed &&
        h(
          "div",
          { class: "tool-row" },
          h("span", { class: "accent-icon" }, icon("download", 20)),
          h(
            "div",
            { class: "col grow" },
            h("span", { class: "strong" }, "Install UE Dev Hub"),
            h("span", { class: "muted small" }, "Adds it to the Start menu, and all your projects share one hub. It closes and reopens installed."),
          ),
          h("button", { class: "btn primary", onclick: installHub }, "Install"),
        ),
    ),
  );
}
