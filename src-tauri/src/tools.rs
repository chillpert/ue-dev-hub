//! The Tools screen: terminals, IDEs, and cleaning up build files.

use crate::proc;
use crate::project::Project;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct App {
    /// gitBash, windowsTerminal, powershell, cmd, rider, visualStudio, vscode, zed
    pub id: String,
    pub name: String,
    #[serde(skip)]
    pub exe: PathBuf,
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

fn git_bash() -> Option<PathBuf> {
    let mut candidates = vec![];
    for var in ["ProgramFiles", "ProgramW6432", "LOCALAPPDATA"] {
        if let Some(p) = env_path(var) {
            candidates.push(p.join(r"Git\git-bash.exe"));
            candidates.push(p.join(r"Programs\Git\git-bash.exe"));
        }
    }
    // Next to the git on PATH: <Git>\cmd\git.exe -> <Git>\git-bash.exe
    if let Ok(o) = proc::run("where", &["git"], None)
        && let Some(first) = o.stdout.lines().next()
        && let Some(root) = Path::new(first.trim()).parent().and_then(Path::parent)
    {
        candidates.push(root.join("git-bash.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub fn terminals() -> Vec<App> {
    if !cfg!(windows) {
        // The desktop's default first, then common terminals.
        return [
            ("x-terminal-emulator", "Default terminal"),
            ("gnome-terminal", "GNOME Terminal"),
            ("konsole", "Konsole"),
            ("kitty", "kitty"),
            ("alacritty", "Alacritty"),
            ("wezterm", "WezTerm"),
            ("xterm", "xterm"),
        ]
        .iter()
        .filter_map(|(bin, name)| proc::which(bin).map(|exe| App { id: bin.to_string(), name: name.to_string(), exe }))
        .collect();
    }
    let mut out = vec![];
    if let Some(p) = git_bash() {
        out.push(App { id: "gitBash".into(), name: "Git Bash".into(), exe: p });
    }
    if let Some(p) = env_path("LOCALAPPDATA").map(|p| p.join(r"Microsoft\WindowsApps\wt.exe")).filter(|p| p.is_file()) {
        out.push(App { id: "windowsTerminal".into(), name: "Windows Terminal".into(), exe: p });
    }
    out.push(App { id: "powershell".into(), name: "PowerShell".into(), exe: "powershell.exe".into() });
    out.push(App { id: "cmd".into(), name: "Command Prompt".into(), exe: "cmd.exe".into() });
    out
}

/// Opens a terminal window in `dir`.
pub fn open_terminal(id: Option<&str>, dir: &Path) -> Result<(), String> {
    let all = terminals();
    let app = id.and_then(|id| all.iter().find(|a| a.id == id)).or(all.first()).ok_or("No terminal was found.")?;
    let mut cmd = Command::new(&app.exe);
    match app.id.as_str() {
        "gnome-terminal" | "alacritty" => {
            cmd.arg("--working-directory").arg(dir);
        }
        "konsole" => {
            cmd.arg("--workdir").arg(dir);
        }
        "kitty" => {
            cmd.arg("--directory").arg(dir);
        }
        "wezterm" => {
            cmd.args(["start", "--cwd"]).arg(dir);
        }
        "gitBash" => {
            cmd.arg(format!("--cd={}", dir.display()));
        }
        "windowsTerminal" => {
            cmd.arg("-d").arg(dir);
        }
        _ => {
            cmd.current_dir(dir);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
                cmd.creation_flags(CREATE_NEW_CONSOLE);
            }
        }
    }
    proc::spawn_detached(cmd)
}

fn rider() -> Option<PathBuf> {
    let mut candidates = vec![];
    if let Some(local) = env_path("LOCALAPPDATA") {
        candidates.push(local.join(r"JetBrains\Toolbox\scripts\rider.cmd"));
        candidates.push(local.join(r"Programs\Rider\bin\rider64.exe"));
        if let Ok(dirs) = std::fs::read_dir(local.join("Programs")) {
            candidates.extend(dirs.flatten().filter(|d| d.file_name().to_string_lossy().contains("Rider")).map(|d| d.path().join(r"bin\rider64.exe")));
        }
    }
    for var in ["ProgramFiles", "ProgramW6432"] {
        if let Some(pf) = env_path(var)
            && let Ok(dirs) = std::fs::read_dir(pf.join("JetBrains"))
        {
            let mut found: Vec<PathBuf> = dirs.flatten().filter(|d| d.file_name().to_string_lossy().contains("Rider")).map(|d| d.path().join(r"bin\rider64.exe")).collect();
            // Newest version last, so put it first.
            found.sort();
            found.reverse();
            candidates.extend(found);
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn visual_studio() -> Option<PathBuf> {
    let vswhere = env_path("ProgramFiles(x86)")?.join(r"Microsoft Visual Studio\Installer\vswhere.exe");
    let out = proc::run(&vswhere.to_string_lossy(), &["-latest", "-prerelease", "-property", "productPath"], None).ok()?;
    Some(PathBuf::from(out.stdout.lines().next()?.trim())).filter(|p| p.is_file())
}

fn vscode() -> Option<PathBuf> {
    let mut c = vec![];
    if let Some(l) = env_path("LOCALAPPDATA") {
        c.push(l.join(r"Programs\Microsoft VS Code\Code.exe"));
    }
    if let Some(p) = env_path("ProgramFiles") {
        c.push(p.join(r"Microsoft VS Code\Code.exe"));
    }
    c.into_iter().find(|p| p.is_file())
}

fn zed() -> Option<PathBuf> {
    let mut c = vec![];
    if let Some(l) = env_path("LOCALAPPDATA") {
        c.push(l.join(r"Programs\Zed\Zed.exe"));
    }
    if let Some(p) = env_path("ProgramFiles") {
        c.push(p.join(r"Zed\Zed.exe"));
    }
    c.into_iter().find(|p| p.is_file()).or_else(|| proc::which("zed"))
}

pub fn ides() -> Vec<App> {
    if !cfg!(windows) {
        let mut out = vec![];
        let toolbox = env_path("HOME").map(|h| h.join(".local/share/JetBrains/Toolbox/scripts/rider")).filter(|p| p.is_file());
        if let Some(p) = proc::which("rider").or(toolbox) {
            out.push(App { id: "rider".into(), name: "Rider".into(), exe: p });
        }
        if let Some(p) = proc::which("code") {
            out.push(App { id: "vscode".into(), name: "VS Code".into(), exe: p });
        }
        // Some distributions name the binary zeditor.
        let local = env_path("HOME").map(|h| h.join(".local/bin/zed")).filter(|p| p.is_file());
        if let Some(p) = proc::which("zed").or_else(|| proc::which("zeditor")).or(local) {
            out.push(App { id: "zed".into(), name: "Zed".into(), exe: p });
        }
        return out;
    }
    let mut out = vec![];
    if let Some(p) = rider() {
        out.push(App { id: "rider".into(), name: "Rider".into(), exe: p });
    }
    if let Some(p) = visual_studio() {
        out.push(App { id: "visualStudio".into(), name: "Visual Studio".into(), exe: p });
    }
    if let Some(p) = vscode() {
        out.push(App { id: "vscode".into(), name: "VS Code".into(), exe: p });
    }
    if let Some(p) = zed() {
        out.push(App { id: "zed".into(), name: "Zed".into(), exe: p });
    }
    out
}

pub fn solution(project: &Project) -> Option<PathBuf> {
    std::fs::read_dir(&project.root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("sln")))
}

/// Opens the project in an IDE. Visual Studio needs the .sln, which the caller generates first.
pub fn open_ide(id: Option<&str>, project: &Project) -> Result<(), String> {
    let all = ides();
    let app = id.and_then(|id| all.iter().find(|a| a.id == id)).or(all.first()).ok_or("No IDE was found. Install Rider, Visual Studio, VS Code or Zed.")?;
    let target = match app.id.as_str() {
        // Rider opens .uproject files directly.
        "rider" => project.uproject.clone(),
        "visualStudio" => solution(project).ok_or("There's no .sln yet. Use Regenerate project files first.")?,
        _ => project.root.clone(),
    };
    let mut cmd = if is_batch(&app.exe) {
        // Toolbox's rider.cmd is a batch script. The project's file name comes from the repository,
        // so never hand it to `cmd /c` as text: a name like "Game&calc.uproject" would run calc.
        // Rust starts batch files itself and quotes the arguments safely, or refuses to.
        if !batch_safe(&target) {
            return Err(format!("{} can't be opened with {}, because its name has characters like & or % in it. Rename it, or pick another IDE.", file_name(&target), app.name));
        }
        proc::command(&app.exe)
    } else {
        Command::new(&app.exe)
    };
    cmd.arg(&target).current_dir(&project.root);
    proc::spawn_detached(cmd)
}

fn is_batch(exe: &Path) -> bool {
    exe.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// Whether `arg` has none of the characters cmd.exe treats specially, even inside quotes.
fn batch_safe(arg: &Path) -> bool {
    !arg.to_string_lossy().chars().any(|c| "&|<>^%!\"\r\n".contains(c))
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap_or(p.as_os_str()).to_string_lossy().to_string()
}

/// Plugin folders under Plugins/ that are compiled from their own source, so their Binaries can
/// be rebuilt. Plugins without Source ship prebuilt binaries, which must never be deleted.
pub fn source_plugins(project: &Project) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let entries: Vec<_> = entries.flatten().collect();
        if entries.iter().any(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("uplugin"))) {
            if dir.join("Source").is_dir() {
                out.push(dir.to_path_buf());
            }
            return;
        }
        if depth == 0 {
            return;
        }
        for e in entries {
            if e.file_type().is_ok_and(|t| t.is_dir()) && !e.file_name().to_string_lossy().starts_with('.') {
                walk(&e.path(), depth - 1, out);
            }
        }
    }
    let mut out = vec![];
    walk(&project.root.join("Plugins"), 3, &mut out);
    out
}

/// Deletes Binaries, Intermediate and the .sln in the project root, and Binaries and
/// Intermediate of plugins built from source.
pub fn clean(project: &Project) -> Result<Vec<String>, String> {
    if proc::editor_running_for(&project.uproject) {
        return Err("Unreal Editor is open. Close it first, it holds these files open.".into());
    }
    let mut removed = vec![];
    let mut dirs: Vec<PathBuf> = vec![project.root.join("Binaries"), project.root.join("Intermediate")];
    for plugin in source_plugins(project) {
        dirs.push(plugin.join("Binaries"));
        dirs.push(plugin.join("Intermediate"));
    }
    // Outside Git nothing is tracked. Inside, a folder Git can't answer for (Git missing, slow or
    // broken) counts as tracked and stays.
    let in_git = project.root.ancestors().any(|d| d.join(".git").exists());
    for p in dirs {
        if p.is_dir() {
            let rel = p.strip_prefix(&project.root).unwrap_or(&p).display().to_string();
            // Some teams commit plugin binaries. Never delete files that are in Git.
            let rel_git = rel.replace('\\', "/");
            let tracked = in_git && project.git(&["ls-files", "--", &rel_git]).is_none_or(|o| !o.trim().is_empty());
            if tracked {
                continue;
            }
            std::fs::remove_dir_all(&p).map_err(|e| format!("Couldn't delete {rel}: {e}. Close Rider, Visual Studio and the game, then try again."))?;
            removed.push(rel);
        }
    }
    if let Some(sln) = solution(project) {
        std::fs::remove_file(&sln).map_err(|e| e.to_string())?;
        removed.push(sln.file_name().unwrap_or_default().to_string_lossy().to_string());
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> Project {
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-clean-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Binaries")).unwrap();
        std::fs::write(dir.join("Game.uproject"), br#"{"EngineAssociation":"5.8"}"#).unwrap();
        Project::load(&dir).unwrap()
    }

    #[test]
    fn keeps_folders_when_git_cant_tell_whether_theyre_tracked() {
        let p = project("broken-git");
        // A .git that isn't a repository: every git command fails.
        std::fs::create_dir_all(p.root.join(".git")).unwrap();
        assert!(clean(&p).unwrap().is_empty());
        assert!(p.root.join("Binaries").is_dir());
        let _ = std::fs::remove_dir_all(&p.root);
    }

    #[test]
    fn cleans_outside_git() {
        let p = project("no-git");
        assert_eq!(clean(&p).unwrap(), vec!["Binaries".to_string()]);
        let _ = std::fs::remove_dir_all(&p.root);
    }

    #[test]
    fn refuses_names_cmd_would_run() {
        assert!(is_batch(Path::new(r"C:\Toolbox\scripts\rider.cmd")) && is_batch(Path::new("x.BAT")));
        assert!(!is_batch(Path::new("rider64.exe")));
        assert!(batch_safe(Path::new(r"C:\Repos\My Game\Game (1).uproject")));
        for bad in [r"C:\Repos\Game&calc.uproject", "Game|x.uproject", "Game%PATH%.uproject", "Game^.uproject", "Game!x!.uproject", "Ga\"me.uproject"] {
            assert!(!batch_safe(Path::new(bad)), "{bad}");
        }
    }
}
