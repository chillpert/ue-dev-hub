//! What the home screen shows: incoming changes, the state of your branch, and the last compile.

use crate::build;
use crate::git::Commit;
use crate::proc;
use crate::project::Project;
use crate::report;
use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HomeStatus {
    pub branch: String,
    pub base_branch: String,
    pub on_base_branch: bool,
    /// Your commits that aren't on the base branch.
    pub ahead: usize,
    /// Commits on the base branch that you don't have yet.
    pub incoming: Vec<Commit>,
    /// How many commits there are in all, when there are more than `incoming` lists.
    pub incoming_total: usize,
    /// "upToDate", "ahead", "behind", "diverged" or "notOnServer".
    pub server: String,
    pub unsaved_files: usize,
    pub last_compile: Option<String>,
    pub last_compile_failed: Option<bool>,
    pub editor_running: bool,
    /// Set when fetching failed, for example without internet.
    pub fetch_error: Option<String>,
}

/// The most incoming commits the home screen lists.
const INCOMING_LIMIT: usize = 300;

fn count(project: &Project, range: &str) -> usize {
    project.git(&["rev-list", "--count", range]).and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Reads the state of the repo. With `fetch`, downloads the latest refs first. Fetching only
/// updates `origin/*`; it never changes your branches or files.
pub fn status(project: &Project, fetch: bool) -> Result<HomeStatus, String> {
    let branch = project.current_branch().ok_or("This folder isn't a Git repository.")?;
    let base = project.default_branch();
    let mut fetch_error = None;
    // Never fetch while a step runs: a fetch in the middle of an update or push could move the
    // server refs its safety checks rely on.
    if fetch && crate::jobs::busy_with().is_none() {
        let _fetching = crate::git::FETCH.lock().unwrap_or_else(|e| e.into_inner());
        // Fetching updates refs, which runs the reference-transaction hook; this fetch only
        // happens because the home screen is open, so it runs no hooks.
        let args: Vec<&str> = crate::git::READ_ONLY.iter().copied().chain(["fetch", "--quiet", "origin"]).collect();
        match proc::run_timeout("git", &args, Some(&project.root), 180) {
            Ok(out) if !out.ok => fetch_error = Some(out.message()),
            Ok(_) => {}
            Err(e) => fetch_error = Some(e),
        }
    }
    // Full ref names, so a local branch that happens to be called "origin/develop" can't stand in.
    let upstream = format!("refs/remotes/origin/{base}");
    let range = format!("HEAD..{upstream}");
    let incoming = Commit::parse_log(&project.git(&["log", Commit::FORMAT, "-n", &INCOMING_LIMIT.to_string(), &range]).unwrap_or_default());
    let incoming_total = if incoming.len() < INCOMING_LIMIT { incoming.len() } else { count(project, &range) };

    let remote_ref = format!("refs/remotes/origin/{branch}");
    let server = if project.git(&["rev-parse", "--verify", "--quiet", &remote_ref]).is_none() {
        "notOnServer"
    } else {
        let mine = count(project, &format!("{remote_ref}..HEAD"));
        let theirs = count(project, &format!("HEAD..{remote_ref}"));
        match (mine, theirs) {
            (0, 0) => "upToDate",
            (_, 0) => "ahead",
            (0, _) => "behind",
            _ => "diverged",
        }
    };

    let build_log = build::build_log_path(project);
    let unsaved_files = project.git(&["--no-optional-locks", "status", "--porcelain"]).map_or(0, |s| s.lines().filter(|l| !l.is_empty()).count());

    Ok(HomeStatus {
        on_base_branch: branch == base,
        ahead: if branch == base { 0 } else { count(project, &format!("{upstream}..HEAD")) },
        branch,
        base_branch: base,
        incoming,
        incoming_total,
        server: server.into(),
        unsaved_files,
        last_compile: report::friendly_time(&build_log),
        last_compile_failed: report::last_build_failed(&build_log),
        editor_running: proc::editor_running_for(&project.uproject),
        fetch_error,
    })
}
