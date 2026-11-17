// Settings > Addons: turning addons on and off for this project, installing and removing them.
import { open } from "@tauri-apps/plugin-dialog";
import { api, NewerAddonInstalled, type AddonInfo, type AddonManifest, type AvailableAddon } from "../api";
import { refreshAddons } from "../addons";
import { rerender, store } from "../store";
import { confirmDialog, errorText, h, icon, toast } from "../ui";

let available: AvailableAddon[] | null = null;
let availableError: string | null = null;
/// The addon being installed, updated or removed.
let working: string | null = null;

export async function loadAddonsCard(fresh = false) {
  await refreshAddons();
  try {
    available = await api.addonsAvailable(fresh);
    availableError = null;
  } catch (e) {
    available = null;
    availableError = errorText(e);
  }
  rerender();
}

async function busyWith(id: string, f: () => Promise<unknown>) {
  if (working) return;
  working = id;
  rerender();
  try {
    await f();
  } catch (e) {
    toast(errorText(e), "error");
  }
  working = null;
  await loadAddonsCard();
}

async function setEnabled(a: AddonInfo, enabled: boolean) {
  try {
    await api.addonSetEnabled(a.id, enabled);
  } catch (e) {
    toast(errorText(e), "error");
  }
  await refreshAddons();
}

/// Runs an install. When a newer version is installed, asks whether to install the older one
/// anyway and tries again. Returns null if the person said no.
async function allowingOlder(install: (downgrade: boolean) => Promise<AddonManifest>): Promise<AddonManifest | null> {
  try {
    return await install(false);
  } catch (e) {
    if (!(e instanceof NewerAddonInstalled)) throw e;
    if (!(await confirmDialog("Install an older version?", `This is older than the installed version ${e.installed}. Install it anyway?`, "Install"))) return null;
    return await install(true);
  }
}

async function installFromDisk(folder: boolean) {
  const picked = await open(folder ? { directory: true, title: "Pick the addon's folder (the one with addon.toml)" } : { title: "Pick the addon's .zip file", filters: [{ name: "Addon", extensions: ["zip"] }] });
  if (typeof picked !== "string") return;
  const ok = await confirmDialog(
    "Install an addon from disk?",
    "Addons from disk aren't checked by anyone. An addon's page and program get full access to the hub and your computer, so only install one from someone you trust.",
    "Install",
  );
  if (!ok) return;
  await busyWith("disk", async () => {
    const m = await allowingOlder((downgrade) => api.addonInstallFromDisk(picked, downgrade));
    if (m) toast(`Installed ${m.name} ${m.version}.`);
  });
}

async function remove(a: AddonInfo) {
  const again = a.projectDefault ? ` This project uses it, so it ${a.source === "official" ? "installs itself again the next time the project opens" : "will be missing"}. To keep it off, turn it off instead.` : "";
  if (!(await confirmDialog(`Remove ${a.name}?`, `Its files are deleted from this computer.${again}`, "Remove", true))) return;
  await busyWith(a.id, () => api.addonUninstall(a.id));
}

function addonRow(a: AddonInfo) {
  const update = available?.find((x) => x.id === a.id && x.update);
  const busyHere = working === a.id;
  const follows = a.projectDefault && !a.chosen ? "On for everyone in this project" : a.projectDefault ? "This project turns it on for everyone" : null;
  return h(
    "div",
    { class: "tool-row" },
    h("span", { class: "accent-icon" }, icon(a.pages[0]?.icon ?? "box", 20)),
    h(
      "div",
      { class: "col grow min0" },
      h("span", { class: "strong" }, a.name, " ", h("span", { class: "muted small" }, a.version), " ", h("span", { class: "pill" }, a.source === "official" ? "Official" : "From disk")),
      a.description && h("span", { class: "muted small" }, a.description),
      follows && h("span", { class: "muted small" }, follows),
      a.problem && h("span", { class: "small warn-text" }, a.problem),
    ),
    update && h("button", { class: "btn small", disabled: !!working, onclick: () => busyWith(a.id, () => api.addonInstall(a.id).then((m) => toast(`Updated ${m.name} to ${m.version}.`))) }, busyHere ? "Updating..." : `Update to ${update.version}`),
    h("button", { class: "btn small", disabled: !!working, onclick: () => remove(a) }, busyHere && !update ? "Removing..." : "Remove"),
    h("input", { type: "checkbox", class: "switch", checked: a.enabled, disabled: !!a.problem, "aria-label": `Use ${a.name} in this project`, onchange: (e: Event) => setEnabled(a, (e.target as HTMLInputElement).checked) }),
  );
}

function availableRow(a: AvailableAddon) {
  return h(
    "div",
    { class: "tool-row" },
    h("span", { class: "accent-icon" }, icon("download", 20)),
    h("div", { class: "col grow min0" }, h("span", { class: "strong" }, a.name, " ", h("span", { class: "muted small" }, a.version)), a.description && h("span", { class: "muted small" }, a.description), a.problem && h("span", { class: "small warn-text" }, a.problem)),
    h(
      "button",
      { class: "btn small", disabled: !!working || !!a.problem, onclick: () => busyWith(a.id, () => allowingOlder((downgrade) => api.addonInstall(a.id, downgrade)).then((m) => m && toast(`Installed ${m.name} ${m.version}.`))) },
      working === a.id ? "Installing..." : "Install",
    ),
  );
}

export function addonsCard() {
  const notInstalled = (available ?? []).filter((a) => !a.installed);
  return h(
    "section",
    { class: "card" },
    h("h2", {}, "Addons"),
    h("p", { class: "muted small" }, "Extra pages for the hub. Turning one on or off only changes it for you, in this project."),
    store.addonNotice && h("p", { class: "banner warn small" }, store.addonNotice),
    store.addons.length === 0 && h("p", { class: "muted" }, "No addons installed."),
    ...store.addons.map(addonRow),
    notInstalled.length > 0 && h("div", { class: "section-label" }, "Official addons"),
    ...notInstalled.map(availableRow),
    availableError && h("p", { class: "muted small" }, availableError),
    h(
      "div",
      { class: "actions" },
      h("button", { class: "btn", disabled: !!working, onclick: () => installFromDisk(false) }, icon("box", 16), working === "disk" ? "Installing..." : "Install from a zip..."),
      h("button", { class: "btn", disabled: !!working, onclick: () => installFromDisk(true) }, "From a folder..."),
      h("button", { class: "btn", disabled: !!working, onclick: () => loadAddonsCard(true) }, icon("refresh", 16), "Check for updates"),
    ),
  );
}
