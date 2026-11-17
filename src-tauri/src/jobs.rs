//! Long-running work like packaging, which keeps going while the user looks at other screens or
//! closes the window. One job runs at a time. The UI reads a snapshot and listens to `job` events.

use crate::log::{Log, Progress};
use crate::proc;
use serde::Serialize;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub const CANCELLED: &str = "Cancelled.";
const MAX_LINES: usize = 3000;

/// A lock that keeps working after a thread panicked while holding it.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StepState {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

/// What a job does. The UI picks the screen that shows it by this.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum JobKind {
    #[default]
    Package,
    Lighting,
    TestMap,
    Upload,
    Soundbanks,
    Dependencies,
    Regenerate,
    RepairGit,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub title: String,
    pub note: String,
    pub state: StepState,
    /// Milliseconds since the Unix epoch, when the step started and when it ended.
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: u64,
    pub kind: JobKind,
    pub title: String,
    pub subtitle: String,
    pub started: String,
    /// Milliseconds since the Unix epoch, for the elapsed time.
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
    pub steps: Vec<Step>,
    pub progress: Option<Progress>,
    pub finished: bool,
    pub ok: bool,
    pub cancelled: bool,
    pub message: Option<String>,
    /// A folder to show when done, like the packaged build.
    pub output_path: Option<PathBuf>,
    pub log_path: Option<PathBuf>,
    /// Compiler errors and similar, without the noise around them.
    pub errors: Vec<String>,
}

pub struct JobControl {
    cancel: AtomicBool,
    pids: Mutex<Vec<u32>>,
    snap: Mutex<Snapshot>,
    lines: Mutex<VecDeque<(String, String)>>,
    app: Option<AppHandle>,
    /// When progress was last sent to the UI, so fast updates don't flood it.
    progress_sent: Mutex<Option<Instant>>,
}

/// Progress reaches the UI at most this often, unless it switches between a bar and busy.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

impl JobControl {
    fn new(snap: Snapshot, app: Option<AppHandle>) -> Arc<JobControl> {
        Arc::new(JobControl {
            cancel: AtomicBool::new(false),
            pids: Mutex::new(vec![]),
            snap: Mutex::new(snap),
            lines: Mutex::new(VecDeque::new()),
            app,
            progress_sent: Mutex::new(None),
        })
    }

    /// Marks it cancelled and stops the programs it started.
    fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let pids = lock(&self.pids).clone();
        for pid in pids {
            proc::kill_tree(pid);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn track_pid(&self, pid: u32) {
        lock(&self.pids).push(pid);
        // Cancel may have come in while the process was starting.
        if self.is_cancelled() {
            proc::kill_tree(pid);
        }
    }

    pub fn untrack_pid(&self, pid: u32) {
        lock(&self.pids).retain(|p| *p != pid);
    }

    pub fn push_line(&self, level: &str, text: &str) {
        let mut lines = lock(&self.lines);
        lines.push_back((level.to_string(), text.to_string()));
        while lines.len() > MAX_LINES {
            lines.pop_front();
        }
    }

    /// Stores the progress and sends it to the UI, which otherwise only hears about step changes.
    pub fn set_progress(&self, p: Option<Progress>) {
        let kind_changed = {
            let mut s = lock(&self.snap);
            let had_bar = s.progress.as_ref().map(|p| p.percent.is_some());
            let has_bar = p.as_ref().map(|p| p.percent.is_some());
            s.progress = p;
            had_bar != has_bar
        };
        let now = Instant::now();
        let mut sent = lock(&self.progress_sent);
        if kind_changed || sent.is_none_or(|t| now - t >= PROGRESS_EVERY) {
            *sent = Some(now);
            drop(sent);
            self.emit();
        }
    }

    fn emit(&self) {
        if let Some(app) = &self.app {
            crate::log::send_lines(app);
            let snap = lock(&self.snap).clone();
            let _ = app.emit("job", snap);
        }
    }

    fn title_if_running(&self) -> Option<String> {
        let s = lock(&self.snap);
        (!s.finished).then(|| s.title.clone())
    }
}

/// Everything that decides whether new work may start, under one lock, so checking and claiming
/// happen together and a job and a step can never start side by side.
struct State {
    /// The running job, or the last one until it's dismissed.
    current: Option<Arc<JobControl>>,
    /// A step outside jobs that changes the repo or project, like rebasing or compiling.
    busy: Option<String>,
    /// Lets Cancel and Quit stop the running step, for steps that are safe to stop, like compiling.
    step_ctl: Option<Arc<JobControl>>,
    /// A multi-step flow the UI drives, like Update & Launch. Other work waits until it ends.
    flow: Option<String>,
}

impl State {
    fn running_job(&self) -> Option<String> {
        self.current.as_ref().and_then(|j| j.title_if_running())
    }

    fn busy_with(&self) -> Option<String> {
        self.busy_except_flow().or_else(|| self.flow.clone())
    }

    fn busy_except_flow(&self) -> Option<String> {
        self.running_job().or_else(|| self.busy.clone())
    }
}

static STATE: Mutex<State> = Mutex::new(State { current: None, busy: None, step_ctl: None, flow: None });
static QUITTING: AtomicBool = AtomicBool::new(false);

fn state() -> MutexGuard<'static, State> {
    lock(&STATE)
}

/// Held while a step runs; ends the busy state when dropped, also on errors and panics.
#[derive(Debug)]
pub struct BusyGuard;

impl Drop for BusyGuard {
    fn drop(&mut self) {
        let mut st = state();
        st.busy = None;
        st.step_ctl = None;
    }
}

/// What's keeping the hub busy, if anything: a job, a step or a flow.
pub fn busy_with() -> Option<String> {
    state().busy_with()
}

/// Like `busy_with`, but a flow alone doesn't count: only a job or a step that's running now.
/// The update check uses it, since Update & Launch checks for updates as part of its own flow.
pub fn busy_except_flow() -> Option<String> {
    state().busy_except_flow()
}

fn refuse_if_quitting() -> Result<(), String> {
    if QUITTING.load(Ordering::SeqCst) { Err("The hub is closing.".into()) } else { Ok(()) }
}

/// The step that's changing the repo right now, if any. Unlike `busy_with`, the running
/// Update & Launch flow alone doesn't count, so the flow can still open the editor.
pub fn step_running() -> Option<String> {
    state().busy.clone()
}

/// Starts a step that changes the repo or project. Fails while a job or another step runs.
/// Steps of the running flow pass `in_flow`, so the flow doesn't block itself.
pub fn begin(what: &str, in_flow: bool) -> Result<BusyGuard, String> {
    refuse_if_quitting()?;
    let mut st = state();
    if let Some(job) = st.running_job() {
        return Err(format!("{job} is still running. Wait for it to finish first."));
    }
    if !in_flow && let Some(flow) = &st.flow {
        return Err(format!("{flow} is still running. Wait for it to finish first."));
    }
    if let Some(other) = &st.busy {
        return Err(format!("{other} is still running. Wait for it to finish first."));
    }
    st.busy = Some(what.to_string());
    Ok(BusyGuard)
}

/// Like `begin`, for a step that may be stopped halfway, like compiling. Cancel and Quit stop
/// the programs it starts through the returned log. Never use it for a rebase or anything else
/// that must not be cut off.
pub fn begin_cancellable(app: &AppHandle, what: &str, in_flow: bool) -> Result<(BusyGuard, Log), String> {
    let guard = begin(what, in_flow)?;
    // No app on the control: a step's progress goes to the UI through the log, not as a job.
    let ctl = JobControl::new(Snapshot { title: what.to_string(), ..Default::default() }, None);
    state().step_ctl = Some(ctl.clone());
    Ok((guard, Log { app: Some(app.clone()), job: Some(ctl), addon: None }))
}

/// Marks the start or end of a flow like Update & Launch.
pub fn set_flow(name: Option<String>) -> Result<(), String> {
    let mut st = state();
    if name.is_some() {
        refuse_if_quitting()?;
        if let Some(other) = st.busy_with() {
            return Err(format!("{other} is still running. Wait for it to finish first."));
        }
    }
    st.flow = name;
    Ok(())
}

/// Stops new work from starting and cancels the running job. The caller waits for `busy_with`
/// to be empty before exiting, so cleanup (putting files back, aborting a rebase) still runs.
pub fn begin_quit() {
    QUITTING.store(true, Ordering::SeqCst);
    state().flow = None;
    // Called from the tray menu on the main thread; stopping programs can take a while.
    std::thread::spawn(cancel);
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// What a job hands to its work function.
pub struct Job {
    pub ctl: Arc<JobControl>,
    pub log: Log,
}

impl Job {
    pub fn step(&self, index: usize, state: StepState, note: impl Into<String>) {
        {
            let mut s = lock(&self.ctl.snap);
            if let Some(step) = s.steps.get_mut(index) {
                let now = now_ms();
                if state == StepState::Running {
                    step.started_ms.get_or_insert(now);
                    step.ended_ms = None;
                } else if step.started_ms.is_some() && state != StepState::Pending {
                    step.ended_ms = Some(now);
                }
                step.state = state;
                step.note = note.into();
            }
            if state != StepState::Running {
                s.progress = None;
            }
        }
        self.ctl.emit();
    }

    pub fn running(&self, index: usize, note: impl Into<String>) {
        self.step(index, StepState::Running, note);
    }

    pub fn done(&self, index: usize, note: impl Into<String>) {
        self.step(index, StepState::Done, note);
    }

    pub fn failed(&self, index: usize, note: impl Into<String>) {
        self.step(index, StepState::Failed, note);
    }

    pub fn skipped(&self, index: usize, note: impl Into<String>) {
        self.step(index, StepState::Skipped, note);
    }

    pub fn set_output(&self, path: PathBuf) {
        lock(&self.ctl.snap).output_path = Some(path);
    }

    pub fn set_log_path(&self, path: PathBuf) {
        lock(&self.ctl.snap).log_path = Some(path);
        self.ctl.emit();
    }

    pub fn set_errors(&self, errors: Vec<String>) {
        lock(&self.ctl.snap).errors = errors;
    }
}

pub fn is_running() -> bool {
    state().running_job().is_some()
}

/// Starts `work` on its own thread. Fails if another job, a step or a flow is still running.
pub fn start(
    app: &AppHandle,
    kind: JobKind,
    title: String,
    subtitle: String,
    steps: Vec<String>,
    work: impl FnOnce(&Job) -> Result<String, String> + Send + 'static,
) -> Result<Snapshot, String> {
    refuse_if_quitting()?;
    let mut st = state();
    if let Some(other) = st.busy.clone().or_else(|| st.flow.clone()) {
        return Err(format!("{other} is still running. Wait for it to finish first."));
    }
    if let Some(job) = st.running_job() {
        return Err(format!("{job} is still running. Wait for it or cancel it first."));
    }

    let snap = Snapshot {
        id: NEXT_ID.fetch_add(1, Ordering::SeqCst),
        kind,
        title,
        subtitle,
        started: chrono::Local::now().format("%H:%M").to_string(),
        started_ms: now_ms(),
        steps: steps.into_iter().map(|t| Step { title: t, note: String::new(), state: StepState::Pending, started_ms: None, ended_ms: None }).collect(),
        ..Default::default()
    };
    let ctl = JobControl::new(snap.clone(), Some(app.clone()));
    st.current = Some(ctl.clone());
    drop(st);
    crate::log::write_file("job", &format!("{} started", snap.title));

    let app = app.clone();
    let job = Job { ctl: ctl.clone(), log: Log { app: Some(app.clone()), job: Some(ctl.clone()), addon: None } };
    ctl.emit();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&job)))
            .unwrap_or_else(|_| Err("The hub hit an internal error. Copy the output above and report it with the Report a problem page.".into()));
        let (title, body) = {
            let mut s = lock(&ctl.snap);
            s.finished = true;
            s.ended_ms = Some(now_ms());
            s.progress = None;
            s.cancelled = ctl.is_cancelled();
            match result {
                Ok(msg) => {
                    s.ok = true;
                    s.message = Some(msg);
                }
                Err(e) => {
                    s.ok = false;
                    s.message = Some(if s.cancelled { CANCELLED.into() } else { e });
                }
            }
            // Steps still marked running didn't finish.
            let unfinished = if s.cancelled { StepState::Skipped } else { StepState::Failed };
            let ended = s.ended_ms;
            for step in s.steps.iter_mut().filter(|st| st.state == StepState::Running) {
                step.state = unfinished;
                step.ended_ms = ended;
            }
            let verdict = if s.ok { "finished" } else if s.cancelled { "was cancelled" } else { "failed" };
            (format!("{} {verdict}", s.title), s.message.clone().unwrap_or_default())
        };
        crate::log::write_file("job", &format!("{title}: {body}"));
        ctl.emit();
        notify_if_hidden(&app, &title, &body);
    });
    Ok(snap)
}

pub fn snapshot() -> Option<(Snapshot, Vec<(String, String)>)> {
    let j = state().current.clone()?;
    let snap = lock(&j.snap).clone();
    let lines = lock(&j.lines).iter().cloned().collect();
    Some((snap, lines))
}

/// Cancels the running job, or a running step that can be stopped, and stops the programs it
/// started. Killing can take a while, so it happens after every lock is released; call it off
/// the main thread.
pub fn cancel() {
    let (job, step) = {
        let st = state();
        (st.current.clone(), st.step_ctl.clone())
    };
    for ctl in job.iter().chain(step.iter()) {
        ctl.cancel();
    }
}

/// Forgets a finished job, so its screen goes back to the form.
pub fn dismiss() {
    let mut st = state();
    if st.current.as_ref().is_some_and(|j| j.title_if_running().is_none()) {
        st.current = None;
    }
}

fn notify_if_hidden(app: &AppHandle, title: &str, body: &str) {
    let visible = app
        .get_webview_window("main")
        .map(|w| w.is_visible().unwrap_or(true) && w.is_focused().unwrap_or(true) && !w.is_minimized().unwrap_or(false))
        .unwrap_or(true);
    if visible {
        return;
    }
    use tauri_plugin_notification::NotificationExt;
    let body: String = body.chars().take(200).collect();
    let _ = app.notification().builder().title(title).body(body).show();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state is global; tests that use it take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn only_one_thing_changes_the_project_at_a_time() {
        let _serial = lock(&SERIAL);
        let a = begin("Compiling", false).unwrap();
        assert!(begin("Cleaning build files", false).unwrap_err().contains("Compiling"));
        assert!(set_flow(Some("Update & Launch".into())).is_err());
        drop(a);

        set_flow(Some("Update & Launch".into())).unwrap();
        // Other work waits for the flow; the flow's own steps go ahead.
        assert!(begin("Cleaning build files", false).unwrap_err().contains("Update & Launch"));
        let step = begin("Updating", true).unwrap();
        assert_eq!(busy_with().as_deref(), Some("Updating"));
        drop(step);
        set_flow(None).unwrap();
        assert!(busy_with().is_none());
    }

    #[test]
    fn a_flow_alone_doesnt_stop_the_update_check_from_fetching() {
        let _serial = lock(&SERIAL);
        set_flow(Some("Update & Launch".into())).unwrap();
        assert_eq!(busy_with().as_deref(), Some("Update & Launch"));
        assert!(busy_except_flow().is_none());
        let step = begin("Updating", true).unwrap();
        assert_eq!(busy_except_flow().as_deref(), Some("Updating"));
        drop(step);
        set_flow(None).unwrap();
        assert!(busy_except_flow().is_none());
    }

    #[test]
    fn steps_started_at_the_same_time_never_both_run() {
        let _serial = lock(&SERIAL);
        for _ in 0..50 {
            let barrier = Arc::new(std::sync::Barrier::new(8));
            let handles: Vec<_> = (0..8)
                .map(|i| {
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        let guard = begin(&format!("Step {i}"), false).ok();
                        // Hold it until every thread has tried.
                        barrier.wait();
                        guard.is_some()
                    })
                })
                .collect();
            let started = handles.into_iter().map(|h| h.join().unwrap()).filter(|ok| *ok).count();
            assert_eq!(started, 1);
            assert!(busy_with().is_none());
        }
    }

    #[test]
    fn a_panic_while_busy_doesnt_block_later_work() {
        let _serial = lock(&SERIAL);
        let r = std::panic::catch_unwind(|| {
            let _guard = begin("Compiling", false).unwrap();
            panic!("boom");
        });
        assert!(r.is_err());
        assert!(busy_with().is_none());
        drop(begin("Compiling", false).unwrap());
    }

    #[test]
    fn cancel_reaches_a_step_that_can_be_stopped() {
        let _serial = lock(&SERIAL);
        let guard = begin("Compiling", false).unwrap();
        let ctl = JobControl::new(Snapshot::default(), None);
        state().step_ctl = Some(ctl.clone());
        let log = Log { app: None, job: Some(ctl), addon: None };
        assert!(log.check_cancel().is_ok());
        cancel();
        assert_eq!(log.check_cancel().unwrap_err(), CANCELLED);
        drop(guard);
        assert!(state().step_ctl.is_none());
    }

    #[test]
    fn step_states_and_kinds_keep_their_names_for_the_ui() {
        assert_eq!(serde_json::to_value(StepState::Running).unwrap(), "running");
        assert_eq!(serde_json::to_value(JobKind::TestMap).unwrap(), "testMap");
        assert_eq!(serde_json::to_value(JobKind::RepairGit).unwrap(), "repairGit");
    }
}
