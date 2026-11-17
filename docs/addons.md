# Making addons

An addon adds pages to UE Dev Hub. It has two parts:

- **A page**: an ES module the hub's window loads. It draws its page with the hub's own
  helpers, so it looks like the rest of the hub.
- **A helper** (optional): a program the hub starts and talks to. Use one for anything the
  page can't do in the window, like running Git. Helpers are usually Rust, built on the
  [`ue-dev-hub-addon`](../addon-sdk) crate, but any program that speaks the protocol works.

The hub is compiled, so it can't load new Rust code while it runs. That's why the helper is a
separate program instead of a library.

## Addons are trusted code

An addon runs with the hub's own rights. Its page runs in the hub's window, next to the
hub's code: it can change anything on screen and call every command the hub has, the
ones that start programs and change files included. Its helper is a normal program running as you.
Nothing limits an addon to what `Hub` offers; that's only the part that stays compatible.

So only install addons you trust. Official addons are signed (see below); addons installed from
disk aren't checked by anyone, and the hub says so before installing one.

The window's content security policy blocks network access from the page: `fetch()` and other
requests only reach the addon's own files. Anything that needs the internet goes through the helper.

## Folder layout

An installed addon is a folder in the hub's data folder:

- Windows: `%LOCALAPPDATA%\com.chillpert.uedevhub\addons\<id>\`
- Linux: `~/.local/share/com.chillpert.uedevhub/addons/<id>/`

It's installed from a zip with the same layout:

```
addon.toml
ui/index.js        the page, an ES module
ui/style.css       optional; loaded while the addon is on
ui/...             anything else the page loads (see "Other files" below)
bin/...            optional helper for each platform
```

```toml
id = "git"                 # lowercase letters, digits and dashes
name = "Git"
description = "Commit, push and switch branches."
version = "1.0.0"
api = 1                    # must match the hub's API version exactly
# min_hub = "0.2.0"   # optional

[[pages]]                  # one sidebar entry per page
id = "git"
label = "Git"
icon = "git"               # a hub icon, or one the page registers

[helper]                   # optional; relative to the addon folder
windows = "bin/ue-dev-hub-git.exe"
linux = "bin/ue-dev-hub-git"
```

## The page

`ui/index.js` exports one function. The hub calls it when the addon is turned on, with a
`Hub` made for this addon, and shows the pages it returns. The optional `deactivate` runs when
the addon is turned off, updated or removed. Turned on again, `activate` runs again in the same
module, so top-level variables keep their values; set things up in `activate`, not at the top:

```ts
import type { Activate } from "ue-dev-hub-addon-api";

const activate: Activate = (hub) => {
  const { h } = hub.ui;
  return {
    pages: {
      main: {
        render: () => h("main", { class: "main" }, h("h1", {}, "Hello")),
        enter: () => { /* the page was opened */ },
        reset: () => { /* another project was opened */ },
      },
    },
    deactivate: () => { /* stop timers, remove listeners */ },
  };
};
export default activate;
```

The whole API is in [addon-api/index.d.ts](../addon-api/index.d.ts). In short:

- `hub.call(method, params, { step })` calls the helper. With `step: "Pushing"`, the call is a
  step. A step may change the project, so it never runs alongside a job, Update & Launch or another
  step. Calls without a step are reads and run in parallel.
- `hub.onLog((level, text) => ...)` gets the log lines of this addon's steps, so the page can
  show its own output. They don't show anywhere else. It returns a function that stops listening.
- `hub.ui`: `h`, `icon`, `registerIcon`, `toast`, `modal`, `confirmDialog`, `errorText`, `plural`.
- `hub.avatars`: GitHub avatars by email.
- `hub.app`: `busy`, `busyLabel`, `rerender`, `project`, `setBranch`, `activePage`, `go`,
  `revealPath`, `checkUpdates`. `busy()` is also true while one of this addon's own steps runs.
  `project()` returns a frozen copy.

### Types

The types are a single file, `addon-api/index.d.ts` in the hub repository, imported as
`ue-dev-hub-addon-api`. There's no package to install: point a TypeScript path at a checkout of
the hub, here one next to your addon's folder:

```jsonc
// tsconfig.json
{
  "compilerOptions": {
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "paths": {
      "ue-dev-hub-addon-api": ["../ue-dev-hub/addon-api/index.d.ts"]
    }
  }
}
```

Use `import type` only. The import is gone after compiling, so `ui/index.js` doesn't load anything
by that name.

### Repaints

`render()` runs on every repaint of the window and builds the page from scratch, like the
hub's own screens. Keep state in variables, not in the elements. To keep the parts a person
is using:

- **Focus and cursor** come back only for an `input`, `textarea` or `select` with an `aria-label`,
  `placeholder` or `name`. The hub finds the field again by that text, so make it unique on
  the page.
- **Scroll positions** come back only for elements with `data-keep-scroll="<unique key>"`. The page
  itself keeps its scroll position.
- Long lists are slow to rebuild every frame. Build the element once and return the same one from
  `render()` until its data changes.

### Other files

Find the addon's other files from the module's own address, never with a hardcoded `addon://`
address, because it differs per platform (`http://addon.localhost/...` on Windows):

```ts
const logo = new URL("./logo.svg", import.meta.url).href;
```

Only `index.js` is loaded fresh when the addon is updated: modules it imports keep their old
version until the hub restarts. Bundle the page into a single `index.js`, like the official
Git addon does with Vite. Images, fonts and files read with `fetch()` are always read fresh.

## The helper

The hub starts the helper on the page's first call, with the addon folder as its working
folder. It stops the helper when the addon is turned off, updated or removed, and when the
hub exits. A helper whose stdin closes should exit, which `serve` does for you.

```rust
fn main() {
    ue_dev_hub_addon::serve(|ctx, method, params| match method {
        "hello" => Ok(serde_json::json!(format!("Hello from {}", ctx.project()?.name))),
        _ => Err(format!("Unknown call: {method}")),
    })
}
```

`ctx.project()` is the open project: its folder, .uproject, base branch and Git settings.
`ctx.is_step()` says whether the call runs as a step; refuse calls that change files when it's
false. During steps, `ctx.info` and `ctx.detail` send log lines to the page's `hub.onLog`.

Depend on the crate from the hub's repository, at the tag of the hub version you build
for:

```toml
[dependencies]
ue-dev-hub-addon = { git = "https://github.com/chillpert/ue-dev-hub", tag = "vX.Y.Z" }
```

`ctx.host(name, params)` asks the hub for its own services, so addons don't copy them:

| Name | Does |
|---|---|
| `checkUpdates` | The hub's update check. Returns its `UpdateInfo`. |
| `applyUpdate` `{push}` | The hub's protected Update flow. Steps only. |
| `repoBusy` | An unfinished merge or rebase, or a Git lock file, as a message. Null when idle. |
| `foreignServerCommits` `{branch, server}` | How many commits on the server's branch aren't yours. Commits the hub kept as backups of your rebases count as yours. |
| `updateSubmodules` | Brings submodules to what the branch wants. Returns warnings. Steps only. |
| `editorRunning` | Whether Unreal has this project open. |

### Protocol

If you don't use the crate: one JSON object per line. The hub writes to the helper's stdin, and the
helper writes to its stdout. Never print anything else to stdout. stderr is free text; the hub
shows its last lines if the helper crashes.

```jsonc
// hub → helper
{"type":"call","id":7,"method":"overview","params":{},"project":{"root":"…","uproject":"…","name":"…","baseBranch":"develop","gitUpdate":"rebase","gitPush":"ask","branchPrefixes":["feat","fix"]},"step":false}
{"type":"hostResult","id":7,"value":null}            // or "error": "…"
// helper → hub
{"type":"log","id":7,"level":"info","text":"> git status"}
{"type":"host","id":7,"name":"repoBusy","params":null}
{"type":"result","id":7,"value":{…}}                 // or {"type":"error","id":7,"error":"…"}
```

Calls can overlap; each has its own id. Within a call, wait for the `hostResult` before sending the
next `host`.

## Official addons and signing

Official addons are built and published by the `ue-dev-hub-addons` repository. Its release workflow
signs each zip with the addons key and updates a list, `index.json`, which the hub reads:

```json
{ "addons": [ { "id": "git", "name": "Git", "description": "…", "version": "1.0.0", "api": 1,
  "platforms": { "windows-x86_64": { "url": "https://…/git-1.0.0-windows-x86_64.zip", "sha256": "…", "signature": "…" } } } ] }
```

The hub installs an official addon only if the sha256 matches and the signature is valid and
made for that very file name. The public key is `PUBKEY` in `src-tauri/src/addon_index.rs`. It is a
different key from the hub's update key.

To try a list without publishing it, start the hub with `UE_DEV_HUB_ADDON_INDEX` set to the
path or URL of an `index.json`. Relative download URLs are read next to it.

Addons installed from disk aren't checked. The hub warns before installing one.
