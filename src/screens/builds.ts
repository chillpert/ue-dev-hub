// Builds: every packaged build on this computer, ready to play.
import { api, type BuildsInfo } from "../api";
import { go } from "../nav";
import { busy, current, rerender, store } from "../store";
import { jobView, startJob } from "../job";
import { nextcloudConnected } from "../nextcloud";
import { errorText, formatDate, h, icon, showInFolderLabel, toast } from "../ui";

const b = { info: null as BuildsInfo | null, error: null as string | null };

export async function loadBuilds() {
  const gen = store.gen;
  try {
    const info = await api.listBuilds();
    if (!current(gen)) return;
    b.info = info;
    b.error = null;
  } catch (e) {
    if (!current(gen)) return;
    b.error = errorText(e);
  }
  rerender();
}

export function resetBuilds() {
  b.info = null;
}

async function play(path: string) {
  try {
    await api.runBuild(path);
    toast("Starting the game...");
  } catch (e) {
    toast(errorText(e), "error");
  }
}

export function buildsScreen() {
  if (store.job?.snapshot.kind === "upload") return jobView([], loadBuilds);
  const info = b.info;
  const canUpload = !!store.project?.uploadConfigured;
  return h(
    "main",
    { class: "main narrow" },
    h(
      "header",
      { class: "page-head" },
      h("div", { class: "col gap-xs" }, h("h1", {}, "Builds"), h("p", { class: "muted" }, "Packaged builds on this computer, newest first.")),
      h(
        "div",
        { class: "row gap-s" },
        info && h("button", { class: "btn", onclick: () => api.openPath(info.buildsDir).catch((e) => toast(errorText(e), "error")) }, icon("folder", 16), "Open the Builds folder"),
        h("button", { class: "btn primary", onclick: () => go("package") }, icon("package", 16), "Package a new build"),
      ),
    ),
    b.error && h("div", { class: "banner error" }, b.error),
    !info
      ? h("p", { class: "muted" }, "Loading...")
      : info.builds.length === 0
        ? h("section", { class: "card" }, h("p", { class: "muted" }, "No builds yet. Package the game and it shows up here."))
        : h(
            "section",
            { class: "card", "aria-label": "Builds" },
            h(
              "ul",
              { class: "list builds" },
              ...info.builds.map((x, i) =>
                h(
                  "li",
                  { class: "list-row" },
                  h(
                    "button",
                    {
                      class: i === 0 ? "btn primary play-btn" : "btn play-btn",
                      disabled: !x.exe,
                      title: x.exe ? `Play ${x.name}` : "This build has no game to start",
                      "aria-label": `Play ${x.name}`,
                      onclick: () => play(x.path),
                    },
                    icon("launch", 15, 'fill="currentColor" stroke="none"'),
                    "Play",
                  ),
                  h(
                    "span",
                    { class: "col grow min0" },
                    h("span", { class: "ellipsis strong" }, x.name),
                    h(
                      "span",
                      { class: "row gap-s muted small" },
                      formatDate(x.modified),
                      h("span", { class: `pill ${x.forPlayers ? "on" : "changed"}` }, x.forPlayers ? "Public build" : "Internal testing"),
                      !x.exe && h("span", {}, "No game inside"),
                    ),
                  ),
                  canUpload &&
                    h(
                      "button",
                      {
                        class: "btn small",
                        disabled: !x.exe || busy(),
                        title: nextcloudConnected() ? "Upload this build to Nextcloud" : "Connect to Nextcloud in Settings first",
                        onclick: () => (nextcloudConnected() ? startJob(() => api.startUpload(x.path)) : go("settings")),
                      },
                      icon("upload", 14),
                      "Upload",
                    ),
                  x.zip && h("button", { class: "btn small", title: x.zip, onclick: () => api.revealPath(x.zip!).catch((e) => toast(errorText(e), "error")) }, icon("file", 14), "Zip"),
                  h("button", { class: "icon-btn", title: showInFolderLabel(), "aria-label": `${showInFolderLabel()}: ${x.name}`, onclick: () => api.revealPath(x.exe ?? x.path).catch((e) => toast(errorText(e), "error")) }, icon("folder", 16)),
                ),
              ),
            ),
          ),
  );
}
