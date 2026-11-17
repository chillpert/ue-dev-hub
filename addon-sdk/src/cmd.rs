//! Running programs from a helper without flashing console windows on Windows. Adapted from the
//! hub's own `proc.rs`; keep the two in step when one of them changes.

use crate::Ctx;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Windows tools that are always taken from System32, so a file with the same name in a project
/// can't stand in for them.
const SYSTEM_TOOLS: &[&str] = &["cmd", "tar", "where", "tasklist", "taskkill", "powershell"];

fn resolve(program: &OsStr) -> std::path::PathBuf {
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

/// A `Command` that never opens a console window on Windows and never waits for a Git prompt.
///
/// Git also speaks English, whatever the user's language: helpers recognize some of its messages,
/// like "would be overwritten", and a translated Git would slip past those checks.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let program = resolve(program.as_ref());
    let is_git = program.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("git"));
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    if is_git {
        // LANGUAGE wins over LC_ALL for messages, and LC_MESSAGES covers a Git without LANGUAGE
        // support. File names and text keep the user's encoding, since LC_CTYPE is untouched.
        cmd.env("LANGUAGE", "C").env("LC_MESSAGES", "C");
    }
    cmd
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
    cmd.args(args).stdin(Stdio::null());
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

/// What a program printed, byte for byte.
#[derive(Debug)]
pub struct RawOutput {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Like `run`, but kills the process (and what it started) after `secs` seconds.
pub fn run_timeout(program: &str, args: &[&str], cwd: Option<&Path>, secs: u64) -> Result<Output, String> {
    let mut cmd = command(program);
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let out = capture(cmd, secs)?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    Ok(Output { ok: out.ok, stdout: text(&out.stdout), stderr: text(&out.stderr) })
}

/// Runs `cmd` and returns its output untouched (not trimmed, not decoded), killing it and what
/// it started after `secs` seconds. For output where every byte counts, like diffs.
pub fn capture(mut cmd: Command, secs: u64) -> Result<RawOutput, String> {
    let program = cmd.get_program().to_string_lossy().to_string();
    let what = cmd.get_args().map(|a| a.to_string_lossy()).find(|a| !a.starts_with('-')).map_or(program.clone(), |a| format!("{program} {a}"));
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Could not run `{program}`: {e}"))?;
    let read = |stream: Option<Box<dyn Read + Send>>| {
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
    let out_rx = read(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err_rx = read(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));

    let deadline = Instant::now() + Duration::from_secs(secs);
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break st;
        }
        if Instant::now() > deadline {
            kill_tree(child.id());
            let _ = child.kill();
            // Reap it, so it doesn't stay behind as a zombie on Linux.
            let _ = child.wait();
            return Err(format!("`{what}` took longer than {secs} seconds and was stopped. Check your internet connection, and whether a sign-in window is waiting."));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let grace = Duration::from_secs(3);
    let bytes = |rx: mpsc::Receiver<Vec<u8>>| rx.recv_timeout(grace).unwrap_or_default();
    Ok(RawOutput { ok: status.success(), stdout: bytes(out_rx), stderr: bytes(err_rx) })
}

/// Splits output into lines at `\n`, `\r\n` and a lone `\r`. A line ended by a lone `\r` is a
/// progress update that the next one replaces, like Git's "Writing objects: 45%".
#[derive(Default)]
struct Lines {
    buf: Vec<u8>,
    after_cr: bool,
}

impl Lines {
    fn feed(&mut self, bytes: &[u8], mut emit: impl FnMut(String, bool)) {
        for &b in bytes {
            if self.after_cr {
                self.after_cr = false;
                if b == b'\n' {
                    emit(self.take(), false);
                    continue;
                }
                emit(self.take(), true);
            }
            match b {
                b'\n' => emit(self.take(), false),
                b'\r' => self.after_cr = true,
                _ => self.buf.push(b),
            }
        }
    }

    fn finish(&mut self, mut emit: impl FnMut(String, bool)) {
        if self.after_cr || !self.buf.is_empty() {
            let progress = self.after_cr;
            self.after_cr = false;
            emit(self.take(), progress);
        }
    }

    fn take(&mut self) -> String {
        let line = String::from_utf8_lossy(&self.buf).into_owned();
        self.buf.clear();
        line
    }
}

/// Runs `cmd`, sending every output line to the hub's log unless `on_line` returns false.
/// Returns whether it succeeded, and all lines.
pub fn stream(ctx: &Ctx, cmd: Command, on_line: impl FnMut(&str) -> bool) -> Result<(bool, Vec<String>), String> {
    stream_idle(ctx, cmd, None, on_line)
}

/// Like [`stream`], but stops the program (and what it started) once it printed nothing for
/// `idle`, instead of letting it hang. For long transfers that report progress, where a total
/// time limit would also stop a big upload that's going fine.
///
/// Progress updates (lines ended by a lone `\r`) count as output and reach the log at most once
/// a second. They aren't passed to `on_line` or returned.
pub fn stream_idle(ctx: &Ctx, mut cmd: Command, idle: Option<Duration>, mut on_line: impl FnMut(&str) -> bool) -> Result<(bool, Vec<String>), String> {
    let program = cmd.get_program().to_string_lossy().to_string();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start {program}: {e}"))?;
    let pid = child.id();

    let (tx, rx) = mpsc::channel::<(String, bool)>();
    for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)]
        .into_iter()
        .flatten()
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut lines = Lines::default();
            let mut open = true;
            while open {
                let n = match reader.fill_buf() {
                    Ok(buf) if !buf.is_empty() => {
                        lines.feed(buf, |line, progress| open &= tx.send((line, progress)).is_ok());
                        buf.len()
                    }
                    _ => break,
                };
                reader.consume(n);
            }
            lines.finish(|line, progress| {
                let _ = tx.send((line, progress));
            });
        });
    }
    drop(tx);

    // Wait for the process on its own thread: something it started can keep the pipes open.
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done_tx.send(child.wait());
    });

    let mut lines = Vec::new();
    let mut status = None;
    let mut exited_at: Option<Instant> = None;
    let mut last_output = Instant::now();
    let mut last_progress_log: Option<Instant> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok((line, progress)) => {
                last_output = Instant::now();
                if progress {
                    if !line.trim().is_empty() && last_progress_log.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
                        last_progress_log = Some(Instant::now());
                        ctx.detail(line);
                    }
                } else {
                    if on_line(&line) {
                        ctx.detail(line.clone());
                    }
                    lines.push(line);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if status.is_none()
            && let Ok(s) = done_rx.try_recv()
        {
            status = Some(s);
            exited_at = Some(Instant::now());
        }
        if exited_at.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
            break;
        }
        if let Some(limit) = idle
            && status.is_none()
            && last_output.elapsed() > limit
        {
            kill_tree(pid);
            return Err(format!("{program} showed no progress for {} and was stopped. Check your internet connection, and whether a sign-in window is waiting.", duration_text(limit)));
        }
    }
    if status.is_none() {
        status = done_rx.recv().ok();
    }
    let status = status.ok_or("The process ended unexpectedly.")?.map_err(|e| e.to_string())?;
    Ok((status.success(), lines))
}

/// "5 minutes", "a minute" or "30 seconds".
fn duration_text(d: Duration) -> String {
    match d.as_secs() {
        60 => "a minute".into(),
        s if s >= 60 => format!("{} minutes", s.div_ceil(60)),
        1 => "a second".into(),
        s => format!("{s} seconds"),
    }
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
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && tree.iter().any(|p| alive_unix(*p)) {
                std::thread::sleep(Duration::from_millis(200));
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

/// Whether a process with this executable name runs, like "git.exe". When it can't tell, it
/// says yes, so nothing treats a program as closed (and deletes its lock files) by mistake.
pub fn is_running(image: &str) -> bool {
    if cfg!(windows) {
        // CSV, since the table cuts long names off.
        match run("tasklist", &["/FI", &format!("IMAGENAME eq {image}"), "/FO", "CSV", "/NH"], None) {
            Ok(o) if o.ok => o.stdout.to_lowercase().contains(&format!("\"{}\"", image.to_lowercase())),
            _ => true,
        }
    } else {
        let name = image.trim_end_matches(".exe");
        let Ok(dir) = std::fs::read_dir("/proc") else { return true };
        dir.flatten().any(|e| {
            let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
            let cmdline = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
            process_is(&comm, &cmdline, name)
        })
    }
}

/// Whether a Linux process with this `/proc/<pid>/comm` and `cmdline` runs the program `name`.
/// `comm` only holds the first 15 bytes of the name (so `pgrep -x` misses longer names), so
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell command that runs `script`: PowerShell on Windows, sh elsewhere.
    fn shell(windows: &str, unix: &str) -> Command {
        let mut cmd = if cfg!(windows) { command("powershell") } else { command("sh") };
        if cfg!(windows) {
            cmd.args(["-NoProfile", "-NonInteractive", "-Command", windows]);
        } else {
            cmd.args(["-c", unix]);
        }
        cmd
    }

    #[test]
    fn splits_lines_and_progress() {
        let mut got = vec![];
        let mut lines = Lines::default();
        for chunk in [&b"one\r\ntw"[..], b"o\n10%\r", b"50%\r", b"\nlast"] {
            lines.feed(chunk, |l, p| got.push((l, p)));
        }
        lines.finish(|l, p| got.push((l, p)));
        let want = [("one", false), ("two", false), ("10%", true), ("50%", false), ("last", false)];
        assert_eq!(got, want.map(|(l, p)| (l.to_string(), p)));
    }

    #[test]
    fn capture_keeps_every_byte() {
        let out = capture(shell("[Console]::Out.Write(\"a \n\n\")", "printf 'a \n\n'"), 30).unwrap();
        assert!(out.ok);
        assert_eq!(out.stdout, b"a \n\n");
        assert!(run_timeout("git", &["--version"], None, 30).unwrap().stdout.starts_with("git version"));
    }

    #[test]
    fn finds_processes_with_long_names() {
        assert!(process_is("git\n", b"/usr/bin/git\0status\0", "git"));
        assert!(process_is("UnrealEditor-Cm\n", b"/opt/UE/UnrealEditor-Cmd\0-run\0", "UnrealEditor-Cmd"));
        assert!(!process_is("gitk\n", b"/usr/bin/gitk\0", "git"));
        assert!(!process_is("bash\n", b"/tmp/UnrealEditor-Cmd\0", "UnrealEditor-Cmd"));
    }

    #[test]
    fn sees_its_own_process_running() {
        let me = std::env::current_exe().unwrap();
        let name = me.file_name().unwrap().to_string_lossy().to_string();
        assert!(is_running(&name));
    }

    #[test]
    fn git_speaks_english() {
        let cmd = command("git");
        let env: Vec<_> = cmd.get_envs().collect();
        assert!(env.contains(&(OsStr::new("LANGUAGE"), Some(OsStr::new("C")))));
        assert!(env.contains(&(OsStr::new("GIT_TERMINAL_PROMPT"), Some(OsStr::new("0")))));
        assert!(!command("cargo").get_envs().any(|(k, _)| k == "LANGUAGE"));
    }

    #[test]
    fn stops_a_program_that_shows_no_progress() {
        let ctx = Ctx::standalone();
        let started = Instant::now();
        let err = stream_idle(&ctx, shell("Start-Sleep -Seconds 30", "sleep 30"), Some(Duration::from_secs(2)), |_| true).unwrap_err();
        assert!(err.contains("no progress for 2 seconds"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(20));

        // Progress keeps it alive past the idle limit.
        let busy = shell(
            "foreach ($i in 1..6) { [Console]::Error.Write(\"$i%`r\"); Start-Sleep -Milliseconds 500 }; Write-Output done",
            "for i in 1 2 3 4 5 6; do printf \"$i%%\r\" >&2; sleep 0.5; done; echo done",
        );
        let (ok, lines) = stream_idle(&ctx, busy, Some(Duration::from_secs(2)), |_| true).unwrap();
        assert!(ok);
        assert_eq!(lines, vec!["done".to_string()]);
    }
}
