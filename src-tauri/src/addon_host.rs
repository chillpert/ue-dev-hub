//! Addon helper programs: starting them, passing calls from the window, and answering their
//! requests for the hub's own services. The protocol is in `ue_dev_hub_addon::protocol`.
//!
//! One helper runs per addon, started on its first call. It's stopped when the addon is turned
//! off, updated or removed, and when the hub exits. A helper also exits by itself when the
//! hub goes away, because its stdin closes. Turning off, updating or removing an addon waits
//! until its helper has no calls running, so a step is never cut off halfway that way. Quitting
//! waits for a running step too; Cancel, or quitting a second time after a long wait, stops it.

use crate::log::Log;
use crate::project::Project;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, mpsc};
use std::time::{Duration, Instant};
use ue_dev_hub_addon::protocol::{FromHelper, ProjectContext, ToHelper, host};

struct Helper {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    /// Calls waiting for messages, by call id. Emptied when the helper's output ends.
    waiting: Arc<Mutex<HashMap<u64, mpsc::Sender<FromHelper>>>>,
    /// The last lines the helper wrote to stderr, for when it crashes.
    stderr: Arc<Mutex<VecDeque<String>>>,
    /// Steps running on it right now.
    steps: AtomicUsize,
    /// Stopped on purpose, by cancelling or quitting.
    stopped: AtomicBool,
}

static HELPERS: LazyLock<Mutex<HashMap<String, Arc<Helper>>>> = LazyLock::new(Default::default);
/// Addons being updated or removed. Their helpers don't start until that's done.
static BLOCKED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// How long a call that isn't a step may take. Generous, since one may wait for a sign-in window.
const CALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

impl Helper {
    fn start(id: &str, exe: &Path, dir: &Path) -> Result<Helper, String> {
        let mut cmd = crate::proc::command(exe);
        cmd.current_dir(dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("Couldn't start the {id} addon's helper: {e}"))?;
        let stdin = child.stdin.take().ok_or("The helper has no input.")?;
        let stdout = child.stdout.take().ok_or("The helper has no output.")?;
        let stderr_pipe = child.stderr.take().ok_or("The helper has no error output.")?;

        let waiting: Arc<Mutex<HashMap<u64, mpsc::Sender<FromHelper>>>> = Default::default();
        let stderr: Arc<Mutex<VecDeque<String>>> = Default::default();
        {
            let waiting = waiting.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(msg) = serde_json::from_str::<FromHelper>(&line) else { continue };
                    let tx = waiting.lock().unwrap_or_else(|e| e.into_inner()).get(&msg.id()).cloned();
                    if let Some(tx) = tx {
                        let _ = tx.send(msg);
                    }
                }
                // The helper is gone: every call still waiting gets a closed channel.
                waiting.lock().unwrap_or_else(|e| e.into_inner()).clear();
            });
        }
        {
            let stderr = stderr.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr_pipe).lines() {
                    let Ok(line) = line else { break };
                    let mut tail = stderr.lock().unwrap_or_else(|e| e.into_inner());
                    tail.push_back(line);
                    if tail.len() > 20 {
                        tail.pop_front();
                    }
                }
            });
        }
        Ok(Helper { child: Mutex::new(child), stdin: Mutex::new(stdin), waiting, stderr, steps: AtomicUsize::new(0), stopped: AtomicBool::new(false) })
    }

    fn busy(&self) -> bool {
        !self.waiting.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    fn running(&self) -> bool {
        self.child.lock().unwrap_or_else(|e| e.into_inner()).try_wait().is_ok_and(|s| s.is_none())
    }

    fn send(&self, msg: &ToHelper) -> Result<(), String> {
        let mut line = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        stdin.write_all(&line).and_then(|_| stdin.flush()).map_err(|_| "The addon's helper stopped.".to_string())
    }

    fn crash_message(&self, name: &str) -> String {
        let tail: Vec<String> = self.stderr.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect();
        let mut msg = format!("The {name} addon's helper stopped unexpectedly. Try again; it starts again by itself.");
        if !tail.is_empty() {
            msg.push_str(&format!("\n\nIt said:\n{}", tail.join("\n")));
        }
        msg
    }

    fn kill(&self) {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        if child.try_wait().is_ok_and(|s| s.is_none()) {
            crate::proc::kill_tree(child.id());
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

/// The helper for `id`, started if it isn't running.
fn helper(name: &str, id: &str, exe: &Path, dir: &Path) -> Result<Arc<Helper>, String> {
    let mut helpers = HELPERS.lock().unwrap_or_else(|e| e.into_inner());
    if BLOCKED.lock().unwrap_or_else(|e| e.into_inner()).contains(id) {
        return Err(format!("{name} is being updated or removed. Try again in a moment."));
    }
    if let Some(h) = helpers.get(id)
        && h.running()
    {
        return Ok(h.clone());
    }
    let h = Arc::new(Helper::start(id, exe, dir)?);
    helpers.insert(id.to_string(), h.clone());
    Ok(h)
}

/// Keeps an addon's helper from starting while it exists, for replacing or removing its files.
pub struct Blocked(String);

impl Drop for Blocked {
    fn drop(&mut self) {
        BLOCKED.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

/// Stops an addon's helper and waits until it has exited, so its files can be replaced. Refuses
/// while the helper is running a call, like a step that's halfway through changing the project.
/// No new helper starts until the returned guard is dropped.
pub fn stop_when_idle(name: &str, id: &str) -> Result<Blocked, String> {
    let h = {
        let mut helpers = HELPERS.lock().unwrap_or_else(|e| e.into_inner());
        if helpers.get(id).is_some_and(|h| h.running() && h.busy()) {
            return Err(format!("Wait for {name} to finish, then try again."));
        }
        let mut blocked = BLOCKED.lock().unwrap_or_else(|e| e.into_inner());
        if !blocked.insert(id.to_string()) {
            return Err(format!("{name} is already being updated or removed."));
        }
        helpers.remove(id)
    };
    let guard = Blocked(id.to_string());
    if let Some(h) = h {
        h.kill();
    }
    Ok(guard)
}

pub fn stop_all() {
    let all: Vec<Arc<Helper>> = HELPERS.lock().unwrap_or_else(|e| e.into_inner()).drain().map(|(_, h)| h).collect();
    for h in all {
        h.stopped.store(true, Ordering::SeqCst);
        h.kill();
    }
}

/// Whether any helper is running a step right now.
pub fn step_running() -> bool {
    HELPERS.lock().unwrap_or_else(|e| e.into_inner()).values().any(|h| h.steps.load(Ordering::SeqCst) > 0)
}

/// Stops the helpers that are running a step, for Cancel. The step's call ends as cancelled.
pub fn cancel_steps() {
    let running: Vec<Arc<Helper>> = {
        let mut helpers = HELPERS.lock().unwrap_or_else(|e| e.into_inner());
        let ids: Vec<String> = helpers.iter().filter(|(_, h)| h.steps.load(Ordering::SeqCst) > 0).map(|(id, _)| id.clone()).collect();
        ids.iter().filter_map(|id| helpers.remove(id)).collect()
    };
    for h in running {
        h.stopped.store(true, Ordering::SeqCst);
        h.kill();
    }
}

pub fn context(p: &Project) -> ProjectContext {
    ProjectContext {
        root: p.root.clone(),
        uproject: p.uproject.clone(),
        name: p.display_name(),
        base_branch: p.default_branch(),
        git_update: p.config.git.update.as_str().into(),
        git_push: p.config.git.push.as_str().into(),
        branch_prefixes: p.config.git.branch_prefixes.clone(),
    }
}

/// The addon a call goes to.
pub struct Target<'a> {
    /// For messages, like "Git".
    pub name: &'a str,
    pub id: &'a str,
    pub exe: &'a Path,
    /// The addon's folder, where the helper runs.
    pub dir: &'a Path,
}

/// Runs one call on a helper and returns its result. Log lines go to `log`; requests for the
/// hub's services go to `serve`.
pub fn call(
    target: &Target,
    method: &str,
    params: Value,
    project: Option<ProjectContext>,
    step: bool,
    log: &Log,
    serve: &dyn Fn(&str, Value) -> Result<Value, String>,
) -> Result<Value, String> {
    let (name, id) = (target.name, target.id);
    let helper = helper(name, id, target.exe, target.dir)?;
    let call_id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = mpsc::channel();
    helper.waiting.lock().unwrap_or_else(|e| e.into_inner()).insert(call_id, tx);
    if step {
        helper.steps.fetch_add(1, Ordering::SeqCst);
    }
    // Steps can take as long as they need; Cancel stops them.
    let deadline = (!step).then(|| Instant::now() + CALL_TIMEOUT);
    let result = (|| {
        helper.send(&ToHelper::Call { id: call_id, method: method.into(), params, project, step })?;
        loop {
            let msg = match deadline {
                Some(d) => match rx.recv_timeout(d.saturating_duration_since(Instant::now())) {
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        stop_hung(id, &helper);
                        return Err(format!("The {name} addon didn't answer for {} minutes, so it was stopped. Try again.", CALL_TIMEOUT.as_secs() / 60));
                    }
                    other => other.map_err(|_| ()),
                },
                None => rx.recv().map_err(|_| ()),
            };
            match msg {
                Ok(FromHelper::Log { level, text, .. }) => {
                    if level == "info" {
                        log.info(text);
                    } else {
                        log.detail(text);
                    }
                }
                Ok(FromHelper::Host { name, params, .. }) => {
                    let reply = match serve(&name, params) {
                        Ok(value) => ToHelper::HostResult { id: call_id, value, error: None },
                        Err(e) => ToHelper::HostResult { id: call_id, value: Value::Null, error: Some(e) },
                    };
                    helper.send(&reply)?;
                }
                Ok(FromHelper::Result { value, .. }) => return Ok(value),
                Ok(FromHelper::Error { error, .. }) => return Err(error),
                Err(_) if helper.stopped.load(Ordering::SeqCst) => return Err(crate::jobs::CANCELLED.into()),
                Err(_) => return Err(helper.crash_message(name)),
            }
        }
    })();
    if step {
        helper.steps.fetch_sub(1, Ordering::SeqCst);
    }
    helper.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&call_id);
    result
}

/// Stops a helper that stopped answering, unless it was replaced meanwhile.
fn stop_hung(id: &str, helper: &Arc<Helper>) {
    let mut helpers = HELPERS.lock().unwrap_or_else(|e| e.into_inner());
    if helpers.get(id).is_some_and(|h| Arc::ptr_eq(h, helper)) {
        helpers.remove(id);
    }
    drop(helpers);
    helper.kill();
}

/// The hub's services for helpers, see `ue_dev_hub_addon::protocol::host`. The ones that
/// change the project only work in a step, which holds the job lock.
pub fn serve(log: &Log, project: Option<&Project>, step: bool, name: &str, params: Value) -> Result<Value, String> {
    let project = project.ok_or("Open a project first.")?;
    let in_step = |what: &str| if step { Ok(()) } else { Err(format!("{what} only works in a step. This is a bug in the addon.")) };
    match name {
        // A step owns the repo and may fetch; otherwise something else might be running.
        host::CHECK_UPDATES if step => to_json(crate::git::check_updates(log, project)),
        host::CHECK_UPDATES => to_json(crate::git::check_updates_if_idle(log, project)),
        host::APPLY_UPDATE => {
            in_step("Updating")?;
            let push = params.get("push").and_then(Value::as_bool).unwrap_or(false);
            to_json(crate::git::apply_update(log, project, push))
        }
        host::REPO_BUSY => Ok(json!(crate::git::repo_busy(project))),
        host::FOREIGN_SERVER_COMMITS => {
            let field = |k: &str| params.get(k).and_then(Value::as_str).map(str::to_string).ok_or(format!("{name} needs '{k}'."));
            let (branch, server) = (field("branch")?, field("server")?);
            if !crate::config::is_git_branch_name(&branch) || !server.chars().all(|c| c.is_ascii_hexdigit()) || server.is_empty() {
                return Err(format!("{name} got a bad branch or commit."));
            }
            to_json(crate::git::foreign_server_commits(log, project, &branch, &server))
        }
        host::UPDATE_SUBMODULES => {
            in_step("Updating submodules")?;
            Ok(json!(crate::git::update_submodules(log, project)))
        }
        host::EDITOR_RUNNING => Ok(json!(crate::proc::editor_running_for(&project.uproject))),
        _ => Err(format!("The hub doesn't offer '{name}'. The addon may need a newer hub.")),
    }
}

fn to_json<T: serde::Serialize>(v: Result<T, String>) -> Result<Value, String> {
    v.and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string()))
}
