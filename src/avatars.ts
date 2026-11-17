// GitHub pictures for commit authors, shared by the home screen and the Git page.
import { api } from "./api";
import { rerender } from "./store";
import { h, hue, initials } from "./ui";

/// Pictures by commit email, as data URLs. `null` means the author has none.
const avatarCache: Record<string, string | null> = {};
/// Emails asked for but not answered yet. Requests that overlap wait for each other, so none
/// gets dropped.
const waiting = new Set<string>();
let loading: Promise<void> | null = null;
/// Increases whenever pictures arrive, so lists built from them know to redraw.
let version = 0;

export function avatarsVersion(): number {
  return version;
}

export async function loadAvatars(emails: string[]) {
  for (const e of emails) if (e && !(e in avatarCache)) waiting.add(e);
  if (loading || waiting.size === 0) return;
  loading = (async () => {
    while (waiting.size) {
      const batch = [...waiting];
      waiting.clear();
      try {
        Object.assign(avatarCache, await api.avatars(batch));
        version++;
        rerender();
      } catch {
        // Without pictures the initials stay, which is fine.
        break;
      }
    }
  })();
  await loading;
  loading = null;
}

/// The author's GitHub picture, or their initials while it loads or when they have none.
export function avatar(name: string, email: string) {
  const src = avatarCache[email];
  return src
    ? h("img", { class: "avatar", src, alt: "", title: name })
    : h("span", { class: "avatar", style: `--h: ${hue(name)}`, title: name }, initials(name));
}
