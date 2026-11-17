// What a UE Dev Hub addon's page can use: API version 1.
//
// An addon's ui/index.js is an ES module whose default export is `activate`. The hub calls it
// once each time the addon is turned on, with a `Hub` made for that addon, and shows the pages
// it returns under the ids the addon.toml lists. `deactivate` runs when it's turned off again. Everything here stays compatible within version 1;
// a breaking change makes it version 2, and the hub then refuses addons made for 1.
//
// See docs/addons.md in the hub repository.

export interface AddonPage {
  /**
   * Builds the page: a `<main class="main">` element, like the hub's own screens. Called on
   * every repaint. Focus comes back only to fields with an `aria-label`, `placeholder` or `name`,
   * and scroll positions only to elements with `data-keep-scroll="<key>"`.
   */
  render(): HTMLElement;
  /** The page was opened. */
  enter?(): void;
  /** Another project was opened: forget everything about the previous one. */
  reset?(): void;
}

export interface AddonUi {
  pages: Record<string, AddonPage>;
  /**
   * The addon was turned off, updated or removed: stop timers, remove listeners. If it's turned on
   * again, `activate` runs again, in the same module (its top-level state stays).
   */
  deactivate?(): void;
}

export type Activate = (hub: Hub) => AddonUi;

export interface CallOptions {
  /**
   * Run as a step, named like "Pushing": the call may change the project, so it never runs
   * alongside a job, Update & Launch or another step, and its log lines go to `onLog`.
   */
  step?: string;
}

export interface Hub {
  readonly version: 1;
  /** This addon's id. */
  readonly id: string;
  /** Calls the addon's helper program. Rejects with the helper's error message. */
  call<T>(method: string, params?: Record<string, unknown>, options?: CallOptions): Promise<T>;
  /**
   * Listens to the log lines of this addon's steps ("info" for what happens, "detail" for raw
   * tool output), so the page can show them. Returns a function that stops listening.
   */
  onLog(fn: (level: "info" | "detail", text: string) => void): () => void;
  ui: {
    /** Makes an element: attributes (on* are event listeners) and children. False, null and undefined children are left out. */
    h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs?: Record<string, unknown>, ...children: unknown[]): HTMLElementTagNameMap[K];
    /** One of the hub's icons, or one registered with `registerIcon`. */
    icon(name: string, size?: number, extra?: string): SVGSVGElement;
    /** Adds an icon: the inside of a 24×24 stroked SVG. Built-in names can't be replaced. */
    registerIcon(name: string, svg: string): void;
    toast(text: string, kind?: "ok" | "error"): void;
    /** Shows a dialog. `build` gets a close function; the promise resolves with what it's given, or `onDismiss`. */
    modal<T>(build: (close: (value: T) => void) => HTMLElement, onDismiss: T): Promise<T>;
    confirmDialog(title: string, message: string, ok: string, danger?: boolean): Promise<boolean>;
    errorText(e: unknown): string;
    plural(n: number, one: string, many?: string): string;
  };
  avatars: {
    /** Loads GitHub avatars for these emails, then repaints. */
    loadAvatars(emails: string[]): Promise<void>;
    /** An avatar picture, or initials until it has loaded. */
    avatar(name: string, email: string): HTMLElement;
  };
  app: {
    /** A job, Update & Launch or an addon step (this addon's own too) is running. */
    busy(): boolean;
    /** What's running, for "X is running" messages. */
    busyLabel(): string;
    /** Repaints the window, at most once per frame. */
    rerender(): void;
    /** The open project. */
    project(): ProjectInfo | null;
    /** Tells the hub the project's branch changed, so the sidebar shows it. */
    setBranch(branch: string): void;
    /** This addon's page that's open, or null. */
    activePage(): string | null;
    /** Opens one of this addon's pages. */
    go(page: string): void;
    revealPath(path: string): Promise<void>;
    /** The hub's update check for the current branch. */
    checkUpdates(): Promise<UpdateInfo>;
  };
}

export interface ProjectInfo {
  root: string;
  name: string;
  displayName: string;
  branch: string | null;
  baseBranch: string;
}

export interface Commit {
  hash: string;
  author: string;
  email: string;
  /** Like "3 hours ago". */
  when: string;
  subject: string;
}

export interface UpdateInfo {
  branch: string;
  defaultBranch: string;
  onDefaultBranch: boolean;
  /** Commits from the team that an update brings in. */
  commits: Commit[];
  /** Your commits that aren't on the base branch. */
  myCommits: number;
  hasLocalChanges: boolean;
  canPush: boolean;
  conflictingFiles: string[];
  editorRunning: boolean;
  serverHasMore: number;
  repoBusy: string | null;
  strategy: "merge" | "rebase";
  pushPolicy: "ask" | "always" | "never";
  blockedReason: string | null;
}

export interface UpdateResult {
  backup: string;
  pushed: boolean;
  commits: number;
  warnings: string[];
}
