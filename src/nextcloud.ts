// Connecting to the team's Nextcloud, used by Settings, Package and Builds.
import { api } from "./api";
import { current, rerender, store } from "./store";
import { errorText, h, icon, isWindows, toast } from "./ui";

export async function loadNextcloud(probe = false) {
  if (!store.project?.uploadConfigured) {
    store.nextcloud = null;
    return;
  }
  const gen = store.gen;
  const status = await api.nextcloudStatus(probe).catch(() => null);
  if (!current(gen)) return;
  store.nextcloud = status;
  rerender();
}

export function nextcloudConnected(): boolean {
  return !!store.nextcloud?.loginName;
}

async function connect() {
  store.connecting = true;
  rerender();
  try {
    const who = await api.nextcloudConnect();
    toast(`Connected to Nextcloud as ${who}.`);
  } catch (e) {
    if (errorText(e) !== "Cancelled.") toast(errorText(e), "error");
  }
  store.connecting = false;
  await loadNextcloud(true);
}

async function disconnect() {
  try {
    await api.nextcloudDisconnect();
    toast("Disconnected. The hub's login was removed from this computer.");
  } catch (e) {
    toast(errorText(e), "error");
  }
  await loadNextcloud(true);
}

export function nextcloudCard() {
  const nc = store.nextcloud;
  if (!store.project?.uploadConfigured) return null;
  const host = nc?.url?.replace(/^https?:\/\//, "") ?? "";
  const needsCloudflared = nc?.cloudflareAccess === true && !nc.cloudflaredInstalled;
  return h(
    "section",
    { class: "card" },
    h("h2", {}, "Nextcloud"),
    h(
      "div",
      { class: "tool-row" },
      h("span", { class: "accent-icon" }, icon("cloud", 20)),
      h(
        "div",
        { class: "col grow min0" },
        h("span", { class: "strong" }, nc?.loginName ? `Connected as ${nc.loginName}` : "Not connected"),
        h(
          "span",
          { class: "muted small" },
          store.connecting
            ? "Finish signing in in your browser. This page continues by itself."
            : nc?.loginName
              ? `Builds upload to ${nc.folder} on ${host}.`
              : `Sign in with your own Nextcloud account on ${host} to upload builds. Your login is kept in ${isWindows() ? "Windows Credential Manager" : "your system keyring"}.`,
        ),
        needsCloudflared &&
          h(
            "span",
            { class: "warn-text small" },
            `This server is behind Cloudflare Access. Install cloudflared first: ${isWindows() ? "run winget install Cloudflare.cloudflared" : "get it from your package manager or Cloudflare's website"}, then connect.`,
          ),
      ),
      nc?.loginName && nc.webFolderUrl && h("button", { class: "btn", onclick: () => api.openUrl(nc.webFolderUrl!).catch((e) => toast(errorText(e), "error")) }, icon("external", 14), "Open folder"),
      store.connecting && h("button", { class: "btn", onclick: () => api.nextcloudCancelConnect() }, "Cancel"),
      nc?.loginName
        ? h("button", { class: "btn", onclick: disconnect }, "Disconnect")
        : h("button", { class: "btn primary", disabled: store.connecting || needsCloudflared, onclick: connect }, store.connecting ? "Waiting for you..." : "Connect"),
    ),
  );
}
