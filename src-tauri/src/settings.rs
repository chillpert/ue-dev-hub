//! Personal hub settings, stored as JSON in the app config directory. Team settings live in
//! each project's `ue-dev-hub.toml` instead.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    /// Every project in the swapper, most recently opened first.
    pub projects: Vec<PathBuf>,
    pub selected: Option<PathBuf>,
    /// Engine folders picked by hand, keyed by engine version (or source build id).
    pub engine_paths: HashMap<String, PathBuf>,
    /// Personal choices per project, keyed by the project root.
    pub prefs: HashMap<String, Prefs>,
    /// Start Update & Launch by itself when the hub opens a project, after a short
    /// countdown that can be cancelled. Off when not set.
    pub auto_launch: Option<bool>,
    /// What the hub does once the editor it started is running: "keep", "minimize" or
    /// "close". Keep when not set.
    pub after_launch: Option<String>,
    /// "Don't ask again" on the first-run "Install UE Dev Hub?" question.
    pub never_offer_install: bool,
    /// The old "Show the Git page" switch. Read once to turn on the Git addon, then cleared.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub git_page: bool,
    /// Installed addons by id: where each came from. The files are in the addons folder.
    pub addons: HashMap<String, crate::addons::Record>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    /// Home widgets in order. The project's default when not set.
    pub widgets: Option<Vec<String>>,
    /// "rider", "visualStudio", "vscode" or "zed".
    pub ide: Option<String>,
    /// "gitBash", "windowsTerminal", "powershell" or "cmd".
    pub terminal: Option<String>,
    /// Long package names, most recent first.
    pub recent_maps: Vec<String>,
    pub last_build_type: Option<String>,
    /// Addons turned on (true) or off (false) by hand for this project, by id. Others follow
    /// the project's `[addons] default`.
    pub addons: HashMap<String, bool>,
    /// Whether to use the project's Git hooks folder: yes, no, or not asked yet.
    pub git_hooks: Option<bool>,
    /// The core.hooksPath the hub set itself. It never changes a value it didn't set.
    pub git_hooks_set: Option<String>,
}

pub fn key(root: &Path) -> String {
    root.to_string_lossy().to_string()
}

impl Settings {
    /// The key `root`'s prefs are stored under: the existing one if it's the same folder written
    /// differently (like another capitalisation on Windows), so its prefs aren't orphaned.
    fn prefs_key(&self, root: &Path) -> String {
        let exact = key(root);
        if self.prefs.contains_key(&exact) {
            return exact;
        }
        self.prefs.keys().find(|k| same_path(Path::new(k), root)).cloned().unwrap_or(exact)
    }

    pub fn prefs(&self, root: &Path) -> Prefs {
        self.prefs.get(&self.prefs_key(root)).cloned().unwrap_or_default()
    }

    pub fn prefs_mut(&mut self, root: &Path) -> &mut Prefs {
        let key = self.prefs_key(root);
        self.prefs.entry(key).or_default()
    }

    /// Adds `root` to the front of the list and selects it. A project that's already in the list
    /// keeps the spelling it was stored with, so the prefs keyed by it still match.
    pub fn open(&mut self, root: PathBuf) {
        let root: PathBuf = root.components().collect();
        let root = self.projects.iter().find(|p| same_path(p, &root)).cloned().unwrap_or(root);
        self.projects.retain(|p| !same_path(p, &root));
        self.projects.insert(0, root.clone());
        self.selected = Some(root);
    }
}

/// Whether two paths are the same folder. Only Windows ignores case; Linux folders that differ
/// in case are different folders.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let clean: PathBuf = p.components().collect();
        let s = clean.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_string();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

static LOCK: Mutex<()> = Mutex::new(());

fn file(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("settings.json"))
}

/// The settings. If the file can't be opened right now (another program has it locked, or no
/// permission), the backup is used, without changing anything.
pub fn load(app: &AppHandle) -> Settings {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    file(app).map(|f| load_file(&f)).unwrap_or_default()
}

fn load_file(path: &Path) -> Settings {
    read_file(path).unwrap_or_else(|_| match read_text(&path.with_extension("json.bak")) {
        Ok(Some(t)) => serde_json::from_str(&t).unwrap_or_default(),
        _ => Settings::default(),
    })
}

/// Loads, changes and saves the settings as one step, so parallel commands don't lose changes.
/// Fails without writing anything if the current file can't be read, so it's never replaced
/// by defaults.
pub fn update<T>(app: &AppHandle, f: impl FnOnce(&mut Settings) -> T) -> Result<T, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = file(app).ok_or("Could not find the settings folder.")?;
    let mut s = read_file(&path)?;
    let result = f(&mut s);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?;
    write_file(&path, &json)?;
    Ok(result)
}

/// The file's text, None if it doesn't exist. Text that isn't UTF-8 comes back lossily, so it
/// counts as broken rather than unreadable. A file another program holds for a moment (like a
/// virus scanner) is tried again a few times.
fn read_text(path: &Path) -> std::io::Result<Option<String>> {
    let mut tries = 0;
    loop {
        match std::fs::read(path) {
            Ok(bytes) => return Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) if tries < 3 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Reads the settings. A missing file means no settings yet. A file with broken contents is kept
/// as settings.json.broken, and the backup is used. A file that can't be opened is an error, so
/// nobody writes defaults over it.
fn read_file(path: &Path) -> Result<Settings, String> {
    let text = read_text(path).map_err(|e| format!("Couldn't read the hub's settings ({}): {e}", path.display()))?;
    let Some(text) = text else { return Ok(Settings::default()) };
    match serde_json::from_str::<Settings>(&text) {
        Ok(s) => Ok(s),
        Err(_) => {
            let _ = std::fs::copy(path, path.with_extension("json.broken"));
            Ok(match read_text(&path.with_extension("json.bak")) {
                Ok(Some(t)) => serde_json::from_str(&t).unwrap_or_default(),
                _ => Settings::default(),
            })
        }
    }
}

/// Writes to a temp file and renames it over the old one, so a crash mid-write can't leave a
/// half-written file. The previous version is kept as settings.json.bak.
fn write_file(path: &Path, json: &str) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("Couldn't save the settings: {e}"))?;
    // Only a valid file becomes the backup; never replace a good backup with a broken file.
    let current_is_valid = std::fs::read_to_string(path).ok().is_some_and(|t| serde_json::from_str::<Settings>(&t).is_ok());
    if current_is_valid {
        let _ = std::fs::copy(path, path.with_extension("json.bak"));
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("Couldn't save the settings: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survives_a_broken_settings_file() {
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        let mut s = Settings::default();
        s.open(PathBuf::from("C:/Projects/Game"));
        write_file(&file, &serde_json::to_string(&s).unwrap()).unwrap();
        s.open(PathBuf::from("C:/Projects/Other"));
        write_file(&file, &serde_json::to_string(&s).unwrap()).unwrap();

        std::fs::write(&file, "{ broken").unwrap();
        let read = read_file(&file).unwrap();
        // The backup has the previous save, and the broken file is kept for tech.
        assert_eq!(read.projects, vec![PathBuf::from("C:/Projects/Game")]);
        assert!(dir.join("settings.json.broken").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_settings_file_that_cant_be_opened_is_never_replaced() {
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-settings-locked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A folder where the file should be can't be read as a file, like a locked file.
        let file = dir.join("settings.json");
        std::fs::create_dir_all(&file).unwrap();
        assert!(read_file(&file).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopening_with_other_capitalisation_keeps_the_prefs() {
        let mut s = Settings::default();
        s.open(PathBuf::from("C:/Projects/Game"));
        s.prefs_mut(Path::new("C:/Projects/Game")).ide = Some("rider".into());
        let other = PathBuf::from("c:/projects/game");
        s.open(other.clone());
        if cfg!(windows) {
            assert_eq!(s.projects, vec![PathBuf::from("C:/Projects/Game")]);
            assert_eq!(s.prefs(&other).ide.as_deref(), Some("rider"));
            s.prefs_mut(&other).terminal = Some("cmd".into());
            assert_eq!(s.prefs.len(), 1);
        } else {
            // Different folders on Linux.
            assert_eq!(s.projects.len(), 2);
            assert!(s.prefs(&other).ide.is_none());
        }
    }
}
