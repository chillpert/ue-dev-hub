//! Streams log lines and progress to the UI, and to the running job if there is one. Every line
//! also goes to `hub.log` in the app's log folder, so a problem on someone else's machine
//! can be looked at afterwards.

use crate::jobs::JobControl;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// The hub log starts over in a new file past this size; the previous one is kept.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

struct FileLog {
    path: PathBuf,
    file: std::fs::File,
    size: u64,
}

static FILE_LOG: Mutex<Option<FileLog>> = Mutex::new(None);
static LOG_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The previous log next to `path`, like hub.old.log.
fn old_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    path.with_file_name(format!("{stem}.old{ext}"))
}

/// Moves the current log aside as the old one and starts an empty one.
fn start_file(path: &Path) -> std::io::Result<std::fs::File> {
    if path.exists() {
        let old = old_path(path);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(path, &old)?;
    }
    std::fs::File::create(path)
}

/// Starts this run's `hub.log` in `dir`, keeping the last run's as `hub.old.log`.
pub fn init_file_log(dir: &Path) {
    *LOG_DIR.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.to_path_buf());
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join("hub.log");
    let Ok(file) = start_file(&path).or_else(|_| std::fs::OpenOptions::new().create(true).append(true).open(&path)) else { return };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    *FILE_LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(FileLog { path, file, size });
}

/// The folder the hub writes its logs to, once it has started.
pub fn log_dir() -> Option<PathBuf> {
    LOG_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The hub's own log, to attach to bug reports.
pub fn hub_log_path() -> Option<PathBuf> {
    log_dir().map(|d| d.join("hub.log"))
}

/// Adds a line to `hub.log`, if it was started.
pub fn write_file(level: &str, text: &str) {
    let mut guard = FILE_LOG.lock().unwrap_or_else(|e| e.into_inner());
    let Some(log) = guard.as_mut() else { return };
    write_line(log, level, text);
}

fn write_line(log: &mut FileLog, level: &str, text: &str) {
    if log.size > MAX_LOG_BYTES
        && let Ok(file) = start_file(&log.path)
    {
        log.file = file;
        log.size = 0;
    }
    let time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let line = format!("{time} [{level}] {text}\n");
    if log.file.write_all(line.as_bytes()).is_ok() {
        log.size += line.len() as u64;
    }
}

/// Writes panics to `crash.log` in the log folder (and the hub log), then lets the default
/// handler run as before.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let report = format!(
            "{} UE Dev Hub {} panicked on thread '{thread}': {info}\n{backtrace}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            env!("CARGO_PKG_VERSION"),
        );
        let dir = log_dir().unwrap_or_else(std::env::temp_dir);
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) {
            let _ = f.write_all(report.as_bytes());
        }
        // A panic while the log is being written must not deadlock here.
        if let Ok(mut guard) = FILE_LOG.try_lock()
            && let Some(log) = guard.as_mut()
        {
            write_line(log, "panic", &report);
        }
        default(info);
    }));
}

/// A log line on its way to the UI. `id` is set for an addon's steps: those lines are for that
/// addon's page only.
#[derive(Serialize, Clone)]
struct OutLine {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    level: &'static str,
    text: String,
}

// Lines go to the UI in batches, as `log-lines`: a tool can print thousands of lines a second,
// and one event per line keeps the window too busy to answer clicks.
static OUTBOX: Mutex<Vec<OutLine>> = Mutex::new(Vec::new());
static OUTBOX_FILLED: Condvar = Condvar::new();
/// Held while a batch is sent, so two senders can't deliver batches out of order.
static SENDING: Mutex<()> = Mutex::new(());
static SENDER: OnceLock<()> = OnceLock::new();
const SEND_EVERY: Duration = Duration::from_millis(50);

fn queue(app: &AppHandle, line: OutLine) {
    SENDER.get_or_init(|| {
        let app = app.clone();
        std::thread::spawn(move || loop {
            {
                let mut q = OUTBOX.lock().unwrap_or_else(|e| e.into_inner());
                while q.is_empty() {
                    q = OUTBOX_FILLED.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            }
            std::thread::sleep(SEND_EVERY);
            send_lines(&app);
        });
    });
    OUTBOX.lock().unwrap_or_else(|e| e.into_inner()).push(line);
    OUTBOX_FILLED.notify_one();
}

/// Sends the lines waiting for the UI now. Called before a job's state goes out, so a job's last
/// lines arrive while the UI still shows it as running.
pub fn send_lines(app: &AppHandle) {
    let _sending = SENDING.lock().unwrap_or_else(|e| e.into_inner());
    let lines = std::mem::take(&mut *OUTBOX.lock().unwrap_or_else(|e| e.into_inner()));
    if !lines.is_empty() {
        let _ = app.emit("log-lines", lines);
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Progress {
    /// 0 to 100, or None while the tool hasn't said yet.
    pub percent: Option<f32>,
    pub label: String,
}

#[derive(Clone, Default)]
pub struct Log {
    pub app: Option<AppHandle>,
    pub job: Option<Arc<JobControl>>,
    /// Set for an addon's steps: their lines go to that addon's page only.
    pub addon: Option<String>,
}

impl Log {
    pub fn new(app: &AppHandle) -> Self {
        Self { app: Some(app.clone()), job: None, addon: None }
    }

    /// Log for a step of the addon `id`.
    pub fn for_addon(app: &AppHandle, id: &str) -> Self {
        Self { app: Some(app.clone()), job: None, addon: Some(id.to_string()) }
    }

    #[cfg(test)]
    pub fn none() -> Self {
        Self::default()
    }

    fn emit(&self, level: &'static str, text: impl Into<String>) {
        let text = text.into();
        match &self.addon {
            Some(id) => write_file(level, &format!("[addon {id}] {text}")),
            None => write_file(level, &text),
        }
        if let Some(job) = &self.job {
            job.push_line(level, &text);
        }
        if let Some(app) = &self.app {
            queue(app, OutLine { id: self.addon.clone(), level, text });
        }
    }

    /// A step the user might care about.
    pub fn info(&self, text: impl Into<String>) {
        self.emit("info", text);
    }

    /// Raw tool output.
    pub fn detail(&self, text: impl Into<String>) {
        self.emit("detail", text);
    }

    pub fn progress(&self, progress: Progress) {
        if let Some(job) = &self.job {
            job.set_progress(Some(progress.clone()));
        }
        if let Some(app) = &self.app {
            let _ = app.emit("build-progress", progress);
        }
    }

    pub fn cancelled(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.is_cancelled())
    }

    /// Errors with "Cancelled." once the user cancelled the running job.
    pub fn check_cancel(&self) -> Result<(), String> {
        if self.cancelled() { Err(crate::jobs::CANCELLED.into()) } else { Ok(()) }
    }

    pub fn track_pid(&self, pid: u32) {
        if let Some(job) = &self.job {
            job.track_pid(pid);
        }
    }

    pub fn untrack_pid(&self, pid: u32) {
        if let Some(job) = &self.job {
            job.untrack_pid(pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_log_starts_over_and_keeps_the_previous_one() {
        let dir = std::env::temp_dir().join(format!("uel-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hub.log");
        std::fs::write(&path, "last run\n").unwrap();
        let mut log = FileLog { file: start_file(&path).unwrap(), path: path.clone(), size: 0 };
        assert_eq!(std::fs::read_to_string(dir.join("hub.old.log")).unwrap(), "last run\n");
        write_line(&mut log, "info", "first");
        log.size = MAX_LOG_BYTES + 1;
        write_line(&mut log, "info", "second");
        drop(log);
        assert!(std::fs::read_to_string(dir.join("hub.old.log")).unwrap().contains("[info] first"));
        let now = std::fs::read_to_string(&path).unwrap();
        assert!(now.contains("[info] second") && !now.contains("first"), "{now}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
