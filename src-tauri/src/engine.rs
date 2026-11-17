//! Finding the Unreal Engine installation a project needs.

use crate::project::Project;
use crate::settings::Settings;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::enum_variant_names)]
pub enum Source {
    /// Picked by hand in the hub.
    Settings,
    /// `HKLM\SOFTWARE\EpicGames\Unreal Engine\<version>`, what the Epic Games Launcher writes.
    Registry,
    /// A source build registered under `HKCU\Software\Epic Games\Unreal Engine\Builds`.
    SourceBuild,
    /// The Epic Games Launcher's own install list.
    LauncherManifest,
    /// Found by looking in the usual install folders.
    Search,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Engine {
    pub path: PathBuf,
    pub source: Source,
}

/// The .NET SDK an engine ships with. UnrealBuildTool and RunUAT run on it, so nobody needs to
/// install .NET themselves.
#[derive(Debug, Clone)]
pub struct DotNet {
    /// Like "10.0".
    pub version: String,
    /// The folder with dotnet.exe.
    pub dir: PathBuf,
}

impl Engine {
    pub fn editor(&self) -> PathBuf {
        editor_path(&self.path)
    }

    /// The console editor that commandlets run in.
    pub fn editor_cmd(&self) -> PathBuf {
        if cfg!(windows) {
            self.path.join("Engine/Binaries/Win64/UnrealEditor-Cmd.exe")
        } else {
            self.path.join("Engine/Binaries/Linux/UnrealEditor")
        }
    }

    pub fn uat(&self) -> PathBuf {
        if cfg!(windows) {
            self.path.join("Engine/Build/BatchFiles/RunUAT.bat")
        } else {
            self.path.join("Engine/Build/BatchFiles/RunUAT.sh")
        }
    }

    /// Engines built from source can build Test and Debug targets; the Epic launcher's can't.
    pub fn is_source_build(&self) -> bool {
        self.source == Source::SourceBuild || !self.path.join("Engine/Build/InstalledBuild.txt").is_file()
    }

    /// The engine's bundled .NET SDK, found the way `GetDotnetPath.bat` finds it.
    pub fn bundled_dotnet(&self) -> Option<DotNet> {
        let base = self.path.join("Engine/Binaries/ThirdParty/DotNet");
        let arch = if cfg!(windows) {
            if std::env::var("PROCESSOR_ARCHITECTURE").is_ok_and(|a| a == "ARM64") { "win-arm64" } else { "win-x64" }
        } else if cfg!(target_arch = "aarch64") {
            "linux-arm64"
        } else {
            "linux-x64"
        };
        let exe = if cfg!(windows) { "dotnet.exe" } else { "dotnet" };
        // GetDotnetPath.bat names the exact version the engine wants; fall back to the newest folder.
        let wanted = std::fs::read_to_string(self.path.join("Engine/Build/BatchFiles/GetDotnetPath.bat"))
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.trim().strip_prefix("set UE_DOTNET_VERSION=").map(|v| v.trim().to_string()))
            });
        let mut versions: Vec<String> = std::fs::read_dir(&base)
            .ok()?
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        versions.sort_by_key(|v| crate::config::parse_version(v));
        let version = wanted.filter(|w| versions.contains(w)).or_else(|| versions.pop())?;
        let dir = base.join(&version).join(arch);
        dir.join(exe).is_file().then_some(DotNet { version, dir })
    }

    /// Makes `cmd` use the engine's bundled .NET, like Epic's batch files do. Without this,
    /// UnrealBuildTool.exe looks for a system-wide .NET of exactly its version and fails if
    /// there is none. `UE_USE_SYSTEM_DOTNET=1` keeps the system .NET, as in Epic's scripts.
    pub fn use_bundled_dotnet(&self, cmd: &mut std::process::Command) {
        if std::env::var("UE_USE_SYSTEM_DOTNET").is_ok_and(|v| v == "1") {
            return;
        }
        let Some(dn) = self.bundled_dotnet() else { return };
        // Built on the PATH a started program gets, so an AppImage's own folders stay out of it.
        let path = crate::proc::child_var("PATH").unwrap_or_default();
        let mut paths = vec![dn.dir.clone()];
        paths.extend(std::env::split_paths(&path));
        if let Ok(joined) = std::env::join_paths(paths) {
            cmd.env("PATH", joined);
        }
        cmd.env("DOTNET_ROOT", &dn.dir)
            .env("DOTNET_MULTILEVEL_LOOKUP", "0")
            .env("DOTNET_ROLL_FORWARD", "LatestMajor")
            .env("UE_DOTNET_VERSION", &dn.version);
    }

    pub fn ubt(&self) -> PathBuf {
        if cfg!(windows) {
            self.path.join("Engine/Binaries/DotNET/UnrealBuildTool/UnrealBuildTool.exe")
        } else {
            self.path.join("Engine/Build/BatchFiles/Linux/Build.sh")
        }
    }
}

pub fn platform() -> &'static str {
    if cfg!(windows) { "Win64" } else { "Linux" }
}

fn editor_path(engine: &Path) -> PathBuf {
    if cfg!(windows) {
        engine.join("Engine/Binaries/Win64/UnrealEditor.exe")
    } else {
        engine.join("Engine/Binaries/Linux/UnrealEditor")
    }
}

/// True if `path` is an engine root, i.e. the folder that contains `Engine/`.
pub fn is_engine_root(path: &Path) -> bool {
    editor_path(path).is_file()
}

fn is_source_build_id(version: &str) -> bool {
    version.starts_with('{')
}

pub fn locate(project: &Project, settings: &Settings) -> Option<Engine> {
    let version = project.engine_version.as_str();
    let registry = registry_path(version).filter(|p| is_engine_root(p));
    let found = |path: PathBuf, source| Some(Engine { path, source });

    if let Some(p) = settings.engine_paths.get(version).filter(|p| is_engine_root(p)) {
        return found(p.clone(), Source::Settings);
    }
    if let Some(p) = registry {
        let source = if is_source_build_id(version) { Source::SourceBuild } else { Source::Registry };
        return found(p, source);
    }
    if is_source_build_id(version) {
        return None;
    }
    if let Some(p) = launcher_manifest_path(version).filter(|p| is_engine_root(p)) {
        return found(p, Source::LauncherManifest);
    }
    if let Some(p) = search_common_folders(version) {
        return found(p, Source::Search);
    }
    if let Some(p) = registered_engine_with_version(version) {
        return found(p, Source::Search);
    }
    None
}

#[cfg(windows)]
fn registry_path(version: &str) -> Option<PathBuf> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let path: String = if is_source_build_id(version) {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Epic Games\Unreal Engine\Builds")
            .ok()?
            .get_value(version)
            .ok()?
    } else {
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(format!(r"SOFTWARE\EpicGames\Unreal Engine\{version}"))
            .ok()?
            .get_value("InstalledDirectory")
            .ok()?
    };
    Some(PathBuf::from(path))
}

/// On Linux, Unreal records engines in `~/.config/Epic/UnrealEngine/Install.ini` under
/// `[Installations]`, keyed by a source build's {GUID} or a custom name.
#[cfg(not(windows))]
fn registry_path(version: &str) -> Option<PathBuf> {
    install_ini_entries(&read_install_ini()?)
        .into_iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(version))
        .map(|(_, path)| PathBuf::from(path))
}

#[cfg(not(windows))]
fn read_install_ini() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    std::fs::read_to_string(PathBuf::from(home).join(".config/Epic/UnrealEngine/Install.ini")).ok()
}

/// The `key=path` lines of an Install.ini's `[Installations]` section.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn install_ini_entries(ini: &str) -> Vec<(String, String)> {
    let mut in_section = false;
    let mut out = vec![];
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line.eq_ignore_ascii_case("[Installations]");
        } else if in_section
            && !line.starts_with(';')
            && let Some((key, value)) = line.split_once('=')
        {
            let value = value.trim().trim_matches('"');
            if !key.trim().is_empty() && !value.is_empty() {
                out.push((key.trim().to_string(), value.to_string()));
            }
        }
    }
    out
}

/// A registered engine (a source build under its {GUID} or another name) whose
/// `Engine/Build/Build.version` says it is `version`. Linux only: there is no Epic Games Launcher,
/// so source builds and Epic's Linux download are the usual engines.
#[cfg(not(windows))]
fn registered_engine_with_version(version: &str) -> Option<PathBuf> {
    install_ini_entries(&read_install_ini()?)
        .into_iter()
        .map(|(_, path)| PathBuf::from(path))
        .find(|p| is_engine_root(p) && engine_has_version(p, version))
}

#[cfg(windows)]
fn registered_engine_with_version(_version: &str) -> Option<PathBuf> {
    None
}

#[cfg(not(windows))]
fn engine_has_version(engine: &Path, version: &str) -> bool {
    std::fs::read_to_string(engine.join("Engine/Build/Build.version"))
        .ok()
        .and_then(|text| build_version(&text))
        .zip(crate::config::parse_version(version))
        .is_some_and(|((major, minor), (want_major, want_minor, _))| major == want_major && minor == want_minor)
}

/// Major and minor version from an engine's `Build.version` file.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn build_version(json: &str) -> Option<(u64, u64)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some((v.get("MajorVersion")?.as_u64()?, v.get("MinorVersion")?.as_u64()?))
}

/// Whether an engine folder called `name` is likely `version`: `UE_5.8`, or how Epic's Linux
/// download unpacks, `Linux_Unreal_Engine_5.8.1`.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn engine_folder_matches(name: &str, version: &str) -> bool {
    if name == format!("UE_{version}") {
        return true;
    }
    name.strip_prefix("Linux_Unreal_Engine_")
        .is_some_and(|rest| rest == version || rest.strip_prefix(version).is_some_and(|patch| patch.starts_with('.')))
}

/// The Epic Games Launcher keeps a list of everything it installed in `LauncherInstalled.dat`.
fn launcher_manifest_path(version: &str) -> Option<PathBuf> {
    let program_data = std::env::var_os("ProgramData")?;
    let file = PathBuf::from(program_data).join(r"Epic\UnrealEngineLauncher\LauncherInstalled.dat");
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    let app_name = format!("UE_{version}");
    json.get("InstallationList")?
        .as_array()?
        .iter()
        .find(|e| e.get("AppName").and_then(|n| n.as_str()) == Some(app_name.as_str()))
        .and_then(|e| e.get("InstallLocation")?.as_str())
        .map(PathBuf::from)
}

fn search_common_folders(version: &str) -> Option<PathBuf> {
    let folder = format!("UE_{version}");
    let mut candidates = Vec::new();

    if cfg!(windows) {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            candidates.push(PathBuf::from(pf).join("Epic Games").join(&folder));
        }
        // People often install to another drive, e.g. E:\Epic Games\UE_5.8 or E:\Epic\UE_5.8.
        // Only local drives: looking at a disconnected network drive can take seconds.
        for letter in 'C'..='Z' {
            let root = format!("{letter}:\\");
            if !is_local_drive(&root) {
                continue;
            }
            let drive = PathBuf::from(root);
            for parent in ["Program Files\\Epic Games", "Epic Games", "Epic", "EpicGames", "Unreal", ""] {
                candidates.push(drive.join(parent).join(&folder));
            }
        }
    } else {
        // Linux engines are built from source or unpacked from Epic's download, into a folder of
        // the person's choosing.
        let mut parents = vec![];
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            for parent in ["", "Epic", "Epic Games", "Unreal", "UnrealEngine", "Engines", "dev", "src", "Repos", "repos", "Downloads", "Applications", "Games"] {
                parents.push(home.join(parent));
            }
            candidates.push(home.join("UnrealEngine"));
        }
        parents.push(PathBuf::from("/opt"));
        candidates.push(PathBuf::from("/opt/UnrealEngine"));
        let mut matching = vec![];
        for parent in parents {
            let Ok(entries) = std::fs::read_dir(&parent) else { continue };
            matching.extend(entries.flatten().filter(|e| engine_folder_matches(&e.file_name().to_string_lossy(), version)).map(|e| e.path()));
        }
        // The newest patch first.
        matching.sort_by_key(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            std::cmp::Reverse(crate::config::parse_version(name.rsplit('_').next().unwrap_or("")))
        });
        candidates.splice(0..0, matching);
    }

    candidates.into_iter().find(|p| is_engine_root(p))
}

/// A fixed or removable drive. Network drives and unused drive letters are skipped without
/// touching them.
#[cfg(windows)]
fn is_local_drive(root: &str) -> bool {
    unsafe extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
    let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
    kind == DRIVE_FIXED || kind == DRIVE_REMOVABLE
}

#[cfg(not(windows))]
fn is_local_drive(_root: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_version_the_engine_asks_for() {
        let root = std::env::temp_dir().join(format!("ue-dev-hub-engine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let arch = if cfg!(windows) { "win-x64" } else { "linux-x64" };
        let exe = if cfg!(windows) { "dotnet.exe" } else { "dotnet" };
        for v in ["8.0", "10.0"] {
            let d = root.join("Engine/Binaries/ThirdParty/DotNet").join(v).join(arch);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(exe), b"").unwrap();
        }
        let bat = root.join("Engine/Build/BatchFiles");
        std::fs::create_dir_all(&bat).unwrap();
        std::fs::write(bat.join("GetDotnetPath.bat"), "rem\r\nset UE_DOTNET_VERSION=8.0\r\n").unwrap();

        let e = Engine { path: root.clone(), source: Source::Settings };
        assert_eq!(e.bundled_dotnet().unwrap().version, "8.0");
        // Without the script, the newest folder wins.
        std::fs::remove_file(bat.join("GetDotnetPath.bat")).unwrap();
        assert_eq!(e.bundled_dotnet().unwrap().version, "10.0");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_install_ini() {
        let ini = "[Installations]\r\n{1A2B-3C}=/home/a/UnrealEngine\r\n; old=/nowhere\r\nMy 5.8 = \"/opt/UE 5.8\"\r\nempty=\r\n\r\n[Other]\r\nx=/y\r\n";
        assert_eq!(
            install_ini_entries(ini),
            vec![("{1A2B-3C}".to_string(), "/home/a/UnrealEngine".to_string()), ("My 5.8".to_string(), "/opt/UE 5.8".to_string())]
        );
        assert!(install_ini_entries("x=/y").is_empty());
    }

    #[test]
    fn reads_the_engine_version() {
        let json = r#"{ "MajorVersion": 5, "MinorVersion": 8, "PatchVersion": 1, "Changelist": 0, "BranchName": "++UE5+Release-5.8" }"#;
        assert_eq!(build_version(json), Some((5, 8)));
        assert_eq!(build_version("{}"), None);
        assert_eq!(build_version("not json"), None);
    }

    #[test]
    fn recognizes_engine_folders() {
        assert!(engine_folder_matches("UE_5.8", "5.8"));
        assert!(engine_folder_matches("Linux_Unreal_Engine_5.8", "5.8"));
        assert!(engine_folder_matches("Linux_Unreal_Engine_5.8.1", "5.8"));
        assert!(!engine_folder_matches("Linux_Unreal_Engine_5.80", "5.8"));
        assert!(!engine_folder_matches("Linux_Unreal_Engine_5.7.4", "5.8"));
        assert!(!engine_folder_matches("UE_5.8.1", "5.8"));
    }
}
