//! The setup checklist: every tool the project needs, each with a way to fix it.

use crate::engine::{self, Engine, Source};
use std::path::Path;
use crate::proc;
use crate::project::Project;
use crate::settings::Settings;
use serde::Serialize;

#[derive(Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Ok,
    /// Works, but something should be fixed.
    Warning,
    /// Blocks building or launching.
    Error,
    /// Not installed, but only nice to have. Never counts as a problem.
    Optional,
}

#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Action {
    OpenUrl { url: String },
    BrowseEngine,
    CreateConfig,
    OpenConfig,
    /// Use the project's Git hooks folder (true) or not (false).
    UseGitHooks { yes: bool },
}

#[derive(Serialize, Clone)]
pub struct Fix {
    pub label: String,
    #[serde(flatten)]
    pub action: Action,
}

#[derive(Serialize, Clone)]
pub struct Check {
    pub id: &'static str,
    pub title: String,
    pub status: Status,
    pub detail: String,
    pub fixes: Vec<Fix>,
}

fn url_fix(label: &str, url: &str) -> Fix {
    Fix { label: label.into(), action: Action::OpenUrl { url: url.into() } }
}

pub fn run_all(project: &Project, settings: &Settings) -> Vec<Check> {
    let mut checks = vec![config_check(project), git(), git_lfs()];
    checks.extend(git_hooks(project, settings));
    checks.extend([engine_check(project, settings), dotnet(project, settings)]);
    if cfg!(windows) {
        checks.push(visual_studio(project));
        if project.config.requirements.wwise.is_some() {
            checks.push(wwise(project));
        }
    } else {
        checks.push(linux_tool("unzip", "unzip", "Needed to download dependencies."));
        checks.push(linux_tool("zip", "zip", "Needed to zip builds, unless 7-Zip is installed."));
    }
    checks.push(seven_zip());
    checks
}

fn config_check(project: &Project) -> Check {
    use crate::config::{FILE_NAME, Level};
    let errors = project.config_issues.iter().filter(|i| i.level == Level::Error).count();
    let detail = if !project.has_config {
        format!("This project has no {FILE_NAME}, so the hub uses its defaults. Create one to add links, packaging settings and tools for everyone.")
    } else if project.config_issues.is_empty() {
        format!("{FILE_NAME} is valid{}.", if project.has_local_config { ", with your ue-dev-hub.local.toml on top" } else { "" })
    } else {
        project.config_issues.iter().map(|i| i.message.clone()).collect::<Vec<_>>().join("
")
    };
    Check {
        id: "config",
        title: "Project settings".into(),
        status: if !project.has_config {
            Status::Optional
        } else if errors > 0 {
            Status::Error
        } else if project.config_issues.is_empty() {
            Status::Ok
        } else {
            Status::Warning
        },
        detail,
        fixes: if project.has_config {
            if project.config_issues.is_empty() { vec![] } else { vec![Fix { label: format!("Open {FILE_NAME}"), action: Action::OpenConfig }] }
        } else {
            vec![Fix { label: format!("Create {FILE_NAME}"), action: Action::CreateConfig }]
        },
    }
}

fn wwise(project: &Project) -> Check {
    let version = project.config.requirements.wwise.clone().unwrap_or_default();
    let found = crate::wwise::console(project).is_some();
    Check {
        id: "wwise",
        title: format!("Wwise {version}"),
        status: if found { Status::Ok } else { Status::Warning },
        detail: if found { "Only needed for packaging".into() } else { "Not installed. Only needed to generate soundbanks when packaging. Install it with the Audiokinetic Hub.".into() },
        fixes: if found { vec![] } else { vec![url_fix("Get the Audiokinetic Hub", "https://www.audiokinetic.com/en/download/")] },
    }
}

fn linux_tool(id: &'static str, name: &str, why: &str) -> Check {
    let found = proc::which(name).is_some();
    Check {
        id,
        title: name.into(),
        status: if found { Status::Ok } else { Status::Warning },
        detail: if found { "Installed".into() } else { format!("{why} Install it with your package manager.") },
        fixes: vec![],
    }
}

fn seven_zip() -> Check {
    let found = crate::proc::seven_zip().is_some();
    Check {
        id: "sevenZip",
        title: "7-Zip (optional)".into(),
        status: if found { Status::Ok } else { Status::Optional },
        detail: if found { "Used to zip builds".into() } else { if cfg!(windows) { "Not installed. Builds are zipped with Windows tar instead.".into() } else { "Not installed. Builds are zipped with zip instead.".into() } },
        fixes: if found { vec![] } else { vec![url_fix("Download 7-Zip", "https://www.7-zip.org/")] },
    }
}

fn git() -> Check {
    let ok = proc::exists("git", "--version");
    Check {
        id: "git",
        title: "Git".into(),
        status: if ok { Status::Ok } else { Status::Error },
        detail: if ok { "Installed".into() } else { "Git is needed to download the project. Install it, then restart the hub.".into() },
        fixes: if ok { vec![] } else { vec![url_fix("Download Git", "https://git-scm.com/downloads")] },
    }
}

fn git_lfs() -> Check {
    let ok = proc::run("git", &["lfs", "version"], None).map(|o| o.ok).unwrap_or(false);
    Check {
        id: "gitLfs",
        title: "Git LFS".into(),
        status: if ok { Status::Ok } else { Status::Error },
        detail: if ok { "Installed".into() } else { "Git LFS is needed to download the game's art and audio files.".into() },
        fixes: if ok { vec![] } else { vec![url_fix("Download Git LFS", "https://git-lfs.com")] },
    }
}

/// The project's Git hooks folder: used only after the person says yes, since hooks are programs
/// from the repository that Git runs on commit and push.
fn git_hooks(project: &Project, settings: &Settings) -> Option<Check> {
    let h = crate::git::hooks(project)?;
    let prefs = settings.prefs(&project.root);
    let mk = |status, detail: String, fixes| Check { id: "gitHooks", title: "Git hooks".into(), status, detail, fixes };
    let yes = Fix { label: "Use the project's hooks".into(), action: Action::UseGitHooks { yes: true } };
    let no = Fix { label: "Don't use them".into(), action: Action::UseGitHooks { yes: false } };
    let global = if h.global.is_empty() {
        String::new()
    } else {
        format!("\n\nYou also have a hooks folder for all your repositories ({}). In this project, the project's folder would be used instead.", h.global)
    };
    Some(if h.in_use() {
        mk(Status::Ok, format!("Uses the project's Git hooks from the {} folder.", h.folder), vec![no])
    } else if !h.hub_may_change(prefs.git_hooks_set.as_deref()) {
        mk(
            Status::Optional,
            format!("This project wants to use its Git hooks from the {} folder, but this copy already uses hooks from {}. The hub leaves that alone.", h.folder, h.local),
            vec![],
        )
    } else if prefs.git_hooks == Some(false) {
        mk(Status::Optional, format!("You chose not to use the project's Git hooks from the {} folder.{global}", h.folder), vec![yes])
    } else {
        mk(
            Status::Warning,
            format!(
                "This project wants to use its Git hooks from the {} folder. Hooks are programs from the repository that Git runs when you commit or push, for example to check your changes. Only use them if you trust this project.{global}",
                h.folder
            ),
            vec![yes, no],
        )
    })
}

fn engine_check(project: &Project, settings: &Settings) -> Check {
    let title = format!("Unreal Engine {}", project.engine_version);
    match engine::locate(project, settings) {
        Some(Engine { path, source }) => Check {
            id: "engine",
            title,
            status: Status::Ok,
            detail: format!("{} ({})", path.display(), describe(source)),
            fixes: vec![],
        },
        None => Check {
            id: "engine",
            title,
            status: Status::Error,
            detail: engine_missing(&project.engine_version),
            fixes: vec![Fix { label: "Find the engine folder...".into(), action: Action::BrowseEngine }, get_engine_fix()],
        },
    }
}

fn engine_missing(version: &str) -> String {
    if cfg!(windows) {
        format!("Unreal Engine {version} wasn't found. Install it from the Epic Games Launcher, or show the hub where it is.")
    } else if version.starts_with('{') {
        format!(
            "This project uses a source build of Unreal Engine ({version}), probably from a teammate's PC. That ID only works there. Show the hub your own engine folder."
        )
    } else {
        format!("Unreal Engine {version} wasn't found. Download it from Epic or build it from source, then show the hub the engine folder.")
    }
}

/// Where to get the engine: the Epic Games Launcher on Windows, Epic's Linux download on Linux.
pub fn get_engine_fix() -> Fix {
    if cfg!(windows) {
        url_fix("Open Epic Games Launcher", "com.epicgames.launcher://ue")
    } else {
        url_fix("Unreal Engine for Linux", "https://www.unrealengine.com/linux")
    }
}

fn describe(source: Source) -> &'static str {
    match source {
        Source::Settings => "picked by you",
        Source::Registry if !cfg!(windows) => "from Install.ini",
        Source::Registry => "from the registry",
        Source::SourceBuild => "source build",
        Source::LauncherManifest => "from the Epic Games Launcher",
        Source::Search => "found by searching",
    }
}

fn visual_studio(project: &Project) -> Check {
    let wanted = &project.config.requirements.visual_studio;
    let vswhere = std::env::var_os("ProgramFiles(x86)")
        .map(|p| std::path::PathBuf::from(p).join(r"Microsoft Visual Studio\Installer\vswhere.exe"));
    let versions = vswhere
        .filter(|p| p.is_file())
        .and_then(|p| {
            proc::run(
                &p.to_string_lossy(),
                &["-products", "*", "-prerelease", "-property", "catalog_productLineVersion"],
                None,
            )
            .ok()
        })
        .map(|o| o.stdout)
        .unwrap_or_default();

    // Accept the wanted version or anything newer.
    let found = versions.lines().filter_map(|v| v.trim().parse::<u32>().ok()).max();
    let ok = match (found, wanted.parse::<u32>()) {
        (Some(f), Ok(w)) => f >= w,
        _ => versions.contains(wanted.as_str()),
    };
    Check {
        id: "visualStudio",
        title: format!("Visual Studio {wanted}"),
        status: if ok { Status::Ok } else { Status::Error },
        detail: if ok {
            format!("Visual Studio {} installed", found.map(|f| f.to_string()).unwrap_or_default())
        } else {
            format!("Visual Studio {wanted} or newer is needed to compile the game's code. Install the free Community edition with the \"Game development with C++\" workload.")
        },
        fixes: if ok { vec![] } else { vec![url_fix("Download Visual Studio", "https://visualstudio.microsoft.com/downloads/")] },
    }
}

/// UnrealBuildTool runs on the .NET SDK that comes with the engine, so only that copy matters.
/// People who set UE_USE_SYSTEM_DOTNET=1 need a system-wide SDK of the same major version.
fn dotnet(project: &Project, settings: &Settings) -> Check {
    let mk = |status, detail: String, fixes| Check { id: "dotnet", title: ".NET".into(), status, detail, fixes };
    let Some(engine) = engine::locate(project, settings) else {
        return mk(Status::Optional, "Checked once Unreal Engine is found.".into(), vec![]);
    };
    let bundled = engine.bundled_dotnet();
    let use_system = std::env::var("UE_USE_SYSTEM_DOTNET").is_ok_and(|v| v == "1");

    if use_system {
        let wanted = bundled.as_ref().and_then(|b| b.version.split('.').next()?.parse::<u32>().ok());
        let sdks = proc::run("dotnet", &["--list-sdks"], None).ok().filter(|o| o.ok).map(|o| o.stdout).unwrap_or_default();
        let newest = sdks.lines().filter_map(|l| l.split('.').next()?.trim().parse::<u32>().ok()).max();
        return match (newest, wanted) {
            (Some(n), Some(w)) if n >= w => mk(Status::Ok, format!("Your own .NET {n} (UE_USE_SYSTEM_DOTNET is set)"), vec![]),
            (Some(n), None) => mk(Status::Ok, format!("Your own .NET {n} (UE_USE_SYSTEM_DOTNET is set)"), vec![]),
            _ => mk(
                Status::Error,
                format!(
                    "UE_USE_SYSTEM_DOTNET is set, so Unreal uses the .NET installed on this PC, and .NET {} or newer isn't installed. Install it, or remove that variable to use the .NET that comes with Unreal.",
                    wanted.map(|w| w.to_string()).unwrap_or_default()
                ),
                vec![url_fix("Download .NET", "https://dotnet.microsoft.com/download")],
            ),
        };
    }

    match bundled {
        Some(dn) => mk(Status::Ok, format!("Uses the .NET {} that comes with Unreal Engine", dn.version), vec![]),
        None if !engine.path.join("Engine/Binaries/ThirdParty/DotNet").exists() && !has_dotnet_scripts(&engine.path) => {
            // Engines before UE5 don't bundle .NET.
            mk(Status::Optional, "This engine doesn't use .NET.".into(), vec![])
        }
        None => mk(
            Status::Error,
            if cfg!(windows) {
                "The .NET that comes with Unreal Engine is missing from the engine folder, so the game code can't compile. Repair the engine: in the Epic Games Launcher, open the menu next to the engine's Launch button and choose Verify.".into()
            } else {
                "The .NET that comes with Unreal Engine is missing from the engine folder, so the game code can't compile. Download the engine again, or run Setup.sh in a source build.".into()
            },
            vec![get_engine_fix()],
        ),
    }
}

fn has_dotnet_scripts(engine: &Path) -> bool {
    engine.join("Engine/Build/BatchFiles/GetDotnetPath.bat").is_file()
}
