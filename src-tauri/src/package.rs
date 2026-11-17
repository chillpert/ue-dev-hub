//! Packaging the game: compile the editor, optionally generate soundbanks and bake lighting,
//! cook and package, then zip.

use crate::build;
use crate::engine::{self, Engine};
use crate::jobs::{self, Job, Snapshot};
use crate::lighting;
use crate::log::Log;
use crate::proc;
use crate::project::Project;
use crate::wwise;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// A release build for players, instead of an internal test build.
    pub for_players: bool,
    /// Long package names of the maps to bake. Empty skips the bake.
    pub bake_maps: Vec<String>,
    pub quality: String,
    pub soundbanks: bool,
    pub zip: bool,
    /// Upload the zip to the team's Nextcloud when done. Needs `zip`.
    #[serde(default)]
    pub upload: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BuildEntry {
    pub name: String,
    pub path: PathBuf,
    /// Seconds since the epoch.
    pub modified: i64,
    pub zip: Option<PathBuf>,
    /// The game to start, if the build has one.
    pub exe: Option<PathBuf>,
    /// Internal builds are named InternalOnly-...
    pub for_players: bool,
}


/// Packaged builds, newest first.
pub fn builds(project: &Project) -> Vec<BuildEntry> {
    let Ok(entries) = std::fs::read_dir(project.builds_dir()) else { return vec![] };
    let mut builds: Vec<BuildEntry> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let modified = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            let zip = Some(e.path().join(format!("{name}.zip"))).filter(|z| z.is_file());
            let exe = find_exe(project, &e.path());
            BuildEntry { for_players: !name.starts_with("InternalOnly-"), name, path: e.path(), modified, zip, exe }
        })
        .collect();
    builds.sort_by_key(|b| std::cmp::Reverse(b.modified));
    builds
}

/// The game in a packaged build: `<build>/<build>/<Project>.exe`, or older layouts.
pub fn find_exe(project: &Project, build: &Path) -> Option<PathBuf> {
    let name = build.file_name()?.to_string_lossy().to_string();
    [build.join(&name), build.join(platform_folder()), build.to_path_buf()]
        .iter()
        .flat_map(|dir| game_candidates(dir, &project.name, "Shipping", cfg!(windows)))
        .find(|p| p.is_file())
}

/// Where the game can be in a staged platform folder (`Windows/` or `Linux/`), most likely first.
///
/// UAT puts a starter named after the target next to the game's folder, for every configuration:
/// `<Project>.exe` on Windows, a `<Project>.sh` script on Linux. The real program is in
/// `<Project>/Binaries/<Platform>/`, named `<Project>` for Development and
/// `<Project>-<Platform>-<Configuration>` otherwise.
pub(crate) fn game_candidates(dir: &Path, project: &str, build_type: &str, windows: bool) -> Vec<PathBuf> {
    let (platform, ext) = if windows { ("Win64", ".exe") } else { ("Linux", "") };
    let binary = if build_type == "Development" { project.to_string() } else { format!("{project}-{platform}-{build_type}") };
    let real = dir.join(project).join("Binaries").join(platform).join(format!("{binary}{ext}"));
    if windows {
        let mut out = vec![];
        if build_type != "Development" {
            out.push(dir.join(format!("{binary}.exe")));
        }
        out.extend([dir.join(format!("{project}.exe")), real]);
        out
    } else {
        vec![dir.join(format!("{project}.sh")), real]
    }
}

/// A command that starts the game at `exe`. Linux starter scripts run through `sh`, so they work
/// even if unzipping lost their executable bit.
pub(crate) fn game_command(exe: &Path) -> std::process::Command {
    if !cfg!(windows) && exe.extension().is_some_and(|e| e == "sh") {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg(exe);
        cmd
    } else {
        std::process::Command::new(exe)
    }
}

/// Starts a packaged build. `build` must be a folder in the project's builds folder.
pub fn run_build(project: &Project, build: &Path) -> Result<(), String> {
    let dir = project.builds_dir();
    let inside = build.canonicalize().ok().zip(dir.canonicalize().ok()).is_some_and(|(b, d)| b.starts_with(&d) && b != d);
    if !inside {
        return Err("That build isn't in the project's builds folder.".into());
    }
    let exe = find_exe(project, build).ok_or("This build has no game to start. It may be unfinished or only zipped.")?;
    refuse_if_tracked(project, build)?;
    let mut cmd = game_command(&exe);
    cmd.current_dir(exe.parent().unwrap());
    proc::spawn_detached(cmd)
}

/// Whether Git has files in `dir`, which then came with the repository instead of being built on
/// this computer.
pub fn tracked(project: &Project, dir: &Path) -> bool {
    let (Ok(dir), Ok(root)) = (dir.canonicalize(), project.root.canonicalize()) else { return false };
    let Ok(rel) = dir.strip_prefix(&root) else { return false };
    let rel = rel.to_string_lossy().replace('\\', "/");
    let rel = if rel.is_empty() { ".".to_string() } else { rel };
    project.git(&["ls-files", "--recurse-submodules", "--", &rel]).is_some_and(|o| !o.trim().is_empty())
}

/// Builds are only started when they were made here: a repository could commit a "build" with
/// any program in it, to be started by a click on Play.
pub fn refuse_if_tracked(project: &Project, dir: &Path) -> Result<(), String> {
    if tracked(project, dir) {
        return Err("This build is part of the repository, not one made on this computer, so the hub doesn't start it. Open its folder if you trust it.".into());
    }
    Ok(())
}

/// Zips a finished build folder for uploading: the folder inside it with the game.
pub fn zip_build(log: &Log, build: &Path, name: &str) -> Result<PathBuf, String> {
    let inner = [name.to_string(), platform_folder().to_string()].into_iter().find(|n| build.join(n).is_dir()).ok_or("This build has no game folder to zip.")?;
    zip(log, build, &inner, name)
}

pub fn start(app: &AppHandle, project: Project, engine: Engine, opts: Options) -> Result<Snapshot, String> {
    if proc::editor_running_for(&project.uproject) {
        return Err("Unreal Editor is open. Save your work and close it first.".into());
    }
    // Only when Wwise is installed; the Package screen says it's skipped otherwise.
    let soundbanks = opts.soundbanks && project.config.requirements.wwise.is_some() && wwise::console(&project).is_some();
    let mut steps = vec!["Compile the editor".to_string()];
    let i_sound = soundbanks.then(|| {
        steps.push("Generate Wwise soundbanks".into());
        steps.len() - 1
    });
    let i_light = steps.len();
    steps.push(if opts.bake_maps.is_empty() { "Skip lighting".into() } else { "Bake lighting".into() });
    let i_cook = steps.len();
    steps.push("Cook and package".into());
    let i_zip = opts.zip.then(|| {
        steps.push("Zip it".into());
        steps.len() - 1
    });
    let upload = opts.upload && opts.zip && project.config.upload.nextcloud_url.is_some();
    let i_upload = upload.then(|| {
        steps.push("Upload to Nextcloud".into());
        steps.len() - 1
    });
    let state_dir = tauri::Manager::path(app).app_config_dir().unwrap_or_else(|_| std::env::temp_dir());

    let subtitle = if opts.for_players { "Public build" } else { "Internal testing build" };
    jobs::start(app, jobs::JobKind::Package, "Packaging".into(), subtitle.into(), steps, move |job| {
        let log = &job.log;

        job.running(0, "So the cook matches your C++");
        compile(job, &project, &engine, 0)?;

        if let Some(i) = i_sound {
            job.running(i, "For Windows");
            match wwise::generate(log, &project) {
                Ok(()) => job.done(i, "Done"),
                // Old soundbanks are better than no build.
                Err(e) => {
                    log.info(format!("{e} Packaging continues with the soundbanks already there."));
                    job.failed(i, "Failed, using the existing soundbanks");
                }
            }
        }
        log.check_cancel()?;

        if opts.bake_maps.is_empty() {
            job.skipped(i_light, "Uses the lighting already in the maps");
        } else {
            let n = opts.bake_maps.len();
            job.running(i_light, format!("{n} map{} at {} quality", if n == 1 { "" } else { "s" }, opts.quality));
            lighting::bake(log, &project, &engine, &opts.bake_maps, &opts.quality)?;
            job.done(i_light, format!("{n} map{} baked", if n == 1 { "" } else { "s" }));
        }

        job.running(i_cook, if opts.for_players { "Shipping, for distribution" } else { "Shipping, with debug info" });
        let (out_dir, folder_name, inner) = cook_and_package(job, &project, &engine, opts.for_players, i_cook)?;
        job.set_output(out_dir.clone());
        job.done(i_cook, format!("Saved to {}", out_dir.display()));

        let mut zipped = None;
        if let Some(i) = i_zip {
            let tool = if proc::seven_zip().is_some() { "7-Zip" } else if cfg!(windows) { "Windows tar" } else { "zip" };
            job.running(i, tool);
            match zip(log, &out_dir, &inner, &folder_name) {
                Ok(z) => {
                    job.done(i, format!("{}", z.file_name().unwrap_or_default().to_string_lossy()));
                    zipped = Some(z);
                }
                Err(e) => {
                    job.failed(i, "The build is there, just not zipped");
                    return Err(format!("{e} The uncompressed build is in {}.", out_dir.display()));
                }
            }
        }

        if let (Some(i), Some(z)) = (i_upload, zipped.as_ref()) {
            job.running(i, "");
            match crate::nextcloud::upload(log, &project, z, "", &state_dir) {
                Ok(up) => job.done(i, up.note()),
                Err(e) => {
                    job.failed(i, "The build is fine; upload it again from Builds");
                    return Err(format!("The build is ready, but uploading it failed: {e}"));
                }
            }
        }

        let _ = tauri_plugin_opener::reveal_item_in_dir(&out_dir);
        Ok(if opts.for_players {
            "The build is ready.".into()
        } else {
            "The build is ready. Don't publish it anywhere: it's only for internal testing.".into()
        })
    })
}

/// Compiles the editor as step `i`. Used by packaging, baking and testing a map.
pub fn compile(job: &Job, project: &Project, engine: &Engine, i: usize) -> Result<(), String> {
    let result = build::build(&job.log, project, engine)?;
    if let Some(p) = &result.log_path {
        job.set_log_path(p.clone());
    }
    if !result.ok {
        job.set_errors(result.errors.clone());
        job.failed(i, "The game code didn't compile");
        return Err(result.hint.unwrap_or_else(|| {
            "The game code didn't compile. Usually someone pushed code that doesn't compile, or something on your computer is missing.".into()
        }));
    }
    job.done(i, if result.skipped { "No C++ code" } else { "Up to date" });
    Ok(())
}

/// The folder UAT stages a build of this platform into.
pub(crate) fn platform_folder() -> &'static str {
    if cfg!(windows) { "Windows" } else { "Linux" }
}

/// Runs UAT BuildCookRun with the given arguments, streaming its output to `log_file`.
pub fn run_uat(job: &Job, project: &Project, engine: &Engine, args: &[String], log_file: &Path, step: usize) -> Result<bool, String> {
    let uat = engine.uat();
    if !uat.is_file() {
        return Err(format!("RunUAT is missing from the engine install ({}).", uat.display()));
    }
    job.set_log_path(log_file.to_path_buf());
    let mut cmd = proc::command(&uat);
    cmd.args(args).current_dir(&project.root);
    let phases = [
        ("BUILD COMMAND STARTED", "Compiling the game"),
        ("COOK COMMAND STARTED", "Cooking content"),
        ("STAGE COMMAND STARTED", "Staging files"),
        ("PACKAGE COMMAND STARTED", "Packaging"),
        ("ARCHIVE COMMAND STARTED", "Copying to the output folder"),
    ];
    let (ok, _) = proc::stream(&job.log, cmd, Some(log_file), |line| {
        if let Some((_, label)) = phases.iter().find(|(m, _)| line.contains(m)) {
            job.running(step, *label);
        }
        true
    })?;
    Ok(ok)
}

/// Returns the build's folder, its name, and the name of the folder inside it with the game.
fn cook_and_package(job: &Job, project: &Project, engine: &Engine, for_players: bool, step: usize) -> Result<(PathBuf, String, String), String> {
    let date = chrono::Local::now().format("%y.%m.%d-%H.%M").to_string();
    let platform = engine::platform();
    let mut folder_name = format!("{}-{date}-{platform}", project.name);
    if !for_players {
        folder_name = format!("InternalOnly-{folder_name}");
    }
    // Two builds in the same minute get their own folders.
    let base_name = folder_name.clone();
    let mut n = 2;
    while project.builds_dir().join(&folder_name).exists() {
        folder_name = format!("{base_name}-{n}");
        n += 1;
    }
    let out_dir = project.builds_dir().join(&folder_name);

    let mut args: Vec<String> = [
        "BuildCookRun",
        "-nop4",
        "-utf8output",
        "-nocompileeditor",
        "-skipbuildeditor",
        "-cook",
        "-installed",
        "-stage",
        "-archive",
        "-package",
        "-build",
        "-pak",
        "-iostore",
        "-compressed",
        "-prereqs",
        "-CrashReporter",
        "-clientconfig=Shipping",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(format!("-project={}", project.uproject.display()));
    args.push(format!("-target={}", project.name));
    args.push(format!("-platform={platform}"));
    args.push(format!("-archivedirectory={}", out_dir.display()));
    if for_players {
        args.push("-nodebuginfo".into());
        args.push("-distribution".into());
    }

    let log_file = project.logs_dir().join(format!("{}_Package.log", project.name));
    if !run_uat(job, project, engine, &args, &log_file, step)? {
        return Err(format!("Packaging failed. The output above and {} say why.", log_file.display()));
    }

    // Put the build in a folder named like the zip, with a README describing it.
    let staged = out_dir.join(platform_folder());
    let commit = project.git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let readme = format!(
        "# {}\n## Build Specifications\n\nDate: {date}<br>\nCommit: {commit}<br>\nPlatform: {platform}<br>\n",
        project.name
    );
    let mut inner = folder_name.clone();
    if staged.is_dir() {
        let _ = std::fs::write(staged.join("README.md"), readme);
        // Antivirus and the search indexer often hold new files for a moment, so retry.
        let target = out_dir.join(&folder_name);
        let mut renamed = false;
        for attempt in 0..10 {
            if std::fs::rename(&staged, &target).is_ok() {
                renamed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500 * (attempt + 1)));
        }
        if !renamed {
            // The build is complete either way; only its folder name differs.
            job.log.info(format!("Couldn't rename the {} folder, so the build stays in {}.", platform_folder(), staged.display()));
            inner = platform_folder().to_string();
        }
    }
    Ok((out_dir, folder_name, inner))
}

/// Zips `<out_dir>/<folder>` to `<out_dir>/<folder>.zip`. Compress-Archive fails on files over
/// 2 GB, so this uses 7-Zip if installed and otherwise the bsdtar that ships with Windows 10+.
fn zip(log: &Log, out_dir: &Path, folder: &str, name: &str) -> Result<PathBuf, String> {
    // Written under a temporary name, so a cancelled or failed zip never looks finished.
    let archive = out_dir.join(format!("{name}.zip"));
    let partial = out_dir.join(format!("{name}.zip.partial"));
    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_file(&partial);
    let cmd = proc::zip_command(out_dir, &partial, &[folder]);
    log.info("Creating the zip ...");
    let ok = proc::stream(log, cmd, None, |line| {
        // 7-Zip prints "  42% 13 + file"; show it as progress instead of flooding the log.
        if let Some(p) = line.trim().split('%').next().and_then(|p| p.trim().parse::<f32>().ok()) {
            log.progress(crate::log::Progress { percent: Some(p), label: "Zipping".into() });
            return false;
        }
        true
    });
    let ok = matches!(ok, Ok((true, _))) && partial.is_file();
    if !ok || std::fs::rename(&partial, &archive).is_err() {
        let _ = std::fs::remove_file(&partial);
        return Err("Zipping failed.".into());
    }
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_where_windows_games_are() {
        let d = Path::new("S/Windows");
        assert_eq!(
            game_candidates(d, "Game", "Development", true),
            vec![d.join("Game.exe"), d.join("Game/Binaries/Win64/Game.exe")]
        );
        assert_eq!(
            game_candidates(d, "Game", "Shipping", true),
            vec![d.join("Game-Win64-Shipping.exe"), d.join("Game.exe"), d.join("Game/Binaries/Win64/Game-Win64-Shipping.exe")]
        );
    }

    #[test]
    fn knows_where_linux_games_are() {
        let d = Path::new("S/Linux");
        assert_eq!(game_candidates(d, "Game", "Development", false), vec![d.join("Game.sh"), d.join("Game/Binaries/Linux/Game")]);
        assert_eq!(game_candidates(d, "Game", "Test", false), vec![d.join("Game.sh"), d.join("Game/Binaries/Linux/Game-Linux-Test")]);
        assert_eq!(game_candidates(d, "Game", "Shipping", false), vec![d.join("Game.sh"), d.join("Game/Binaries/Linux/Game-Linux-Shipping")]);
    }

    #[test]
    fn starts_linux_scripts_through_sh() {
        let cmd = game_command(Path::new("/b/Game.sh"));
        if cfg!(windows) {
            assert_eq!(cmd.get_program(), "/b/Game.sh");
        } else {
            assert_eq!(cmd.get_program(), "/bin/sh");
            assert_eq!(cmd.get_args().collect::<Vec<_>>(), vec!["/b/Game.sh"]);
        }
        assert_eq!(game_command(Path::new("/b/Game")).get_program(), "/b/Game");
    }

    #[test]
    fn finds_and_lists_builds() {
        let root = std::env::temp_dir().join(format!("ue-dev-hub-builds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Game.uproject"), br#"{"EngineAssociation":"5.8"}"#).unwrap();
        let exe = if cfg!(windows) { "Game.exe" } else { "Game.sh" };
        let release = root.join("Builds/Game-26.11.10-21.31-Win64/Game-26.11.10-21.31-Win64");
        std::fs::create_dir_all(&release).unwrap();
        std::fs::write(release.join(exe), b"").unwrap();
        std::fs::create_dir_all(root.join("Builds/InternalOnly-Game-26.11.11-09.00-Win64")).unwrap();

        let p = Project::load(&root).unwrap();
        let list = builds(&p);
        assert_eq!(list.len(), 2);
        let rel = list.iter().find(|b| b.for_players).unwrap();
        assert_eq!(rel.exe.as_deref(), Some(release.join(exe).as_path()));
        let internal = list.iter().find(|b| !b.for_players).unwrap();
        assert!(internal.exe.is_none());
        // Folders outside the builds folder are refused.
        assert!(run_build(&p, &root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_builds_that_came_with_the_repository() {
        let root = std::env::temp_dir().join(format!("ue-dev-hub-tracked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Builds/Committed")).unwrap();
        std::fs::create_dir_all(root.join("Builds/Mine")).unwrap();
        std::fs::write(root.join("Game.uproject"), br#"{"EngineAssociation":"5.8"}"#).unwrap();
        std::fs::write(root.join("Builds/Committed/Game.exe"), b"x").unwrap();
        std::fs::write(root.join("Builds/Mine/Game.exe"), b"x").unwrap();
        let git = |args: &[&str]| assert!(proc::run("git", args, Some(&root)).unwrap().ok, "{args:?}");
        git(&["init", "-q"]);
        git(&["add", "Builds/Committed/Game.exe"]);
        let p = Project::load(&root).unwrap();
        assert!(tracked(&p, &root.join("Builds/Committed")));
        assert!(refuse_if_tracked(&p, &root.join("Builds/Committed")).is_err());
        assert!(!tracked(&p, &root.join("Builds/Mine")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
