// Types and calls shared with the Rust side.
import { invoke } from "@tauri-apps/api/core";

export type AfterLaunch = "keep" | "minimize" | "close";

export interface Link {
  label: string;
  url: string;
  icon?: string | null;
}

export interface ProjectInfo {
  root: string;
  name: string;
  displayName: string;
  engineVersion: string;
  branch: string | null;
  baseBranch: string;
  hasConfig: boolean;
  color: string | null;
  links: Link[];
  tools: string[];
  defaultWidgets: string[];
  checklistUrl: string | null;
  issueUrl: string | null;
  launchArgs: string;
  mainLevelSuffix: string;
  lightingQuality: string;
  wwiseVersion: string | null;
  soundbanks: boolean;
  zip: boolean;
  hasDependencies: boolean;
  uploadConfigured: boolean;
  uploadAfterPackaging: boolean;
}

export interface NextcloudStatus {
  configured: boolean;
  url: string | null;
  folder: string | null;
  loginName: string | null;
  cloudflareAccess: boolean | null;
  cloudflaredInstalled: boolean;
  webFolderUrl: string | null;
}

export interface ProjectEntry {
  root: string;
  displayName: string;
  color: string | null;
  branch: string | null;
  engineVersion: string | null;
  missing: boolean;
  selected: boolean;
}

export interface Prefs {
  widgets: string[] | null;
  ide: string | null;
  terminal: string | null;
  recentMaps: string[];
  lastBuildType: string | null;
  /** Addons turned on (true) or off (false) by hand for this project, by id. */
  addons: Record<string, boolean>;
}

export type Action =
  | { kind: "openUrl"; url: string }
  | { kind: "browseEngine" }
  | { kind: "createConfig" }
  | { kind: "openConfig" }
  | { kind: "useGitHooks"; yes: boolean };
export type Fix = { label: string } & Action;

export interface Check {
  id: string;
  title: string;
  status: "ok" | "warning" | "error" | "optional";
  detail: string;
  fixes: Fix[];
}

export interface Commit {
  hash: string;
  author: string;
  email: string;
  /** How long ago, like "3 hours ago". */
  when: string;
  subject: string;
}

export interface UpdateInfo {
  branch: string;
  defaultBranch: string;
  onDefaultBranch: boolean;
  commits: Commit[];
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

export interface AddonPageInfo {
  id: string;
  label: string;
  icon: string;
}

export interface AddonInfo {
  id: string;
  name: string;
  description: string;
  version: string;
  source: "official" | "disk";
  /** On for the current project and usable by this hub. */
  enabled: boolean;
  /** Turned on or off by hand instead of following the project. */
  chosen: boolean;
  projectDefault: boolean;
  pages: AddonPageInfo[];
  hasStyle: boolean;
  problem: string | null;
}

export interface AddonsState {
  installed: AddonInfo[];
  platform: string;
}

export interface AvailableAddon {
  id: string;
  name: string;
  description: string;
  version: string;
  installed: string | null;
  update: boolean;
  problem: string | null;
}

export interface AddonManifest {
  id: string;
  name: string;
  version: string;
}

export interface DefaultsOffer {
  /** Official addons the project uses that aren't installed yet. */
  offer: { id: string; name: string; description: string }[];
  /** [id, why] for addons the project lists that can't be installed. */
  missing: [string, string][];
}

export interface DefaultsResult {
  installed: string[];
  /** [id, why] for addons the project lists that couldn't be installed. */
  missing: [string, string][];
}

export interface BuildResult {
  ok: boolean;
  skipped: boolean;
  errors: string[];
  hint: string | null;
  logPath: string | null;
}

export interface Progress {
  percent: number | null;
  label: string;
}

export interface HomeStatus {
  branch: string;
  baseBranch: string;
  onBaseBranch: boolean;
  ahead: number;
  incoming: Commit[];
  incomingTotal: number;
  server: "upToDate" | "ahead" | "behind" | "diverged" | "notOnServer";
  unsavedFiles: number;
  lastCompile: string | null;
  lastCompileFailed: boolean | null;
  editorRunning: boolean;
  fetchError: string | null;
}

export interface MapStatus {
  name: string;
  package: string;
  kind: "unchanged" | "changed" | "never" | "missing";
  status: string;
  changedFiles: string[];
}

export interface BuildEntry {
  name: string;
  path: string;
  modified: number;
  zip: string | null;
  exe: string | null;
  forPlayers: boolean;
}

export interface BuildsInfo {
  builds: BuildEntry[];
  buildsDir: string;
}

export interface PackageInfo {
  maps: MapStatus[];
  qualities: string[];
  wwiseFound: boolean;
  /** Why soundbanks can't be generated on this platform at all (Linux). */
  wwiseUnsupported: string | null;
  sevenZip: boolean;
  editorRunning: boolean;
  buildsDir: string;
  /** What build folder names end in, like "Win64". */
  buildPlatform: string;
}

export interface MapInfo {
  name: string;
  package: string;
}

export interface TestMapInfo {
  maps: MapInfo[];
  recent: string[];
  sourceBuild: boolean;
  engineLabel: string;
  runs: TestRun[];
  lastBuildType: string | null;
  qualities: string[];
  editorRunning: boolean;
}

export interface Attachment {
  path: string;
  exists: boolean;
  modified: string | null;
  failed: boolean | null;
}

export interface ReportContext {
  branch: string;
  commit: string;
  commitSubject: string;
  commitUrl: string | null;
  engineLog: Attachment;
  buildLog: Attachment;
  hubLog: Attachment;
  reporter: string;
  hasWebhook: boolean;
  maxUploadMb: number;
}

export interface ReportInput {
  title: string;
  steps: string;
  happened: string;
  expected: string;
  includeCommit: boolean;
  setup: string;
  logs: string[];
  media: string[];
}

export interface Sent {
  skipped: string[];
  trimmed: string[];
}

export interface AppChoice {
  id: string;
  name: string;
}

export interface ToolsInfo {
  terminals: AppChoice[];
  ides: AppChoice[];
  terminal: string | null;
  ide: string | null;
  dependencies: { name: string; upToDate: boolean }[];
  changedMaps: number;
  wwiseFound: boolean;
  wwiseUnsupported: string | null;
}

export interface Step {
  title: string;
  note: string;
  state: "pending" | "running" | "done" | "failed" | "skipped";
  startedMs: number | null;
  endedMs: number | null;
}

export interface JobSnapshot {
  id: number;
  kind: string;
  title: string;
  subtitle: string;
  started: string;
  startedMs: number;
  endedMs: number | null;
  steps: Step[];
  progress: Progress | null;
  finished: boolean;
  ok: boolean;
  cancelled: boolean;
  message: string | null;
  outputPath: string | null;
  logPath: string | null;
  errors: string[];
}

export interface JobState {
  snapshot: JobSnapshot;
  lines: [string, string][];
}

export interface HubInfo {
  version: string;
  installed: boolean;
  /** Installed with a Linux package: updates come from the package manager. */
  packageManaged: boolean;
  offerInstall: boolean;
  platform: Platform;
  /** The addon API version this hub implements. */
  addonApi: number;
}

export type Platform = "windows" | "linux";

export interface PackageOptions {
  forPlayers: boolean;
  bakeMaps: string[];
  quality: string;
  soundbanks: boolean;
  zip: boolean;
  upload: boolean;
}

export interface TestRun {
  id: string;
  name: string;
  map: string;
  buildType: string;
  bakeQuality: string | null;
  launchArgs: string;
  createdMs: number;
  commit: string;
  branch: string;
  size: number;
}

export interface TestMapOptions {
  map: string;
  buildType: string;
  name: string;
  bakeQuality: string | null;
  launchArgs: string;
}

/// An addon install refused because a newer version is installed. Installing again with
/// `downgrade` replaces it anyway.
export class NewerAddonInstalled extends Error {
  constructor(
    message: string,
    readonly installed: string,
  ) {
    super(message);
  }
}

/// Addon installs fail with `{ kind, message, installed? }`; this turns that into an Error.
async function installAddon(cmd: string, args: Record<string, unknown>): Promise<AddonManifest> {
  try {
    return await invoke<AddonManifest>(cmd, args);
  } catch (e) {
    if (e && typeof e === "object" && "message" in e) {
      const err = e as { kind?: string; message: string; installed?: string };
      throw err.kind === "newerInstalled" && err.installed ? new NewerAddonInstalled(err.message, err.installed) : new Error(err.message);
    }
    throw e;
  }
}

export const api = {
  listProjects: () => invoke<ProjectEntry[]>("list_projects"),
  getProject: () => invoke<ProjectInfo | null>("get_project"),
  addProject: (path: string) => invoke<ProjectInfo>("add_project", { path }),
  selectProject: (path: string) => invoke<ProjectInfo>("select_project", { path }),
  removeProject: (path: string) => invoke<void>("remove_project", { path }),
  getPrefs: () => invoke<Prefs>("get_prefs"),
  setWidgets: (widgets: string[] | null) => invoke<void>("set_widgets", { widgets }),
  getAutoLaunch: () => invoke<boolean>("get_auto_launch"),
  setAutoLaunch: (enabled: boolean) => invoke<void>("set_auto_launch", { enabled }),
  getAfterLaunch: () => invoke<AfterLaunch>("get_after_launch"),
  setAfterLaunch: (action: AfterLaunch) => invoke<void>("set_after_launch", { action }),
  editorStarted: () => invoke<void>("editor_started"),
  setAppChoice: (kind: "ide" | "terminal", id: string) => invoke<void>("set_app_choice", { kind, id }),

  runChecks: () => invoke<Check[]>("run_checks"),
  setEnginePath: (path: string) => invoke<void>("set_engine_path", { path }),
  createConfig: () => invoke<string>("create_config"),
  openConfig: () => invoke<void>("open_config"),

  homeStatus: (fetch: boolean) => invoke<HomeStatus>("home_status", { fetch }),
  avatars: (emails: string[]) => invoke<Record<string, string | null>>("avatars", { emails }),
  prepareRepo: () => invoke<void>("prepare_repo"),
  /** Uses the project's Git hooks folder, or stops using it. */
  setGitHooks: (yes: boolean) => invoke<void>("set_git_hooks", { yes }),
  checkUpdates: () => invoke<UpdateInfo>("check_updates"),
  applyUpdate: (push: boolean) => invoke<UpdateResult>("apply_update", { push }),
  addonsList: () => invoke<AddonsState>("addons_list"),
  addonsAvailable: (fresh: boolean) => invoke<AvailableAddon[]>("addons_available", { fresh }),
  addonSetEnabled: (id: string, enabled: boolean) => invoke<void>("addon_set_enabled", { id, enabled }),
  /** `downgrade` allows replacing a newer installed version; it's refused otherwise. */
  addonInstall: (id: string, downgrade = false) => installAddon("addon_install", { id, downgrade }),
  addonInstallFromDisk: (path: string, downgrade = false) => installAddon("addon_install_from_disk", { path, downgrade }),
  addonUninstall: (id: string) => invoke<void>("addon_uninstall", { id }),
  addonDefaultOffers: () => invoke<DefaultsOffer>("addon_default_offers"),
  /** Installs the project's default addons the person said yes to, or remembers the no. */
  addonInstallDefaults: (ids: string[], install: boolean) => invoke<DefaultsResult>("addon_install_defaults", { ids, install }),
  addonCall: <T>(id: string, method: string, params: unknown, step: string | null) => invoke<T>("addon_call", { id, method, params, step }),
  buildProject: () => invoke<BuildResult>("build_project"),
  launchEditor: () => invoke<void>("launch_editor"),
  /** Whether an Unreal Editor has the open project open; one on another project doesn't count. */
  editorRunning: () => invoke<boolean>("editor_running"),

  packageInfo: () => invoke<PackageInfo>("package_info"),
  listBuilds: () => invoke<BuildsInfo>("list_builds"),
  runBuild: (path: string) => invoke<void>("run_build", { path }),
  nextcloudStatus: (probe: boolean) => invoke<NextcloudStatus>("nextcloud_status", { probe }),
  nextcloudConnect: () => invoke<string>("nextcloud_connect"),
  nextcloudCancelConnect: () => invoke<void>("nextcloud_cancel_connect"),
  nextcloudDisconnect: () => invoke<void>("nextcloud_disconnect"),
  startUpload: (path: string) => invoke<JobSnapshot>("start_upload", { path }),
  startPackage: (options: PackageOptions) => invoke<JobSnapshot>("start_package", { options }),
  startBake: (maps: string[], quality: string) => invoke<JobSnapshot>("start_bake", { maps, quality }),

  testMapInfo: () => invoke<TestMapInfo>("test_map_info"),
  mapLighting: (map: string) => invoke<MapStatus>("map_lighting", { map }),
  startTestMap: (options: TestMapOptions) => invoke<JobSnapshot>("start_test_map", { options }),
  playTestRun: (id: string) => invoke<void>("play_test_run", { id }),
  renameTestRun: (id: string, name: string) => invoke<void>("rename_test_run", { id, name }),
  deleteTestRun: (id: string) => invoke<void>("delete_test_run", { id }),
  deleteAllTestRuns: () => invoke<number>("delete_all_test_runs"),

  reportContext: () => invoke<ReportContext>("report_context"),
  sendReport: (input: ReportInput) => invoke<Sent>("send_report", { input }),
  /** The file dialog for screenshots and videos. Reports only attach media picked here or in the project. */
  reportPickMedia: () => invoke<string[]>("report_pick_media"),
  reportMarkdown: (input: ReportInput) => invoke<string>("report_markdown", { input }),

  toolsInfo: () => invoke<ToolsInfo>("tools_info"),
  ideChoices: () => invoke<{ ides: AppChoice[]; ide: string | null }>("ide_choices"),
  openTerminal: (id: string | null) => invoke<void>("open_terminal", { id }),
  openIde: (id: string | null) => invoke<void>("open_ide", { id }),
  setFlow: (name: string | null) => invoke<void>("set_flow", { name }),
  cleanBuildFiles: () => invoke<string[]>("clean_build_files"),
  startTool: (tool: string, force = false) => invoke<JobSnapshot>("start_tool", { tool, force }),

  jobState: () => invoke<JobState | null>("job_state"),
  cancelJob: () => invoke<void>("cancel_job"),
  dismissJob: () => invoke<void>("dismiss_job"),

  openUrl: (url: string) => invoke<void>("open_url", { url }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  hubInfo: () => invoke<HubInfo>("hub_info"),
  neverOfferInstall: () => invoke<void>("never_offer_install"),
  installHub: () => invoke<void>("install_hub"),
  updatePortable: () => invoke<string | null>("update_portable"),
  newerHubRelease: () => invoke<string | null>("newer_hub_release"),
};
