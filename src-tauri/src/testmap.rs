//! "Test a map": cook one map and run it as a standalone game.

use crate::engine::{self, Engine};
use crate::jobs::{self, Snapshot};
use crate::maps;
use crate::package;
use crate::proc;
use crate::project::Project;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// Long package name.
    pub map: String,
    /// "Development", "Test" or "Shipping".
    pub build_type: String,
    /// What to call the saved run. Empty: map, build type and bake.
    #[serde(default)]
    pub name: String,
    /// Bakes the map's lighting at this quality before cooking. None skips the bake.
    pub bake_quality: Option<String>,
    pub launch_args: String,
}

/// Splits launch options like a command line: spaces separate, "double quotes" group.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Development for testing with the usual tools, Test for profiling (source builds only),
/// Shipping to check what players get.
pub const BUILD_TYPES: &[&str] = &["Development", "Test", "Shipping"];

/// The game inside a staged build folder. If it's missing, where it was expected.
fn staged_exe(project: &Project, staging_dir: &Path, build_type: &str) -> PathBuf {
    let candidates = package::game_candidates(&staging_dir.join(package::platform_folder()), &project.name, build_type, cfg!(windows));
    candidates.iter().find(|p| p.is_file()).unwrap_or(&candidates[0]).clone()
}

// ---------- Saved runs ----------

/// A test build kept so it can be played again. Each one is a full staged build in
/// `Saved/TestRuns/<id>`, described by the `run.json` next to it.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub name: String,
    pub map: String,
    pub build_type: String,
    pub bake_quality: Option<String>,
    pub launch_args: String,
    pub created_ms: u64,
    pub commit: String,
    pub branch: String,
    /// Bytes on disk. Worked out when listing, not stored.
    #[serde(default, skip_deserializing)]
    pub size: u64,
}

pub fn runs_dir(project: &Project) -> PathBuf {
    project.root.join("Saved/TestRuns")
}

const RUN_FILE: &str = "run.json";

/// The folder of run `id`. Ids are made by the hub; anything else could point outside.
fn run_dir(project: &Project, id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("That test run doesn't exist.".into());
    }
    Ok(runs_dir(project).join(id))
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(_) => e.metadata().map_or(0, |m| m.len()),
            Err(_) => 0,
        })
        .sum()
}

fn read_run(dir: &Path) -> Option<Run> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(RUN_FILE)).ok()?).ok()
}

/// Every saved run, newest first.
pub fn list_runs(project: &Project) -> Vec<Run> {
    let Ok(entries) = std::fs::read_dir(runs_dir(project)) else { return vec![] };
    let mut runs: Vec<Run> = entries
        .flatten()
        .filter_map(|e| {
            let mut run = read_run(&e.path())?;
            run.size = dir_size(&e.path());
            Some(run)
        })
        .collect();
    runs.sort_by_key(|r| std::cmp::Reverse(r.created_ms));
    runs
}

fn launch(project: &Project, dir: &Path, run: &Run) -> Result<(), String> {
    let exe = staged_exe(project, dir, &run.build_type);
    if !exe.is_file() {
        return Err(format!("The game wasn't found at {}.", exe.display()));
    }
    package::refuse_if_tracked(project, dir)?;
    let mut cmd = package::game_command(&exe);
    cmd.arg(&run.map).args(split_args(&run.launch_args)).current_dir(exe.parent().unwrap());
    proc::spawn_detached(cmd)
}

pub fn play_run(project: &Project, id: &str) -> Result<(), String> {
    let dir = run_dir(project, id)?;
    let run = read_run(&dir).ok_or("That test run doesn't exist anymore.")?;
    launch(project, &dir, &run)
}

pub fn rename_run(project: &Project, id: &str, name: &str) -> Result<(), String> {
    let dir = run_dir(project, id)?;
    let mut run = read_run(&dir).ok_or("That test run doesn't exist anymore.")?;
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the run a name.".into());
    }
    run.name = name.chars().take(120).collect();
    write_run(&dir, &run)
}

fn write_run(dir: &Path, run: &Run) -> Result<(), String> {
    let text = serde_json::to_string_pretty(run).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(RUN_FILE), text).map_err(|e| format!("Couldn't save the test run: {e}"))
}

fn remove(dir: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Couldn't delete it completely: {e}. Close the game if it's still running, then try again.")),
    }
}

pub fn delete_run(project: &Project, id: &str) -> Result<(), String> {
    remove(&run_dir(project, id)?)
}

/// Deletes every saved run, and leftovers of runs that never finished unless one is being made.
pub fn delete_all_runs(project: &Project) -> Result<usize, String> {
    let Ok(entries) = std::fs::read_dir(runs_dir(project)) else { return Ok(0) };
    let making = jobs::is_running();
    let mut deleted = 0;
    for e in entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
        let complete = read_run(&e.path()).is_some();
        if complete || !making {
            remove(&e.path())?;
            deleted += usize::from(complete);
        }
    }
    Ok(deleted)
}

pub fn start(app: &AppHandle, project: Project, engine: Engine, opts: Options) -> Result<Snapshot, String> {
    if !BUILD_TYPES.contains(&opts.build_type.as_str()) {
        return Err(format!("Unknown build type {}.", opts.build_type));
    }
    if opts.build_type == "Test" && !engine.is_source_build() {
        return Err("Test builds need an engine built from source.".into());
    }
    if let Some(q) = opts.bake_quality.as_deref()
        && !crate::config::LIGHTING_QUALITIES.contains(&q)
    {
        return Err(format!("Unknown lighting quality {q}."));
    }
    maps::check_map_name(&opts.map)?;
    if opts.bake_quality.is_some() && proc::editor_running_for(&project.uproject) {
        return Err("Unreal Editor is open. Close it first, the bake needs the map's files.".into());
    }
    let name = maps::short_name(&opts.map);
    let id = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let dir = run_dir(&project, &id)?;
    if dir.exists() {
        return Err("A test run was just started. Wait a second and try again.".into());
    }
    let run = Run {
        id,
        name: match opts.name.trim() {
            "" => {
                let bake = opts.bake_quality.as_deref().map(|q| format!(", {q} bake")).unwrap_or_default();
                format!("{name}, {}{bake}", opts.build_type)
            }
            n => n.chars().take(120).collect(),
        },
        map: opts.map.clone(),
        build_type: opts.build_type.clone(),
        bake_quality: opts.bake_quality.clone(),
        launch_args: opts.launch_args.clone(),
        created_ms: jobs::now_ms(),
        commit: project.git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default().trim().to_string(),
        branch: project.current_branch().unwrap_or_default(),
        size: 0,
    };

    let mut steps = vec!["Compile the editor".to_string()];
    if opts.bake_quality.is_some() {
        steps.push("Bake lighting".to_string());
    }
    steps.push(format!("Cook {name}"));
    steps.push("Start the game".to_string());

    jobs::start(app, jobs::JobKind::TestMap, format!("Testing {name}"), run.name.clone(), steps, move |job| {
        let result = make_run(job, &project, &engine, &opts, &run, &dir);
        if result.is_err() {
            // Half a build can't be played; don't leave gigabytes of it behind.
            let _ = std::fs::remove_dir_all(&dir);
        }
        result
    })
}

fn make_run(job: &jobs::Job, project: &Project, engine: &Engine, opts: &Options, run: &Run, dir: &Path) -> Result<String, String> {
    job.running(0, "");
    package::compile(job, project, engine, 0)?;
    let mut c = 1;
    if let Some(q) = opts.bake_quality.as_deref() {
        job.running(1, format!("{q} quality"));
        crate::lighting::bake(&job.log, project, engine, std::slice::from_ref(&opts.map), q)?;
        job.done(1, format!("{q} quality"));
        c = 2;
    }
    job.running(c, format!("{} build", opts.build_type));
    let args: Vec<String> = vec![
        "BuildCookRun".into(),
        "-nop4".into(),
        "-utf8output".into(),
        "-nocompileeditor".into(),
        "-skipbuildeditor".into(),
        format!("-project={}", project.uproject.display()),
        format!("-target={}", project.name),
        format!("-platform={}", engine::platform()),
        "-installed".into(),
        format!("-clientconfig={}", opts.build_type),
        "-cook".into(),
        "-iterativecooking".into(),
        "-build".into(),
        "-stage".into(),
        format!("-Map={}", opts.map),
        // Each run gets its own copy, so earlier runs can still be played.
        format!("-stagingdirectory={}", dir.display()),
    ];
    let log_file = project.logs_dir().join(format!("{}_TestMap.log", project.name));
    if !package::run_uat(job, project, engine, &args, &log_file, c)? {
        return Err(format!("Cooking failed. The output above and {} say why.", log_file.display()));
    }
    job.done(c, "Done");

    job.running(c + 1, "");
    write_run(dir, run)?;
    launch(project, dir, run)?;
    job.done(c + 1, "The game window opens in a moment");
    Ok(format!("{} is starting. The build is kept under Your test runs, to play again later.", maps::short_name(&opts.map)))
}

#[cfg(test)]
mod tests {
    use super::split_args;

    #[test]
    fn keeps_quoted_launch_options_together() {
        assert_eq!(
            split_args(r#"-trace=default -ExecCmds="stat fps, stat unit"  -nosound"#),
            vec!["-trace=default", "-ExecCmds=stat fps, stat unit", "-nosound"]
        );
        assert!(split_args("   ").is_empty());
    }
}
