//! Reading the Unreal project: its .uproject and its `ue-dev-hub.toml`.

use crate::config::{self, Config, Issue};
use crate::proc;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    /// The .uproject's name, used for targets and file names.
    pub name: String,
    pub uproject: PathBuf,
    /// "5.8" for hub installs, or a "{GUID}" for registered source builds.
    pub engine_version: String,
    pub config: Config,
    pub config_issues: Vec<Issue>,
    pub has_config: bool,
    pub has_local_config: bool,
    /// Set when the Git settings in the config may be wrong, so updating must wait.
    pub config_git_problem: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub root: PathBuf,
    pub name: String,
    pub display_name: String,
    pub engine_version: String,
    pub branch: Option<String>,
    pub base_branch: String,
    pub has_config: bool,
    pub color: Option<String>,
    pub links: Vec<config::Link>,
    pub tools: Vec<String>,
    pub default_widgets: Vec<String>,
    pub checklist_url: Option<String>,
    pub issue_url: Option<String>,
    pub launch_args: String,
    pub main_level_suffix: String,
    pub lighting_quality: String,
    pub wwise_version: Option<String>,
    pub soundbanks: bool,
    pub zip: bool,
    pub has_dependencies: bool,
    pub upload_configured: bool,
    pub upload_after_packaging: bool,
}

/// The branch in `git ls-remote --symref origin HEAD` output: "ref: refs/heads/develop\tHEAD".
fn parse_symref_head(out: &str) -> Option<String> {
    out.lines()
        .find_map(|l| l.strip_prefix("ref: refs/heads/"))
        .and_then(|rest| rest.split_whitespace().next())
        .map(str::to_string)
        .filter(|b| config::is_branch_name(b))
}

/// The .uproject in `root`, if there is one.
pub fn find_uproject(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("uproject")))
}

/// The nearest folder at or above `start` with a .uproject.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|d| find_uproject(d).is_some()).map(Path::to_path_buf)
}

impl Project {
    pub fn load(root: &Path) -> Result<Project, String> {
        if !root.is_dir() {
            return Err(format!("The folder {} doesn't exist anymore.", root.display()));
        }
        let uproject = find_uproject(root).ok_or("This folder doesn't contain an Unreal project (.uproject file).")?;
        let name = uproject.file_stem().unwrap().to_string_lossy().to_string();
        let loaded = config::load(root);

        let association = std::fs::read_to_string(&uproject)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("EngineAssociation")?.as_str().map(str::to_string))
            .unwrap_or_default();

        let engine_version = loaded
            .config
            .project
            .engine_version
            .clone()
            .filter(|v| !v.is_empty())
            .or(Some(association).filter(|v| !v.is_empty()))
            .ok_or("The .uproject doesn't say which engine version it uses (EngineAssociation).")?;
        if !config::is_engine_version(&engine_version) {
            return Err(format!("The engine version '{engine_version}' in the .uproject isn't one the hub understands."));
        }

        Ok(Project {
            root: root.to_path_buf(),
            name,
            uproject,
            engine_version,
            config: loaded.config,
            config_issues: loaded.issues,
            has_config: loaded.has_file,
            has_local_config: loaded.has_local_file,
            config_git_problem: loaded.git_problem,
        })
    }

    pub fn display_name(&self) -> String {
        self.config.project.display_name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| self.name.clone())
    }

    /// Runs a Git command that only reads. These run just because a project was opened, so they
    /// never start the repository's file monitor: a folder from an archive or USB stick could name
    /// any program as its core.fsmonitor.
    pub fn git(&self, args: &[&str]) -> Option<String> {
        let args: Vec<&str> = crate::git::READ_ONLY.iter().copied().chain(args.iter().copied()).collect();
        proc::run_timeout("git", &args, Some(&self.root), 120).ok().filter(|o| o.ok).map(|o| o.stdout)
    }

    pub fn current_branch(&self) -> Option<String> {
        self.git(&["rev-parse", "--abbrev-ref", "HEAD"])
    }

    pub fn default_branch(&self) -> String {
        if let Some(b) = self.config.project.base_branch.clone().filter(|b| !b.is_empty()) {
            return b;
        }
        self.git(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
            .and_then(|o| o.strip_prefix("origin/").map(str::to_string))
            .filter(|b| config::is_branch_name(b))
            .or_else(|| self.server_head_branch())
            .or_else(|| {
                ["main", "master", "develop"]
                    .into_iter()
                    .find(|b| self.git(&["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{b}")]).is_some())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "main".into())
    }

    /// The server's default branch, asked from the server when the clone doesn't know it (no
    /// `origin/HEAD`). Answers are kept for this run; failures are tried again after a while.
    fn server_head_branch(&self) -> Option<String> {
        use std::collections::HashMap;
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        /// The answer for each project, and when it was asked.
        type Answers = HashMap<PathBuf, (Option<String>, Instant)>;
        static CACHE: Mutex<Option<Answers>> = Mutex::new(None);
        let lock = || CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((branch, when)) = lock().get_or_insert_with(HashMap::new).get(&self.root)
            && (branch.is_some() || when.elapsed() < Duration::from_secs(300))
        {
            return branch.clone();
        }
        let branch = proc::run_timeout("git", &["ls-remote", "--symref", "origin", "HEAD"], Some(&self.root), 15)
            .ok()
            .filter(|o| o.ok)
            .and_then(|o| parse_symref_head(&o.stdout));
        lock().get_or_insert_with(HashMap::new).insert(self.root.clone(), (branch.clone(), Instant::now()));
        branch
    }

    pub fn tools(&self) -> Vec<String> {
        match &self.config.tools.enabled {
            Some(t) => t.clone(),
            None => config::TOOLS.iter().map(|t| t.to_string()).collect(),
        }
    }

    pub fn info(&self) -> ProjectInfo {
        let c = &self.config;
        ProjectInfo {
            root: self.root.clone(),
            name: self.name.clone(),
            display_name: self.display_name(),
            engine_version: self.engine_version.clone(),
            branch: self.current_branch(),
            base_branch: self.default_branch(),
            has_config: self.has_config,
            color: c.project.color.clone(),
            links: c.links.clone(),
            tools: self.tools(),
            default_widgets: c
                .home
                .widgets
                .clone()
                .unwrap_or_else(|| config::DEFAULT_WIDGETS.iter().map(|w| w.to_string()).collect()),
            checklist_url: c.package.checklist_url.clone(),
            issue_url: c.report.issue_url.clone(),
            launch_args: c.test_map.launch_args.clone(),
            main_level_suffix: c.test_map.main_level_suffix.clone(),
            lighting_quality: c.package.lighting_quality.clone(),
            wwise_version: c.requirements.wwise.clone(),
            soundbanks: c.package.soundbanks,
            zip: c.package.zip,
            has_dependencies: !c.dependencies.is_empty(),
            upload_configured: c.upload.nextcloud_url.is_some(),
            upload_after_packaging: c.upload.after_packaging,
        }
    }

    pub fn has_source(&self) -> bool {
        self.root.join("Source").is_dir()
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("Saved").join("Logs")
    }

    pub fn builds_dir(&self) -> PathBuf {
        self.root.join(&self.config.package.output_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_servers_default_branch() {
        assert_eq!(parse_symref_head("ref: refs/heads/develop\tHEAD\n1234abcd\tHEAD").as_deref(), Some("develop"));
        assert_eq!(parse_symref_head("1234abcd\tHEAD"), None);
        assert_eq!(parse_symref_head("ref: refs/heads/-x\tHEAD"), None);
    }
}
