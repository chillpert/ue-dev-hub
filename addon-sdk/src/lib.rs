//! For the helper programs of UE Dev Hub addons.
//!
//! A helper's `main` calls [`serve`] with one function that handles every call from the addon's
//! page. The hub starts the helper the first time the page calls it and stops it when the
//! addon is turned off, updated or the hub closes.
//!
//! ```no_run
//! ue_dev_hub_addon::serve(|ctx, method, _params| match method {
//!     "hello" => Ok(serde_json::json!(format!("Hello from {}", ctx.project()?.name))),
//!     _ => Err(format!("Unknown call: {method}")),
//! })
//! ```
//!
//! Never print to stdout yourself: the hub reads it as the protocol. Use [`Ctx::info`] and
//! [`Ctx::detail`] for the hub's log, and stderr for anything else.

pub mod cmd;
pub mod protocol;

use protocol::{FromHelper, ProjectContext, ToHelper};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

trait Link: Send + Sync {
    fn send(&self, msg: FromHelper);
    fn host(&self, id: u64, name: &str, params: Value) -> Result<Value, String>;
}

/// One call from the hub: its project, and the way back for log lines and host requests.
///
/// It can be cloned and sent to other threads. Host requests from clones of one call take turns,
/// because the hub answers them by call id, one at a time.
#[derive(Clone)]
pub struct Ctx {
    id: u64,
    project: Option<ProjectContext>,
    step: bool,
    quiet: bool,
    link: Arc<dyn Link>,
    asking: Arc<Mutex<()>>,
}

impl Ctx {
    /// The project the call is about. Errors when the hub has no project open.
    pub fn project(&self) -> Result<&ProjectContext, String> {
        self.project.as_ref().ok_or_else(|| "Open a project first.".to_string())
    }

    /// Whether the hub runs this call as a step, so nothing else changes the project meanwhile.
    pub fn is_step(&self) -> bool {
        self.step
    }

    /// The same call, without log lines.
    pub fn quiet(&self) -> Ctx {
        Ctx { quiet: true, ..self.clone() }
    }

    fn log(&self, level: &str, text: String) {
        if !self.quiet {
            self.link.send(FromHelper::Log { id: self.id, level: level.into(), text });
        }
    }

    /// A step the user might care about.
    pub fn info(&self, text: impl Into<String>) {
        self.log("info", text.into());
    }

    /// Raw tool output.
    pub fn detail(&self, text: impl Into<String>) {
        self.log("detail", text.into());
    }

    /// Asks the hub for one of its services, see [`protocol::host`].
    pub fn host(&self, name: &str, params: Value) -> Result<Value, String> {
        let _turn = self.asking.lock().unwrap_or_else(|e| e.into_inner());
        self.link.host(self.id, name, params)
    }

    /// [`Ctx::host`], read into a type.
    pub fn host_as<T: DeserializeOwned>(&self, name: &str, params: Value) -> Result<T, String> {
        serde_json::from_value(self.host(name, params)?).map_err(|e| format!("The hub's answer to {name} couldn't be read: {e}"))
    }

    /// A call without a hub or project: no log lines, and host requests fail. For running
    /// code that needs a `Ctx` outside of a call.
    pub fn standalone() -> Ctx {
        let mut ctx = Ctx::for_tests(ProjectContext::default(), false, |name, _| Err(format!("'{name}' needs the hub.")));
        ctx.project = None;
        ctx.quiet = true;
        ctx
    }

    /// A call that isn't connected to a hub, for tests. `host` answers host requests;
    /// log lines go to stderr.
    pub fn for_tests(project: ProjectContext, step: bool, host: impl Fn(&str, Value) -> Result<Value, String> + Send + Sync + 'static) -> Ctx {
        struct TestLink<H>(H);
        impl<H: Fn(&str, Value) -> Result<Value, String> + Send + Sync> Link for TestLink<H> {
            fn send(&self, msg: FromHelper) {
                if let FromHelper::Log { text, .. } = msg {
                    eprintln!("{text}");
                }
            }
            fn host(&self, _id: u64, name: &str, params: Value) -> Result<Value, String> {
                (self.0)(name, params)
            }
        }
        Ctx { id: 0, project: Some(project), step, quiet: false, link: Arc::new(TestLink(host)), asking: Arc::default() }
    }
}

struct StdioLink {
    out: Mutex<std::io::Stdout>,
    waiting: Mutex<HashMap<u64, mpsc::Sender<Result<Value, String>>>>,
}

impl Link for StdioLink {
    fn send(&self, msg: FromHelper) {
        let Ok(mut line) = serde_json::to_vec(&msg) else { return };
        line.push(b'\n');
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = out.write_all(&line).and_then(|_| out.flush());
    }

    fn host(&self, id: u64, name: &str, params: Value) -> Result<Value, String> {
        let (tx, rx) = mpsc::channel();
        self.waiting.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
        self.send(FromHelper::Host { id, name: name.into(), params });
        rx.recv().unwrap_or_else(|_| Err("The hub stopped answering.".into()))
    }
}

type Handler = dyn Fn(&Ctx, &str, Value) -> Result<Value, String> + Send + Sync;

/// Answers the hub's calls until it closes the helper's stdin. Each call runs on its own
/// thread, so a slow step doesn't hold up the page's reads.
pub fn serve(handler: impl Fn(&Ctx, &str, Value) -> Result<Value, String> + Send + Sync + 'static) -> ! {
    let handler: Arc<Handler> = Arc::new(handler);
    let link = Arc::new(StdioLink { out: Mutex::new(std::io::stdout()), waiting: Mutex::new(HashMap::new()) });
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ToHelper>(&line) {
            Ok(ToHelper::Call { id, method, params, project, step }) => {
                let (handler, link) = (handler.clone(), link.clone());
                std::thread::spawn(move || {
                    let ctx = Ctx { id, project, step, quiet: false, link: link.clone(), asking: Arc::default() };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(&ctx, &method, params)))
                        .unwrap_or_else(|_| Err(format!("The addon hit an internal error in '{method}'.")));
                    link.send(match result {
                        Ok(value) => FromHelper::Result { id, value },
                        Err(error) => FromHelper::Error { id, error },
                    });
                });
            }
            Ok(ToHelper::HostResult { id, value, error }) => {
                if let Some(tx) = link.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
                    let _ = tx.send(match error {
                        Some(e) => Err(e),
                        None => Ok(value),
                    });
                }
            }
            Err(e) => eprintln!("Ignored a message the helper couldn't read: {e}"),
        }
    }
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_calls_reach_the_host_function() {
        let ctx = Ctx::for_tests(ProjectContext { name: "Demo".into(), ..Default::default() }, true, |name, params| Ok(json!([name, params])));
        assert_eq!(ctx.project().unwrap().name, "Demo");
        assert!(ctx.is_step());
        assert_eq!(ctx.host("repoBusy", json!(1)).unwrap(), json!(["repoBusy", 1]));
        let n: Vec<Value> = ctx.host_as("x", Value::Null).unwrap();
        assert_eq!(n.len(), 2);
    }

    /// The hub answers host requests by call id, so a call's threads must never wait for
    /// two answers at once: one would get the other's answer.
    #[test]
    fn host_requests_of_one_call_take_turns() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let waiting = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let (w, m) = (waiting.clone(), most.clone());
        let ctx = Ctx::for_tests(ProjectContext::default(), false, move |name, _| {
            let now = w.fetch_add(1, Ordering::SeqCst) + 1;
            m.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(20));
            w.fetch_sub(1, Ordering::SeqCst);
            Ok(json!(name))
        });
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let ctx = ctx.clone();
                std::thread::spawn(move || ctx.host(&format!("ask{i}"), Value::Null).unwrap())
            })
            .collect();
        for (i, t) in threads.into_iter().enumerate() {
            assert_eq!(t.join().unwrap(), json!(format!("ask{i}")));
        }
        assert_eq!(most.load(Ordering::SeqCst), 1);
    }
}
