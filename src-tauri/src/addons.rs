//! Installed addons: what they are, where they live, and which ones are on for a project.
//!
//! An addon is a folder in `<local app data>/addons/<id>/` with an `addon.toml`, a `ui/index.js`
//! the window loads through the `addon://` scheme (see `addon_protocol.rs`), and optionally a
//! helper program for this platform that the hub starts and talks to (see `addon_host.rs`).
//! Addons come from the official addons repository (`addon_index.rs`) or from a zip or folder
//! on disk. A project can list addons that should be on by default in `[addons] default`; each
//! person can still turn them off for themselves.

use crate::project::Project;
use crate::settings::{self, Settings};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use ue_dev_hub_addon::protocol::API_VERSION;

pub const MANIFEST: &str = "addon.toml";

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub version: String,
    /// The hub API the addon is made for. Must match `API_VERSION` exactly.
    pub api: u32,
    /// The oldest hub version the addon works with.
    pub min_hub: Option<String>,
    #[serde(default)]
    pub pages: Vec<Page>,
    pub helper: Option<Helper>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub id: String,
    pub label: String,
    /// One of the hub's icon names, or one the addon registers.
    #[serde(default = "default_icon")]
    pub icon: String,
}

fn default_icon() -> String {
    "box".into()
}

/// The helper program for each platform, relative to the addon folder.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Helper {
    pub windows: Option<String>,
    pub linux: Option<String>,
}

/// Where an installed addon came from. Only official addons update from the index or install
/// by themselves.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    #[default]
    Official,
    Disk,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Record {
    pub source: Source,
    pub version: String,
}

/// "git", "level-tools": lowercase letters, digits and dashes, starting with a letter. Used in
/// paths, URLs and settings keys, so nothing else gets through.
pub fn is_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && !id.ends_with('-')
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// "windows-x86_64", "linux-x86_64" or "linux-arm64": the key for platform files in the index.
/// The same names the addons repository's build scripts use (Node's names for ARM).
pub fn platform() -> String {
    let os = if cfg!(windows) { "windows" } else { "linux" };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{arch}")
}

impl Manifest {
    /// The helper for this platform, relative to the addon folder.
    pub fn helper_path(&self) -> Option<&str> {
        let h = self.helper.as_ref()?;
        if cfg!(windows) { h.windows.as_deref() } else { h.linux.as_deref() }
    }

    /// Why this hub can't use the addon, if it can't.
    pub fn problem(&self, hub_version: &str) -> Option<String> {
        if self.api != API_VERSION {
            let which = if self.api > API_VERSION { "a newer" } else { "an older" };
            return Some(format!("{} {} is made for {which} UE Dev Hub.", self.name, self.version));
        }
        if let Some(min) = &self.min_hub
            && crate::config::parse_version(min) > crate::config::parse_version(hub_version)
        {
            return Some(format!("{} {} needs UE Dev Hub {min} or newer. Update the hub first.", self.name, self.version));
        }
        if self.helper.is_some() && self.helper_path().is_none() {
            return Some(format!("{} doesn't work on {}.", self.name, platform()));
        }
        None
    }
}

/// Reads and checks a manifest. `folder` is the folder the addon is in, which must have the
/// files it names.
pub fn read_manifest(folder: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(folder.join(MANIFEST)).map_err(|_| format!("There's no {MANIFEST} in {}.", folder.display()))?;
    let m: Manifest = toml::from_str(&text).map_err(|e| format!("{MANIFEST} couldn't be read: {}", e.message()))?;
    if !is_id(&m.id) {
        return Err(format!("'{}' isn't a valid addon id. Use lowercase letters, digits and dashes.", m.id));
    }
    if crate::config::parse_version(&m.version).is_none() {
        return Err(format!("'{}' isn't a version like 1.2.0.", m.version));
    }
    for p in &m.pages {
        if !is_id(&p.id) {
            return Err(format!("'{}' isn't a valid page id.", p.id));
        }
    }
    let paths = m.helper.iter().flat_map(|h| [h.windows.as_deref(), h.linux.as_deref()]).flatten();
    for p in paths {
        if !crate::config::is_inside(p) {
            return Err(format!("The helper path '{p}' must be inside the addon folder."));
        }
    }
    if !folder.join("ui").join("index.js").is_file() {
        return Err("The addon has no ui/index.js.".into());
    }
    if let Some(helper) = m.helper_path()
        && !folder.join(helper).is_file()
    {
        return Err(format!("The addon's helper {helper} is missing."));
    }
    Ok(m)
}

pub fn root(app: &AppHandle) -> Result<PathBuf, String> {
    root_for(app)
}

pub fn root_for<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?.join("addons");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create the addons folder: {e}"))?;
    Ok(dir)
}

#[derive(Clone, Debug)]
pub struct Installed {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// Every addon in `root` that can be read. Folders starting with "." are installs in progress
/// or left over.
pub fn installed_in(root: &Path) -> Vec<Installed> {
    let Ok(entries) = std::fs::read_dir(root) else { return vec![] };
    let mut list: Vec<Installed> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let folder = e.file_name().to_string_lossy().to_string();
            if folder.starts_with('.') {
                return None;
            }
            let manifest = read_manifest(&e.path()).ok().filter(|m| m.id == folder)?;
            Some(Installed { dir: e.path(), manifest })
        })
        .collect();
    list.sort_by_key(|a| a.manifest.name.to_lowercase());
    list
}

pub fn find(app: &AppHandle, id: &str) -> Result<Installed, String> {
    if !is_id(id) {
        return Err(format!("'{id}' isn't an addon."));
    }
    installed_in(&root(app)?).into_iter().find(|a| a.manifest.id == id).ok_or_else(|| format!("The addon '{id}' isn't installed."))
}

/// Whether an addon is on for a project: the person's own choice if they made one, otherwise
/// whether the project lists it as a default.
pub fn enabled_for(settings: &Settings, project: Option<&Project>, id: &str) -> bool {
    let Some(p) = project else { return false };
    settings.prefs(&p.root).addons.get(id).copied().unwrap_or_else(|| is_default(p, id))
}

pub fn is_default(project: &Project, id: &str) -> bool {
    project.config.addons.default.iter().any(|d| d == id)
}

/// Turns the old "Show the Git page" switch into the Git addon being on for every project.
pub fn migrate(settings: &mut Settings) {
    if !settings.git_page {
        return;
    }
    for root in settings.projects.clone() {
        settings.prefs_mut(&root).addons.entry("git".into()).or_insert(true);
    }
    settings.git_page = false;
}

/// What the window shows about an addon.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AddonInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub source: Source,
    /// On for the current project, and usable by this hub.
    pub enabled: bool,
    /// The person turned it on or off themselves, instead of following the project.
    pub chosen: bool,
    /// The project lists it in `[addons] default`.
    pub project_default: bool,
    pub pages: Vec<Page>,
    pub has_style: bool,
    /// Why it can't be used, like "made for a newer UE Dev Hub".
    pub problem: Option<String>,
}

pub fn infos(app: &AppHandle, project: Option<&Project>) -> Result<Vec<AddonInfo>, String> {
    let s = settings::load(app);
    let version = app.package_info().version.to_string();
    Ok(installed_in(&root(app)?)
        .into_iter()
        .map(|a| {
            let m = a.manifest;
            let problem = m.problem(&version);
            let chosen = project.is_some_and(|p| s.prefs(&p.root).addons.contains_key(&m.id));
            AddonInfo {
                enabled: problem.is_none() && enabled_for(&s, project, &m.id),
                chosen,
                project_default: project.is_some_and(|p| is_default(p, &m.id)),
                source: s.addons.get(&m.id).map(|r| r.source).unwrap_or(Source::Disk),
                has_style: a.dir.join("ui").join("style.css").is_file(),
                id: m.id,
                name: m.name,
                description: m.description,
                version: m.version,
                pages: m.pages,
                problem,
            }
        })
        .collect())
}

/// Installs from a zip or a folder. Replaces an installed version of the same addon, after
/// stopping its helper. Returns the new manifest.
/// One install or removal at a time, so two can't swap the same folder.
static CHANGING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// What an install must be. The default takes any addon that isn't older than the installed one.
#[derive(Default, Clone, Copy, Debug)]
pub struct Expect<'a> {
    /// The id and version the official index promised for the download.
    pub id: Option<&'a str>,
    pub version: Option<&'a str>,
    /// Allow replacing a newer installed version with an older one.
    pub downgrade: bool,
}

/// Why an install didn't happen. The UI offers installing anyway for `NewerInstalled`.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum InstallError {
    /// A newer version is installed, and the install didn't allow going back.
    NewerInstalled { message: String, installed: String },
    Failed { message: String },
}

impl From<String> for InstallError {
    fn from(message: String) -> Self {
        InstallError::Failed { message }
    }
}

impl From<&str> for InstallError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::NewerInstalled { message, .. } | InstallError::Failed { message } => f.write_str(message),
        }
    }
}

pub fn install(app: &AppHandle, from: &Path, source: Source, expect: Expect) -> Result<Manifest, InstallError> {
    let _one = CHANGING.lock().unwrap_or_else(|e| e.into_inner());
    let root = root(app)?;
    let manifest = install_into(&root, from, &app.package_info().version.to_string(), expect)?;
    settings::update(app, |s| {
        s.addons.insert(manifest.id.clone(), Record { source, version: manifest.version.clone() });
    })?;
    Ok(manifest)
}

/// The file work of `install`, without the settings, so tests can run it.
pub fn install_into(root: &Path, from: &Path, hub_version: &str, expect: Expect) -> Result<Manifest, InstallError> {
    cleanup(root);
    let staging = root.join(format!(".incoming-{}-{}", std::process::id(), crate::jobs::now_ms()));
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let result = (|| -> Result<Manifest, InstallError> {
        if from.is_dir() {
            copy_dir(from, &staging)?;
        } else if from.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
            crate::proc::unzip(from, &staging)?;
        } else {
            return Err("Choose an addon's .zip file or its folder.".into());
        }
        let top = addon_top(&staging)?;
        let manifest = read_manifest(&top)?;
        if let Some(problem) = manifest.problem(hub_version) {
            return Err(problem.into());
        }
        if let Some(id) = expect.id.filter(|id| *id != manifest.id) {
            return Err(format!("The download for '{id}' contains '{}' instead. Nothing was installed.", manifest.id).into());
        }
        if let Some(v) = expect.version.filter(|v| crate::config::parse_version(v) != crate::config::parse_version(&manifest.version)) {
            return Err(format!("The download for {} {v} contains version {} instead. Nothing was installed.", manifest.name, manifest.version).into());
        }
        let dest = root.join(&manifest.id);
        if !expect.downgrade
            && let Ok(old) = read_manifest(&dest)
            && crate::config::parse_version(&old.version) > crate::config::parse_version(&manifest.version)
        {
            return Err(InstallError::NewerInstalled {
                message: format!("{} {} is installed, which is newer than {}. Nothing was installed.", old.name, old.version, manifest.version),
                installed: old.version,
            });
        }
        make_runnable(&top, &manifest)?;
        let _stopped = crate::addon_host::stop_when_idle(&manifest.name, &manifest.id)?;
        let old = root.join(format!(".old-{}-{}", manifest.id, crate::jobs::now_ms()));
        if dest.exists() {
            std::fs::rename(&dest, &old).map_err(|e| format!("Couldn't replace the installed {}: {e}. Close anything that uses its files and try again.", manifest.name))?;
        }
        if let Err(e) = std::fs::rename(&top, &dest) {
            let _ = std::fs::rename(&old, &dest);
            return Err(format!("Couldn't put {} in place: {e}", manifest.name).into());
        }
        let _ = std::fs::remove_dir_all(&old);
        Ok(manifest)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Zips made on Windows don't keep the Unix "executable" permission, so the helper gets it here.
#[cfg(unix)]
fn make_runnable(top: &Path, manifest: &Manifest) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let Some(helper) = manifest.helper_path() else { return Ok(()) };
    std::fs::set_permissions(top.join(helper), std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("Couldn't make the helper {helper} runnable: {e}"))
}

#[cfg(not(unix))]
fn make_runnable(_top: &Path, _manifest: &Manifest) -> Result<(), String> {
    Ok(())
}

/// The folder with addon.toml: the top of the zip, or its only folder.
fn addon_top(staging: &Path) -> Result<PathBuf, String> {
    if staging.join(MANIFEST).is_file() {
        return Ok(staging.to_path_buf());
    }
    let dirs: Vec<PathBuf> = std::fs::read_dir(staging).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    match dirs.as_slice() {
        [only] if only.join(MANIFEST).is_file() => Ok(only.clone()),
        _ => Err(format!("This isn't an addon: there's no {MANIFEST} in it.")),
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(from).map_err(|e| format!("Couldn't read {}: {e}", from.display()))?.flatten() {
        let kind = e.file_type().map_err(|e| e.to_string())?;
        let target = to.join(e.file_name());
        if kind.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(e.path(), &target).map_err(|err| format!("Couldn't copy {}: {err}", e.path().display()))?;
        }
        // Links are left out, so nothing in the addon points outside its folder.
    }
    Ok(())
}

/// Removes leftovers of installs that were cut off, or old versions that were still in use.
fn cleanup(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with(".incoming-") || name.starts_with(".old-") {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

pub fn uninstall(app: &AppHandle, id: &str) -> Result<(), String> {
    let _one = CHANGING.lock().unwrap_or_else(|e| e.into_inner());
    let addon = find(app, id)?;
    let _stopped = crate::addon_host::stop_when_idle(&addon.manifest.name, id)?;
    let old = addon.dir.with_file_name(format!(".old-{id}-{}", crate::jobs::now_ms()));
    std::fs::rename(&addon.dir, &old).map_err(|e| format!("Couldn't remove {}: {e}. Close anything that uses its files and try again.", addon.manifest.name))?;
    let _ = std::fs::remove_dir_all(&old);
    settings::update(app, |s| {
        s.addons.remove(id);
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ue-dev-hub-addons-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_addon(dir: &Path, toml: &str) {
        std::fs::create_dir_all(dir.join("ui")).unwrap();
        std::fs::write(dir.join(MANIFEST), toml).unwrap();
        std::fs::write(dir.join("ui").join("index.js"), "export default () => ({ pages: {} });").unwrap();
    }

    const DEMO: &str = "id = \"demo\"\nname = \"Demo\"\nversion = \"1.0.0\"\napi = 1\n[[pages]]\nid = \"main\"\nlabel = \"Demo\"\n";

    #[test]
    fn ids_are_plain_names() {
        assert!(is_id("git") && is_id("level-tools2"));
        for bad in ["", "Git", "-git", "git-", "../git", "git/x", "1git", "g it", &"a".repeat(41)] {
            assert!(!is_id(bad), "{bad}");
        }
    }

    #[test]
    fn refuses_addons_for_another_hub() {
        let m: Manifest = toml::from_str(&DEMO.replace("api = 1", "api = 2")).unwrap();
        assert!(m.problem("1.0.0").unwrap().contains("newer"));
        let m: Manifest = toml::from_str(&DEMO.replace("api = 1", "api = 1\nmin_hub = \"9.0.0\"")).unwrap();
        assert!(m.problem("1.0.0").unwrap().contains("9.0.0"));
        let m: Manifest = toml::from_str(DEMO).unwrap();
        assert_eq!(m.problem("1.0.0"), None);
    }

    #[test]
    fn manifest_paths_stay_inside_the_addon() {
        let d = temp("paths");
        write_addon(&d, &format!("{DEMO}[helper]\nwindows = \"../evil.exe\"\nlinux = \"../evil\"\n"));
        assert!(read_manifest(&d).unwrap_err().contains("inside"));
        write_addon(&d, &DEMO.replace("id = \"demo\"", "id = \"../demo\""));
        assert!(read_manifest(&d).unwrap_err().contains("valid addon id"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn installs_from_a_folder_and_replaces_the_old_version() {
        let root = temp("install-root");
        let src = temp("install-src").join("demo-addon");
        write_addon(&src, DEMO);
        let m = install_into(&root, &src, "1.0.0", Expect::default()).unwrap();
        assert_eq!(m.id, "demo");
        assert!(root.join("demo").join("ui").join("index.js").is_file());

        write_addon(&src, &DEMO.replace("1.0.0", "1.1.0"));
        install_into(&root, &src, "1.0.0", Expect::default()).unwrap();
        let list = installed_in(&root);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].manifest.version, "1.1.0");
        assert!(std::fs::read_dir(&root).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with('.')));

        // An addon for another hub is refused and leaves the installed one alone.
        write_addon(&src, &DEMO.replace("api = 1", "api = 2"));
        assert!(install_into(&root, &src, "1.0.0", Expect::default()).is_err());
        assert_eq!(installed_in(&root)[0].manifest.version, "1.1.0");

        // An older version only replaces a newer one when asked to.
        write_addon(&src, DEMO);
        let err = install_into(&root, &src, "1.0.0", Expect::default()).unwrap_err();
        assert!(matches!(&err, InstallError::NewerInstalled { installed, .. } if installed == "1.1.0"), "{err}");
        assert_eq!(serde_json::to_value(&err).unwrap()["kind"], "newerInstalled");
        assert_eq!(installed_in(&root)[0].manifest.version, "1.1.0");
        install_into(&root, &src, "1.0.0", Expect { downgrade: true, ..Expect::default() }).unwrap();
        assert_eq!(installed_in(&root)[0].manifest.version, "1.0.0");

        // A download must be the addon and version the index promised, checked before anything
        // installed is touched.
        write_addon(&src, &DEMO.replace("1.0.0", "2.0.0"));
        let other = Expect { id: Some("other"), version: Some("2.0.0"), downgrade: false };
        assert!(install_into(&root, &src, "1.0.0", other).unwrap_err().to_string().contains("'other'"));
        let wrong_version = Expect { id: Some("demo"), version: Some("2.1.0"), downgrade: false };
        assert!(install_into(&root, &src, "1.0.0", wrong_version).unwrap_err().to_string().contains("2.1.0"));
        assert_eq!(installed_in(&root)[0].manifest.version, "1.0.0");
        install_into(&root, &src, "1.0.0", Expect { id: Some("demo"), version: Some("2.0.0"), downgrade: false }).unwrap();
        assert_eq!(installed_in(&root)[0].manifest.version, "2.0.0");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(src.parent().unwrap());
    }

    #[test]
    fn uses_the_addons_repositorys_platform_names() {
        let p = platform();
        assert!(["windows-x86_64", "linux-x86_64", "linux-arm64", "windows-arm64"].contains(&p.as_str()), "{p}");
    }

    #[cfg(unix)]
    #[test]
    fn makes_the_helper_runnable() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp("exec-root");
        let src = temp("exec-src").join("demo-addon");
        // Its own id: installing takes a lock per addon id, which another test holds for "demo".
        let manifest = DEMO.replace("id = \"demo\"", "id = \"demo-exec\"");
        write_addon(&src, &format!("{manifest}[helper]\nlinux = \"bin/helper\"\n"));
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("bin/helper"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(src.join("bin/helper"), std::fs::Permissions::from_mode(0o644)).unwrap();
        install_into(&root, &src, "1.0.0", Expect::default()).unwrap();
        let mode = std::fs::metadata(root.join("demo-exec/bin/helper")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(src.parent().unwrap());
    }

    #[test]
    fn personal_choice_wins_over_the_project_default() {
        let root = temp("enabled");
        std::fs::write(root.join("Demo.uproject"), r#"{"EngineAssociation":"5.8"}"#).unwrap();
        std::fs::write(root.join("ue-dev-hub.toml"), "[addons]\ndefault = [\"git\"]\n").unwrap();
        let p = Project::load(&root).unwrap();
        let mut s = Settings::default();
        assert!(enabled_for(&s, Some(&p), "git"));
        assert!(!enabled_for(&s, Some(&p), "other"));
        assert!(!enabled_for(&s, None, "git"));
        s.prefs_mut(&p.root).addons.insert("git".into(), false);
        s.prefs_mut(&p.root).addons.insert("other".into(), true);
        assert!(!enabled_for(&s, Some(&p), "git"));
        assert!(enabled_for(&s, Some(&p), "other"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_old_git_switch_turns_on_the_git_addon() {
        let mut s = Settings::default();
        s.open(PathBuf::from("C:/A"));
        s.open(PathBuf::from("C:/B"));
        let (b, a) = (s.projects[0].clone(), s.projects[1].clone());
        s.prefs_mut(&b).addons.insert("git".into(), false);
        s.git_page = true;
        migrate(&mut s);
        assert!(!s.git_page);
        assert_eq!(s.prefs(&a).addons.get("git"), Some(&true));
        // A choice made since then stays.
        assert_eq!(s.prefs(&b).addons.get("git"), Some(&false));
    }
}
