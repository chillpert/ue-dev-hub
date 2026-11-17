//! The messages between the hub and an addon's helper program: one JSON object per line,
//! the hub writing to the helper's stdin and reading its stdout. stderr is free text the
//! hub keeps the end of, to show when the helper crashes.
//!
//! Every exchange belongs to a call (`id`). While a call runs, the helper may log lines and ask
//! the hub for its own services (`Host`); each `Host` gets exactly one `HostResult` before
//! the helper asks again. A call ends with `Result` or `Error`. Calls can overlap.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// The version of this protocol and of the frontend API. A hub only loads addons made for
/// the same version.
pub const API_VERSION: u32 = 1;

/// The project a call is about, from its `ue-dev-hub.toml`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContext {
    pub root: PathBuf,
    pub uproject: PathBuf,
    pub name: String,
    /// The team's main branch, like "develop".
    pub base_branch: String,
    /// `git.update`: "rebase" or "merge".
    pub git_update: String,
    /// `git.push`.
    pub git_push: String,
    /// `git.branch_prefixes`: the kinds of branch to pick from, like "feat". Empty: any name.
    #[serde(default)]
    pub branch_prefixes: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ToHelper {
    Call {
        id: u64,
        method: String,
        #[serde(default)]
        params: Value,
        project: Option<ProjectContext>,
        /// Set when the hub runs this call as a step that may change the project.
        #[serde(default)]
        step: bool,
    },
    HostResult {
        id: u64,
        #[serde(default)]
        value: Value,
        error: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FromHelper {
    /// `level` is "info" (a step the user might care about) or "detail" (raw tool output).
    Log { id: u64, level: String, text: String },
    Host {
        id: u64,
        name: String,
        #[serde(default)]
        params: Value,
    },
    Result {
        id: u64,
        #[serde(default)]
        value: Value,
    },
    Error { id: u64, error: String },
}

impl FromHelper {
    pub fn id(&self) -> u64 {
        match self {
            FromHelper::Log { id, .. } | FromHelper::Host { id, .. } | FromHelper::Result { id, .. } | FromHelper::Error { id, .. } => *id,
        }
    }
}

/// What the hub offers helpers through `Host`. Params and results are JSON.
pub mod host {
    /// The hub's update check for the current branch. Returns its `UpdateInfo`.
    pub const CHECK_UPDATES: &str = "checkUpdates";
    /// The hub's protected Update flow (`{"push": bool}`). Only in a step.
    pub const APPLY_UPDATE: &str = "applyUpdate";
    /// An unfinished merge, rebase or a Git lock file, as a message. Null when the repo is idle.
    pub const REPO_BUSY: &str = "repoBusy";
    /// How many commits on the server's copy of `branch` (at commit `server`) aren't yours
    /// (`{"branch", "server"}`). Commits the hub kept as backups of your rebases count as yours.
    pub const FOREIGN_SERVER_COMMITS: &str = "foreignServerCommits";
    /// Brings submodules to the commits the branch wants. Returns warnings. Only in a step.
    pub const UPDATE_SUBMODULES: &str = "updateSubmodules";
    /// Whether an Unreal Editor has this project open.
    pub const EDITOR_RUNNING: &str = "editorRunning";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_round_trip_as_tagged_json() {
        let call = ToHelper::Call { id: 3, method: "overview".into(), params: json!({"a": 1}), project: Some(ProjectContext::default()), step: false };
        let text = serde_json::to_string(&call).unwrap();
        assert!(text.contains(r#""type":"call""#), "{text}");
        assert_eq!(serde_json::from_str::<ToHelper>(&text).unwrap(), call);

        let host: FromHelper = serde_json::from_str(r#"{"type":"host","id":3,"name":"repoBusy"}"#).unwrap();
        assert_eq!(host, FromHelper::Host { id: 3, name: "repoBusy".into(), params: Value::Null });
        let reply: ToHelper = serde_json::from_str(r#"{"type":"hostResult","id":3,"error":"x"}"#).unwrap();
        assert_eq!(reply, ToHelper::HostResult { id: 3, value: Value::Null, error: Some("x".into()) });
    }
}
