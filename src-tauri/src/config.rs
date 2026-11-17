//! `ue-dev-hub.toml`: the team's settings for a project, committed to the project's repository.
//!
//! Everything is optional. A repo without the file still works with defaults read from the
//! .uproject. The file comes from the repo, so it's validated: problems are reported on the Setup
//! screen instead of being silently ignored, and nothing in it can run arbitrary commands.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

pub const FILE_NAME: &str = "ue-dev-hub.toml";
/// Personal, gitignored overrides for one checkout. Same format, merged over the team file.
pub const LOCAL_FILE_NAME: &str = "ue-dev-hub.local.toml";
pub const SCHEMA: u32 = 1;

pub const TOOLS: &[&str] = &[
    "terminal",
    "ide",
    "bake_lighting",
    "soundbanks",
    "dependencies",
    "clean",
    "regenerate",
    "repair_git",
];

pub const WIDGETS: &[&str] = &["incoming", "work", "ide", "shortcuts"];
pub const DEFAULT_WIDGETS: &[&str] = &["incoming", "work", "shortcuts"];

pub const LIGHTING_QUALITIES: &[&str] = &["Preview", "Medium", "High", "Production"];

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(default)]
pub struct Config {
    pub schema: u32,
    pub min_hub_version: Option<String>,
    pub project: ProjectSection,
    pub requirements: Requirements,
    pub links: Vec<Link>,
    pub home: Home,
    pub package: Package,
    pub test_map: TestMap,
    pub report: Report,
    pub upload: Upload,
    pub git: GitSettings,
    pub tools: Tools,
    pub addons: Addons,
    pub dependencies: Vec<Dependency>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            min_hub_version: None,
            project: Default::default(),
            requirements: Default::default(),
            links: vec![],
            home: Default::default(),
            package: Default::default(),
            test_map: Default::default(),
            report: Default::default(),
            upload: Default::default(),
            git: Default::default(),
            tools: Default::default(),
            addons: Default::default(),
            dependencies: vec![],
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(default)]
pub struct ProjectSection {
    /// Shown in the hub. Defaults to the .uproject's name.
    pub display_name: Option<String>,
    /// Overrides the EngineAssociation from the .uproject.
    pub engine_version: Option<String>,
    /// The branch everyone rebases onto. Defaults to origin's HEAD branch.
    pub base_branch: Option<String>,
    /// Sets `core.hooksPath`, relative to the project root.
    pub git_hooks_path: Option<String>,
    /// The letter tile's color in the project swapper, like "#3a68e8".
    pub color: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(default)]
pub struct Requirements {
    pub visual_studio: String,
    /// Full Wwise version, like "2024.1.0.1234". Enables the soundbank steps.
    pub wwise: Option<String>,
}

impl Default for Requirements {
    fn default() -> Self {
        Self { visual_studio: "2022".into(), wwise: None }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Link {
    pub label: String,
    pub url: String,
    /// One of "docs", "tasks", "drive", "lock", "chat", "link".
    pub icon: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(default)]
pub struct Home {
    /// The team's default home widgets. Each person can change theirs.
    pub widgets: Option<Vec<String>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(default)]
pub struct Package {
    /// The ini with the `+MapsToCook=` list.
    pub maps_from: String,
    /// Used instead of `maps_from` when set. Long package names like "/Game/Maps/Menu".
    pub maps: Vec<String>,
    pub checklist_url: Option<String>,
    pub output_dir: String,
    pub lighting_quality: String,
    /// The .wproj, relative to the project root. Defaults to `<Name>_WwiseProject/<Name>_WwiseProject.wproj`.
    pub wwise_project: Option<String>,
    pub soundbanks: bool,
    pub zip: bool,
}

impl Default for Package {
    fn default() -> Self {
        Self {
            maps_from: "Config/DefaultGame.ini".into(),
            maps: vec![],
            checklist_url: None,
            output_dir: "Builds".into(),
            lighting_quality: "Medium".into(),
            wwise_project: None,
            soundbanks: true,
            zip: true,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(default)]
pub struct TestMap {
    pub launch_args: String,
    /// Maps whose name ends with this count as main levels.
    pub main_level_suffix: String,
}

impl Default for TestMap {
    fn default() -> Self {
        Self {
            launch_args: String::new(),
            main_level_suffix: "_P".into(),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(default)]
pub struct Report {
    /// A Discord webhook (Channel settings > Integrations > Webhooks) that reports are posted to.
    /// It can only post to that one channel. Treat it like a password anyway: it's in the repo.
    pub discord_webhook: Option<String>,
    /// Discord's upload limit in MB for all files of one report: 10 by default, more for
    /// boosted servers. Logs are cut to fit; screenshots and videos that don't fit are left out.
    pub max_upload_mb: u64,
    /// Your task tracker, linked from the Report screen.
    pub issue_url: Option<String>,
}

/// How "Update" brings the base branch's changes into a feature branch.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(default)]
pub struct GitSettings {
    pub update: UpdateStrategy,
    pub push: PushPolicy,
    /// The kinds of branch to pick from when making one, like `["feat", "fix"]`, which makes
    /// names like `feat/my-work`. Empty: any name.
    pub branch_prefixes: Vec<String>,
}

/// `git.update`.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateStrategy {
    /// Merges the base branch into the feature branch and pushes normally.
    #[default]
    Merge,
    /// Replays the branch on top of the base branch, which needs a force push.
    Rebase,
}

/// `git.push`.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PushPolicy {
    /// Shows a checkbox.
    #[default]
    Ask,
    /// Pushes after updating.
    Always,
    /// Only updates locally.
    Never,
}

impl UpdateStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
        }
    }
}

impl PushPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

/// Uploading packaged builds to the team's Nextcloud. Only the address and folder live here;
/// each person signs in with their own account, and the login stays in Windows Credential Manager.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(default)]
pub struct Upload {
    pub nextcloud_url: Option<String>,
    /// The folder in each person's Nextcloud files, usually a folder shared with the team.
    /// `{year}` and `{month}` are replaced when uploading, like Team/Builds/{year}.
    pub folder: String,
    /// Whether "Upload to Nextcloud" starts ticked on the Package screen.
    pub after_packaging: bool,
}

impl Default for Upload {
    fn default() -> Self {
        Self { nextcloud_url: None, folder: "Builds".into(), after_packaging: false }
    }
}

impl Default for Report {
    fn default() -> Self {
        Self { discord_webhook: None, max_upload_mb: 10, issue_url: None }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(default)]
pub struct Addons {
    /// Addons that are on for everyone in the project, unless someone turns one off for
    /// themselves. Official addons that aren't installed yet install by themselves.
    pub default: Vec<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(default)]
pub struct Tools {
    /// Which tools appear. All of them when not set.
    pub enabled: Option<Vec<String>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Dependency {
    /// The rclone remote's name.
    pub name: String,
    /// The rclone remote type, like "drive".
    #[serde(rename = "type")]
    pub remote_type: String,
    /// The zip on the remote. Put the version in its name: it's downloaded again when this changes.
    pub source: String,
    /// The folder to extract it to, relative to the project root.
    pub dest: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Warning,
    Error,
}

#[derive(Serialize, Debug, Clone)]
pub struct Issue {
    pub level: Level,
    pub message: String,
}

impl Issue {
    fn warn(message: impl Into<String>) -> Self {
        Self { level: Level::Warning, message: message.into() }
    }
    fn error(message: impl Into<String>) -> Self {
        Self { level: Level::Error, message: message.into() }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Loaded {
    pub config: Config,
    pub issues: Vec<Issue>,
    pub has_file: bool,
    pub has_local_file: bool,
    /// Set when the Git settings may not be what the team wrote, so Update must not run.
    pub git_problem: Option<String>,
}

/// Reads both files from `root`. Never fails: problems become issues and the defaults are used.
/// Each top-level section is read on its own, so a wrong value only resets its own section.
pub fn load(root: &Path) -> Loaded {
    let mut issues = Vec::new();
    let mut git_problem = None;
    let mut read = |name: &'static str| match read_table(&root.join(name), name) {
        Ok(t) => t,
        Err(e) => {
            issues.push(Issue::error(e));
            git_problem.get_or_insert_with(|| paused_message(name));
            None
        }
    };
    let team = read(FILE_NAME);
    let local = read(LOCAL_FILE_NAME);
    let has_file = team.is_some();
    let has_local_file = local.is_some();

    let mut merged = team.clone().unwrap_or_default();
    if let Some(local) = local.clone() {
        merge(&mut merged, local);
    }

    let mut s = Sections { merged, team: team.as_ref(), local: local.as_ref(), issues: &mut issues, unknown: vec![], broken: vec![] };
    let d = Config::default();
    let config = Config {
        schema: s.take("schema", d.schema),
        min_hub_version: s.take("min_hub_version", d.min_hub_version),
        project: s.take("project", d.project),
        requirements: s.take("requirements", d.requirements),
        links: s.take("links", d.links),
        home: s.take("home", d.home),
        package: s.take("package", d.package),
        test_map: s.take("test_map", d.test_map),
        report: s.take("report", d.report),
        upload: s.take("upload", d.upload),
        git: s.take("git", d.git),
        tools: s.take("tools", d.tools),
        addons: s.take("addons", d.addons),
        dependencies: s.take("dependencies", d.dependencies),
    };
    let mut unknown = std::mem::take(&mut s.unknown);
    unknown.extend(s.merged.keys().cloned());
    if let Some((_, file)) = s.broken.iter().find(|(key, _)| key == "git" || key == "project") {
        git_problem.get_or_insert_with(|| paused_message(file));
    }
    for key in unknown {
        issues.push(Issue::warn(format!("Unknown setting '{key}' is ignored. Check for a typo, or update the hub.")));
    }

    let config = validate(config, &mut issues, &mut git_problem);
    Loaded { config, issues, has_file, has_local_file, git_problem }
}

/// Why Update is refused while the Git settings may be wrong.
fn paused_message(file: &str) -> String {
    format!("Updating is paused until the problem in {file} is fixed, so it doesn't update with the wrong Git settings. The Setup screen shows what's wrong.")
}

/// The merged settings, taken apart one top-level section at a time.
struct Sections<'a> {
    merged: toml::Table,
    team: Option<&'a toml::Table>,
    local: Option<&'a toml::Table>,
    issues: &'a mut Vec<Issue>,
    unknown: Vec<String>,
    /// Sections that had a wrong value, with the file that probably has it.
    broken: Vec<(String, &'static str)>,
}

impl Sections<'_> {
    /// The section `key`, or `default` if it's missing or has a wrong value.
    fn take<T: serde::de::DeserializeOwned>(&mut self, key: &str, default: T) -> T {
        let Some(value) = self.merged.remove(key) else { return default };
        let label = match &value {
            toml::Value::Table(_) => format!("[{key}]"),
            toml::Value::Array(a) if !a.is_empty() && a.iter().all(toml::Value::is_table) => format!("[[{key}]]"),
            _ => key.to_string(),
        };
        let mut unknown = vec![];
        match serde_ignored::deserialize(value, |path| unknown.push(path.to_string())) {
            Ok(v) => {
                self.unknown.extend(unknown.into_iter().map(|p| format!("{key}.{p}")));
                v
            }
            Err(e) => {
                let file = self.blame::<T>(key);
                self.issues.push(Issue::error(format!("{label} in {file} has a wrong value, so the hub uses the defaults for it: {e}")));
                self.broken.push((key.to_string(), file));
                default
            }
        }
    }

    /// Which file the wrong value in `key` is in: the team file if its section is wrong on its
    /// own, otherwise the local file that overrides it.
    fn blame<T: serde::de::DeserializeOwned>(&self, key: &str) -> &'static str {
        match (self.team.and_then(|t| t.get(key)), self.local.and_then(|t| t.get(key))) {
            (Some(team), Some(_)) if T::deserialize(team.clone()).is_ok() => LOCAL_FILE_NAME,
            (None, Some(_)) => LOCAL_FILE_NAME,
            _ => FILE_NAME,
        }
    }
}

/// The file as a table, None if it doesn't exist, or an error saying why it can't be read.
fn read_table(path: &Path, name: &str) -> Result<Option<toml::Table>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{name} couldn't be read, so the hub ignores it: {e}")),
    };
    text.parse::<toml::Table>().map(Some).map_err(|e| format!("{name} couldn't be read, so the hub ignores it:\n{e}"))
}

/// Merges `over` into `base`. Tables merge key by key; everything else, arrays included, is replaced.
fn merge(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(key, v);
            }
        }
    }
}

/// A path inside the project: relative, without `..`.
/// The same answer on every platform, since one ue-dev-hub.toml is shared by Windows and Linux
/// machines: no drive letters (`C:`), no leading `/` or `\`, and no `..`.
pub fn is_inside(path: &str) -> bool {
    let p = Path::new(path);
    !path.is_empty()
        && !path.contains(':')
        && !path.starts_with(['/', '\\'])
        && !path.split(['/', '\\']).any(|part| part == "..")
        && !p.is_absolute()
        && p.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        // "." would be the project root itself.
        && p.components().any(|c| matches!(c, Component::Normal(_)))
}

fn is_web_url(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    u.starts_with("https://") && u.len() > "https://".len()
}

/// A Git branch name that can't be mistaken for an option or escape the refs folder.
pub fn is_branch_name(b: &str) -> bool {
    !b.is_empty()
        && !b.starts_with(['-', '/', '.'])
        && !b.ends_with(['/', '.'])
        && !b.contains("..")
        && !b.ends_with(".lock")
        && b.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
}

/// Whether Git takes `b` as a branch name, by the rules of `git check-ref-format --branch`, so
/// names with non-ASCII letters work too. Never starts with '-', so it can't be read as an option.
pub fn is_git_branch_name(b: &str) -> bool {
    let bad_char = |c: char| c.is_ascii_control() || " ~^:?*[\\".contains(c);
    !b.is_empty()
        && b != "@"
        && b != "HEAD"
        && !b.starts_with('-')
        && !b.ends_with('.')
        && !b.contains("..")
        && !b.contains("@{")
        && !b.chars().any(bad_char)
        && b.split('/').all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// "5.8", or a source build's "{GUID}".
pub fn is_engine_version(v: &str) -> bool {
    let plain = !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.');
    let guid = v.starts_with('{') && v.ends_with('}') && v[1..v.len() - 1].chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    plain || guid
}

/// rclone names and types: letters, digits, `-` and `_`, not starting with `-`.
fn is_plain_name(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// "1.2.3" to (1, 2, 3). Missing parts count as 0.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0)))
}

fn validate(mut c: Config, issues: &mut Vec<Issue>, git_problem: &mut Option<String>) -> Config {
    if c.schema > SCHEMA {
        issues.push(Issue::warn(format!(
            "{FILE_NAME} is written for a newer hub (schema {}). Some settings may not work until the hub updates.",
            c.schema
        )));
    }

    if let Some(min) = &c.min_hub_version {
        let current = env!("CARGO_PKG_VERSION");
        match (parse_version(min), parse_version(current)) {
            (Some(m), Some(cur)) if m > cur => issues.push(Issue::warn(format!(
                "This project needs hub {min} or newer, you have {current}. Restart the hub to update it."
            ))),
            (None, _) => issues.push(Issue::warn(format!("min_hub_version '{min}' isn't a version like 1.2.0."))),
            _ => {}
        }
    }

    let mut paths = |field: &str, value: &mut Option<String>| {
        if let Some(v) = value.as_deref()
            && !is_inside(v)
        {
            issues.push(Issue::error(format!("{field} = '{v}' must be a path inside the project. It's ignored.")));
            *value = None;
            if field == "project.git_hooks_path" {
                git_problem.get_or_insert_with(|| paused_message(FILE_NAME));
            }
        }
    };
    paths("project.git_hooks_path", &mut c.project.git_hooks_path);
    paths("package.wwise_project", &mut c.package.wwise_project);

    let mut seen = Vec::new();
    c.git.branch_prefixes.retain(|p| {
        let ok = !p.is_empty() && p.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') && !p.starts_with('-');
        if !ok {
            issues.push(Issue::warn(format!("git.branch_prefixes: '{p}' isn't a prefix like \"feat\" (letters, digits, - and _). It's ignored.")));
        }
        let new = ok && !seen.contains(p);
        if new {
            seen.push(p.clone());
        }
        new
    });

    c.package.maps.retain(|m| {
        let ok = crate::maps::is_map_name(m);
        if !ok {
            issues.push(Issue::error(format!("package.maps has '{m}', which isn't a map name like /Game/Maps/MyMap. It's skipped.")));
        }
        ok
    });
    if !is_inside(&c.package.maps_from) {
        issues.push(Issue::error(format!("package.maps_from = '{}' must be a path inside the project. It's ignored.", c.package.maps_from)));
        c.package.maps_from = Package::default().maps_from;
    }
    if !is_inside(&c.package.output_dir) {
        issues.push(Issue::error(format!("package.output_dir = '{}' must be a path inside the project. It's ignored.", c.package.output_dir)));
        c.package.output_dir = Package::default().output_dir;
    }
    if !LIGHTING_QUALITIES.contains(&c.package.lighting_quality.as_str()) {
        issues.push(Issue::warn(format!(
            "package.lighting_quality = '{}' isn't one of {}. Using Medium.",
            c.package.lighting_quality,
            LIGHTING_QUALITIES.join(", ")
        )));
        c.package.lighting_quality = "Medium".into();
    }

    for (field, url) in [
        ("package.checklist_url", &mut c.package.checklist_url),
        ("report.issue_url", &mut c.report.issue_url),
    ] {
        if let Some(u) = url.as_deref()
            && !is_web_url(u)
        {
            issues.push(Issue::error(format!("{field} = '{u}' must start with https://. It's ignored.")));
            *url = None;
        }
    }

    if let Some(hook) = c.report.discord_webhook.as_deref() {
        let ok = ["https://discord.com/api/webhooks/", "https://discordapp.com/api/webhooks/", "https://ptb.discord.com/api/webhooks/", "https://canary.discord.com/api/webhooks/"]
            .iter()
            .any(|p| hook.starts_with(p));
        if !ok {
            issues.push(Issue::error("report.discord_webhook must be a Discord webhook address (https://discord.com/api/webhooks/...). It's ignored."));
            c.report.discord_webhook = None;
        }
    }
    if let Some(u) = c.upload.nextcloud_url.as_deref()
        && !u.trim().to_ascii_lowercase().starts_with("https://")
    {
        issues.push(Issue::error(format!("upload.nextcloud_url = '{u}' must start with https://. Uploading is off.")));
        c.upload.nextcloud_url = None;
    }
    let unknown = c.upload.folder.replace("{year}", "").replace("{month}", "");
    if c.upload.folder.split('/').any(|p| p == ".." || p == ".") || c.upload.folder.contains('\\') || unknown.contains(['{', '}']) {
        issues.push(Issue::error(format!("upload.folder = '{}' must be a plain folder path like Team/Builds, optionally with {{year}} or {{month}}. Using Builds.", c.upload.folder)));
        c.upload.folder = "Builds".into();
    }
    if c.report.max_upload_mb == 0 || c.report.max_upload_mb > 500 {
        issues.push(Issue::warn(format!("report.max_upload_mb = {} isn't between 1 and 500. Using 10.", c.report.max_upload_mb)));
        c.report.max_upload_mb = 10;
    }

    c.links.retain(|l| {
        let ok = is_web_url(&l.url);
        if !ok {
            issues.push(Issue::error(format!("The link '{}' must start with https://, so it's hidden.", l.label)));
        }
        ok
    });

    if let Some(color) = c.project.color.as_deref()
        && !(color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|ch| ch.is_ascii_hexdigit()))
    {
        issues.push(Issue::warn(format!("project.color = '{color}' isn't a color like #3a68e8.")));
        c.project.color = None;
    }

    if let Some(tools) = c.tools.enabled.as_mut() {
        tools.retain(|t| {
            let ok = TOOLS.contains(&t.as_str());
            if !ok {
                issues.push(Issue::warn(format!("Unknown tool '{t}' in tools.enabled. Known tools: {}.", TOOLS.join(", "))));
            }
            ok
        });
    }

    c.addons.default.retain(|id| {
        let ok = crate::addons::is_id(id);
        if !ok {
            issues.push(Issue::warn(format!("'{id}' in addons.default isn't an addon id like \"git\".")));
        }
        ok
    });
    c.addons.default.dedup();

    if let Some(widgets) = c.home.widgets.as_mut() {
        widgets.retain(|w| {
            let ok = WIDGETS.contains(&w.as_str());
            if !ok {
                issues.push(Issue::warn(format!("Unknown widget '{w}' in home.widgets. Known widgets: {}.", WIDGETS.join(", "))));
            }
            ok
        });
    }

    c.dependencies.retain(|d| {
        let source_ok = !d.source.is_empty() && !d.source.starts_with(['-', '/', '\\']) && !d.source.contains(':') && !d.source.contains("..");
        let ok = is_inside(&d.dest) && is_plain_name(&d.name) && is_plain_name(&d.remote_type) && source_ok;
        if !ok {
            issues.push(Issue::error(format!(
                "The dependency '{}' needs a plain name and type (letters, digits, - and _), a source path on the storage, and a dest inside the project. It's skipped.",
                d.name
            )));
        }
        ok
    });

    if let Some(b) = c.project.base_branch.as_deref()
        && !is_branch_name(b)
    {
        issues.push(Issue::error(format!("project.base_branch = '{b}' isn't a valid branch name. It's ignored.")));
        c.project.base_branch = None;
        git_problem.get_or_insert_with(|| paused_message(FILE_NAME));
    }
    if let Some(v) = c.project.engine_version.as_deref()
        && !is_engine_version(v)
    {
        issues.push(Issue::error(format!("project.engine_version = '{v}' must look like 5.8, or a source build's {{GUID}}. It's ignored.")));
        c.project.engine_version = None;
    }
    if let Some(v) = c.requirements.wwise.as_deref()
        && !(v.chars().all(|ch| ch.is_ascii_digit() || ch == '.') && !v.is_empty())
    {
        issues.push(Issue::error(format!("requirements.wwise = '{v}' must be a version like 2024.1.0.1234. It's ignored.")));
        c.requirements.wwise = None;
    }

    c
}

/// The file a new project starts with, written by "Create ue-dev-hub.toml" on the Setup screen.
pub fn template(name: &str, base_branch: &str) -> String {
    format!(
        r##"# Settings for UE Dev Hub. Commit this file so everyone gets the same setup.
# Your personal overrides go in ue-dev-hub.local.toml next to it (add that to .gitignore).
schema = 1
# The oldest hub that can open this project. Older ones ask to update.
# min_hub_version = "0.1.0"

[project]
display_name = "{name}"
base_branch = "{base_branch}"
# Overrides the engine version from the .uproject: "5.8", or a source build's "{{GUID}}".
# engine_version = "5.8"
# git_hooks_path = "Hooks"
# color = "#3a68e8"

[requirements]
visual_studio = "2022"
# wwise = "2024.1.0.1234"

# Links shown in the sidebar. icon is one of docs, tasks, drive, lock, chat, link.
# [[links]]
# label = "Documentation"
# url = "https://"
# icon = "docs"

[home]
# The default home screen. Everyone can change their own.
widgets = ["incoming", "work", "shortcuts"]

[package]
maps_from = "Config/DefaultGame.ini"
output_dir = "Builds"
lighting_quality = "Medium"
# checklist_url = "https://"
# Maps to cook, used instead of maps_from. Long package names.
# maps = ["/Game/Maps/Menu", "/Game/Maps/Level_P"]
# The Wwise project. Defaults to <Name>_WwiseProject/<Name>_WwiseProject.wproj.
# wwise_project = "MyGame_WwiseProject/MyGame_WwiseProject.wproj"
# Generate soundbanks before packaging (needs requirements.wwise).
# soundbanks = true
# Zip the packaged build.
# zip = true

[test_map]
# launch_args = "-windowed -ResX=1280 -ResY=720"
# Maps whose name ends with this count as main levels.
# main_level_suffix = "_P"

[report]
# A Discord channel webhook that "Report a problem" posts to.
# discord_webhook = "https://discord.com/api/webhooks/..."
# max_upload_mb = 10
# issue_url = "https://"

# How Update brings the base branch into a feature branch: "merge" (pushes normally) or
# "rebase" (keeps history linear; needs a force push). push: "ask", "always" or "never".
[git]
update = "merge"
push = "ask"
# The kinds of branch people pick from when they make one, like feat/my-work. Leave it out to
# allow any name.
# branch_prefixes = ["feat", "fix", "chore"]

# Uploading packaged builds. Everyone signs in with their own Nextcloud account.
# [upload]
# nextcloud_url = "https://cloud.example.com"
# folder = "Team/Builds/{{year}}"   # {{year}} and {{month}} are filled in when uploading
# after_packaging = true

# Addons that are on for everyone. Official ones install by themselves; each person can still
# turn them off in Settings > Addons.
# [addons]
# default = ["git"]

# [tools]
# enabled = ["terminal", "ide", "bake_lighting", "soundbanks", "dependencies", "clean", "regenerate", "repair_git"]

# Zips downloaded with rclone and extracted into the project.
# [[dependencies]]
# name = "wwise-plugin"
# type = "drive"
# source = "ThirdParty/Wwise-2024.1.0.1234.zip"
# dest = "Plugins"
"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_str(team: &str, local: Option<&str>) -> Loaded {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-config-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), team).unwrap();
        if let Some(l) = local {
            std::fs::write(dir.join(LOCAL_FILE_NAME), l).unwrap();
        }
        let r = load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        r
    }

    #[test]
    fn template_is_valid() {
        let r = load_str(&template("Game", "develop"), None);
        assert!(r.issues.is_empty(), "{:?}", r.issues);
        assert_eq!(r.config.project.base_branch.as_deref(), Some("develop"));
    }

    #[test]
    fn reports_unknown_keys_and_bad_links() {
        let r = load_str(
            r#"
            [projet]
            name = "x"
            [[links]]
            label = "Bad"
            url = "file:///C:/Windows"
            [[links]]
            label = "Good"
            url = "https://example.com"
            "#,
            None,
        );
        assert_eq!(r.config.links.len(), 1);
        assert!(r.issues.iter().any(|i| i.message.contains("projet")));
        assert!(r.issues.iter().any(|i| i.message.contains("Bad")));
    }

    #[test]
    fn rejects_paths_outside_the_project() {
        let r = load_str("[package]\noutput_dir = \"../elsewhere\"\n[project]\ngit_hooks_path = \"C:/hooks\"", None);
        assert_eq!(r.config.package.output_dir, "Builds");
        assert!(r.config.project.git_hooks_path.is_none());
        assert!(!is_inside("."));
        assert!(!is_inside("./"));
        assert!(is_inside("./Plugins"));
        assert_eq!(r.issues.iter().filter(|i| i.level == Level::Error).count(), 2);
    }

    #[test]
    fn broken_file_falls_back_to_defaults() {
        let r = load_str("[package\n", None);
        assert!(r.issues[0].message.contains("couldn't be read"));
        assert_eq!(r.config.package.output_dir, "Builds");
    }

    #[test]
    fn local_file_overrides_team_file() {
        let r = load_str(
            "[package]\nlighting_quality = \"High\"\nzip = true\n",
            Some("[package]\nzip = false\n"),
        );
        assert_eq!(r.config.package.lighting_quality, "High");
        assert!(!r.config.package.zip);
    }

    #[test]
    fn a_wrong_value_only_resets_its_own_section() {
        let r = load_str(
            r#"[git]
update = "rebase"
[project]
base_branch = "develop"
[package]
zip = "yes"
"#,
            None,
        );
        assert_eq!(r.config.git.update, UpdateStrategy::Rebase);
        assert_eq!(r.config.project.base_branch.as_deref(), Some("develop"));
        assert!(r.config.package.zip);
        let errors: Vec<_> = r.issues.iter().filter(|i| i.level == Level::Error).collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].message.contains("[package] in ue-dev-hub.toml"), "{}", errors[0].message);
        assert!(r.git_problem.is_none());
    }

    #[test]
    fn wrong_git_settings_pause_updates_and_name_the_file() {
        let r = load_str(r#"[git]
update = "rebase"
"#, Some(r#"[git]
push = "sometimes"
"#));
        assert!(r.issues.iter().any(|i| i.message.contains("[git] in ue-dev-hub.local.toml")), "{:?}", r.issues);
        assert!(r.git_problem.as_deref().is_some_and(|p| p.contains("ue-dev-hub.local.toml")));

        let r = load_str(r#"[git]
update = "squash"
"#, None);
        assert!(r.issues.iter().any(|i| i.message.contains("[git] in ue-dev-hub.toml")), "{:?}", r.issues);
        assert!(r.git_problem.is_some());

        let r = load_str(r#"[project
"#, None);
        assert!(r.git_problem.is_some());
    }

    #[test]
    fn unknown_keys_in_links_and_dependencies_are_only_warnings() {
        let r = load_str(
            r#"[[links]]
label = "Docs"
url = "https://example.com"
new_thing = 1
[[dependencies]]
name = "a"
type = "drive"
source = "x.zip"
dest = "Plugins"
checksum = "abc"
"#,
            None,
        );
        assert_eq!(r.config.links.len(), 1);
        assert_eq!(r.config.dependencies.len(), 1);
        assert!(r.issues.iter().all(|i| i.level == Level::Warning), "{:?}", r.issues);
        assert!(r.issues.iter().any(|i| i.message.contains("new_thing")));
        assert!(r.issues.iter().any(|i| i.message.contains("checksum")));
    }

    #[test]
    fn template_lists_every_option() {
        // Uncommenting every "# key = value" line must still give a valid file, with no unknown keys.
        let full: String = template("Game", "develop")
            .lines()
            .map(|l| match l.strip_prefix("# ") {
                Some(rest) if rest.contains(" = ") && !rest.starts_with('[') => rest.split("   #").next().unwrap().to_string(),
                Some(rest) if rest.starts_with('[') && rest.ends_with(']') => rest.to_string(),
                _ => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let r = load_str(&full, None);
        assert!(r.issues.iter().all(|i| !i.message.contains("Unknown")), "{:?}", r.issues);
        for key in ["min_hub_version", "engine_version", "maps = ", "wwise_project", "soundbanks", "zip", "main_level_suffix"] {
            assert!(full.contains(key), "{key}");
        }
    }

    #[test]
    fn compares_versions() {
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("1"), Some((1, 0, 0)));
    }

    #[test]
    fn takes_the_branch_names_git_takes() {
        for ok in ["develop", "feature/boss-arena", "fix/ümlaut-käfer", "release-1.2", "a.b/c", "x@y", "日本語"] {
            assert!(is_git_branch_name(ok), "{ok}");
        }
        for bad in [
            "", "-f", "--upload-pack=x", "@", "HEAD", "a..b", "a.", "a/", "/a", "a//b", ".a", "a/.b", "a.lock", "a/b.lock/c", "a b", "a~1", "a^", "a:b",
            "a?", "a*", "a[b", "a\\b", "a@{1}", "a\tb", "a\u{7f}",
        ] {
            assert!(!is_git_branch_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn keeps_good_branch_prefixes_once() {
        let r = load_str("[git]\nbranch_prefixes = [\"feat\", \"fix\", \"feat\", \"has/slash\", \"\", \"-x\", \"wip_2\"]\n", None);
        assert_eq!(r.config.git.branch_prefixes, ["feat", "fix", "wip_2"]);
        assert_eq!(r.issues.iter().filter(|i| i.message.contains("branch_prefixes")).count(), 3);
        assert!(r.git_problem.is_none());
        assert!(load_str("[git]\nupdate = \"rebase\"\n", None).config.git.branch_prefixes.is_empty());
    }
}
