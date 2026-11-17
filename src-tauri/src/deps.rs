//! External dependencies: zips downloaded with rclone and extracted into the project.
//! `Saved/UEDevHub/dependencies.txt` records which version of each is installed.

use crate::config::Dependency;
use crate::log::Log;
use crate::proc;
use crate::project::Project;
use serde::Serialize;
use std::path::{Path, PathBuf};

const RCLONE_VERSION: &str = "v1.68.2";
/// rclone's download per platform, and its SHA-256 from https://downloads.rclone.org/v1.68.2/SHA256SUMS
fn rclone_build() -> Option<(&'static str, &'static str)> {
    if cfg!(windows) {
        Some(("windows-amd64", "812bf76cc02c04cf6327f3683f3d5a88e47d36c39db84c1a745777496be7d993"))
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(("linux-amd64", "0e6fa18051e67fc600d803a2dcb10ddedb092247fc6eee61be97f64ec080a13c"))
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(("linux-arm64", "c6e9d4cf9c88b279f6ad80cd5675daebc068e404890fa7e191412c1bc7a4ac5f"))
    } else {
        None
    }
}

/// Where the hub keeps its own tools: never inside a project.
fn tools_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("UE Dev Hub Tools"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|d| d.join("ue-dev-hub-tools"))
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DepStatus {
    pub name: String,
    pub up_to_date: bool,
}

/// The hub's working folder in the project. Saved/ is ignored by Unreal projects' Git setup.
fn work_dir(project: &Project) -> PathBuf {
    project.root.join("Saved").join("UEDevHub")
}

/// Which version of each dependency is installed: `name=source` per line.
fn versions_file(project: &Project) -> PathBuf {
    work_dir(project).join("dependencies.txt")
}

fn installed(project: &Project) -> Vec<(String, String)> {
    std::fs::read_to_string(versions_file(project))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('=').map(|(a, b)| (a.to_string(), b.to_string())))
        .collect()
}

pub fn status(project: &Project) -> Vec<DepStatus> {
    let have = installed(project);
    project
        .config
        .dependencies
        .iter()
        .map(|d| DepStatus { name: d.name.clone(), up_to_date: have.iter().any(|(n, s)| *n == d.name && *s == d.source) })
        .collect()
}

/// rclone lives in the hub's own folder, never in a project: a repository could otherwise
/// ship its own rclone.exe. The download is checked against rclone's published SHA-256.
fn rclone(log: &Log) -> Result<PathBuf, String> {
    let (platform, sha256) = rclone_build().ok_or("Downloading dependencies isn't supported on this platform.")?;
    let tools = tools_dir().ok_or("Couldn't find a folder for the hub's tools.")?.join(format!("rclone-{RCLONE_VERSION}"));
    let exe = tools.join(if cfg!(windows) { "rclone.exe" } else { "rclone" });
    if exe.is_file() {
        return Ok(exe);
    }
    log.info(format!("Downloading rclone {RCLONE_VERSION} ..."));
    std::fs::create_dir_all(&tools).map_err(|e| e.to_string())?;
    let zip_name = format!("rclone-{RCLONE_VERSION}-{platform}.zip");
    let zip = tools.join(&zip_name);
    let url = format!("https://downloads.rclone.org/{RCLONE_VERSION}/{zip_name}");
    let out = proc::run_timeout("curl", &["-fsSL", &url, "-o", &zip.to_string_lossy()], None, 600)?;
    if !out.ok {
        return Err(format!("Couldn't download rclone: {}", out.message()));
    }
    let bytes = std::fs::read(&zip).map_err(|e| e.to_string())?;
    use sha2::Digest;
    let hash: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if hash != sha256 {
        let _ = std::fs::remove_file(&zip);
        return Err("The rclone download doesn't match its published checksum, so it wasn't used.".into());
    }
    let tmp = tools.join("extract");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    extract(&zip, &tmp)?;
    let _ = std::fs::remove_file(&zip);
    let name = exe.file_name().unwrap_or_default().to_string_lossy().to_string();
    let found = find_file(&tmp, &name).ok_or("rclone wasn't in the download.")?;
    std::fs::rename(found, &exe).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755));
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(exe)
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, name) {
                return Some(f);
            }
        } else if e.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
            return Some(p);
        }
    }
    None
}

fn extract(zip: &Path, dest: &Path) -> Result<(), String> {
    proc::unzip(zip, dest)
}

/// Downloads the dependencies that are missing or outdated. `force` downloads all of them again.
pub fn download(log: &Log, project: &Project, force: bool) -> Result<usize, String> {
    let deps: Vec<&Dependency> = project.config.dependencies.iter().collect();
    if deps.is_empty() {
        log.info("This project has no external dependencies.");
        return Ok(0);
    }
    let have = installed(project);
    let todo: Vec<&&Dependency> = deps.iter().filter(|d| force || !have.iter().any(|(n, s)| *n == d.name && *s == d.source)).collect();
    if todo.is_empty() {
        log.info("All dependencies are up to date.");
        return Ok(0);
    }
    let rclone = rclone(log)?;
    let rc = rclone.to_string_lossy().to_string();

    for d in &todo {
        log.check_cancel()?;
        let remote = remote_name(project, d);
        let remotes = proc::run(&rc, &["listremotes"], None)?.stdout;
        if !remotes.lines().any(|l| l.trim() == format!("{remote}:")) {
            log.info(format!("Setting up '{}' ({}). A browser window may open to sign in.", d.name, d.remote_type));
            let mut cmd = proc::command(&rclone);
            cmd.args(["config", "create", &remote, &d.remote_type]);
            if d.remote_type == "drive" {
                // Read-only access is all a download needs.
                cmd.args(["scope", "drive.readonly"]);
            }
            let (ok, _) = proc::stream(log, cmd, None, |_| true)?;
            if !ok {
                return Err(format!("Couldn't set up the download source '{}'.", d.name));
            }
        }

        // Work inside the project: moving files from %TEMP% fails when the project is on another drive.
        let tmp = work_dir(project).join(format!("download-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let zip = tmp.join("download.zip");
        let src = format!("{remote}:{}", d.source);
        log.info(format!("Downloading {src} ..."));
        let copy = |extra: &[&str]| -> Result<bool, String> {
            let mut cmd = proc::command(&rclone);
            cmd.args(["copyto", &src]).arg(&zip).args(["--stats", "2s", "--stats-one-line"]).args(extra);
            Ok(proc::stream(log, cmd, None, |_| true)?.0)
        };
        let result = (|| {
            let mut ok = copy(&[])?;
            if !ok && d.remote_type == "drive" {
                log.info("Trying again with files shared with you ...");
                ok = copy(&["--drive-shared-with-me"])?;
            }
            if !ok {
                return Err(format!("Couldn't download {src}. Nothing in the project was changed."));
            }
            let extracted = tmp.join("x");
            std::fs::create_dir_all(&extracted).map_err(|e| e.to_string())?;
            log.info(format!("Extracting to {} ...", d.dest));
            extract(&zip, &extracted)?;
            install(&extracted, &project.root.join(&d.dest))
        })();
        let _ = std::fs::remove_dir_all(&tmp);
        result?;

        let mut lines: Vec<String> = installed(project).into_iter().filter(|(n, _)| *n != d.name).map(|(n, s)| format!("{n}={s}")).collect();
        lines.push(format!("{}={}", d.name, d.source));
        let _ = std::fs::create_dir_all(work_dir(project));
        std::fs::write(versions_file(project), lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    }
    Ok(todo.len())
}

/// The rclone remote for a dependency. rclone's settings are shared with everything else on the
/// computer, so the hub's remotes get their own names: a project's "drive" must never pick
/// up a remote of the person's own, which may have full access to their files.
fn remote_name(project: &Project, d: &Dependency) -> String {
    let slug = |s: &str| -> String {
        let s: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
        s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
    };
    format!("uel-{}-{}", slug(&project.name), slug(&d.name))
}

/// Moves everything in `from` into `dest`, replacing what's there. Replaced items are renamed
/// aside first and only deleted once every new item is in place; if anything fails, the items
/// already swapped are put back, so `dest` ends up either fully updated or as it was.
pub fn install(from: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("Couldn't create {}: {e}", dest.display()))?;
    let items: Vec<_> = std::fs::read_dir(from).map_err(|e| e.to_string())?.flatten().collect();
    // (installed path, where the old one was put aside)
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = vec![];
    let rollback = |done: &[(PathBuf, Option<PathBuf>)]| {
        for (target, old) in done.iter().rev() {
            if target.is_dir() {
                let _ = std::fs::remove_dir_all(target);
            } else {
                let _ = std::fs::remove_file(target);
            }
            if let Some(old) = old {
                let _ = std::fs::rename(old, target);
            }
        }
    };
    for item in items {
        let name = item.file_name();
        let target = dest.join(&name);
        let mut old = None;
        if target.exists() {
            let aside = dest.join(format!(".ue-dev-hub-old-{}", name.to_string_lossy()));
            if aside.is_dir() {
                let _ = std::fs::remove_dir_all(&aside);
            } else {
                let _ = std::fs::remove_file(&aside);
            }
            if let Err(e) = std::fs::rename(&target, &aside) {
                rollback(&done);
                return Err(format!(
                    "Couldn't replace {}: {e}. Close Unreal Editor and anything else using it, then try again. Nothing was changed.",
                    target.display()
                ));
            }
            old = Some(aside);
        }
        if let Err(e) = std::fs::rename(item.path(), &target) {
            if let Some(o) = &old {
                let _ = std::fs::rename(o, &target);
            }
            rollback(&done);
            return Err(format!("Couldn't move {} into place: {e}. Nothing was changed.", target.display()));
        }
        done.push((target, old));
    }
    for (_, old) in done {
        if let Some(old) = old {
            if old.is_dir() {
                let _ = std::fs::remove_dir_all(&old);
            } else {
                let _ = std::fs::remove_file(&old);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_remotes_apart_from_the_persons_own() {
        let root = std::env::temp_dir().join(format!("ue-dev-hub-deps-remote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("My Game.uproject"), r#"{"EngineAssociation":"5.8"}"#).unwrap();
        std::fs::write(root.join("ue-dev-hub.toml"), "[[dependencies]]\nname = \"drive\"\ntype = \"drive\"\nsource = \"Wwise.zip\"\ndest = \"Plugins\"\n").unwrap();
        let p = Project::load(&root).unwrap();
        let d = p.config.dependencies.first().expect("the dependency is valid");
        assert_eq!(remote_name(&p, d), "uel-my-game-drive");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn replaces_folders_and_puts_everything_back_on_failure() {
        let root = std::env::temp_dir().join(format!("ue-dev-hub-deps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dest = root.join("Plugins");
        std::fs::create_dir_all(dest.join("Wwise")).unwrap();
        std::fs::write(dest.join("Wwise/old.txt"), b"old").unwrap();
        std::fs::create_dir_all(dest.join("Other")).unwrap();

        let new = root.join("x");
        std::fs::create_dir_all(new.join("Wwise")).unwrap();
        std::fs::write(new.join("Wwise/new.txt"), b"new").unwrap();
        install(&new, &dest).unwrap();
        assert!(dest.join("Wwise/new.txt").is_file());
        assert!(!dest.join("Wwise/old.txt").exists());
        assert!(dest.join("Other").is_dir(), "folders not in the zip stay");
        assert!(!dest.join(".ue-dev-hub-old-Wwise").exists());

        // A second item that can't be moved in: the first one is rolled back too.
        std::fs::create_dir_all(new.join("Wwise")).unwrap();
        std::fs::write(new.join("Wwise/newer.txt"), b"newer").unwrap();
        std::fs::create_dir_all(new.join("Zzz")).unwrap();
        std::fs::create_dir_all(dest.join("Zzz")).unwrap();
        // Holding a file open inside dest/Zzz makes renaming it fail on Windows.
        #[cfg(windows)]
        {
            let _held = std::fs::File::create(dest.join("Zzz/held.txt")).unwrap();
            let err = install(&new, &dest).unwrap_err();
            assert!(err.contains("Nothing was changed"), "{err}");
            assert!(dest.join("Wwise/new.txt").is_file(), "the old Wwise is back");
            assert!(!dest.join("Wwise/newer.txt").exists());
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
