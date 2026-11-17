//! Running external programs without flashing console windows on Windows.

use crate::log::Log;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;

/// Windows tools the hub runs. They're always taken from System32, so a file with the same
/// name next to the hub or in a project can't stand in for them.
const SYSTEM_TOOLS: &[&str] = &["cmd", "tar", "where", "tasklist", "taskkill", "curl", "powershell"];

pub fn resolve(program: &OsStr) -> std::path::PathBuf {
    let name = program.to_string_lossy().to_lowercase();
    let name = name.trim_end_matches(".exe");
    if cfg!(windows)
        && SYSTEM_TOOLS.contains(&name)
        && let Some(root) = std::env::var_os("SystemRoot")
    {
        let sys = std::path::PathBuf::from(root).join("System32");
        return if name == "powershell" { sys.join(r"WindowsPowerShell\v1.0\powershell.exe") } else { sys.join(format!("{name}.exe")) };
    }
    std::path::PathBuf::from(program)
}

/// A `Command` that never opens a console window on Windows.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(resolve(program.as_ref()));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // There is no terminal to answer prompts, so fail instead of hanging.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    clean_env(&mut cmd);
    if is_git(program.as_ref()) {
        // Git's messages are matched in English (network errors, for one), so keep them
        // untranslated. Only for Git: the editor and the game keep the person's language.
        cmd.env("LC_ALL", "C.UTF-8").env_remove("LANGUAGE");
    }
    cmd
}

fn is_git(program: &OsStr) -> bool {
    let path = program.to_string_lossy().to_lowercase();
    let name = path.rsplit(['/', '\\']).next().unwrap_or_default();
    let name = name.strip_suffix(".exe").unwrap_or(name);
    name == "git" || name == "git-lfs"
}

/// Variables the hub sets for itself that programs it starts shouldn't get. Filled in once at
/// startup, before any other thread runs.
static OWN_VARS: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();

/// Remembers that the hub set `vars` for itself only.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn set_own_vars(vars: Vec<&'static str>) {
    let _ = OWN_VARS.set(vars);
}

fn own_env() -> Vec<(String, String)> {
    std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect()
}

/// The value of `key` a program the hub starts gets, after the AppImage's changes are
/// undone. Use it to build on a variable, like adding a folder to PATH, which `cmd.env` would
/// otherwise set from the hub's own value.
pub fn child_var(key: &str) -> Option<std::ffi::OsString> {
    let own = OWN_VARS.get().map(Vec::as_slice).unwrap_or_default();
    match child_env(&own_env(), own).into_iter().find(|(k, _)| k == key) {
        Some((_, value)) => value.map(Into::into),
        None => std::env::var_os(key),
    }
}

/// Removes what the hub's own environment would wrongly pass on, keeping anything `cmd`
/// already sets explicitly.
fn clean_env(cmd: &mut Command) {
    let env = own_env();
    let own = OWN_VARS.get().map(Vec::as_slice).unwrap_or_default();
    let explicit: Vec<String> = cmd.get_envs().map(|(k, _)| k.to_string_lossy().to_string()).collect();
    for (key, value) in child_env(&env, own) {
        if explicit.contains(&key) {
            continue;
        }
        match value {
            Some(v) => cmd.env(key, v),
            None => cmd.env_remove(key),
        };
    }
}

/// Set by the AppImage runtime about itself.
const APPIMAGE_VARS: &[&str] = &["APPDIR", "APPIMAGE", "ARGV0", "OWD"];

/// Search paths and settings an AppImage (AppRun and linuxdeploy's hooks) points into its own
/// files. Programs started from it would load the AppImage's libraries and modules instead of the
/// system's and crash or misbehave.
const APPIMAGE_PATHS: &[&str] = &[
    "PATH",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "GTK_PATH",
    "GTK_EXE_PREFIX",
    "GTK_DATA_PREFIX",
    "GTK_IM_MODULE_FILE",
    "GDK_PIXBUF_MODULE_FILE",
    "GDK_PIXBUF_MODULEDIR",
    "GIO_MODULE_DIR",
    "GIO_EXTRA_MODULES",
    "GSETTINGS_SCHEMA_DIR",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
    "PYTHONHOME",
    "PYTHONPATH",
    "PERLLIB",
    "PERL5LIB",
    "QT_PLUGIN_PATH",
    "QT_QPA_PLATFORM_PLUGIN_PATH",
    "GST_PLUGIN_PATH",
    "GST_PLUGIN_PATH_1_0",
    "GST_PLUGIN_SYSTEM_PATH",
    "GST_PLUGIN_SYSTEM_PATH_1_0",
    "GST_PLUGIN_SCANNER",
    "GST_PLUGIN_SCANNER_1_0",
];

/// The environment changes for a program the hub starts, from the hub's own
/// environment `env`: `Some` sets a variable, `None` removes it. `own` are variables the hub
/// set only for itself.
///
/// Inside an AppImage this undoes what the AppImage changed: a variable saved as `<NAME>_ORIG`
/// gets that value back, and otherwise the entries pointing into the AppImage are dropped.
pub(crate) fn child_env(env: &[(String, String)], own: &[&str]) -> Vec<(String, Option<String>)> {
    let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
    let mut out: Vec<(String, Option<String>)> = own.iter().map(|k| (k.to_string(), None)).collect();
    if get("APPIMAGE").is_none() && get("APPDIR").is_none() {
        return out;
    }
    let appdir = get("APPDIR").map(|d| d.trim_end_matches('/')).filter(|d| !d.is_empty());
    let inside = |entry: &str| appdir.is_some_and(|d| entry == d || entry.starts_with(&format!("{d}/")));
    for key in APPIMAGE_VARS {
        if get(key).is_some() {
            out.push((key.to_string(), None));
        }
    }
    for key in APPIMAGE_PATHS {
        let orig = format!("{key}_ORIG");
        if let Some(saved) = get(&orig) {
            out.push((key.to_string(), Some(saved.to_string()).filter(|s| !s.is_empty())));
            out.push((orig, None));
            continue;
        }
        let Some(value) = get(key) else { continue };
        // LD_PRELOAD may also be separated by spaces.
        let sep = if *key == "LD_PRELOAD" { [':', ' '].as_slice() } else { [':'].as_slice() };
        let kept: Vec<&str> = value.split(sep).filter(|e| !e.is_empty() && !inside(e)).collect();
        let joined = kept.join(":");
        if joined == value {
            continue;
        }
        if joined.is_empty() && *key != "PATH" {
            out.push((key.to_string(), None));
        } else if !joined.is_empty() {
            out.push((key.to_string(), Some(joined)));
        }
    }
    out
}

#[derive(Debug)]
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// stderr if there is any, otherwise stdout. Useful for error messages.
    pub fn message(&self) -> String {
        if self.stderr.is_empty() { self.stdout.clone() } else { self.stderr.clone() }
    }
}

pub fn run(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<Output, String> {
    let mut cmd = command(program);
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let out = cmd.output().map_err(|e| format!("Could not run `{program}`: {e}"))?;
    Ok(Output {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

/// Like `run`, but kills the process (and what it started) after `secs` seconds, so a stalled
/// network or a sign-in window nobody answers can't leave the hub waiting forever.
pub fn run_timeout(program: &str, args: &[&str], cwd: Option<&Path>, secs: u64) -> Result<Output, String> {
    run_cancellable(program, args, cwd, secs, &|| false)
}

/// Like `run_timeout`, and also stops the process when `cancelled` turns true, for waits the
/// person can cancel, like a sign-in in the browser. Fails with `jobs::CANCELLED` then.
pub fn run_cancellable(program: &str, args: &[&str], cwd: Option<&Path>, secs: u64, cancelled: &dyn Fn() -> bool) -> Result<Output, String> {
    let mut cmd = command(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().map_err(|e| format!("Could not run `{program}`: {e}"))?;
    let read = |stream: Option<Box<dyn std::io::Read + Send>>| {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        if let Some(mut s) = stream {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf);
                let _ = tx.send(buf);
            });
        }
        rx
    };
    let out_rx = read(child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>));
    let err_rx = read(child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break st;
        }
        let timed_out = std::time::Instant::now() > deadline;
        if timed_out || cancelled() {
            kill_tree(child.id());
            let _ = child.kill();
            // Reap it, so it doesn't stay behind as a zombie on Linux.
            let _ = child.wait();
            if !timed_out {
                return Err(crate::jobs::CANCELLED.into());
            }
            return Err(format!("`{program} {}` took longer than {secs} seconds and was stopped. Check your internet connection, and whether a sign-in window is waiting.", args.first().unwrap_or(&"")));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    // Something the process started may keep its output open; don't wait for that forever.
    let grace = std::time::Duration::from_secs(3);
    let text = |rx: mpsc::Receiver<Vec<u8>>| String::from_utf8_lossy(&rx.recv_timeout(grace).unwrap_or_default()).trim().to_string();
    Ok(Output { ok: status.success(), stdout: text(out_rx), stderr: text(err_rx) })
}

/// True if `program` can be started at all.
pub fn exists(program: &str, version_arg: &str) -> bool {
    run(program, &[version_arg], None).map(|o| o.ok).unwrap_or(false)
}

/// Runs `cmd`, passing every output line to `on_line` (which returns false to hide a line from the
/// log) and optionally to a file. The process can be killed by cancelling the running job.
/// Returns whether it succeeded, and all lines.
pub fn stream(
    log: &Log,
    mut cmd: Command,
    log_file: Option<&Path>,
    mut on_line: impl FnMut(&str) -> bool,
) -> Result<(bool, Vec<String>), String> {
    log.check_cancel()?;
    let program = cmd.get_program().to_string_lossy().to_string();
    // Open the log first: if that fails, nothing has started that would need stopping.
    let mut file = match log_file {
        Some(p) => {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            Some(std::fs::File::create(p).map_err(|e| format!("Couldn't write the log {}: {e}", p.display()))?)
        }
        None => None,
    };
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start {program}: {e}"))?;
    let pid = child.id();
    log.track_pid(pid);

    let (tx, rx) = mpsc::channel::<String>();
    let readers: Vec<_> = [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|stream| {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut buf = Vec::new();
            // Lines may not be valid UTF-8 (Windows tools print in the console code page).
            while reader.read_until(b'\n', &mut buf).map(|n| n > 0).unwrap_or(false) {
                let line = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string();
                buf.clear();
                if tx.send(line).is_err() {
                    break;
                }
            }
        })
    })
    .collect();
    drop(tx);

    // Wait for the process on its own thread: helpers it started (like MSBuild nodes) can keep
    // the output pipes open long after it exits, so the end of output can't be the signal.
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done_tx.send(child.wait());
    });

    let mut lines = Vec::new();
    let mut status = None;
    let mut exited_at: Option<std::time::Instant> = None;
    let grace = std::time::Duration::from_secs(3);
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(line) => {
                if let Some(f) = file.as_mut() {
                    let _ = writeln!(f, "{line}");
                }
                if on_line(&line) {
                    log.detail(line.clone());
                }
                lines.push(line);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if status.is_none()
            && let Ok(s) = done_rx.try_recv()
        {
            status = Some(s);
            exited_at = Some(std::time::Instant::now());
        }
        if exited_at.is_some_and(|t| t.elapsed() > grace) {
            // The process is gone; stop listening to whatever still holds its output open.
            break;
        }
    }
    if status.is_none() {
        // The output ended first; the process exits right after.
        status = done_rx.recv().ok();
    }
    let readers_done = readers.iter().all(|r| r.is_finished());
    if readers_done {
        for r in readers {
            let _ = r.join();
        }
    }

    log.untrack_pid(pid);
    log.check_cancel()?;
    let status = status.ok_or("The process ended unexpectedly.")?.map_err(|e| e.to_string())?;
    Ok((status.success(), lines))
}

/// Ends a process and everything it started. On Linux it asks them to stop first, and kills
/// whatever is still alive a few seconds later.
pub fn kill_tree(pid: u32) {
    if cfg!(windows) {
        let _ = run("taskkill", &["/PID", &pid.to_string(), "/T", "/F"], None);
    } else {
        // Collect the whole tree up front: once the parent is gone, its children get re-parented
        // and can't be found from it anymore.
        let mut tree = vec![];
        collect_tree(pid, &mut tree);
        for p in &tree {
            let _ = run("kill", &["-TERM", &p.to_string()], None);
        }
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::time::Instant::now() < deadline && tree.iter().any(|p| alive_unix(*p)) {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            for p in tree.iter().filter(|p| alive_unix(**p)) {
                let _ = run("kill", &["-KILL", &p.to_string()], None);
            }
        });
    }
}

/// `pid` and everything below it, children first.
fn collect_tree(pid: u32, out: &mut Vec<u32>) {
    let children = run("pgrep", &["-P", &pid.to_string()], None).map(|o| o.stdout).unwrap_or_default();
    for child in children.lines().filter_map(|l| l.trim().parse::<u32>().ok()) {
        if !out.contains(&child) && child != pid {
            collect_tree(child, out);
        }
    }
    out.push(pid);
}

/// Whether a process still runs on Linux. A zombie has exited already and only waits to be reaped.
fn alive_unix(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // The state comes after the name, which is in parentheses and may contain spaces.
        Ok(stat) => stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().next()).is_some_and(|state| state != "Z"),
        Err(_) => false,
    }
}

/// Whether a process with this id still runs.
pub fn pid_running(pid: u32) -> bool {
    if cfg!(windows) {
        run("tasklist", &["/FI", &format!("PID eq {pid}"), "/NH"], None).map(|o| o.stdout.contains(&pid.to_string())).unwrap_or(false)
    } else {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

/// A program on PATH.
pub fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat", ""] } else { &[""] };
    std::env::split_paths(&path).flat_map(|d| exts.iter().map(move |e| d.join(format!("{name}{e}")))).find(|p| p.is_file())
}

/// 7-Zip if installed: much faster than the alternatives for big builds.
pub fn seven_zip() -> Option<std::path::PathBuf> {
    if cfg!(windows) {
        ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|p| std::path::PathBuf::from(p).join(r"7-Zip\7z.exe"))
            .find(|p| p.is_file())
    } else {
        which("7zz").or_else(|| which("7z"))
    }
}

/// A command that zips `items` (paths relative to `dir`) into `archive`: 7-Zip if installed,
/// otherwise the tar that ships with Windows 10+, or `zip` on Linux.
pub fn zip_command(dir: &Path, archive: &Path, items: &[&str]) -> Command {
    let mut c = match seven_zip() {
        Some(seven) => {
            let mut c = command(seven);
            c.args(["a", "-tzip", "-bso0", "-bsp1"]).arg(archive);
            c
        }
        None if cfg!(windows) => {
            let mut c = command("tar");
            c.args(["--format", "zip", "-c", "-f"]).arg(archive);
            c
        }
        None => {
            let mut c = command("zip");
            c.args(["-q", "-r"]).arg(archive);
            c
        }
    };
    c.args(items).current_dir(dir);
    c
}

/// Extracts a zip: the tar that ships with Windows 10+, or `unzip` on Linux.
pub fn unzip(zip: &Path, dest: &Path) -> Result<(), String> {
    let (zip_s, dest_s) = (zip.to_string_lossy(), dest.to_string_lossy());
    let out = if cfg!(windows) {
        run("tar", &["-xf", &zip_s, "-C", &dest_s], None)?
    } else {
        run("unzip", &["-q", "-o", &zip_s, "-d", &dest_s], None).map_err(|_| "Extracting zips needs unzip. Install it with your package manager.".to_string())?
    };
    if out.ok { Ok(()) } else { Err(format!("Couldn't extract {}: {}", zip.display(), out.message())) }
}

/// Whether a process with this executable name runs, like "UnrealEditor.exe".
pub fn is_running(image: &str) -> bool {
    if cfg!(windows) {
        run("tasklist", &["/FI", &format!("IMAGENAME eq {image}"), "/NH"], None)
            .map(|o| o.stdout.to_lowercase().contains(&image.to_lowercase()))
            .unwrap_or(false)
    } else {
        let name = image.trim_end_matches(".exe");
        let Ok(dir) = std::fs::read_dir("/proc") else { return false };
        dir.flatten().any(|e| {
            let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
            let cmdline = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
            process_is(&comm, &cmdline, name)
        })
    }
}

/// Whether a Linux process with this `/proc/<pid>/comm` and `cmdline` runs the program `name`.
/// `comm` only holds the first 15 bytes of the name (so `pgrep -x` misses UnrealEditor-Cmd), so
/// longer names are matched against the file name of the first argument instead.
#[cfg_attr(windows, allow(dead_code))]
fn process_is(comm: &str, cmdline: &[u8], name: &str) -> bool {
    const COMM_LEN: usize = 15;
    let comm = comm.trim_end_matches('\n');
    if name.len() <= COMM_LEN && comm == name {
        return true;
    }
    let argv0 = cmdline.split(|b| *b == 0).next().unwrap_or_default();
    let exe = String::from_utf8_lossy(argv0);
    let exe = exe.rsplit('/').next().unwrap_or_default();
    // Matching argv[0] alone could be fooled by a renamed process; the truncated comm must agree.
    exe == name && name.as_bytes().starts_with(comm.as_bytes()) && !comm.is_empty()
}

pub fn editor_running() -> bool {
    // Tests run while people work in Unreal; they must not depend on it being closed.
    if cfg!(test) {
        return false;
    }
    is_running("UnrealEditor.exe") || is_running("UnrealEditor-Cmd.exe")
}

/// Whether an Unreal Editor has this project open. An editor on another project doesn't count.
/// When it can't tell (no permission to read a process's command line, or an editor opened
/// without a project on its command line), it says yes, to stay on the safe side.
pub fn editor_running_for(uproject: &Path) -> bool {
    if !editor_running() {
        return false;
    }
    let Some(command_lines) = editor_command_lines() else { return true };
    let norm = |s: &str| s.replace('/', "\\").to_lowercase();
    let wanted = norm(&uproject.to_string_lossy());
    let lines: Vec<&str> = command_lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    lines.is_empty() || lines.iter().any(|l| {
        let l = norm(l);
        l.contains(&wanted) || !l.contains(".uproject")
    })
}

/// The command lines of running Unreal Editors, or None if they can't be read.
fn editor_command_lines() -> Option<Vec<String>> {
    if cfg!(windows) {
        let script = "Get-CimInstance Win32_Process -Filter \"Name like 'UnrealEditor%'\" | ForEach-Object { $_.CommandLine }";
        let out = run_timeout("powershell", &["-NoProfile", "-NonInteractive", "-Command", script], None, 20).ok()?;
        out.ok.then(|| out.stdout.lines().map(str::to_string).collect())
    } else {
        let mut lines = vec![];
        for e in std::fs::read_dir("/proc").ok()?.flatten() {
            let Ok(bytes) = std::fs::read(e.path().join("cmdline")) else { continue };
            let Some(line) = editor_cmdline(&bytes) else { continue };
            lines.push(line);
        }
        Some(lines)
    }
}

/// The command line from a Linux `/proc/<pid>/cmdline` if it belongs to an Unreal Editor, with
/// the arguments joined by spaces. Arguments are split on NULs, so paths with spaces stay whole.
fn editor_cmdline(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut args = text.split('\0').filter(|a| !a.is_empty());
    let exe = args.next()?;
    if !exe.rsplit('/').next().is_some_and(|n| n.starts_with("UnrealEditor")) {
        return None;
    }
    Some(std::iter::once(exe).chain(args).collect::<Vec<_>>().join(" "))
}

/// Starts a GUI program that should outlive the hub, without a console window.
pub fn spawn_detached(mut cmd: Command) -> Result<(), String> {
    clean_env(&mut cmd);
    let program = cmd.get_program().to_string_lossy().to_string();
    cmd.spawn().map(|_| ()).map_err(|e| format!("Couldn't start {program}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn leaves_the_environment_alone_outside_an_appimage() {
        let e = env(&[("PATH", "/usr/bin"), ("LD_LIBRARY_PATH", "/opt/lib")]);
        assert!(child_env(&e, &[]).is_empty());
        assert_eq!(child_env(&e, &["WEBKIT_DISABLE_DMABUF_RENDERER"]), vec![("WEBKIT_DISABLE_DMABUF_RENDERER".to_string(), None)]);
    }

    #[test]
    fn undoes_what_an_appimage_changed() {
        let e = env(&[
            ("APPIMAGE", "/home/a/ue-dev-hub.AppImage"),
            ("APPDIR", "/tmp/.mount_ueXYZ"),
            ("ARGV0", "./ue-dev-hub.AppImage"),
            ("OWD", "/home/a"),
            ("PATH", "/tmp/.mount_ueXYZ/usr/bin:/usr/local/bin:/usr/bin"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_ueXYZ/usr/lib:/tmp/.mount_ueXYZ/usr/lib/x86_64-linux-gnu"),
            ("XDG_DATA_DIRS", "/tmp/.mount_ueXYZ/usr/share:/usr/local/share:/usr/share"),
            ("GTK_PATH", "/tmp/.mount_ueXYZ/usr/lib/gtk-3.0"),
            ("GDK_PIXBUF_MODULE_FILE", "/tmp/.mount_ueXYZ/usr/lib/gdk-pixbuf-2.0/loaders.cache"),
            ("PYTHONHOME", "/tmp/.mount_ueXYZ/usr"),
            ("QT_PLUGIN_PATH", "/tmp/.mount_ueXYZ/usr/plugins"),
            ("QT_PLUGIN_PATH_ORIG", "/usr/lib/qt/plugins"),
            ("GIO_MODULE_DIR", "/tmp/.mount_ueXYZ/usr/lib/gio/modules"),
            ("GIO_MODULE_DIR_ORIG", ""),
            // Not the AppImage's: kept.
            ("PERLLIB", "/home/a/perl"),
            ("HOME", "/home/a"),
            ("LANG", "de_DE.UTF-8"),
        ]);
        let mut got = child_env(&e, &[]);
        got.sort();
        let s = |v: &str| Some(v.to_string());
        let mut want: Vec<(String, Option<String>)> = vec![
            ("APPDIR".into(), None),
            ("APPIMAGE".into(), None),
            ("ARGV0".into(), None),
            ("OWD".into(), None),
            ("PATH".into(), s("/usr/local/bin:/usr/bin")),
            ("LD_LIBRARY_PATH".into(), None),
            ("XDG_DATA_DIRS".into(), s("/usr/local/share:/usr/share")),
            ("GTK_PATH".into(), None),
            ("GDK_PIXBUF_MODULE_FILE".into(), None),
            ("PYTHONHOME".into(), None),
            ("QT_PLUGIN_PATH".into(), s("/usr/lib/qt/plugins")),
            ("QT_PLUGIN_PATH_ORIG".into(), None),
            ("GIO_MODULE_DIR".into(), None),
            ("GIO_MODULE_DIR_ORIG".into(), None),
        ];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn keeps_path_when_only_appimage_entries_are_in_it() {
        let e = env(&[("APPIMAGE", "/a.AppImage"), ("APPDIR", "/tmp/.mount_a/"), ("PATH", "/tmp/.mount_a/usr/bin"), ("LD_PRELOAD", "/tmp/.mount_a/x.so /usr/lib/y.so")]);
        let got = child_env(&e, &[]);
        assert!(!got.iter().any(|(k, _)| k == "PATH"), "{got:?}");
        assert!(got.contains(&("LD_PRELOAD".to_string(), Some("/usr/lib/y.so".to_string()))), "{got:?}");
        // A folder that only starts with the same name isn't inside the AppImage.
        let e = env(&[("APPDIR", "/tmp/.mount_a"), ("PATH", "/tmp/.mount_ab/bin:/usr/bin")]);
        assert_eq!(child_env(&e, &[]), vec![("APPDIR".to_string(), None)]);
    }

    #[test]
    fn keeps_git_messages_in_english_only_for_git() {
        let has_lc_all = |c: &Command| c.get_envs().any(|(k, v)| k == "LC_ALL" && v.is_some());
        assert!(has_lc_all(&command("git")));
        assert!(has_lc_all(&command("/usr/bin/git")));
        assert!(has_lc_all(&command(r"C:\Program Files\Git\cmd\git.exe")));
        assert!(!has_lc_all(&command("UnrealEditor")));
        assert!(!has_lc_all(&command("gitk")));
    }

    #[test]
    fn stops_a_command_that_takes_too_long() {
        let started = std::time::Instant::now();
        let (prog, args): (&str, Vec<&str>) =
            if cfg!(windows) { ("powershell", vec!["-NoProfile", "-Command", "Start-Sleep 30"]) } else { ("sleep", vec!["30"]) };
        let err = run_timeout(prog, &args, None, 2).unwrap_err();
        assert!(err.contains("took longer than 2 seconds"), "{err}");
        assert!(started.elapsed().as_secs() < 15);
    }

    #[test]
    fn stops_a_command_when_cancelled() {
        let started = std::time::Instant::now();
        let (prog, args): (&str, Vec<&str>) =
            if cfg!(windows) { ("powershell", vec!["-NoProfile", "-Command", "Start-Sleep 30"]) } else { ("sleep", vec!["30"]) };
        let cancel_at = started + std::time::Duration::from_secs(1);
        let err = run_cancellable(prog, &args, None, 60, &|| std::time::Instant::now() > cancel_at).unwrap_err();
        assert_eq!(err, crate::jobs::CANCELLED);
        assert!(started.elapsed().as_secs() < 15);
    }

    #[test]
    fn finds_linux_processes_with_long_names() {
        assert!(process_is("UnrealEditor\n", b"/opt/UE/Engine/Binaries/Linux/UnrealEditor\0/p/G.uproject\0", "UnrealEditor"));
        assert!(process_is("UnrealEditor-Cm\n", b"/opt/UE/Engine/Binaries/Linux/UnrealEditor-Cmd\0-run\0", "UnrealEditor-Cmd"));
        assert!(!process_is("UnrealEditor-Cm\n", b"/opt/UE/UnrealEditor-CmdFoo\0", "UnrealEditor-Cmd"));
        assert!(!process_is("UnrealEditorX\n", b"UnrealEditorX\0", "UnrealEditor"));
        // A kernel thread or a process that's gone has no command line.
        assert!(!process_is("", b"", "UnrealEditor-Cmd"));
    }

    #[test]
    fn reads_editor_command_lines_with_spaces() {
        let line = editor_cmdline(b"/opt/Unreal Engine/Engine/Binaries/Linux/UnrealEditor\0/home/me/My Game/My Game.uproject\0-log\0").unwrap();
        assert_eq!(line, "/opt/Unreal Engine/Engine/Binaries/Linux/UnrealEditor /home/me/My Game/My Game.uproject -log");
        assert!(editor_cmdline(b"/usr/bin/bash\0-c\0UnrealEditor\0").is_none());
        assert!(editor_cmdline(b"").is_none());
    }

    #[test]
    fn a_log_that_cant_be_written_starts_nothing() {
        let dir = std::env::temp_dir().join(format!("uel-stream-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // A folder can't be opened as a file.
        let (prog, args): (&str, Vec<&str>) =
            if cfg!(windows) { ("cmd", vec!["/c", "echo hi"]) } else { ("echo", vec!["hi"]) };
        let mut cmd = command(prog);
        cmd.args(args);
        let err = stream(&Log::default(), cmd, Some(&dir), |_| true).unwrap_err();
        assert!(err.contains("Couldn't write the log"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finishes_when_a_helper_keeps_the_output_open() {
        if !cfg!(windows) {
            return;
        }
        // The parent prints and exits at once; the helper it starts inherits the output pipes and
        // lives on for 30 seconds, like an MSBuild node.
        let mut cmd = command("cmd");
        cmd.args(["/c", "echo hello & start /b powershell -NoProfile -Command Start-Sleep 30"]);
        let started = std::time::Instant::now();
        let (ok, lines) = stream(&Log::default(), cmd, None, |_| true).unwrap();
        assert!(ok);
        assert!(lines.iter().any(|l| l.contains("hello")));
        assert!(started.elapsed().as_secs() < 20, "{:?}", started.elapsed());
    }
}
