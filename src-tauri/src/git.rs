//! Repository setup and the update: rebasing the branch onto the latest base branch.

use crate::config::{PushPolicy, UpdateStrategy};
use crate::log::Log;
use crate::proc;
use crate::project::Project;
use serde::Serialize;

/// Options for Git commands the hub runs without being asked to, like reading the status
/// of a project that was just opened: no file monitor and no hooks, which a repository's own
/// settings could point at any program.
pub const READ_ONLY: &[&str] = &["-c", "core.fsmonitor=false", "-c", if cfg!(windows) { "core.hooksPath=NUL" } else { "core.hooksPath=/dev/null" }];

/// Long enough for a slow fetch or a first sign-in, short enough that nothing hangs forever.
const GIT_TIMEOUT_SECS: u64 = 600;

pub(crate) fn git(log: &Log, project: &Project, args: &[&str]) -> Result<proc::Output, String> {
    log.info(format!("> git {}", args.join(" ")));
    let out = proc::run_timeout("git", args, Some(&project.root), GIT_TIMEOUT_SECS)?;
    for line in out.stdout.lines().chain(out.stderr.lines()) {
        log.detail(line);
    }
    Ok(out)
}

pub(crate) fn git_ok(log: &Log, project: &Project, args: &[&str], error: &str) -> Result<proc::Output, String> {
    let out = git(log, project, args)?;
    if out.ok { Ok(out) } else { Err(format!("{error}\n\n{}", out.message())) }
}

/// The project's Git hooks folder (`project.git_hooks_path`) and the hooks folders Git uses now.
/// Hooks are programs Git runs on commit, push and more, so the folder is only used once the
/// person said yes on the Setup screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Hooks {
    /// As written in ue-dev-hub.toml, like "Hooks".
    pub folder: String,
    /// The value core.hooksPath gets: the full path with forward slashes.
    pub wanted: String,
    /// core.hooksPath in this repository's own settings, or empty.
    pub local: String,
    /// core.hooksPath for all of this person's repositories, or empty.
    pub global: String,
}

impl Hooks {
    pub fn in_use(&self) -> bool {
        self.local == self.wanted
    }

    /// The hub may set or remove the local value: it's empty, or the one the hub set.
    pub fn hub_may_change(&self, hub_set: Option<&str>) -> bool {
        self.local.is_empty() || self.in_use() || hub_set == Some(self.local.as_str())
    }
}

fn config_value(project: &Project, scope: &str) -> String {
    proc::run_timeout("git", &["config", scope, "--get", "core.hooksPath"], Some(&project.root), 30).map(|o| o.stdout).unwrap_or_default()
}

/// The project's hooks folder and the current settings, if the project names a folder.
pub fn hooks(project: &Project) -> Option<Hooks> {
    let folder = project.config.project.git_hooks_path.clone().filter(|h| !h.is_empty())?;
    let wanted = project.root.join(&folder).to_string_lossy().replace('\\', "/");
    Some(Hooks { folder, wanted, local: config_value(project, "--local"), global: config_value(project, "--global") })
}

/// Points this repository's core.hooksPath at the project's hooks folder.
pub fn use_hooks(log: &Log, project: &Project, hooks: &Hooks) -> Result<(), String> {
    git_ok(log, project, &["config", "--local", "core.hooksPath", &hooks.wanted], "Couldn't set the Git hooks folder.").map(|_| ())
}

/// Removes this repository's core.hooksPath, and gives Git LFS its own hooks again.
pub fn stop_using_hooks(log: &Log, project: &Project) -> Result<(), String> {
    git(log, project, &["config", "--local", "--unset", "core.hooksPath"])?;
    lfs_hooks(log, project)
}

/// Without a hooks folder, LFS needs its own hooks. Never overwrites existing ones.
fn lfs_hooks(log: &Log, project: &Project) -> Result<(), String> {
    if git(log, project, &["config", "--get", "core.hooksPath"])?.stdout.is_empty() {
        git_ok(log, project, &["lfs", "install", "--local"], "Couldn't set up Git LFS.")?;
    }
    Ok(())
}

/// Sets up the repository before launching: the team's Git hooks, Git LFS, and submodules.
/// `use_hooks` is the person's answer about the project's hooks folder, `hub_set` the
/// core.hooksPath the hub set before. Returns the value it set, if it set one.
pub fn prepare(log: &Log, project: &Project, use_project_hooks: Option<bool>, hub_set: Option<&str>) -> Result<Option<String>, String> {
    let mut set = None;
    match hooks(project) {
        Some(h) if use_project_hooks == Some(true) => {
            if h.in_use() {
                // Already set up.
            } else if h.hub_may_change(hub_set) {
                // The folder moved, or the setting was lost: point it at the project's folder again.
                use_hooks(log, project, &h)?;
                set = Some(h.wanted.clone());
            } else {
                log.info(format!("This copy already uses Git hooks from {}, so the project's hooks folder isn't used.", h.local));
            }
        }
        Some(h) => {
            if use_project_hooks.is_none() && !h.in_use() {
                log.info(format!("This project wants to use its Git hooks from the {} folder. Choose whether to use them on the Setup screen.", h.folder));
            }
            lfs_hooks(log, project)?;
        }
        None => lfs_hooks(log, project)?,
    }

    for warning in update_submodules(log, project) {
        log.info(warning);
    }
    Ok(set)
}

/// Merges the server's copy of the branch (if it has commits this one doesn't) and then the
/// base branch into the current branch. On any failure everything goes back to `backup`.
fn merge_in(log: &Log, project: &Project, info: &UpdateInfo, target: &str, backup: &str, stashed: bool) -> Result<(), String> {
    let mut sources = vec![];
    if info.server_has_more > 0 {
        sources.push(format!("origin/{}", info.branch));
    }
    sources.push(target.to_string());
    for source in &sources {
        let merge = git(log, project, &["merge", "--no-edit", source])?;
        if merge.ok {
            continue;
        }
        if repo_busy(project).is_some() {
            git(log, project, &["merge", "--abort"])?;
        }
        // Undo an earlier merge in this update too. The working tree has nothing uncommitted
        // (it was stashed), so this only moves the branch back.
        let saved = git(log, project, &["rev-parse", backup])?.stdout;
        if git(log, project, &["rev-parse", "HEAD"])?.stdout != saved {
            git(log, project, &["reset", "--hard", &saved])?;
        }
        if git(log, project, &["rev-parse", "HEAD"])?.stdout != saved || repo_busy(project).is_some() {
            return Err(format!("The update failed and couldn't be undone automatically. {}", recovery_hint(backup)));
        }
        if stashed && !restore_stash(log, project)? {
            return Err(format!("The update didn't work, and your uncommitted changes couldn't be put back automatically. {}", recovery_hint(backup)));
        }
        return Err(format!(
            "Your changes conflict with the latest changes from the team, so the hub couldn't update automatically. Nothing was changed. Whoever changed the same files has to combine the two versions by hand.\n\nGit said:\n{}",
            merge.message()
        ));
    }
    Ok(())
}

/// Brings submodules to the commits the project expects. Submodules with uncommitted changes are
/// left alone, since updating them could fail or detach them from local work. Returns warnings.
pub fn update_submodules(log: &Log, project: &Project) -> Vec<String> {
    if !project.root.join(".gitmodules").is_file() {
        return vec![];
    }
    let mut warnings = vec![];
    let listed = match git(log, project, &["config", "-f", ".gitmodules", "-z", "--get-regexp", r"^submodule\..*\.path$"]) {
        Ok(o) if o.ok => o.stdout,
        // Exit code 1 means no submodule has a path.
        Ok(o) if o.stderr.is_empty() => return vec![],
        Ok(o) => return vec![format!("Couldn't read the submodules: {}", o.message())],
        Err(e) => return vec![e],
    };
    let paths = submodule_paths(&listed);
    if paths.is_empty() {
        return vec![];
    }
    // Only paths that are submodules in the current commit; .gitmodules can list old ones.
    let mut args = vec!["-c", "core.quotePath=false", "ls-files", "-z", "--stage", "--"];
    args.extend(paths.iter().map(String::as_str));
    let registered = match git(log, project, &args) {
        Ok(o) if o.ok => gitlinks(&o.stdout),
        Ok(o) => return vec![format!("Couldn't read the submodules: {}", o.message())],
        Err(e) => return vec![e],
    };
    let mut clean = vec![];
    for path in paths.iter().filter(|p| registered.contains(p)) {
        let path = path.as_str();
        let dir = project.root.join(path);
        // Only an initialized submodule has its own .git; elsewhere `git status` would read the parent.
        if dir.join(".git").exists() {
            let dirty = proc::run_timeout("git", &["--no-optional-locks", "status", "--porcelain"], Some(&dir), 120)
                .map(|o| !o.stdout.trim().is_empty())
                .unwrap_or(false);
            if dirty {
                warnings.push(format!("The submodule {path} has uncommitted changes, so it wasn't updated. Commit or discard them, then launch again."));
                continue;
            }
        }
        clean.push(path.to_string());
    }
    if !clean.is_empty() {
        let mut args = vec!["submodule", "update", "--init", "--recursive", "--"];
        args.extend(clean.iter().map(String::as_str));
        match git(log, project, &args) {
            Ok(o) if o.ok => {}
            Ok(o) => warnings.push(format!("Couldn't update the submodules ({}): {}", clean.join(", "), o.message())),
            Err(e) => warnings.push(e),
        }
    }
    warnings
}

/// Paths from `git config -z --get-regexp` output, where each entry is "key\npath\0". Paths may
/// contain spaces.
fn submodule_paths(out: &str) -> Vec<String> {
    out.split('\0')
        .filter_map(|entry| entry.split_once('\n').map(|(_, path)| path.to_string()))
        .filter(|p| !p.is_empty())
        .collect()
}

/// Submodule paths in `git ls-files -z --stage` output: "160000 <sha> <stage>\t<path>\0".
fn gitlinks(out: &str) -> Vec<String> {
    out.split('\0')
        .filter_map(|entry| entry.split_once('\t'))
        .filter(|(meta, _)| meta.starts_with("160000 "))
        .map(|(_, path)| path.to_string())
        .collect()
}

#[derive(Serialize, Clone)]
pub struct Commit {
    pub hash: String,
    pub author: String,
    pub email: String,
    /// How long ago it was committed, like "3 hours ago".
    pub when: String,
    pub subject: String,
}

impl Commit {
    /// The `git log --pretty` format that [`Commit::parse`] reads.
    pub const FORMAT: &'static str = "--pretty=format:%h%x1f%an%x1f%ae%x1f%ar%x1f%s";

    pub fn parse(line: &str) -> Option<Commit> {
        let mut parts = line.splitn(5, '\u{1f}');
        Some(Commit {
            hash: parts.next()?.into(),
            author: parts.next()?.into(),
            email: parts.next()?.into(),
            when: parts.next()?.into(),
            subject: parts.next()?.into(),
        })
    }

    pub fn parse_log(out: &str) -> Vec<Commit> {
        out.lines().filter_map(Commit::parse).collect()
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub branch: String,
    pub default_branch: String,
    pub on_default_branch: bool,
    /// New commits on the default branch that the current branch doesn't have yet.
    pub commits: Vec<Commit>,
    /// Your commits that get rebased onto the default branch.
    pub my_commits: usize,
    pub has_local_changes: bool,
    /// Whether the branch exists on the server, so it can be force-pushed after the rebase.
    pub can_push: bool,
    /// Files you changed locally that the incoming commits change too. Binary assets like
    /// .umap can't be merged, so updating is refused while this isn't empty.
    pub conflicting_files: Vec<String>,
    /// Unreal Editor holds the project's files open, so updating is refused while it runs.
    pub editor_running: bool,
    /// Commits on your branch on the server that you don't have locally. A force push would
    /// delete them, so updating is refused.
    pub server_has_more: usize,
    /// A rebase, merge or cherry-pick that wasn't finished, or a leftover lock file.
    pub repo_busy: Option<String>,
    /// "merge" or "rebase", from the project's settings.
    pub strategy: UpdateStrategy,
    /// "ask", "always" or "never".
    pub push_policy: PushPolicy,
    /// Set when updating isn't safe right now; the UI then offers only skipping.
    pub blocked_reason: Option<String>,
    /// The commit of your branch on the server when this was checked. A force push only
    /// replaces exactly this commit, so a push that lands later is never overwritten.
    #[serde(skip)]
    pub server_sha: Option<String>,
    /// Why the project's Git settings can't be trusted right now, if they can't.
    #[serde(skip)]
    pub config_problem: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResult {
    /// Where your branch was before, to go back to: `git reset --hard <backup>`.
    pub backup: String,
    pub pushed: bool,
    pub commits: usize,
    /// Things that didn't stop the update but need attention, like a submodule left alone.
    pub warnings: Vec<String>,
}

impl UpdateInfo {
    /// Why updating isn't safe right now, if it isn't.
    pub fn blocker(&self) -> Option<String> {
        if let Some(problem) = &self.config_problem {
            return Some(problem.clone());
        }
        if self.editor_running {
            return Some("Unreal Editor is open. Save your work and close it first, so it doesn't hold or overwrite the files the update changes.".into());
        }
        if let Some(busy) = &self.repo_busy {
            return Some(busy.clone());
        }
        // Merging brings those commits in; only a rebase and force push would delete them.
        if self.server_has_more > 0 && self.strategy == UpdateStrategy::Rebase {
            return Some(format!(
                "Your branch '{}' on the server has {} commit(s) you don't have here, probably pushed from another computer or by a teammate. Updating would rebase and force push over them, so it's blocked until those commits are on this computer too.",
                self.branch, self.server_has_more
            ));
        }
        if !self.conflicting_files.is_empty() {
            return Some(format!(
                "You have uncommitted changes to {} file(s) that the team also changed. Unreal files can't be combined automatically, so updating now could lose your work. Commit, stash or discard your changes to these files first:\n\n{}",
                self.conflicting_files.len(),
                self.conflicting_files.join("\n")
            ));
        }
        None
    }
}

/// Paths of uncommitted changes (staged, unstaged and untracked), relative to the repo root.
fn local_changes(log: &Log, project: &Project) -> Result<Vec<String>, String> {
    let error = "Couldn't read your local changes.";
    let changed = git_ok(log, project, &["-c", "core.quotePath=false", "diff", "--name-only", "HEAD"], error)?.stdout;
    let untracked = git_ok(log, project, &["-c", "core.quotePath=false", "ls-files", "--others", "--exclude-standard"], error)?.stdout;
    Ok(changed.lines().chain(untracked.lines()).filter(|l| !l.is_empty()).map(str::to_string).collect())
}

fn upstream_target(info: &UpdateInfo) -> String {
    update_target(info.on_default_branch, &info.default_branch)
}

/// What an update brings in: the server's base branch when you're on it, otherwise your local
/// copy of it (which the check fast-forwards). Full ref names, so a local branch that happens to
/// be called "origin/develop" can't stand in.
pub(crate) fn update_target(on_base: bool, base: &str) -> String {
    if on_base { format!("refs/remotes/origin/{base}") } else { format!("refs/heads/{base}") }
}

/// An unfinished Git operation, or a lock file another program left behind.
pub(crate) fn repo_busy(project: &Project) -> Option<String> {
    let git_dir = project.git(&["rev-parse", "--absolute-git-dir"]).map(std::path::PathBuf::from)?;
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        return Some("A rebase was started earlier and never finished, probably in another Git program. Finish or abort it there, then try again.".into());
    }
    if git_dir.join("MERGE_HEAD").exists() || git_dir.join("CHERRY_PICK_HEAD").exists() || git_dir.join("REVERT_HEAD").exists() {
        return Some("A merge or cherry-pick was started earlier and never finished, probably in another Git program. Finish or abort it there, then try again.".into());
    }
    if git_dir.join("index.lock").exists() {
        return Some("Another program is using Git right now (index.lock exists). Close Rider, Fork, GitHub Desktop and Unreal's Git plugin, then try again. If none of them are open, Git was probably cut off halfway and left the lock file behind. Then it's safe to delete .git/index.lock.".into());
    }
    None
}

pub(crate) fn count(log: &Log, project: &Project, range: &str) -> Result<usize, String> {
    Ok(git_ok(log, project, &["rev-list", "--count", range], "Couldn't compare branches.")?.stdout.trim().parse().unwrap_or(0))
}

/// Like `check_updates`, for when nothing has been started for it: while a job or step runs it
/// doesn't fetch at all, since fetching moves the refs that step relies on (and `develop:develop`
/// even changes a local branch). It then answers from what was fetched last. A flow alone, like
/// Update & Launch, doesn't count: it checks for updates itself before its steps start.
pub fn check_updates_if_idle(log: &Log, project: &Project) -> Result<UpdateInfo, String> {
    check(log, project, crate::jobs::busy_except_flow().is_none())
}

/// What an update would bring in, after fetching the latest from the server. Only for the step
/// that's about to update, or when nothing else runs.
pub fn check_updates(log: &Log, project: &Project) -> Result<UpdateInfo, String> {
    check(log, project, true)
}

fn check(log: &Log, project: &Project, fetch: bool) -> Result<UpdateInfo, String> {
    let branch = project.current_branch().ok_or("This folder isn't a Git repository.")?;
    if branch == "HEAD" {
        return Err("You're not on a branch right now (detached HEAD), so the hub can't update. Switch to your branch first.".into());
    }
    let default_branch = project.default_branch();
    let on_default_branch = branch == default_branch;
    let _fetching = FETCH.lock().unwrap_or_else(|e| e.into_inner());

    if !fetch {
        log.info("Something else is running, so this shows what was downloaded last.");
    } else if on_default_branch {
        git_ok(log, project, &["fetch", "origin", &default_branch], &format!("Couldn't download the latest '{default_branch}'. Check your internet connection."))?;
    } else {
        // Fast-forward the local default branch without switching to it.
        let refspec = format!("{default_branch}:{default_branch}");
        let out = git(log, project, &["fetch", "origin", &refspec])?;
        if !out.ok {
            return Err(format!(
                "Couldn't update '{default_branch}'. Either there's no internet connection, or your local '{default_branch}' has commits that aren't on the server. Check your connection and try again.\n\n{}",
                out.message()
            ));
        }
        // Also fetch your own branch, to see whether the server has commits you don't.
        let _ = git(log, project, &["fetch", "origin", &branch]);
    }

    let mut info = UpdateInfo {
        branch: branch.clone(),
        default_branch: default_branch.clone(),
        on_default_branch,
        commits: vec![],
        my_commits: 0,
        has_local_changes: false,
        can_push: false,
        conflicting_files: vec![],
        editor_running: proc::editor_running_for(&project.uproject),
        server_has_more: 0,
        repo_busy: repo_busy(project),
        strategy: project.config.git.update,
        push_policy: project.config.git.push,
        blocked_reason: None,
        server_sha: None,
        config_problem: project.config_git_problem.clone(),
    };

    let target = upstream_target(&info);
    let out = git_ok(log, project, &["log", Commit::FORMAT, &format!("{branch}..{target}")], "Couldn't read the list of changes.")?;
    info.commits = Commit::parse_log(&out.stdout);
    info.my_commits = count(log, project, &format!("{target}..{branch}"))?;
    info.has_local_changes = !git(log, project, &["--no-optional-locks", "status", "--porcelain", "--untracked-files=no"])?.stdout.is_empty();

    if !info.commits.is_empty() {
        let local = local_changes(log, project)?;
        if !local.is_empty() {
            let incoming_range = format!("HEAD...{target}");
            let incoming = git_ok(log, project, &["-c", "core.quotePath=false", "diff", "--name-only", &incoming_range], "Couldn't list the incoming changes.")?.stdout;
            info.conflicting_files = incoming.lines().filter(|f| local.iter().any(|l| l == f)).map(str::to_string).collect();
        }
    }

    if !on_default_branch && remote_branch_exists(log, project, &branch)? {
        info.can_push = true;
        let sha = git_ok(log, project, &["rev-parse", &format!("refs/remotes/origin/{branch}")], "Couldn't read your branch on the server.")?.stdout;
        info.server_has_more = if info.strategy == UpdateStrategy::Merge {
            count(log, project, &format!("refs/heads/{branch}..{sha}"))?
        } else {
            foreign_server_commits(log, project, &branch, &sha)?
        };
        info.server_sha = Some(sha);
    }
    if !info.commits.is_empty() {
        info.blocked_reason = info.blocker();
    }
    Ok(info)
}

fn stash_count(log: &Log, project: &Project) -> Result<usize, String> {
    Ok(git_ok(log, project, &["stash", "list"], "Couldn't read the Git stash.")?.stdout.lines().filter(|l| !l.is_empty()).count())
}

/// One fetch at a time: the home screen's "Check for changes" and an update can overlap.
pub static FETCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn backup_dir(branch: &str) -> String {
    format!("{BACKUP_PREFIX}/{}/", branch.replace(['/', '\\'], "-"))
}

/// Commits on the server's copy of the branch that are neither here nor in one of the hub's
/// backups. Commits in a backup were yours before a rebase that wasn't force pushed (unticked, or
/// the push failed), so they don't count: a force push replaces them with their rebased versions.
/// `server` is the server's commit as it was read for this check, never re-read later.
pub(crate) fn foreign_server_commits(log: &Log, project: &Project, branch: &str, server: &str) -> Result<usize, String> {
    let glob = format!("--glob={}*", backup_dir(branch));
    let out = git_ok(log, project, &["rev-list", "--count", server, "--not", "HEAD", &glob], "Couldn't compare your branch with the server.")?;
    Ok(out.stdout.trim().parse().unwrap_or(0))
}

pub(crate) fn remote_branch_exists(log: &Log, project: &Project, branch: &str) -> Result<bool, String> {
    let r = format!("refs/remotes/origin/{branch}");
    Ok(git(log, project, &["rev-parse", "--verify", "--quiet", &r])?.ok)
}

const BACKUP_PREFIX: &str = "refs/ue-dev-hub/backups";
const KEEP_BACKUPS: usize = 20;

/// Saves HEAD (and the uncommitted changes, if given) under refs/ue-dev-hub/backups, so
/// nothing can get lost even if every later step fails. Only the newest 20 per branch are kept.
fn backup(log: &Log, project: &Project, branch: &str, stash: Option<&str>) -> Result<String, String> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dir = backup_dir(branch);
    let name = format!("{dir}{stamp}");
    git_ok(log, project, &["update-ref", &name, "HEAD"], "Couldn't back up your branch, so nothing was changed.")?;
    if let Some(sha) = stash {
        let changes = format!("{name}-uncommitted");
        git_ok(log, project, &["update-ref", &changes, sha], "Couldn't back up your uncommitted changes, so nothing was changed.")?;
    }

    let refs = git(log, project, &["for-each-ref", "--sort=-refname", "--format=%(refname)", &dir])?.stdout;
    let snapshots: Vec<&str> = refs.lines().filter(|r| !r.ends_with("-uncommitted")).collect();
    for old in snapshots.iter().skip(KEEP_BACKUPS) {
        let _ = git(log, project, &["update-ref", "-d", old]);
        let _ = git(log, project, &["update-ref", "-d", &format!("{old}-uncommitted")]);
    }
    Ok(name)
}

/// Puts the stashed changes back. If that conflicts, the half-applied changes are undone so the
/// files match the updated commit, and the stash stays (Git keeps it on a failed pop). The
/// changes are in the backup too, so they can't get lost.
fn restore_stash(log: &Log, project: &Project) -> Result<bool, String> {
    if git(log, project, &["stash", "pop", "--index"])?.ok || git(log, project, &["stash", "pop"])?.ok {
        return Ok(true);
    }
    git(log, project, &["reset", "--hard", "HEAD"])?;
    Ok(false)
}

fn recovery_hint(backup: &str) -> String {
    format!(
        "Nothing is lost: your branch was saved as {backup}, and your uncommitted changes (if any) as {backup}-uncommitted. Don't keep working until this is sorted out, so nothing gets mixed up."
    )
}

/// Brings the latest default branch into the current branch, by merging or rebasing as the
/// project says, and pushes it if `push` is set (or the project says always).
pub fn apply_update(log: &Log, project: &Project, push: bool) -> Result<UpdateResult, String> {
    let info = check_updates(log, project)?;
    // Never update with Git settings that may not be the team's, like merge instead of rebase.
    if let Some(problem) = &info.config_problem {
        return Err(problem.clone());
    }
    let merging = info.strategy == UpdateStrategy::Merge;
    let push = match info.push_policy {
        PushPolicy::Always => true,
        PushPolicy::Never => false,
        PushPolicy::Ask => push,
    };
    if info.commits.is_empty() {
        log.info("Already up to date.");
        return Ok(UpdateResult { backup: String::new(), pushed: false, commits: 0, warnings: vec![] });
    }
    // Checked again here: the dialog may have been open for minutes.
    if let Some(reason) = info.blocker() {
        return Err(reason);
    }

    // Download the new art and audio files first, with progress in the log. The rebase is quick
    // afterwards, and a dropped connection fails here, before anything changed.
    let target = upstream_target(&info);
    log.info("Downloading the new art and audio files (Git LFS) ...");
    let mut lfs = proc::command("git");
    lfs.args(["lfs", "fetch", "origin", &target]).current_dir(&project.root);
    if merging && info.server_has_more > 0 {
        lfs.arg(format!("origin/{}", info.branch));
    }
    let (lfs_ok, lfs_lines) = proc::stream(log, lfs, None, |_| true)?;
    if !lfs_ok {
        let tail: Vec<&str> = lfs_lines.iter().rev().take(6).rev().map(String::as_str).collect();
        return Err(format!(
            "Couldn't download the new art and audio files (Git LFS). Check your internet connection and try again. Nothing was changed.\n\nGit said:\n{}",
            tail.join("\n")
        ));
    }

    // Only trust a stash the hub just made. `git stash push` succeeds without saving anything
    // when the only changes are inside submodules, and then stash@{0} would be an older stash.
    let mut stashed = false;
    let mut stash_sha = None;
    if info.has_local_changes {
        let before = stash_count(log, project)?;
        git_ok(log, project, &["stash", "push", "-m", "ue-dev-hub: before update"], "Couldn't set your changes aside before updating. Nothing was changed.")?;
        if stash_count(log, project)? > before {
            stashed = true;
            stash_sha = Some(git_ok(log, project, &["rev-parse", "stash@{0}"], "Couldn't find the stashed changes.")?.stdout);
        }
    }
    let backup = match backup(log, project, &info.branch, stash_sha.as_deref()) {
        Ok(b) => b,
        Err(e) => {
            if stashed {
                let _ = restore_stash(log, project);
            }
            return Err(e);
        }
    };
    log.info(format!("Saved a backup of '{}' as {backup}.", info.branch));

    // The last moment to back out: the editor may have been started while the dialog was open.
    if proc::editor_running_for(&project.uproject) {
        if stashed && !restore_stash(log, project)? {
            return Err(format!("Unreal Editor was started, so the update was cancelled. {}", recovery_hint(&backup)));
        }
        return Err("Unreal Editor was started, so the update was cancelled. Nothing was changed.".into());
    }

    if merging {
        merge_in(log, project, &info, &target, &backup, stashed)?;
    }
    let rebase = if merging { None } else { Some(git(log, project, &["-c", "rebase.autoStash=false", "rebase", &target])?) };
    if let Some(rebase) = rebase.filter(|r| !r.ok) {
        // Only abort a rebase that actually started; otherwise there's nothing to undo.
        let started = repo_busy(project).is_some();
        let aborted = !started || git(log, project, &["rebase", "--abort"])?.ok;
        let head = git(log, project, &["rev-parse", "HEAD"])?.stdout;
        let saved = git(log, project, &["rev-parse", &backup])?.stdout;
        if !aborted || head != saved {
            return Err(format!("The update failed and couldn't be undone automatically. {}", recovery_hint(&backup)));
        }
        if stashed && !restore_stash(log, project)? {
            return Err(format!("The update didn't work, and your uncommitted changes couldn't be put back automatically. {}", recovery_hint(&backup)));
        }
        let said = rebase.message();
        let network = ["smudge", "git-lfs", "Could not resolve host", "unable to access", "timed out", "Connection reset", "Failed to connect"]
            .iter()
            .any(|n| said.contains(n));
        let what = if network {
            "Downloading files for the update failed, probably because of the internet connection. Try again in a moment."
        } else if started {
            "Your commits conflict with the latest changes from the team, so the hub couldn't update automatically. The two versions have to be combined by hand."
        } else {
            "Git couldn't start the update. Check what Git said below and try again."
        };
        return Err(format!("{what} Nothing was changed.\n\nGit said:\n{said}"));
    }

    let warnings = update_submodules(log, project);

    if stashed && !restore_stash(log, project)? {
        return Err(format!("Updated, but your uncommitted changes couldn't be put back automatically. {}", recovery_hint(&backup)));
    }

    let mut pushed = false;
    if push && info.can_push {
        let refspec = format!("HEAD:refs/heads/{}", info.branch);
        if merging {
            // A merge only adds commits, so a normal push does it.
            git_ok(
                log,
                project,
                &["push", "origin", &refspec],
                &format!("Updated your branch here, but couldn't push it to the server. Your work is safe on this computer (backup: {backup}). Check your connection and push again."),
            )?;
        } else {
            // Only replace the server's branch if it's still what we checked: never someone else's push.
            let expected = info.server_sha.clone().ok_or("Couldn't read your branch on the server.")?;
            let lease = format!("--force-with-lease=refs/heads/{}:{expected}", info.branch);
            git_ok(
                log,
                project,
                &["push", &lease, "origin", &refspec],
                &format!("Updated your branch here, but couldn't force push it to the server. Your work is safe on this computer (backup: {backup}). Check your connection and push again."),
            )?;
        }
        pushed = true;
    }
    log.info(format!(
        "{} {} new change(s) {} '{}'{}.",
        if merging { "Merged" } else { "Rebased onto" },
        info.commits.len(),
        if merging { "into" } else { "for" },
        info.branch,
        if pushed { " and pushed it" } else { "" }
    ));
    Ok(UpdateResult { backup, pushed, commits: info.commits.len(), warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = proc::run("git", args, Some(dir)).unwrap();
        assert!(out.ok, "git {args:?} failed: {}", out.message());
        out.stdout
    }

    fn write(dir: &Path, file: &str, content: &[u8]) {
        std::fs::write(dir.join(file), content).unwrap();
    }

    fn configure(dir: &Path) {
        sh(dir, &["config", "user.name", "Test"]);
        sh(dir, &["config", "user.email", "test@example.com"]);
        sh(dir, &["config", "core.autocrlf", "false"]);
    }

    /// An origin with `develop` and `feat/x`, a clone of it on `feat/x`, and a teammate's new
    /// commit on `develop` that changes `Map.umap`. Returns the clone's path.
    fn scenario(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ue-dev-hub-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let origin = root.join("origin.git");
        sh(&root, &["init", "--bare", "-b", "develop", origin.to_str().unwrap()]);

        let seed = root.join("seed");
        sh(&root, &["clone", "-q", "-c", "core.autocrlf=false", origin.to_str().unwrap(), "seed"]);
        configure(&seed);
        write(&seed, "Game.uproject", br#"{"EngineAssociation":"5.8"}"#);
        write(&seed, "ue-dev-hub.toml", b"[project]\nbase_branch = \"develop\"\n\n[git]\nupdate = \"rebase\"\n");
        write(&seed, "Map.umap", &[0, 1, 2, 3, 0xff]);
        write(&seed, "Other.txt", b"v1\n");
        sh(&seed, &["add", "-A"]);
        sh(&seed, &["commit", "-qm", "Initial"]);
        sh(&seed, &["push", "-q", "origin", "HEAD:develop"]);
        sh(&seed, &["push", "-q", "origin", "HEAD:feat/x"]);

        let work = root.join("work");
        sh(&root, &["clone", "-q", "-c", "core.autocrlf=false", "-b", "feat/x", origin.to_str().unwrap(), "work"]);
        configure(&work);
        // A local commit, so the rebase has something to replay.
        write(&work, "Mine.txt", b"mine\n");
        sh(&work, &["add", "-A"]);
        sh(&work, &["commit", "-qm", "My commit"]);
        sh(&work, &["push", "-q"]);

        // Teammate changes the map on develop.
        sh(&seed, &["checkout", "-q", "develop"]);
        write(&seed, "Map.umap", &[9, 9, 9, 0, 0xfe]);
        sh(&seed, &["commit", "-qam", "Tweak map lighting"]);
        sh(&seed, &["push", "-q", "origin", "develop"]);
        work
    }

    fn project(dir: &Path) -> Project {
        Project::load(dir).unwrap()
    }

    fn stash_count(dir: &Path) -> usize {
        sh(dir, &["stash", "list"]).lines().count()
    }

    #[test]
    fn reads_submodule_paths_with_spaces() {
        let listed = "submodule.Plugins/My Plugin.path\nPlugins/My Plugin\0submodule.Other.path\nThird Party/Other\0";
        assert_eq!(submodule_paths(listed), vec!["Plugins/My Plugin", "Third Party/Other"]);
        let staged = "160000 1234abcd 0\tPlugins/My Plugin\x00100644 5678ef 0\tThird Party/Other\0";
        assert_eq!(gitlinks(staged), vec!["Plugins/My Plugin"]);
    }

    #[test]
    fn refuses_to_update_while_the_git_settings_are_broken() {
        let work = scenario("badconfig");
        write(&work, "ue-dev-hub.local.toml", b"[git]\nupdate = \"squash\"\n");
        let log = Log::none();
        let p = project(&work);
        let info = check_updates(&log, &p).unwrap();
        assert!(info.blocked_reason.as_deref().is_some_and(|r| r.contains("ue-dev-hub.local.toml")), "{:?}", info.blocked_reason);
        let before = sh(&work, &["rev-parse", "HEAD"]);
        let err = apply_update(&log, &p, true).unwrap_err();
        assert!(err.contains("Updating is paused"), "{err}");
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn uses_the_projects_hooks_only_when_asked_and_never_overwrites_others() {
        let root = std::env::temp_dir().join(format!("ue-dev-hub-test-hooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Hooks")).unwrap();
        sh(&root, &["init", "-q"]);
        write(&root, "Game.uproject", br#"{"EngineAssociation":"5.8"}"#);
        write(&root, "ue-dev-hub.toml", b"[project]\ngit_hooks_path = \"Hooks\"\n");
        let p = project(&root);
        let log = Log::none();
        let local = || proc::run("git", &["config", "--local", "--get", "core.hooksPath"], Some(&root)).unwrap().stdout;

        let h = hooks(&p).unwrap();
        assert_eq!(h.folder, "Hooks");
        assert!(h.wanted.ends_with("/Hooks") && !h.wanted.contains('\\'));
        assert!(!h.in_use() && h.hub_may_change(None));

        // Not asked yet, or declined: the repository's folder isn't set.
        if proc::run("git", &["lfs", "version"], None).is_ok_and(|o| o.ok) {
            assert_eq!(prepare(&log, &p, None, None).unwrap(), None);
            assert_eq!(prepare(&log, &p, Some(false), None).unwrap(), None);
            assert_eq!(local(), "");
        }

        // Said yes: set, and reported so the hub remembers it set the value.
        assert_eq!(prepare(&log, &p, Some(true), None).unwrap(), Some(h.wanted.clone()));
        assert_eq!(local(), h.wanted);
        assert_eq!(prepare(&log, &p, Some(true), Some(&h.wanted)).unwrap(), None);

        // A value someone else set stays, even after a yes.
        sh(&root, &["config", "--local", "core.hooksPath", "/elsewhere"]);
        let h = hooks(&p).unwrap();
        assert!(!h.hub_may_change(None));
        assert_eq!(prepare(&log, &p, Some(true), None).unwrap(), None);
        assert_eq!(local(), "/elsewhere");
        // One the hub set itself before is updated.
        assert!(h.hub_may_change(Some("/elsewhere")));
        assert_eq!(prepare(&log, &p, Some(true), Some("/elsewhere")).unwrap(), Some(h.wanted.clone()));
        assert_eq!(local(), h.wanted);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_to_update_when_local_changes_overlap_incoming() {
        let work = scenario("overlap");
        write(&work, "Map.umap", &[7, 7, 7]);
        let log = Log::none();
        let p = project(&work);

        let info = check_updates(&log, &p).unwrap();
        assert_eq!(info.commits.len(), 1);
        assert_eq!(info.conflicting_files, vec!["Map.umap".to_string()]);
        assert!(info.blocked_reason.is_some());

        let before = sh(&work, &["rev-parse", "HEAD"]);
        assert!(apply_update(&log, &p, true).is_err());
        // Nothing touched: same commit, local change intact, no stash created.
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), before);
        assert_eq!(std::fs::read(work.join("Map.umap")).unwrap(), vec![7, 7, 7]);
        assert_eq!(stash_count(&work), 0);
    }

    #[test]
    fn updates_and_keeps_unrelated_local_changes() {
        let work = scenario("clean");
        write(&work, "Other.txt", b"local edit\n");
        let log = Log::none();
        let p = project(&work);

        let info = check_updates(&log, &p).unwrap();
        assert!(info.conflicting_files.is_empty());
        assert!(info.can_push);
        assert_eq!(info.my_commits, 1);

        let before = sh(&work, &["rev-parse", "HEAD"]);
        let result = apply_update(&log, &p, true).unwrap();
        assert!(result.pushed);
        // The backup points at the branch as it was before the rebase.
        assert_eq!(sh(&work, &["rev-parse", &result.backup]), before);
        // Rebased onto develop, local edit restored, stash cleaned up, and pushed.
        assert_eq!(std::fs::read(work.join("Map.umap")).unwrap(), vec![9, 9, 9, 0, 0xfe]);
        assert_eq!(std::fs::read_to_string(work.join("Other.txt")).unwrap(), "local edit\n");
        assert_eq!(stash_count(&work), 0);
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), sh(&work, &["rev-parse", "origin/feat/x"]));
        assert_eq!(sh(&work, &["log", "-1", "--format=%s"]), "My commit");
    }

    #[test]
    fn rebases_without_pushing_when_unticked() {
        let work = scenario("nopush");
        let log = Log::none();
        let p = project(&work);
        let server_before = sh(&work, &["rev-parse", "origin/feat/x"]);

        let result = apply_update(&log, &p, false).unwrap();
        assert!(!result.pushed);
        assert_eq!(std::fs::read(work.join("Map.umap")).unwrap(), vec![9, 9, 9, 0, 0xfe]);
        // The server's branch is untouched.
        sh(&work, &["fetch", "-q", "origin"]);
        assert_eq!(sh(&work, &["rev-parse", "origin/feat/x"]), server_before);
    }

    #[test]
    fn refuses_when_the_server_has_commits_you_dont() {
        let work = scenario("serverahead");
        let root = work.parent().unwrap().to_path_buf();
        // Someone pushes to feat/x from another clone.
        sh(&root, &["clone", "-q", "-c", "core.autocrlf=false", "-b", "feat/x", root.join("origin.git").to_str().unwrap(), "other"]);
        let other = root.join("other");
        configure(&other);
        write(&other, "Theirs.txt", b"theirs
");
        sh(&other, &["add", "-A"]);
        sh(&other, &["commit", "-qm", "Their commit"]);
        sh(&other, &["push", "-q"]);

        let log = Log::none();
        let p = project(&work);
        let info = check_updates(&log, &p).unwrap();
        assert_eq!(info.server_has_more, 1);
        assert!(info.blocked_reason.as_deref().unwrap().contains("force push"));

        let before = sh(&work, &["rev-parse", "HEAD"]);
        assert!(apply_update(&log, &p, true).is_err());
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), before);
        // Their commit is still on the server.
        sh(&work, &["fetch", "-q", "origin"]);
        assert_eq!(sh(&work, &["log", "-1", "--format=%s", "origin/feat/x"]), "Their commit");
    }

    #[test]
    fn refuses_while_a_rebase_is_unfinished_or_git_is_locked() {
        let work = scenario("busy");
        let log = Log::none();
        let p = project(&work);
        let git_dir = work.join(".git");

        std::fs::create_dir_all(git_dir.join("rebase-merge")).unwrap();
        assert!(check_updates(&log, &p).unwrap().blocked_reason.unwrap().contains("rebase"));
        std::fs::remove_dir_all(git_dir.join("rebase-merge")).unwrap();

        std::fs::write(git_dir.join("index.lock"), b"").unwrap();
        let before = sh(&work, &["rev-parse", "HEAD"]);
        assert!(apply_update(&log, &p, true).is_err());
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), before);
        std::fs::remove_file(git_dir.join("index.lock")).unwrap();
    }

    #[test]
    fn conflicting_commits_change_nothing_and_keep_uncommitted_work() {
        let work = scenario("conflict");
        // My commit edits Other.txt, and so does a teammate on develop.
        write(&work, "Other.txt", b"mine
");
        sh(&work, &["commit", "-qam", "My edit"]);
        sh(&work, &["push", "-q"]);
        let seed = work.parent().unwrap().join("seed");
        write(&seed, "Other.txt", b"theirs
");
        sh(&seed, &["commit", "-qam", "Their edit"]);
        sh(&seed, &["push", "-q", "origin", "develop"]);
        // Plus uncommitted work in a file nobody else touched.
        write(&work, "Mine.txt", b"uncommitted
");

        let log = Log::none();
        let p = project(&work);
        let before = sh(&work, &["rev-parse", "HEAD"]);
        let err = apply_update(&log, &p, true).unwrap_err();
        assert!(err.contains("Nothing was changed"), "{err}");
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), before);
        assert_eq!(std::fs::read_to_string(work.join("Mine.txt")).unwrap(), "uncommitted
");
        assert_eq!(std::fs::read_to_string(work.join("Other.txt")).unwrap(), "mine
");
        assert_eq!(stash_count(&work), 0);
        assert!(!work.join(".git/rebase-merge").exists());
    }

    #[test]
    fn a_rebase_that_wasnt_pushed_doesnt_block_the_next_update() {
        let work = scenario("unpushed");
        let log = Log::none();
        let p = project(&work);
        apply_update(&log, &p, false).unwrap();

        // A teammate pushes to develop again.
        let seed = work.parent().unwrap().join("seed");
        write(&seed, "Other.txt", b"v2\n");
        sh(&seed, &["commit", "-qam", "Another change"]);
        sh(&seed, &["push", "-q", "origin", "develop"]);

        // The server still has the commit from before the rebase, but it's ours: it's in the backup.
        let info = check_updates(&log, &p).unwrap();
        assert_eq!(info.server_has_more, 0);
        assert!(info.blocked_reason.is_none(), "{:?}", info.blocked_reason);
        let r = apply_update(&log, &p, true).unwrap();
        assert!(r.pushed);
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), sh(&work, &["rev-parse", "origin/feat/x"]));
    }

    #[test]
    fn skips_a_submodule_with_local_changes() {
        let work = scenario("submodule");
        let root = work.parent().unwrap().to_path_buf();
        // A submodule repo, added to the project.
        sh(&root, &["init", "-q", "-b", "main", "subsrc"]);
        let sub_src = root.join("subsrc");
        configure(&sub_src);
        write(&sub_src, "tool.txt", b"v1\n");
        sh(&sub_src, &["add", "-A"]);
        sh(&sub_src, &["commit", "-qm", "Lib"]);
        sh(&work, &["-c", "protocol.file.allow=always", "submodule", "add", "-q", sub_src.to_str().unwrap(), "Tools"]);
        sh(&work, &["commit", "-qm", "Add Scripts"]);
        sh(&work, &["push", "-q"]);
        write(&work.join("Tools"), "tool.txt", b"my local edit\n");

        let log = Log::none();
        let p = project(&work);
        let warnings = update_submodules(&log, &p);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("Tools"));
        assert_eq!(std::fs::read_to_string(work.join("Tools/tool.txt")).unwrap(), "my local edit\n");
    }

    #[test]
    fn never_pops_an_older_stash() {
        let work = scenario("oldstash");
        let root = work.parent().unwrap().to_path_buf();
        // A submodule, so its uncommitted changes make the tree dirty without anything to stash.
        sh(&root, &["init", "-q", "-b", "main", "subsrc2"]);
        let sub_src = root.join("subsrc2");
        configure(&sub_src);
        write(&sub_src, "tool.txt", b"v1
");
        sh(&sub_src, &["add", "-A"]);
        sh(&sub_src, &["commit", "-qm", "Tool"]);
        sh(&work, &["-c", "protocol.file.allow=always", "submodule", "add", "-q", sub_src.to_str().unwrap(), "Tools"]);
        sh(&work, &["commit", "-qm", "Add Tools"]);
        sh(&work, &["push", "-q"]);
        // The person's own older stash, which must stay where it is.
        write(&work, "Other.txt", b"kept in my stash
");
        sh(&work, &["stash", "push", "-qm", "my own stash"]);
        // Now only the submodule is dirty.
        write(&work.join("Tools"), "tool.txt", b"edited
");

        let log = Log::none();
        let p = project(&work);
        assert!(check_updates(&log, &p).unwrap().has_local_changes);
        apply_update(&log, &p, true).unwrap();
        assert_eq!(stash_count(&work), 1, "the older stash is still there");
        assert_eq!(std::fs::read_to_string(work.join("Other.txt")).unwrap(), "v1
", "and wasn't applied");
        assert_eq!(std::fs::read_to_string(work.join("Tools/tool.txt")).unwrap(), "edited
");
    }

    /// Switches a scenario's clone to merging, through the personal override file.
    fn use_merge(work: &Path) {
        write(work, "ue-dev-hub.local.toml", b"[git]\nupdate = \"merge\"\n");
    }

    #[test]
    fn merges_the_base_branch_and_pushes_without_force() {
        let work = scenario("merge");
        use_merge(&work);
        let log = Log::none();
        let p = project(&work);
        let before = sh(&work, &["rev-parse", "HEAD"]);
        let r = apply_update(&log, &p, true).unwrap();
        assert!(r.pushed);
        // Both the teammate's change and my commit are there, and my old commit is kept as is.
        assert_eq!(std::fs::read(work.join("Map.umap")).unwrap(), vec![9, 9, 9, 0, 0xfe]);
        assert!(work.join("Mine.txt").is_file());
        sh(&work, &["merge-base", "--is-ancestor", &before, "HEAD"]);
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), sh(&work, &["rev-parse", "origin/feat/x"]));
    }

    #[test]
    fn merging_brings_in_commits_pushed_from_elsewhere() {
        let work = scenario("mergeserver");
        use_merge(&work);
        let root = work.parent().unwrap().to_path_buf();
        sh(&root, &["clone", "-q", "-c", "core.autocrlf=false", "-b", "feat/x", root.join("origin.git").to_str().unwrap(), "laptop"]);
        let laptop = root.join("laptop");
        configure(&laptop);
        write(&laptop, "FromLaptop.txt", b"laptop\n");
        sh(&laptop, &["add", "-A"]);
        sh(&laptop, &["commit", "-qm", "From my laptop"]);
        sh(&laptop, &["push", "-q"]);

        let log = Log::none();
        let p = project(&work);
        let info = check_updates(&log, &p).unwrap();
        assert_eq!(info.server_has_more, 1);
        assert!(info.blocked_reason.is_none(), "merging isn't blocked by it: {:?}", info.blocked_reason);
        apply_update(&log, &p, true).unwrap();
        assert!(work.join("FromLaptop.txt").is_file());
        assert_eq!(std::fs::read(work.join("Map.umap")).unwrap(), vec![9, 9, 9, 0, 0xfe]);
    }

    #[test]
    fn a_merge_conflict_changes_nothing() {
        let work = scenario("mergeconflict");
        use_merge(&work);
        write(&work, "Other.txt", b"mine\n");
        sh(&work, &["commit", "-qam", "My edit"]);
        let seed = work.parent().unwrap().join("seed");
        write(&seed, "Other.txt", b"theirs\n");
        sh(&seed, &["commit", "-qam", "Their edit"]);
        sh(&seed, &["push", "-q", "origin", "develop"]);

        let log = Log::none();
        let p = project(&work);
        let before = sh(&work, &["rev-parse", "HEAD"]);
        let err = apply_update(&log, &p, true).unwrap_err();
        assert!(err.contains("Nothing was changed"), "{err}");
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), before);
        assert!(!work.join(".git/MERGE_HEAD").exists());
        assert_eq!(std::fs::read_to_string(work.join("Other.txt")).unwrap(), "mine\n");
    }

    #[test]
    fn push_never_only_updates_locally() {
        let work = scenario("pushnever");
        write(&work, "ue-dev-hub.local.toml", b"[git]\nupdate = \"merge\"\npush = \"never\"\n");
        let log = Log::none();
        let p = project(&work);
        let server = sh(&work, &["rev-parse", "origin/feat/x"]);
        let r = apply_update(&log, &p, true).unwrap();
        assert!(!r.pushed);
        sh(&work, &["fetch", "-q", "origin"]);
        assert_eq!(sh(&work, &["rev-parse", "origin/feat/x"]), server);
    }
}
