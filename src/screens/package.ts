// Package the game: a form with a live plan, then the running view.
import { api, type PackageInfo } from "../api";
import { jobView, startJob } from "../job";
import { busy, busyLabel, current, rerender, store } from "../store";
import { onEditorChange } from "./launch";
import { errorText, h, icon, isWindows, plural, toast } from "../ui";
import { go } from "../nav";
import { nextcloudConnected } from "../nextcloud";
import { bakeMapPicker, changedMaps } from "./bake-maps";

const form = {
  info: null as PackageInfo | null,
  error: null as string | null,
  forPlayers: false,
  selected: new Set<string>(),
  quality: "Medium",
  soundbanks: true,
  zip: true,
  upload: false,
  initialized: "",
};

onEditorChange((running) => {
  if (form.info && form.info.editorRunning !== running) {
    form.info.editorRunning = running;
    rerender();
  }
});

export async function loadPackage() {
  const gen = store.gen;
  try {
    const info = await api.packageInfo();
    if (!current(gen)) return;
    form.info = info;
    form.error = null;
    const p = store.project!;
    // Pick defaults once per project: changed maps, the team's quality.
    if (form.initialized !== p.root) {
      form.initialized = p.root;
      form.selected = changedMaps(form.info.maps);
      form.quality = p.lightingQuality;
      form.soundbanks = p.soundbanks;
      form.zip = p.zip;
      form.upload = p.uploadAfterPackaging;
    }
  } catch (e) {
    if (!current(gen)) return;
    form.error = errorText(e);
  }
  rerender();
}

export function resetPackage() {
  form.info = null;
  form.initialized = "";
}

function plan() {
  const p = store.project!;
  const n = form.selected.size;
  const steps: { title: string; note: string }[] = [{ title: "Compile the editor", note: "So the cook matches your C++" }];
  if (form.soundbanks && p.wwiseVersion && form.info?.wwiseFound) steps.push({ title: "Generate Wwise soundbanks", note: "For Windows" });
  steps.push(n > 0 ? { title: "Bake lighting", note: `${plural(n, "map")} at ${form.quality} quality` } : { title: "Skip lighting", note: "Uses the lighting already in the maps" });
  steps.push({ title: "Cook and package", note: form.forPlayers ? "Optimized, ready to share" : "Optimized, with debug info" });
  if (form.zip) steps.push({ title: "Zip it", note: zipTool() });
  if (uploading()) steps.push({ title: "Upload to Nextcloud", note: store.nextcloud?.folder ?? "" });
  return steps;
}

/// What zips the build, like the backend picks it.
function zipTool(): string {
  return form.info?.sevenZip ? "7-Zip" : isWindows() ? "Windows tar" : "zip";
}

function uploading(): boolean {
  return !!store.project?.uploadConfigured && form.upload && form.zip && nextcloudConnected();
}

function folderName() {
  const p = store.project!;
  const d = new Date();
  const two = (x: number) => String(x).padStart(2, "0");
  const date = `${two(d.getFullYear() % 100)}.${two(d.getMonth() + 1)}.${two(d.getDate())}-${two(d.getHours())}.${two(d.getMinutes())}`;
  return `${form.forPlayers ? "" : "InternalOnly-"}${p.name}-${date}-${form.info?.buildPlatform ?? "Win64"}`;
}

function kindCard(players: boolean, title: string, text: string) {
  const on = form.forPlayers === players;
  return h(
    "label",
    { class: `choice ${on ? "on" : ""}` },
    h(
      "span",
      { class: "row gap-s" },
      h("input", {
        type: "radio",
        name: "kind",
        checked: on,
        onchange: () => {
          form.forPlayers = players;
          rerender();
        },
      }),
      h("span", { class: "strong" }, title),
    ),
    h("span", { class: "muted small" }, text),
  );
}

function mapsSection() {
  const info = form.info!;
  const select = (pred: (kind: string) => boolean) => {
    form.selected = new Set(info.maps.filter((m) => pred(m.kind)).map((m) => m.package));
    rerender();
  };
  return h(
    "section",
    { class: "card", "aria-labelledby": "light" },
    h(
      "div",
      { class: "row between wrap gap-s" },
      h("div", { class: "col gap-xs" }, h("h2", { id: "light" }, "Bake lighting"), h("span", { class: "muted small" }, "Maps in this build. Changed maps are preselected.")),
      h(
        "div",
        { class: "segmented" },
        h("button", { class: "seg", onclick: () => select(() => false) }, "None"),
        h("button", { class: "seg", onclick: () => select((k) => k === "changed" || k === "never") }, "Changed"),
        h("button", { class: "seg", onclick: () => select((k) => k !== "missing") }, "All"),
      ),
    ),
    ...bakeMapPicker(info.maps, info.qualities, form, rerender),
    h("p", { class: "muted small" }, "Changed is a guess: it compares each map's own .umap files with your last bake on this computer. Baked maps stay modified in your working copy."),
  );
}

function optionsSection() {
  const p = store.project!;
  const info = form.info!;
  return h(
    "section",
    { class: "card", "aria-labelledby": "extra" },
    h("h2", { id: "extra" }, "More options"),
    p.wwiseVersion &&
      h(
        "label",
        { class: "option" },
        h("input", {
          type: "checkbox",
          checked: form.soundbanks,
          onchange: () => {
            form.soundbanks = !form.soundbanks;
            rerender();
          },
        }),
        h("span", { class: "col" }, h("span", {}, "Generate Wwise soundbanks"), h("span", { class: `small ${info.wwiseFound ? "muted" : "warn-text"}` }, info.wwiseFound ? `Wwise ${p.wwiseVersion} found` : (info.wwiseUnsupported ?? `Wwise ${p.wwiseVersion} isn't installed, so the soundbanks already in the project are used`))),
      ),
    h(
      "label",
      { class: "option" },
      h("input", {
        type: "checkbox",
        checked: form.zip,
        onchange: () => {
          form.zip = !form.zip;
          rerender();
        },
      }),
      h("span", { class: "col" }, h("span", {}, "Zip the build when it's done"), h("span", { class: "muted small" }, info.sevenZip ? "Uses 7-Zip" : `Uses ${zipTool()} (install 7-Zip for faster zips)`)),
    ),
    p.uploadConfigured &&
      h(
        "label",
        { class: "option" },
        h("input", {
          type: "checkbox",
          checked: form.upload && form.zip && nextcloudConnected(),
          disabled: !form.zip || !nextcloudConnected(),
          onchange: () => {
            form.upload = !form.upload;
            rerender();
          },
        }),
        h(
          "span",
          { class: "col" },
          h("span", {}, "Upload to Nextcloud when it's done"),
          !nextcloudConnected()
            ? h("a", { href: "#", class: "small", onclick: (e: Event) => (e.preventDefault(), go("settings")) }, "Connect to Nextcloud in Settings first")
            : h("span", { class: "muted small" }, !form.zip ? "Needs the zip" : `To ${store.nextcloud?.folder}. Continues where it stopped if the connection drops.`),
        ),
      ),
  );
}

function planAside() {
  const info = form.info!;
  return h(
    "aside",
    { class: "card", "aria-labelledby": "plan" },
    h("h2", { id: "plan" }, "What will happen"),
    h(
      "ol",
      { class: "plan" },
      ...plan().map((s, i) => h("li", {}, h("span", { class: "step-icon pending" }, String(i + 1)), h("span", { class: "col" }, h("span", { class: "strong" }, s.title), h("span", { class: "muted small" }, s.note)))),
    ),
    h("div", { class: "col gap-xs" }, h("span", { class: "muted small" }, "Saved to"), h("span", { class: "mono small break" }, `${info.buildsDir.split(/[\\/]/).pop()}/${folderName()}`)),
    !form.forPlayers && h("p", { class: "banner warn small" }, "Internal build: don't publish it anywhere."),
    h(
      "button",
      {
        class: "btn primary block",
        disabled: info.editorRunning || busy(),
        onclick: () =>
          startJob(() =>
            api.startPackage({
              forPlayers: form.forPlayers,
              bakeMaps: [...form.selected],
              quality: form.quality,
              soundbanks: form.soundbanks,
              zip: form.zip,
              upload: uploading(),
            }),
          ),
      },
      icon("package", 18),
      "Start packaging",
    ),
    info.editorRunning && h("p", { class: "error-text small" }, "Close Unreal Editor first. Packaging needs its files."),
    busy() && h("p", { class: "muted small" }, `${busyLabel()} is running.`),
  );
}

export function packageScreen() {
  if (store.job?.snapshot.kind === "package") {
    return jobView(
      [
        h(
          "section",
          { class: "card" },
          h("h2", {}, "You can do something else"),
          h("p", { class: "muted" }, "Packaging keeps running when you close this window: the hub stays in the tray. You get a notification when it's done or if something fails."),
          h("a", { href: "#", onclick: (e: Event) => (e.preventDefault(), go("builds")) }, "See your earlier builds"),
        ),
      ],
      loadPackage,
    );
  }
  const p = store.project!;
  const info = form.info;
  return h(
    "main",
    { class: "main" },
    h(
      "header",
      { class: "page-head" },
      h("div", { class: "col gap-xs" }, h("h1", {}, "Package the game"), h("p", { class: "muted" }, `Makes a standalone build of ${p.branch ?? "your branch"} that runs without Unreal.`)),
      h(
        "div",
        { class: "row gap-s wrap" },
        info && h("span", { class: `status-pill ${info.editorRunning ? "error" : "ok"}` }, h("span", { class: `dot ${info.editorRunning ? "error" : "ok"}` }), info.editorRunning ? "Unreal Editor is open" : "Unreal Editor is closed"),
        p.checklistUrl && h("a", { href: p.checklistUrl, class: "row gap-xs", onclick: (e: Event) => (e.preventDefault(), api.openUrl(p.checklistUrl!).catch((err) => toast(errorText(err), "error"))) }, icon("docs", 16), "Read the packaging checklist first"),
      ),
    ),
    form.error && h("div", { class: "banner error" }, form.error),
    !info
      ? h("p", { class: "muted" }, "Loading...")
      : h(
          "div",
          { class: "split" },
          h(
            "div",
            { class: "col gap grow-3" },
            h(
              "section",
              { class: "card", "aria-labelledby": "kind" },
              h("h2", { id: "kind" }, "Who is this build for?"),
              h(
                "div",
                { class: "choices" },
                kindCard(false, "Internal testing", "Internal test build with debug info. Never share it outside the team."),
                kindCard(true, "Public build", "Release build for distribution. No debug info, ready to upload."),
              ),
            ),
            mapsSection(),
            optionsSection(),
          ),
          h("div", { class: "col gap grow-1 sticky" }, planAside()),
        ),
  );
}
