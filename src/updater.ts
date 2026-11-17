// The hub updates itself from GitHub releases when it starts.
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";
import { api } from "./api";
import { jobRunning, store } from "./store";
import { errorText, h, modal, toast } from "./ui";

export async function checkForHubUpdate(manual = false) {
  // Dev builds aren't signed releases; updating them would replace them with the installed app.
  if (import.meta.env.DEV && !manual) return;
  if (jobRunning()) {
    if (manual) toast("Wait for the running task to finish first.", "error");
    return;
  }
  if (!store.installed) {
    await updatePortableCopy(manual);
    return;
  }
  // A package's files belong to the package manager; only say there's a new version. The
  // updater can't tell, since latest.json has no entry for packages.
  if (store.packageManaged) {
    await tellAboutPackageUpdate(manual);
    return;
  }
  let update;
  try {
    update = await check();
  } catch (e) {
    if (manual) toast(`Couldn't check for updates: ${errorText(e)}`, "error");
    return;
  }
  if (!update) {
    if (manual) toast("You have the latest hub.");
    return;
  }
  // Holds off everything else, so the restart can't cut off an update or a compile.
  try {
    await api.setFlow("Updating the hub");
  } catch {
    if (manual) toast("Wait for the running task to finish first.", "error");
    return;
  }
  const bar = h("div", { class: "update-bar", role: "status" }, `Updating the hub to ${update.version}...`);
  document.body.append(bar);
  try {
    let total = 0;
    let done = 0;
    await update.downloadAndInstall((e) => {
      if (e.event === "Started") total = e.data.contentLength ?? 0;
      if (e.event === "Progress") {
        done += e.data.chunkLength;
        if (total) bar.textContent = `Updating the hub to ${update.version}... ${Math.round((done / total) * 100)}%`;
      }
    });
    bar.textContent = "Restarting...";
    await relaunch();
  } catch (e) {
    bar.remove();
    await api.setFlow(null).catch(() => {});
    toast(`The hub couldn't update itself: ${errorText(e)}`, "error");
  }
}

async function tellAboutPackageUpdate(manual: boolean) {
  let version;
  try {
    version = await api.newerHubRelease();
  } catch (e) {
    if (manual) toast(`Couldn't check for updates: ${errorText(e)}`, "error");
    return;
  }
  if (version) toast(`UE Dev Hub ${version} is out. Update it with your package manager.`);
  else if (manual) toast("You have the latest hub.");
}

/// A copy that isn't installed downloads the new .exe, checks its signature and swaps itself.
async function updatePortableCopy(manual: boolean) {
  const bar = h("div", { class: "update-bar", role: "status" }, "Checking for a newer hub...");
  if (manual) document.body.append(bar);
  try {
    // On success the hub restarts and this never returns.
    const v = await api.updatePortable();
    if (v === null && manual) toast("You have the latest hub.");
  } catch (e) {
    if (manual) toast(`Couldn't update the hub: ${errorText(e)}`, "error");
  }
  bar.remove();
}

/// Installing: the hub closes, installs, and reopens installed on the same project.
export async function installHub() {
  const bar = h("div", { class: "update-bar", role: "status" }, "Downloading the installer...");
  document.body.append(bar);
  try {
    await api.installHub();
    bar.textContent = "Installing. The hub reopens in a moment...";
  } catch (e) {
    bar.remove();
    toast(errorText(e), "error");
  }
}

/// The first-run question for a copy that isn't installed.
export async function offerInstall() {
  const choice = await modal<"install" | "later" | "never">(
    (close) =>
      h(
        "div",
        { class: "card" },
        h("h2", {}, "Install UE Dev Hub?"),
        h(
          "p",
          { class: "muted" },
          "It's running from this project's folder right now. Installing adds it to the Start menu and lets all your projects share it. It updates itself either way, and you can install later in Settings.",
        ),
        h(
          "div",
          { class: "actions between" },
          h("button", { class: "btn", onclick: () => close("never") }, "Don't ask again"),
          h("div", { class: "row gap-s" }, h("button", { class: "btn", onclick: () => close("later") }, "Not now"), h("button", { class: "btn primary", onclick: () => close("install") }, "Install")),
        ),
      ),
    "later",
  );
  if (choice === "install") await installHub();
  else if (choice === "never") await api.neverOfferInstall().catch(() => {});
}
