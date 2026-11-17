//! Generating Wwise soundbanks with WwiseConsole.

use crate::log::Log;
use crate::proc;
use crate::project::Project;
use std::path::PathBuf;

/// Why soundbanks can't be generated on this platform, if they can't. The hub only knows the
/// Windows WwiseConsole, so on Linux the project's committed banks are used.
pub const UNSUPPORTED: Option<&str> =
    if cfg!(windows) { None } else { Some("Soundbanks can't be generated on Linux. Generate Linux soundbanks on Windows and commit them.") };

/// WwiseConsole.exe for the project's Wwise version, if installed.
pub fn console(project: &Project) -> Option<PathBuf> {
    let version = project.config.requirements.wwise.as_deref()?;
    if UNSUPPORTED.is_some() {
        return None;
    }
    let mut roots = vec![PathBuf::from(r"C:\Audiokinetic")];
    if let Some(pf) = std::env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(pf).join("Audiokinetic"));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        roots.push(PathBuf::from(pf).join("Audiokinetic"));
    }
    // Newer Wwise Hub versions install to "Wwise_<version>", older ones to "Wwise<version>".
    roots
        .iter()
        .flat_map(|r| [r.join(format!("Wwise_{version}")), r.join(format!("Wwise{version}"))])
        .map(|d| d.join(r"Authoring\x64\Release\bin\WwiseConsole.exe"))
        .find(|p| p.is_file())
}

pub fn project_file(project: &Project) -> PathBuf {
    match &project.config.package.wwise_project {
        Some(p) => project.root.join(p),
        None => project.root.join(format!("{0}_WwiseProject/{0}_WwiseProject.wproj", project.name)),
    }
}

/// Generates the Windows soundbanks. Packaging only builds for the platform it runs on, and this
/// runs only on Windows.
pub fn generate(log: &Log, project: &Project) -> Result<(), String> {
    let version = project.config.requirements.wwise.as_deref().ok_or("This project doesn't use Wwise (requirements.wwise isn't set).")?;
    if let Some(why) = UNSUPPORTED {
        return Err(why.into());
    }
    let console = console(project).ok_or(format!("Wwise {version} isn't installed. Install it with the Audiokinetic Hub."))?;
    let wproj = project_file(project);
    if !wproj.is_file() {
        return Err(format!("The Wwise project wasn't found at {}.", wproj.display()));
    }
    log.info(format!("Generating Wwise soundbanks with Wwise {version} ..."));
    let mut cmd = proc::command(console);
    cmd.arg("generate-soundbank").arg(&wproj).args(["--platform", "Windows"]).current_dir(&project.root);
    let (ok, _) = proc::stream(log, cmd, None, |_| true)?;
    if ok { Ok(()) } else { Err("Soundbank generation failed. The output above says why.".into()) }
}
