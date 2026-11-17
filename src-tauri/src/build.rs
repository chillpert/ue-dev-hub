//! Compiling the editor target with UnrealBuildTool, streaming output and progress to the UI.

use crate::engine::{self, Engine};
use crate::log::{Log, Progress};
use crate::proc;
use crate::project::Project;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BuildResult {
    pub ok: bool,
    pub skipped: bool,
    /// The compiler errors, without the noise around them.
    pub errors: Vec<String>,
    /// A plain-language explanation for failures we recognize.
    pub hint: Option<String>,
    pub log_path: Option<PathBuf>,
}

pub fn build(log: &Log, project: &Project, engine: &Engine) -> Result<BuildResult, String> {
    if !project.has_source() {
        log.info("This project has no C++ code, so there's nothing to compile.");
        return Ok(BuildResult { ok: true, skipped: true, errors: vec![], hint: None, log_path: None });
    }

    let ubt = engine.ubt();
    if !ubt.is_file() {
        let repair = if cfg!(windows) { "Try verifying the engine in the Epic Games Launcher." } else { "Download the engine again, or run Setup.sh in a source build." };
        return Err(format!("UnrealBuildTool is missing from the engine install ({}). {repair}", ubt.display()));
    }

    if !project.root.join("Binaries").is_dir() || !project.root.join("Intermediate").is_dir() {
        generate_project_files(log, project, engine)?;
    }

    let log_path = build_log_path(project);
    let uproject = format!("-Project={}", project.uproject.display());

    log.info("Compiling C++ ...");
    let (ok, lines) = run_ubt(
        log,
        project,
        engine,
        &[
            "Development",
            engine::platform(),
            &uproject,
            "-TargetType=Editor",
            "-Progress",
            "-NoEngineChanges",
            "-NoHotReloadFromIDE",
        ],
        Some(&log_path),
    )?;

    let errors = extract_errors(&lines);
    let hint = if ok { None } else { hint_for(&lines) };
    Ok(BuildResult { ok, skipped: false, errors, hint, log_path: Some(log_path) })
}

pub fn build_log_path(project: &Project) -> PathBuf {
    project.logs_dir().join(format!("{}_HubBuild.log", project.name))
}

/// Writes the .sln and IDE files. Needed when an IDE doesn't see new C++ files.
pub fn generate_project_files(log: &Log, project: &Project, engine: &Engine) -> Result<(), String> {
    let uproject = format!("-Project={}", project.uproject.display());
    log.info("Generating project files ...");
    let (ok, _) = run_ubt(log, project, engine, &["-projectFiles", &uproject, "-game", "-rocket", "-progress"], None)?;
    if ok { Ok(()) } else { Err("Couldn't generate the project files. The first error in the output above says why; fix that and try again.".into()) }
}

/// Runs UBT, forwarding every line to the UI and optionally to a log file.
fn run_ubt(log: &Log, project: &Project, engine: &Engine, args: &[&str], log_file: Option<&PathBuf>) -> Result<(bool, Vec<String>), String> {
    let mut cmd = proc::command(engine.ubt());
    cmd.args(args).current_dir(&project.root);
    engine.use_bundled_dotnet(&mut cmd);
    proc::stream(log, cmd, log_file.map(|p| p.as_path()), |line| {
        if let Some(progress) = parse_progress(line) {
            log.progress(progress);
            return !line.trim_start().starts_with("@progress");
        }
        true
    })
}

/// Understands `[12/345] Compile Foo.cpp` and `@progress 'Compiling C++ source files...' 40%`.
pub fn parse_progress(line: &str) -> Option<Progress> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("@progress") {
        let rest = rest.trim();
        let percent = rest
            .rsplit_once(' ')
            .and_then(|(_, p)| p.strip_suffix('%'))
            .and_then(|p| p.parse::<f32>().ok());
        let label = rest.split('\'').nth(1).unwrap_or("").to_string();
        if percent.is_none() && label.is_empty() {
            return None;
        }
        return Some(Progress { percent, label });
    }
    let inner = trimmed.strip_prefix('[')?;
    let (counts, label) = inner.split_once(']')?;
    let (done, total) = counts.split_once('/')?;
    let (done, total) = (done.trim().parse::<f32>().ok()?, total.trim().parse::<f32>().ok()?);
    if total <= 0.0 {
        return None;
    }
    Some(Progress { percent: Some(done / total * 100.0), label: label.trim().to_string() })
}

fn is_error_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains(": error") || l.contains(": fatal error") || l.starts_with("error:") || l.contains(" error lnk") || l.starts_with("error ")
}

pub fn extract_errors(lines: &[String]) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    for line in lines.iter().map(|l| l.trim()).filter(|l| is_error_line(l)) {
        if !errors.iter().any(|e| e == line) {
            errors.push(line.to_string());
        }
        if errors.len() >= 25 {
            break;
        }
    }
    errors
}

fn hint_for(lines: &[String]) -> Option<String> {
    let has = |needle: &str| lines.iter().any(|l| l.contains(needle));
    if has("Live Coding is active") || has("Unable to build while Live Coding") {
        return Some("Unreal Editor is still open. Close it, then try again.".into());
    }
    if has("Unable to find valid") && has("toolchain") || has("No valid Visual C++") || has("is not a valid platform to build") {
        return Some(compiler_missing().into());
    }
    if cfg!(windows) && has("Windows SDK") && has("not installed") {
        return Some("A Windows SDK is missing. Open the Visual Studio Installer and add the Windows SDK it mentions.".into());
    }
    if has("There is not enough space on the disk") || has("No space left on device") {
        return Some("Your disk is full. Free up some space and try again.".into());
    }
    if has("being used by another process") {
        return Some("A file is in use by another program. Close Unreal Editor and any running game, then try again.".into());
    }
    None
}

fn compiler_missing() -> &'static str {
    if cfg!(windows) {
        "The C++ compiler is missing. Open the Visual Studio Installer and add the \"Game development with C++\" workload."
    } else {
        "The C++ compiler (clang) wasn't found. Unreal brings its own: in a source build, run Setup.sh in the engine folder. Otherwise download the clang toolchain Epic lists for this engine version and set LINUX_MULTIARCH_ROOT to it."
    }
}

pub fn launch(project: &Project, engine: &Engine) -> Result<(), String> {
    let editor = engine.editor();
    // A plain Command: the editor is a GUI app and should outlive the hub.
    let mut cmd = std::process::Command::new(&editor);
    cmd.arg(&project.uproject).current_dir(&project.root);
    proc::spawn_detached(cmd).map_err(|e| format!("Couldn't start Unreal Editor ({}): {e}", editor.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_action_counts() {
        let p = parse_progress("[3/12] Compile [x64] Foo.cpp").unwrap();
        assert_eq!(p.percent, Some(25.0));
        assert_eq!(p.label, "Compile [x64] Foo.cpp");
    }

    #[test]
    fn parses_progress_markup() {
        let p = parse_progress("@progress 'Compiling C++ source files...' 40%").unwrap();
        assert_eq!(p.percent, Some(40.0));
        assert_eq!(p.label, "Compiling C++ source files...");
    }

    #[test]
    fn ignores_plain_lines() {
        assert!(parse_progress("Using bundled DotNet SDK").is_none());
    }

    #[test]
    fn finds_msvc_and_clang_errors() {
        let lines: Vec<String> = [
            "Building StarfallEditor...",
            r"C:\P\Source\Foo.cpp(12): error C2065: 'x': undeclared identifier",
            "/p/Source/Foo.cpp:12:5: error: use of undeclared identifier 'x'",
            r"C:\P\Source\Foo.cpp(12): error C2065: 'x': undeclared identifier",
            "Result: Failed (OtherCompilationError)",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(extract_errors(&lines).len(), 2);
    }

    #[test]
    fn explains_a_missing_compiler_for_this_platform() {
        let lines = vec!["Platform Linux is not a valid platform to build. Check that the SDK is installed properly.".to_string()];
        let hint = hint_for(&lines).unwrap();
        assert_eq!(hint.contains("Visual Studio"), cfg!(windows), "{hint}");
    }

    #[test]
    fn explains_live_coding() {
        let lines = vec!["Unable to build while Live Coding is active. Exit the editor and game".to_string()];
        assert!(hint_for(&lines).unwrap().contains("Close it"));
    }
}
