mod addon_host;
mod addon_index;
mod addon_protocol;
mod addons;
mod avatar;
mod build;
mod checks;
mod config;
mod deps;
mod engine;
mod git;
mod home;
mod jobs;
mod lighting;
mod log;
mod maps;
mod nextcloud;
mod package;
mod proc;
mod project;
mod report;
mod selfupdate;
mod settings;
mod tempdir;
mod testmap;
mod tools;
mod wwise;

use crate::log::Log;
use project::{Project, ProjectInfo};
use serde::Serialize;
use settings::Prefs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

fn current_project(app: &AppHandle) -> Result<Project, String> {
    let root = settings::load(app).selected.ok_or("No project selected yet.")?;
    Project::load(&root)
}

fn current_engine(app: &AppHandle, project: &Project) -> Result<engine::Engine, String> {
    engine::locate(project, &settings::load(app))
        .ok_or_else(|| format!("Unreal Engine {} wasn't found. Check the Setup screen.", project.engine_version))
}

/// Runs blocking work off the main thread so the window stays responsive.
async fn blocking<T: Send + 'static, E: From<String> + Send + 'static>(f: impl FnOnce() -> Result<T, E> + Send + 'static) -> Result<T, E> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| E::from(e.to_string()))?
}

// ---------- Projects ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectEntry {
    root: PathBuf,
    display_name: String,
    color: Option<String>,
    branch: Option<String>,
    engine_version: Option<String>,
    /// The folder or its .uproject is gone.
    missing: bool,
    selected: bool,
}

#[tauri::command]
async fn list_projects(app: AppHandle) -> Result<Vec<ProjectEntry>, String> {
    blocking(move || {
        let s = settings::load(&app);
        Ok(s.projects
            .iter()
            .map(|root| {
                let selected = s.selected.as_deref().is_some_and(|sel| settings::same_path(sel, root));
                match Project::load(root) {
                    Ok(p) => ProjectEntry {
                        root: root.clone(),
                        display_name: p.display_name(),
                        color: p.config.project.color.clone(),
                        branch: p.current_branch(),
                        engine_version: Some(p.engine_version.clone()),
                        missing: false,
                        selected,
                    },
                    Err(_) => ProjectEntry {
                        root: root.clone(),
                        display_name: root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                        color: None,
                        branch: None,
                        engine_version: None,
                        missing: true,
                        selected,
                    },
                }
            })
            .collect())
    })
    .await
}

#[tauri::command]
async fn get_project(app: AppHandle) -> Result<Option<ProjectInfo>, String> {
    blocking(move || {
        if settings::load(&app).selected.is_none() {
            return Ok(None);
        }
        current_project(&app).map(|p| Some(p.info()))
    })
    .await
}

/// Adds a project (any folder inside it works) and selects it.
#[tauri::command]
async fn add_project(app: AppHandle, path: PathBuf) -> Result<ProjectInfo, String> {
    if let Some(what) = jobs::busy_with() {
        return Err(format!("{what} is still running. Wait for it to finish before adding a project."));
    }
    blocking(move || {
        let root = project::find_root(&path).ok_or("This folder isn't inside an Unreal project (no .uproject file found).")?;
        let p = Project::load(&root)?;
        settings::update(&app, |s| s.open(p.root.clone()))?;
        Ok(p.info())
    })
    .await
}

#[tauri::command]
async fn select_project(app: AppHandle, path: PathBuf) -> Result<ProjectInfo, String> {
    if let Some(what) = jobs::busy_with() {
        return Err(format!("{what} is still running. Wait for it to finish before switching projects."));
    }
    add_project(app, path).await
}

#[tauri::command]
async fn remove_project(app: AppHandle, path: PathBuf) -> Result<(), String> {
    blocking(move || {
        settings::update(&app, |s| {
            s.projects.retain(|p| !settings::same_path(p, &path));
            if s.selected.as_deref().is_some_and(|sel| settings::same_path(sel, &path)) {
                s.selected = s.projects.first().cloned();
            }
        })
    })
    .await
}

#[tauri::command]
async fn get_prefs(app: AppHandle) -> Result<Prefs, String> {
    blocking(move || {
        let p = current_project(&app)?;
        Ok(settings::load(&app).prefs(&p.root))
    })
    .await
}

#[tauri::command]
async fn set_widgets(app: AppHandle, widgets: Option<Vec<String>>) -> Result<(), String> {
    blocking(move || {
        let p = current_project(&app)?;
        let widgets = widgets.map(|w| w.into_iter().filter(|x| config::WIDGETS.contains(&x.as_str())).collect());
        settings::update(&app, |s| s.prefs_mut(&p.root).widgets = widgets)
    })
    .await
}

#[tauri::command]
fn get_auto_launch(app: AppHandle) -> bool {
    settings::load(&app).auto_launch.unwrap_or(false)
}

#[tauri::command]
async fn set_auto_launch(app: AppHandle, enabled: bool) -> Result<(), String> {
    blocking(move || settings::update(&app, |s| s.auto_launch = Some(enabled))).await
}

#[tauri::command]
fn get_after_launch(app: AppHandle) -> String {
    settings::load(&app).after_launch.unwrap_or_else(|| "keep".into())
}

#[tauri::command]
async fn set_after_launch(app: AppHandle, action: String) -> Result<(), String> {
    if !["keep", "minimize", "close"].contains(&action.as_str()) {
        return Err(format!("Unknown choice '{action}'."));
    }
    blocking(move || settings::update(&app, |s| s.after_launch = Some(action))).await
}

/// Called once the editor the hub started is running. Minimizes or closes the hub if
/// the person asked for that, but never while something is still running.
#[tauri::command]
fn editor_started(app: AppHandle) {
    if jobs::busy_with().is_some() {
        return;
    }
    match settings::load(&app).after_launch.as_deref() {
        Some("minimize") => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.minimize();
            }
        }
        Some("close") => app.exit(0),
        _ => {}
    }
}

#[tauri::command]
async fn set_app_choice(app: AppHandle, kind: String, id: String) -> Result<(), String> {
    blocking(move || {
        let p = current_project(&app)?;
        settings::update(&app, |s| {
            let prefs = s.prefs_mut(&p.root);
            match kind.as_str() {
                "ide" => prefs.ide = Some(id),
                _ => prefs.terminal = Some(id),
            }
        })
    })
    .await
}

// ---------- Setup ----------

#[tauri::command]
async fn run_checks(app: AppHandle) -> Result<Vec<checks::Check>, String> {
    blocking(move || {
        let project = current_project(&app)?;
        Ok(checks::run_all(&project, &settings::load(&app)))
    })
    .await
}

#[tauri::command]
async fn set_engine_path(app: AppHandle, path: PathBuf) -> Result<(), String> {
    blocking(move || {
        if !engine::is_engine_root(&path) {
            return Err("That folder isn't an Unreal Engine install. Pick the folder that contains the \"Engine\" folder, for example UE_5.8.".into());
        }
        let project = current_project(&app)?;
        settings::update(&app, |s| {
            s.engine_paths.insert(project.engine_version.clone(), path);
        })
    })
    .await
}

#[tauri::command]
async fn create_config(app: AppHandle) -> Result<PathBuf, String> {
    blocking(move || {
        let project = current_project(&app)?;
        let path = project.root.join(config::FILE_NAME);
        if path.exists() {
            return Err(format!("{} already exists.", config::FILE_NAME));
        }
        std::fs::write(&path, config::template(&project.name, &project.default_branch())).map_err(|e| e.to_string())?;
        let _ = open_external(&app, &path.to_string_lossy());
        Ok(path)
    })
    .await
}

#[tauri::command]
fn open_config(app: AppHandle) -> Result<(), String> {
    let project = current_project(&app)?;
    let path = project.root.join(config::FILE_NAME);
    open_external(&app, &path.to_string_lossy())
}

// ---------- Launch ----------

#[tauri::command]
async fn avatars(emails: Vec<String>) -> Result<std::collections::HashMap<String, Option<String>>, String> {
    blocking(move || Ok(avatar::avatars(emails))).await
}

#[tauri::command]
async fn home_status(app: AppHandle, fetch: bool) -> Result<home::HomeStatus, String> {
    blocking(move || home::status(&current_project(&app)?, fetch)).await
}

#[tauri::command]
async fn prepare_repo(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let _busy = jobs::begin("Preparing the repository", true)?;
        let p = current_project(&app)?;
        prepare(&app, &Log::new(&app), &p)
    })
    .await
}

/// `git::prepare` with the person's answer about the project's Git hooks.
fn prepare(app: &AppHandle, log: &Log, p: &Project) -> Result<(), String> {
    let prefs = settings::load(app).prefs(&p.root);
    if let Some(set) = git::prepare(log, p, prefs.git_hooks, prefs.git_hooks_set.as_deref())? {
        settings::update(app, |s| s.prefs_mut(&p.root).git_hooks_set = Some(set))?;
    }
    Ok(())
}

/// Uses the project's Git hooks folder, or stops using it. Never changes a core.hooksPath that
/// the person or another tool set.
#[tauri::command]
async fn set_git_hooks(app: AppHandle, yes: bool) -> Result<(), String> {
    blocking(move || {
        let _busy = jobs::begin("Setting up Git hooks", false)?;
        let p = current_project(&app)?;
        let h = git::hooks(&p).ok_or("This project doesn't name a Git hooks folder.")?;
        let hub_set = settings::load(&app).prefs(&p.root).git_hooks_set;
        let log = Log::new(&app);
        if !h.hub_may_change(hub_set.as_deref()) {
            return Err(format!("This copy already uses Git hooks from {}. The hub leaves that alone.", h.local));
        }
        let set = if yes {
            if !h.in_use() {
                git::use_hooks(&log, &p, &h)?;
            }
            Some(h.wanted.clone())
        } else {
            if !h.local.is_empty() {
                git::stop_using_hooks(&log, &p)?;
            }
            None
        };
        settings::update(&app, |s| {
            let prefs = s.prefs_mut(&p.root);
            prefs.git_hooks = Some(yes);
            prefs.git_hooks_set = set;
        })
    })
    .await
}

#[tauri::command]
async fn check_updates(app: AppHandle) -> Result<git::UpdateInfo, String> {
    blocking(move || git::check_updates_if_idle(&Log::new(&app), &current_project(&app)?)).await
}

#[tauri::command]
async fn apply_update(app: AppHandle, push: bool) -> Result<git::UpdateResult, String> {
    blocking(move || {
        let _busy = jobs::begin("Updating", true)?;
        git::apply_update(&Log::new(&app), &current_project(&app)?, push)
    })
    .await
}

// ---------- Addons ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AddonsState {
    installed: Vec<addons::AddonInfo>,
    platform: String,
}

#[tauri::command]
async fn addons_list(app: AppHandle) -> Result<AddonsState, String> {
    blocking(move || {
        let project = current_project(&app).ok();
        Ok(AddonsState { installed: addons::infos(&app, project.as_ref())?, platform: addons::platform() })
    })
    .await
}

/// The official addons. Fails while the addons repository is private or there's no internet.
#[tauri::command]
async fn addons_available(app: AppHandle, fresh: bool) -> Result<Vec<addon_index::Available>, String> {
    blocking(move || addon_index::available(&app, fresh)).await
}

/// Turns an addon on or off for the current project, for this person only.
#[tauri::command]
async fn addon_set_enabled(app: AppHandle, id: String, enabled: bool) -> Result<(), String> {
    blocking(move || {
        let project = current_project(&app)?;
        let addon = addons::find(&app, &id)?;
        // Stops the helper first, so turning it off can't cut a step off halfway.
        let _stopped = if enabled { None } else { Some(addon_host::stop_when_idle(&addon.manifest.name, &id)?) };
        settings::update(&app, |s| {
            s.prefs_mut(&project.root).addons.insert(id.clone(), enabled);
        })
    })
    .await
}

#[tauri::command]
async fn addon_install(app: AppHandle, id: String, downgrade: Option<bool>) -> Result<addons::Manifest, addons::InstallError> {
    blocking(move || {
        addon_index::install(&app, &id, downgrade.unwrap_or(false))
    })
    .await
}

/// `downgrade` allows replacing a newer installed version.
#[tauri::command]
async fn addon_install_from_disk(app: AppHandle, path: PathBuf, downgrade: Option<bool>) -> Result<addons::Manifest, addons::InstallError> {
    blocking(move || {
        addons::install(&app, &path, addons::Source::Disk, addons::Expect { downgrade: downgrade.unwrap_or(false), ..Default::default() })
    })
    .await
}

#[tauri::command]
async fn addon_uninstall(app: AppHandle, id: String) -> Result<(), String> {
    blocking(move || {
        addons::uninstall(&app, &id)
    })
    .await
}

/// The current project's default addons that aren't installed yet, to ask the person about.
#[tauri::command]
async fn addon_default_offers(app: AppHandle) -> Result<addon_index::DefaultsOffer, String> {
    blocking(move || {
        let project = current_project(&app)?;
        if project.config.addons.default.is_empty() {
            return Ok(Default::default());
        }
        Ok(addon_index::default_offers(&app, &project))
    })
    .await
}

/// Installs the default addons the person said yes to (`install`), or remembers the no.
#[tauri::command]
async fn addon_install_defaults(app: AppHandle, ids: Vec<String>, install: bool) -> Result<addon_index::DefaultsResult, String> {
    blocking(move || addon_index::answer_defaults(&app, &current_project(&app)?, &ids, install)).await
}

/// A call from an addon's page to its helper. With `step`, it runs as a step that may change
/// the project: never alongside a job, Update & Launch or another step, and named by `step`.
#[tauri::command]
async fn addon_call(app: AppHandle, id: String, method: String, params: serde_json::Value, step: Option<String>) -> Result<serde_json::Value, String> {
    blocking(move || {
        let addon = addons::find(&app, &id)?;
        let project = current_project(&app).ok();
        if !addons::enabled_for(&settings::load(&app), project.as_ref(), &id) {
            return Err(format!("{} is turned off for this project.", addon.manifest.name));
        }
        if let Some(problem) = addon.manifest.problem(&app.package_info().version.to_string()) {
            return Err(problem);
        }
        let helper = addon.manifest.helper_path().ok_or_else(|| format!("{} has no helper program.", addon.manifest.name))?;
        let _busy = match &step {
            Some(what) => Some(jobs::begin(what, false)?),
            None => None,
        };
        // Lines from calls that aren't steps only go to the hub log, still tagged with the addon.
        let log = if step.is_some() { Log::for_addon(&app, &id) } else { Log { addon: Some(id.clone()), ..Log::default() } };
        let is_step = step.is_some();
        let exe = addon.dir.join(helper);
        let target = addon_host::Target { name: &addon.manifest.name, id: &id, exe: &exe, dir: &addon.dir };
        addon_host::call(
            &target,
            &method,
            params,
            project.as_ref().map(addon_host::context),
            is_step,
            &log,
            &|name, params| addon_host::serve(&log, project.as_ref(), is_step, name, params),
        )
    })
    .await
}

#[tauri::command]
async fn build_project(app: AppHandle) -> Result<build::BuildResult, String> {
    blocking(move || {
        let (_busy, log) = jobs::begin_cancellable(&app, "Compiling", true)?;
        let project = current_project(&app)?;
        let engine = current_engine(&app, &project)?;
        build::build(&log, &project, &engine)
    })
    .await
}

/// Whether an Unreal Editor has the open project open, like `editorRunning` in the screens' info.
#[tauri::command]
async fn editor_running(app: AppHandle) -> bool {
    tauri::async_runtime::spawn_blocking(move || current_project(&app).is_ok_and(|p| proc::editor_running_for(&p.uproject)))
        .await
        .unwrap_or(false)
}

#[tauri::command]
async fn launch_editor(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        // A Git step (switching branches, an update) may be rewriting the files right now.
        if let Some(step) = jobs::step_running() {
            return Err(format!("{step} is still running. Wait for it to finish before you open Unreal Editor."));
        }
        let project = current_project(&app)?;
        let engine = current_engine(&app, &project)?;
        build::launch(&project, &engine)
    })
    .await
}

// ---------- Package ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageInfo {
    maps: Vec<lighting::MapStatus>,
    qualities: Vec<String>,
    wwise_found: bool,
    /// Why soundbanks can't be generated here at all (Linux).
    wwise_unsupported: Option<&'static str>,
    seven_zip: bool,
    editor_running: bool,
    builds_dir: PathBuf,
    /// What build folder names end in, like "Win64".
    build_platform: &'static str,
}

#[tauri::command]
async fn package_info(app: AppHandle) -> Result<PackageInfo, String> {
    blocking(move || {
        let p = current_project(&app)?;
        Ok(PackageInfo {
            maps: lighting::statuses(&p),
            qualities: config::LIGHTING_QUALITIES.iter().map(|q| q.to_string()).collect(),
            wwise_found: wwise::console(&p).is_some(),
            wwise_unsupported: wwise::UNSUPPORTED,
            seven_zip: proc::seven_zip().is_some(),
            editor_running: proc::editor_running_for(&p.uproject),
            builds_dir: p.builds_dir(),
            build_platform: engine::platform(),
        })
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildsInfo {
    builds: Vec<package::BuildEntry>,
    builds_dir: PathBuf,
}

#[tauri::command]
async fn list_builds(app: AppHandle) -> Result<BuildsInfo, String> {
    blocking(move || {
        let p = current_project(&app)?;
        Ok(BuildsInfo { builds: package::builds(&p), builds_dir: p.builds_dir() })
    })
    .await
}

#[tauri::command]
async fn run_build(app: AppHandle, path: PathBuf) -> Result<(), String> {
    blocking(move || package::run_build(&current_project(&app)?, &path)).await
}

#[tauri::command]
async fn start_package(app: AppHandle, options: package::Options) -> Result<jobs::Snapshot, String> {
    blocking(move || {
        let p = current_project(&app)?;
        let e = current_engine(&app, &p)?;
        package::start(&app, p, e, options)
    })
    .await
}

#[tauri::command]
async fn start_bake(app: AppHandle, maps: Vec<String>, quality: String) -> Result<jobs::Snapshot, String> {
    blocking(move || {
        let p = current_project(&app)?;
        let e = current_engine(&app, &p)?;
        if proc::editor_running_for(&p.uproject) {
            return Err("Unreal Editor has this project open. Close it first, the bake needs the map's files.".into());
        }
        if !config::LIGHTING_QUALITIES.contains(&quality.as_str()) {
            return Err(format!("Unknown quality {quality}."));
        }
        let n = maps.len();
        jobs::start(
            &app,
            jobs::JobKind::Lighting,
            "Baking lighting".into(),
            format!("{n} map{} at {quality} quality", if n == 1 { "" } else { "s" }),
            vec!["Compile the editor".into(), "Bake lighting".into()],
            move |job| {
                job.running(0, "");
                package::compile(job, &p, &e, 0)?;
                job.running(1, "");
                lighting::bake(&job.log, &p, &e, &maps, &quality)?;
                job.done(1, "");
                Ok("Lighting is baked. The baked maps are modified in your working copy.".into())
            },
        )
    })
    .await
}

// ---------- Test a map ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestMapInfo {
    maps: Vec<maps::MapInfo>,
    recent: Vec<String>,
    source_build: bool,
    engine_label: String,
    runs: Vec<testmap::Run>,
    last_build_type: Option<String>,
    qualities: Vec<String>,
    editor_running: bool,
}

#[tauri::command]
async fn test_map_info(app: AppHandle) -> Result<TestMapInfo, String> {
    blocking(move || {
        let p = current_project(&app)?;
        let s = settings::load(&app);
        let e = engine::locate(&p, &s);
        let prefs = s.prefs(&p.root);
        Ok(TestMapInfo {
            maps: maps::list_all(&p),
            recent: prefs.recent_maps,
            source_build: e.as_ref().is_some_and(|e| e.is_source_build()),
            engine_label: match &e {
                Some(e) if e.is_source_build() => format!("a source build of {}", p.engine_version),
                _ if cfg!(windows) => format!("the Epic Games Launcher's {}", p.engine_version),
                _ => format!("Epic's {}", p.engine_version),
            },
            runs: testmap::list_runs(&p),
            last_build_type: prefs.last_build_type,
            qualities: config::LIGHTING_QUALITIES.iter().map(|q| q.to_string()).collect(),
            editor_running: proc::editor_running_for(&p.uproject),
        })
    })
    .await
}

#[tauri::command]
async fn play_test_run(app: AppHandle, id: String) -> Result<(), String> {
    blocking(move || testmap::play_run(&current_project(&app)?, &id)).await
}

#[tauri::command]
async fn rename_test_run(app: AppHandle, id: String, name: String) -> Result<(), String> {
    blocking(move || testmap::rename_run(&current_project(&app)?, &id, &name)).await
}

#[tauri::command]
async fn delete_test_run(app: AppHandle, id: String) -> Result<(), String> {
    blocking(move || testmap::delete_run(&current_project(&app)?, &id)).await
}

#[tauri::command]
async fn delete_all_test_runs(app: AppHandle) -> Result<usize, String> {
    blocking(move || testmap::delete_all_runs(&current_project(&app)?)).await
}

#[tauri::command]
async fn map_lighting(app: AppHandle, map: String) -> Result<lighting::MapStatus, String> {
    blocking(move || Ok(lighting::status(&current_project(&app)?, &map))).await
}

#[tauri::command]
async fn start_test_map(app: AppHandle, options: testmap::Options) -> Result<jobs::Snapshot, String> {
    blocking(move || {
        let p = current_project(&app)?;
        let e = current_engine(&app, &p)?;
        let map = options.map.clone();
        let build_type = options.build_type.clone();
        let snap = testmap::start(&app, p.clone(), e, options)?;
        // The job runs already; failing to remember the map shouldn't look like it didn't start.
        let _ = settings::update(&app, |s| {
            let prefs = s.prefs_mut(&p.root);
            prefs.recent_maps.retain(|m| *m != map);
            prefs.recent_maps.insert(0, map);
            prefs.recent_maps.truncate(6);
            prefs.last_build_type = Some(build_type);
        });
        Ok(snap)
    })
    .await
}

// ---------- Report a problem ----------

#[tauri::command]
async fn report_context(app: AppHandle) -> Result<report::Context, String> {
    blocking(move || Ok(report::context(&current_project(&app)?))).await
}

#[tauri::command]
async fn send_report(app: AppHandle, input: report::ReportInput) -> Result<report::Sent, String> {
    blocking(move || {
        let p = current_project(&app)?;
        report::send(&p, input)
    })
    .await
}

/// Asks for screenshots or videos to attach. Only files picked here (or in the project) can be
/// sent with a report.
#[tauri::command]
async fn report_pick_media(app: AppHandle) -> Result<Vec<PathBuf>, String> {
    use tauri_plugin_dialog::DialogExt;
    blocking(move || {
        let picked = app
            .dialog()
            .file()
            .set_title("Add a screenshot or video")
            .add_filter("Images and videos", report::MEDIA_EXTENSIONS)
            .blocking_pick_files()
            .unwrap_or_default();
        let files: Vec<PathBuf> = picked.into_iter().filter_map(|f| f.into_path().ok()).collect();
        report::remember_picked(&files);
        Ok(files)
    })
    .await
}

#[tauri::command]
async fn report_markdown(app: AppHandle, input: report::ReportInput) -> Result<String, String> {
    blocking(move || {
        let p = current_project(&app)?;
        Ok(report::markdown(&report::context(&p), &input))
    })
    .await
}

// ---------- Nextcloud ----------

fn uploads_state_dir(app: &AppHandle) -> PathBuf {
    app.path().app_config_dir().unwrap_or_else(|_| std::env::temp_dir())
}

#[tauri::command]
async fn nextcloud_status(app: AppHandle, probe: bool) -> Result<nextcloud::Status, String> {
    blocking(move || Ok(nextcloud::status(&current_project(&app)?, probe))).await
}

#[tauri::command]
async fn nextcloud_connect(app: AppHandle) -> Result<String, String> {
    blocking(move || {
        let p = current_project(&app)?;
        let opener = app.clone();
        nextcloud::connect(&Log::new(&app), &p, move |url| open_external(&opener, url))
    })
    .await
}

#[tauri::command]
fn nextcloud_cancel_connect() {
    nextcloud::cancel_connect();
}

#[tauri::command]
async fn nextcloud_disconnect(app: AppHandle) -> Result<(), String> {
    blocking(move || nextcloud::disconnect(&current_project(&app)?)).await
}

/// Uploads a packaged build to Nextcloud as a job. `path` is a build folder in the builds folder.
#[tauri::command]
async fn start_upload(app: AppHandle, path: PathBuf) -> Result<jobs::Snapshot, String> {
    blocking(move || {
        let p = current_project(&app)?;
        let build = package::builds(&p).into_iter().find(|b| settings::same_path(&b.path, &path)).ok_or("That build isn't in the project's builds folder.")?;
        let state_dir = uploads_state_dir(&app);
        let name = build.name.clone();
        jobs::start(&app, jobs::JobKind::Upload, "Uploading to Nextcloud".into(), name.clone(), vec!["Zip the build".into(), "Upload".into()], move |job| {
            let zip = match build.zip.clone() {
                Some(z) => {
                    job.skipped(0, "Already zipped");
                    z
                }
                None => {
                    job.running(0, "");
                    let z = package::zip_build(&job.log, &build.path, &name)?;
                    job.done(0, "");
                    z
                }
            };
            job.running(1, "");
            let up = nextcloud::upload(&job.log, &p, &zip, "", &state_dir)?;
            job.done(1, up.note());
            Ok(format!("Uploaded to Nextcloud: {} ({})", up.path, up.summary))
        })
    })
    .await
}

// ---------- Tools ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolsInfo {
    terminals: Vec<tools::App>,
    ides: Vec<tools::App>,
    terminal: Option<String>,
    ide: Option<String>,
    dependencies: Vec<deps::DepStatus>,
    changed_maps: usize,
    wwise_found: bool,
    wwise_unsupported: Option<&'static str>,
}

#[tauri::command]
async fn tools_info(app: AppHandle) -> Result<ToolsInfo, String> {
    blocking(move || {
        let p = current_project(&app)?;
        let prefs = settings::load(&app).prefs(&p.root);
        Ok(ToolsInfo {
            terminals: tools::terminals(),
            ides: tools::ides(),
            terminal: prefs.terminal,
            ide: prefs.ide,
            dependencies: deps::status(&p),
            changed_maps: lighting::statuses(&p).iter().filter(|m| m.kind == "changed" || m.kind == "never").count(),
            wwise_found: wwise::console(&p).is_some(),
            wwise_unsupported: wwise::UNSUPPORTED,
        })
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IdeChoices {
    ides: Vec<tools::App>,
    /// This project's choice; the first one found when not chosen yet.
    ide: Option<String>,
}

#[tauri::command]
async fn ide_choices(app: AppHandle) -> Result<IdeChoices, String> {
    blocking(move || {
        let p = current_project(&app)?;
        let ides = tools::ides();
        let chosen = settings::load(&app).prefs(&p.root).ide.filter(|id| ides.iter().any(|i| &i.id == id));
        Ok(IdeChoices { ide: chosen.or_else(|| ides.first().map(|i| i.id.clone())), ides })
    })
    .await
}

#[tauri::command]
async fn open_terminal(app: AppHandle, id: Option<String>) -> Result<(), String> {
    blocking(move || tools::open_terminal(id.as_deref(), &current_project(&app)?.root)).await
}

#[tauri::command]
async fn open_ide(app: AppHandle, id: Option<String>) -> Result<(), String> {
    blocking(move || tools::open_ide(id.as_deref(), &current_project(&app)?)).await
}

#[tauri::command]
async fn clean_build_files(app: AppHandle) -> Result<Vec<String>, String> {
    blocking(move || {
        let _busy = jobs::begin("Cleaning build files", false)?;
        tools::clean(&current_project(&app)?)
    })
    .await
}

/// The UI marks Update & Launch as one flow, so nothing else starts between its steps.
#[tauri::command]
fn set_flow(name: Option<String>) -> Result<(), String> {
    jobs::set_flow(name)
}

/// Starts one of the simple tool jobs: regenerate, repairGit, soundbanks, dependencies.
#[tauri::command]
async fn start_tool(app: AppHandle, tool: String, force: Option<bool>) -> Result<jobs::Snapshot, String> {
    blocking(move || {
        let p = current_project(&app)?;
        match tool.as_str() {
            "regenerate" => {
                let e = current_engine(&app, &p)?;
                jobs::start(&app, jobs::JobKind::Regenerate, "Regenerating project files".into(), String::new(), vec!["Generate project files".into()], move |job| {
                    job.running(0, "");
                    build::generate_project_files(&job.log, &p, &e)?;
                    job.done(0, "");
                    Ok("Project files are up to date. Reload the project in your IDE.".into())
                })
            }
            "repairGit" => {
                let handle = app.clone();
                jobs::start(&app, jobs::JobKind::RepairGit, "Repairing Git setup".into(), String::new(), vec!["Hooks, LFS and submodules".into()], move |job| {
                    job.running(0, "");
                    prepare(&handle, &job.log, &p)?;
                    job.done(0, "");
                    Ok("Git is set up.".into())
                })
            }
            "soundbanks" => jobs::start(&app, jobs::JobKind::Soundbanks, "Generating soundbanks".into(), "For Windows".into(), vec!["Generate Wwise soundbanks".into()], move |job| {
                job.running(0, "");
                wwise::generate(&job.log, &p)?;
                job.done(0, "");
                Ok("Soundbanks are generated.".into())
            }),
            "dependencies" => {
                let force = force.unwrap_or(false);
                jobs::start(&app, jobs::JobKind::Dependencies, "Downloading dependencies".into(), String::new(), vec!["Download and extract".into()], move |job| {
                    job.running(0, "");
                    let n = deps::download(&job.log, &p, force)?;
                    job.done(0, "");
                    Ok(if n == 0 { "Everything was up to date.".into() } else { format!("Downloaded {n} dependenc{}.", if n == 1 { "y" } else { "ies" }) })
                })
            }
            other => Err(format!("Unknown tool {other}.")),
        }
    })
    .await
}

// ---------- Jobs ----------

#[derive(Serialize)]
struct JobState {
    snapshot: jobs::Snapshot,
    lines: Vec<(String, String)>,
}

#[tauri::command]
fn job_state() -> Option<JobState> {
    jobs::snapshot().map(|(snapshot, lines)| JobState { snapshot, lines })
}

/// Cancels the running job, and stops an addon helper that's running a step. Async, so stopping
/// the programs runs off the main thread and never freezes the window.
#[tauri::command]
async fn cancel_job() -> Result<(), String> {
    blocking(|| {
        jobs::cancel();
        addon_host::cancel_steps();
        Ok(())
    })
    .await
}

#[tauri::command]
fn dismiss_job() {
    jobs::dismiss();
}

// ---------- Misc ----------

#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !["https://", "http://", "com.epicgames.launcher://"].iter().any(|p| lower.starts_with(p)) {
        return Err(format!("The hub only opens web addresses, not {url}."));
    }
    open_external(&app, &url)
}

/// Opens a web address or file with the system's default program. On Linux this goes through
/// `xdg-open` started like any other program, so it doesn't inherit the AppImage's environment;
/// the opener plugin would start it with the hub's own.
fn open_external(app: &AppHandle, target: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if proc::which("xdg-open").is_some() {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(target);
        return proc::spawn_detached(cmd);
    }
    if target.contains("://") {
        app.opener().open_url(target, None::<&str>).map_err(|e| e.to_string())
    } else {
        app.opener().open_path(target, None::<&str>).map_err(|e| e.to_string())
    }
}

/// Paths the UI may open or reveal: inside the current project or its Git repository, or the
/// hub's own folders.
fn allowed_path(app: &AppHandle, path: &Path) -> Result<PathBuf, String> {
    let mut roots = vec![];
    if let Ok(p) = current_project(app) {
        roots.push(p.root.clone());
        roots.extend(git_top_level(&p));
    }
    if let Ok(d) = app.path().app_config_dir() {
        roots.push(d);
    }
    inside_any(path, &roots).ok_or_else(|| "The hub only opens files of the current project.".into())
}

/// The top folder of the project's Git repository, which may be above the .uproject folder.
/// Asked once per project.
fn git_top_level(project: &project::Project) -> Option<PathBuf> {
    static CACHE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<PathBuf, Option<PathBuf>>>> = std::sync::LazyLock::new(Default::default);
    if let Some(top) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(&project.root) {
        return top.clone();
    }
    let top = project.git(&["rev-parse", "--show-toplevel"]).and_then(|t| repo_root_for(&project.root, Path::new(t.trim())));
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).insert(project.root.clone(), top.clone());
    top
}

/// `top` if it's the project folder or a folder above it, and not a whole drive. A repository's
/// settings can point Git somewhere else entirely; that doesn't make that place the project's.
fn repo_root_for(project_root: &Path, top: &Path) -> Option<PathBuf> {
    let (root, top) = (project_root.canonicalize().ok()?, top.canonicalize().ok()?);
    (top.parent().is_some() && path_starts_with(&root, &top)).then_some(top)
}

/// `path` with links resolved, if it's in one of `roots` (also with links resolved).
pub(crate) fn inside_any(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let path = path.canonicalize().ok()?;
    let inside = roots.iter().filter_map(|r| r.canonicalize().ok()).any(|r| path_starts_with(&path, &r));
    inside.then(|| without_verbatim_prefix(path))
}

/// C:\x instead of \\?\C:\x, which canonicalize gives on Windows and Explorer doesn't take.
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

/// Compares whole folder names, so C:\Repos\Game doesn't contain C:\Repos\GameTools. Windows
/// ignores case in paths; Linux doesn't.
fn path_starts_with(path: &Path, root: &Path) -> bool {
    if cfg!(windows) {
        let lower = |p: &Path| -> PathBuf { p.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect() };
        lower(path).starts_with(lower(root))
    } else {
        path.starts_with(root)
    }
}

/// Files that run code when opened: programs, scripts and shortcuts to them.
fn runs_code(path: &Path) -> bool {
    const RUNNABLE: &[&str] = &[
        "exe", "bat", "cmd", "com", "lnk", "ps1", "psm1", "sh", "bash", "msi", "msix", "appx", "scr", "pif", "cpl", "vbs", "vbe", "js", "jse", "wsf", "wsh",
        "hta", "reg", "url", "jar", "py", "pyw", "appimage", "desktop", "run", "bin",
    ];
    if path.extension().is_some_and(|e| RUNNABLE.contains(&e.to_string_lossy().to_lowercase().as_str())) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) {
            return true;
        }
    }
    false
}

#[tauri::command]
fn reveal_path(app: AppHandle, path: PathBuf) -> Result<(), String> {
    let path = allowed_path(&app, &path)?;
    app.opener().reveal_item_in_dir(path).map_err(|e| e.to_string())
}

/// Opens a file or folder with its usual app. Programs and scripts are only shown in the file
/// manager, so nothing from a repository runs by clicking a link in the hub.
#[tauri::command]
fn open_path(app: AppHandle, path: PathBuf) -> Result<(), String> {
    let path = allowed_path(&app, &path)?;
    if runs_code(&path) {
        return app.opener().reveal_item_in_dir(path).map_err(|e| e.to_string());
    }
    open_external(&app, &path.to_string_lossy())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HubInfo {
    version: String,
    installed: bool,
    /// Updates come from the package manager (a Linux .deb install).
    package_managed: bool,
    offer_install: bool,
    /// "windows" or "linux", for text that differs between them.
    platform: &'static str,
    /// The addon API version this hub implements.
    addon_api: u32,
}

#[tauri::command]
fn hub_info(app: AppHandle) -> HubInfo {
    HubInfo {
        version: app.package_info().version.to_string(),
        installed: selfupdate::state().installed,
        package_managed: selfupdate::state().package_managed,
        offer_install: selfupdate::should_offer_install(&app),
        platform: if cfg!(windows) { "windows" } else { "linux" },
        addon_api: ue_dev_hub_addon::protocol::API_VERSION,
    }
}

#[tauri::command]
async fn never_offer_install(app: AppHandle) -> Result<(), String> {
    blocking(move || settings::update(&app, |s| s.never_offer_install = true)).await
}

/// Installs the hub and reopens it, installed, on the current project.
#[tauri::command]
async fn install_hub(app: AppHandle) -> Result<(), String> {
    if let Some(what) = jobs::busy_with() {
        return Err(format!("{what} is still running. Install when it's done."));
    }
    blocking(move || {
        let project = settings::load(&app).selected;
        selfupdate::install(&app, project.as_deref())
    })
    .await
}

/// Updates a hub that isn't installed. Returns the new version, or null when it's current.
#[tauri::command]
async fn update_portable(app: AppHandle) -> Result<Option<String>, String> {
    blocking(move || selfupdate::update_portable(&app)).await
}

/// The newest hub release if it's newer than this one, for package-managed installs.
#[tauri::command]
async fn newer_hub_release(app: AppHandle) -> Result<Option<String>, String> {
    blocking(move || selfupdate::newer_release(&app)).await
}

/// `ue-dev-hub --project <folder>` or `ue-dev-hub <folder>`, as Launch.sh calls it.
fn project_arg(args: &[String], cwd: &Path) -> Option<PathBuf> {
    let mut it = args.iter().skip(1);
    let mut found = None;
    while let Some(a) = it.next() {
        if a == "--project" {
            found = it.next().cloned();
        } else if !a.starts_with('-') && found.is_none() {
            found = Some(a.clone());
        }
    }
    let path = PathBuf::from(found?.trim_end_matches(['\\', '"']).to_string());
    let path = if path.is_absolute() { path } else { cwd.join(path) };
    // Paths like "C:\\Repo\\." lose their "." parts when the components are collected.
    let path: PathBuf = path.components().collect();
    project::find_root(&path)
}

fn open_from_args(app: &AppHandle, args: &[String], cwd: &Path) {
    if let Some(root) = project_arg(args, cwd)
        && jobs::busy_with().is_none()
    {
        let _ = settings::update(app, |s| s.open(root));
        let _ = app.emit("project-changed", ());
    }
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// How long quitting waits for an addon's step before it says so and lets a second Quit stop it.
const QUIT_PATIENCE: std::time::Duration = std::time::Duration::from_secs(60);

/// When Quit was first chosen, while the hub waits for running work to stop.
static QUIT_STARTED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
/// Set once the person was told that quitting again stops the addon step it waits for.
static QUIT_FORCE_OFFERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

/// Quits once the running work has stopped and cleaned up. A rebase is never killed halfway;
/// a job is cancelled, which still puts files back. An addon's step, like a push or a commit, is
/// waited for too. If one takes long, the person is told, and quitting again stops it.
fn quit_when_safe(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    let Some(what) = jobs::busy_with() else {
        app.exit(0);
        return;
    };
    let mut started = QUIT_STARTED.lock().unwrap_or_else(|e| e.into_inner());
    if started.is_some() {
        if QUIT_FORCE_OFFERED.load(Ordering::SeqCst) && addon_host::step_running() {
            notify(app, "Stopping", &format!("Stopping \"{what}\" now. UE Dev Hub closes once it has stopped."));
            std::thread::spawn(addon_host::cancel_steps);
        } else {
            notify(app, "Closing soon", &format!("UE Dev Hub closes as soon as \"{what}\" has stopped safely."));
        }
        return;
    }
    let since = std::time::Instant::now();
    *started = Some(since);
    drop(started);
    jobs::begin_quit();
    notify(app, "Closing soon", &format!("UE Dev Hub closes as soon as \"{what}\" has stopped safely."));
    let app = app.clone();
    std::thread::spawn(move || {
        while let Some(what) = jobs::busy_with() {
            if !QUIT_FORCE_OFFERED.load(Ordering::SeqCst) && since.elapsed() > QUIT_PATIENCE && addon_host::step_running() {
                QUIT_FORCE_OFFERED.store(true, Ordering::SeqCst);
                notify(&app, "Still waiting", &format!("\"{what}\" hasn't finished yet. Quit again to stop it now."));
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        app.exit(0);
    });
}

/// Whether the tray icon is there to bring the window back.
static TRAY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The tray library Tauri loads on Linux. Without it, building the tray icon panics inside
/// `setup`. Release builds unwind on a panic now, but nothing between Tauri's event loop and
/// `setup` recovers from one, so the hub would still end before its window opens (the panic
/// hook only gets to write crash.log). Checking first is what lets it start without a tray.
#[cfg(target_os = "linux")]
fn tray_library_available() -> bool {
    // The names libappindicator-sys tries, in its order.
    ["libayatana-appindicator3.so.1", "libappindicator3.so.1", "libayatana-appindicator3.so", "libappindicator3.so"]
        .iter()
        // SAFETY: these libraries only register GObject types when loaded; Tauri loads the same one.
        .any(|name| unsafe { libloading::Library::new(name) }.is_ok())
}

#[cfg(not(target_os = "linux"))]
fn tray_library_available() -> bool {
    true
}

/// Adds the tray icon if this system can show one. The hub works without it.
fn try_setup_tray(app: &AppHandle) {
    if !tray_library_available() {
        crate::log::write_file("info", "No tray icon: libayatana-appindicator3 isn't installed.");
        return;
    }
    match setup_tray(app) {
        Ok(()) => TRAY.store(true, std::sync::atomic::Ordering::Relaxed),
        Err(e) => crate::log::write_file("info", &format!("No tray icon: {e}")),
    }
}

/// On Linux, Tauri draws with WebKitGTK, whose DMA-BUF renderer shows a blank window with NVIDIA
/// drivers and on some Wayland setups. Turned off unless the person set it themselves; programs the
/// hub starts don't inherit it.
fn prepare_environment() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: runs first thing in `run`, before Tauri or anything else starts a thread.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
        proc::set_own_vars(vec!["WEBKIT_DISABLE_DMABUF_RENDERER"]);
    }
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
    let show = MenuItem::with_id(app, "show", "Show UE Dev Hub", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("UE Dev Hub")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "show" => show_window(app),
            "quit" => quit_when_safe(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, .. } = e {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before anything can start a thread: it sets environment variables.
    prepare_environment();
    crate::log::install_panic_hook();
    // After a self-update the new hub waits for the old one to exit before anything else,
    // so the single-instance check below can't hand this start to the hub that is closing.
    let start_args = selfupdate::wait_for_previous(std::env::args().collect());
    tauri::Builder::default()
        // Must be first: a second launch (Launch.sh in another repo) hands its project to this one.
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            open_from_args(app, &selfupdate::split_wait_pid(args).0, Path::new(&cwd));
            show_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .register_asynchronous_uri_scheme_protocol(addon_protocol::SCHEME, |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            std::thread::spawn(move || responder.respond(addon_protocol::handle(&app, &request)));
        })
        .setup(move |app| {
            if let Ok(dir) = app.path().app_log_dir() {
                crate::log::init_file_log(&dir);
            }
            crate::log::write_file("info", &format!("UE Dev Hub {} started on {}", app.package_info().version, std::env::consts::OS));
            selfupdate::cleanup_after_update();
            let _ = settings::update(app.handle(), addons::migrate);
            let args = start_args.clone();
            let cwd = std::env::current_dir().unwrap_or_default();
            open_from_args(app.handle(), &args, &cwd);
            try_setup_tray(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window while anything runs keeps it going, so a rebase or bake is never
            // cut off halfway. It goes to the tray where there is one. Linux desktops often have
            // no tray area (GNOME) or don't pass clicks on to it, so there it's only minimized.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && jobs::busy_with().is_some()
            {
                api.prevent_close();
                let to_tray = !cfg!(target_os = "linux") && TRAY.load(std::sync::atomic::Ordering::Relaxed);
                let body = if to_tray {
                    let _ = window.hide();
                    "UE Dev Hub keeps going in the background. Click its tray icon to open it again."
                } else {
                    let _ = window.minimize();
                    "UE Dev Hub keeps going until it's done, so it was minimized instead of closed."
                };
                use tauri_plugin_notification::NotificationExt;
                let _ = window.app_handle().notification().builder().title("Still working").body(body).show();
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_projects,
            get_project,
            add_project,
            select_project,
            remove_project,
            get_prefs,
            set_widgets,
            set_app_choice,
            get_auto_launch,
            set_auto_launch,
            get_after_launch,
            set_after_launch,
            editor_started,
            run_checks,
            set_engine_path,
            create_config,
            open_config,
            home_status,
            avatars,
            prepare_repo,
            set_git_hooks,
            check_updates,
            apply_update,
            addons_list,
            addons_available,
            addon_set_enabled,
            addon_install,
            addon_install_from_disk,
            addon_uninstall,
            addon_default_offers,
            addon_install_defaults,
            addon_call,
            build_project,
            launch_editor,
            editor_running,
            package_info,
            list_builds,
            run_build,
            start_package,
            nextcloud_status,
            nextcloud_connect,
            nextcloud_cancel_connect,
            nextcloud_disconnect,
            start_upload,
            start_bake,
            test_map_info,
            start_test_map,
            play_test_run,
            rename_test_run,
            delete_test_run,
            delete_all_test_runs,
            map_lighting,
            report_context,
            send_report,
            report_pick_media,
            report_markdown,
            tools_info,
            open_terminal,
            ide_choices,
            open_ide,
            clean_build_files,
            set_flow,
            start_tool,
            job_state,
            cancel_job,
            dismiss_job,
            open_url,
            reveal_path,
            open_path,
            hub_info,
            never_offer_install,
            install_hub,
            update_portable,
            newer_hub_release,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_, event| {
            if let tauri::RunEvent::Exit = event {
                addon_host::stop_all();
            }
        });
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn allows_only_paths_inside_a_root() {
        let base = std::env::temp_dir().join(format!("ue-dev-hub-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let game = base.join("Game");
        std::fs::create_dir_all(game.join("Saved")).unwrap();
        std::fs::create_dir_all(base.join("GameTools")).unwrap();
        std::fs::write(game.join("Saved/a.log"), "x").unwrap();
        std::fs::write(base.join("GameTools/x.exe"), "x").unwrap();
        let roots = [game.clone()];

        assert!(inside_any(&game.join("Saved/a.log"), &roots).is_some());
        assert!(inside_any(&game, &roots).is_some());
        // A sibling folder whose name starts the same isn't inside.
        assert!(inside_any(&base.join("GameTools/x.exe"), &roots).is_none());
        assert!(inside_any(&game.join("../GameTools/x.exe"), &roots).is_none());
        assert!(inside_any(&game.join("missing.txt"), &roots).is_none());
        assert!(!inside_any(&game, &roots).unwrap().to_string_lossy().starts_with(r"\\?\"));
        if cfg!(windows) {
            let upper = PathBuf::from(game.to_string_lossy().to_uppercase()).join("Saved").join("A.LOG");
            assert!(inside_any(&upper, &roots).is_some());
        }
        #[cfg(unix)]
        {
            // A link inside the project that points outside it doesn't count as inside.
            std::os::unix::fs::symlink(base.join("GameTools"), game.join("link")).unwrap();
            assert!(inside_any(&game.join("link/x.exe"), &roots).is_none());
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn takes_only_a_repo_root_above_the_project() {
        let base = std::env::temp_dir().join(format!("ue-dev-hub-toplevel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let game = base.join("Repo").join("Game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(base.join("Other")).unwrap();

        assert!(repo_root_for(&game, &base.join("Repo")).is_some());
        assert!(repo_root_for(&game, &game).is_some());
        assert!(repo_root_for(&game, &base.join("Other")).is_none());
        assert!(repo_root_for(&game, &game.join("Missing")).is_none());
        let drive = game.ancestors().last().unwrap();
        assert!(repo_root_for(&game, drive).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn knows_files_that_run_code() {
        for f in ["Game.exe", "a.BAT", "x.cmd", "x.lnk", "x.ps1", "Start.sh", "x.js", "x.AppImage"] {
            assert!(runs_code(Path::new(f)), "{f}");
        }
        for f in ["Game.log", "notes.txt", "shot.png", "ue-dev-hub.toml"] {
            assert!(!runs_code(Path::new(f)), "{f}");
        }
    }
}
