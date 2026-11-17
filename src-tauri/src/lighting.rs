//! Baking static lighting with the ResavePackages commandlet.
//!
//! Whether a map needs a bake is a guess: after each bake, the hashes of the persistent .umap and
//! every .umap it references are stored in `Saved/BuildLighting/<Map>.stamp`. Changes to
//! meshes, materials or lighting settings outside those files aren't detected.

use crate::engine::Engine;
use crate::log::Log;
use crate::maps;
use crate::proc;
use crate::project::Project;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MapStatus {
    pub name: String,
    pub package: String,
    /// unchanged, changed, never or missing
    pub kind: String,
    pub status: String,
    pub changed_files: Vec<String>,
}

/// The persistent level's .umap and every other .umap it references, read from its name table.
pub fn level_files(project: &Project, persistent: &str) -> Vec<String> {
    let mut files = vec![persistent.to_string()];
    let Ok(bytes) = std::fs::read(project.root.join(persistent)) else { return files };
    let mut found = BTreeSet::new();
    let needle = b"/Game/";
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let mut j = i + needle.len();
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'/') {
                j += 1;
            }
            found.insert(String::from_utf8_lossy(&bytes[i..j]).to_string());
            i = j;
        } else {
            i += 1;
        }
    }
    for package in found {
        let file = format!("Content/{}.umap", &package["/Game/".len()..]);
        if file != persistent && project.root.join(&file).is_file() {
            files.push(file);
        }
    }
    files
}

fn sha1_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(sha1_smol::Sha1::from(bytes).digest().to_string())
}

/// "<sha1>  <file>" per level file, like `sha1sum` prints it.
pub fn fingerprint(project: &Project, persistent: &str) -> Vec<String> {
    level_files(project, persistent)
        .iter()
        .filter_map(|f| sha1_file(&project.root.join(f)).map(|h| format!("{h}  {f}")))
        .collect()
}

fn stamp_file(project: &Project, map_file: &str) -> PathBuf {
    let stem = Path::new(map_file).file_stem().unwrap_or_default().to_string_lossy().to_string();
    project.root.join("Saved/BuildLighting").join(format!("{stem}.stamp"))
}

pub fn status(project: &Project, package: &str) -> MapStatus {
    let name = maps::short_name(package);
    let mk = |kind: &str, status: String, changed_files: Vec<String>| MapStatus {
        name: name.clone(),
        package: package.to_string(),
        kind: kind.into(),
        status,
        changed_files,
    };
    let Some(file) = maps::map_file(project, package).filter(|f| project.root.join(f).is_file()) else {
        return mk("missing", "Map file not found".into(), vec![]);
    };
    let stamp = stamp_file(project, &file);
    let Ok(recorded) = std::fs::read_to_string(&stamp) else {
        return mk("never", "Never baked here".into(), vec![]);
    };
    let dir = project.root.join(&file).parent().map(Path::to_path_buf).unwrap_or_default();
    let has_built_data = std::fs::read_dir(&dir)
        .map(|d| d.flatten().any(|e| e.file_name().to_string_lossy().ends_with("_BuiltData.uasset")))
        .unwrap_or(false);
    if !has_built_data {
        return mk("never", "No built data on disk".into(), vec![]);
    }

    let recorded: BTreeSet<&str> = recorded.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
    let current = fingerprint(project, &file);
    let current_set: BTreeSet<&str> = current.iter().map(String::as_str).collect();
    let mut changed: BTreeSet<String> = BTreeSet::new();
    for line in recorded.symmetric_difference(&current_set) {
        if let Some((_, f)) = line.split_once("  ") {
            changed.insert(Path::new(f).file_name().unwrap_or_default().to_string_lossy().to_string());
        }
    }
    if changed.is_empty() {
        mk("unchanged", "Unchanged".into(), vec![])
    } else {
        let n = changed.len();
        mk("changed", format!("{n} file{} changed", if n == 1 { "" } else { "s" }), changed.into_iter().collect())
    }
}

pub fn statuses(project: &Project) -> Vec<MapStatus> {
    maps::maps_to_cook(project).iter().map(|m| status(project, m)).collect()
}

fn set_writable(path: &Path, writable: bool) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        // `set_readonly(false)` would make the file writable for everyone on Unix; only the owner
        // gets write access back, like Git LFS gives it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = perms.mode();
            perms.set_mode(if writable { mode | 0o200 } else { mode & !0o222 });
        }
        #[cfg(not(unix))]
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(!writable);
        let _ = std::fs::set_permissions(path, perms);
    }
}

/// Files locked by someone else. The server tells which locks are ours (`--verify`), since lock
/// owners are server accounts that don't have to match the Git user name.
fn locked_by_others(log: &Log, project: &Project) -> BTreeSet<String> {
    let out = match proc::run_timeout("git", &["lfs", "locks", "--verify", "--json"], Some(&project.root), 60) {
        Ok(o) if o.ok => o.stdout,
        _ => {
            log.info("Couldn't read the LFS locks, so maps locked by others aren't flagged.");
            return BTreeSet::new();
        }
    };
    theirs_from_json(&out).unwrap_or_else(|| {
        log.info("Couldn't read the LFS locks, so maps locked by others aren't flagged.");
        BTreeSet::new()
    })
}

/// The paths in "theirs" of `git lfs locks --verify --json`.
fn theirs_from_json(out: &str) -> Option<BTreeSet<String>> {
    let v: serde_json::Value = serde_json::from_str(out).ok()?;
    let theirs = v.get("theirs")?.as_array()?;
    Some(theirs.iter().filter_map(|l| l.get("path")?.as_str().map(str::to_string)).collect())
}

/// Bakes `packages` in one commandlet run and records their fingerprints. The caller compiles
/// the editor first, since the commandlet loads and saves maps with the editor binaries.
pub fn bake(log: &Log, project: &Project, engine: &Engine, packages: &[String], quality: &str) -> Result<(), String> {
    if packages.is_empty() {
        return Ok(());
    }
    for p in packages {
        maps::check_map_name(p)?;
    }
    if proc::editor_running_for(&project.uproject) {
        return Err("Unreal Editor is open. Close it so the bake doesn't fight over the same files.".into());
    }

    let mut level_files = Vec::new();
    let mut map_names = Vec::new();
    for p in packages {
        let file = maps::map_file(project, p).filter(|f| project.root.join(f).is_file()).ok_or(format!("Map not found: {p}"))?;
        level_files.extend(level_files_dedup(project, &file, &level_files));
        map_names.push(maps::short_name(p));
    }

    // Lockable files are read-only until locked, and the commandlet skips read-only maps. Make the
    // level files writable without taking locks: the baked maps are only for this build.
    let others_locked = locked_by_others(log, project);
    let mut made_writable = Vec::new();
    for f in &level_files {
        let path = project.root.join(f);
        if std::fs::metadata(&path).map(|m| m.permissions().readonly()).unwrap_or(false) {
            set_writable(&path, true);
            made_writable.push(f.clone());
        }
    }

    // The commandlet can't overwrite existing built data, so move it aside and let the bake
    // create it from scratch.
    let backup = backup_dir(project);
    // A backup left by a bake that was interrupted (app closed, PC crashed) holds the last good
    // built data. Put it back before anything else, so it's never deleted.
    if backup.exists() {
        log.info("Restoring lighting data that an interrupted bake left behind ...");
        if !restore_backup(&project.root, &backup, true) {
            return Err(format!(
                "Lighting data from an interrupted bake couldn't be put back. It's safe in {}. Close Unreal Editor and anything else that may hold those files, then bake again: the hub puts it back first.",
                backup.display()
            ));
        }
    }
    let mut moved = Vec::new();
    for f in &level_files {
        let built = f.trim_end_matches(".umap").to_string() + "_BuiltData.uasset";
        let src = project.root.join(&built);
        if src.is_file() {
            let dst = backup.join(&built);
            let _ = std::fs::create_dir_all(dst.parent().unwrap());
            if std::fs::rename(&src, &dst).is_ok() {
                moved.push(built);
            }
        }
    }

    let before: Vec<(String, Option<String>)> = level_files.iter().map(|f| (f.clone(), sha1_file(&project.root.join(f)))).collect();
    let log_name = format!("BuildLighting_{}.log", chrono::Local::now().format("%Y%m%d_%H%M%S"));
    let log_file = project.logs_dir().join(&log_name);

    log.info(format!("Baking lighting ({quality}) for {} ...", map_names.join(", ")));
    let mut cmd = proc::command(engine.editor_cmd());
    cmd.current_dir(&project.root).arg(&project.uproject).args([
        "-run=ResavePackages".to_string(),
        "-buildlighting".into(),
        "-allowcommandletrendering".into(),
        format!("-quality={quality}"),
        // Full package names: a short name like Arena_P is ambiguous when two folders have a
        // map with that name, and the commandlet then picks one itself.
        format!("-map={}", packages.join("+")),
        // Keeps the commandlet from taking LFS locks through the Git plugin.
        "-nosourcecontrol".into(),
        "-unattended".into(),
        "-nopause".into(),
        "-nosplash".into(),
        format!("-log={log_name}"),
    ]);
    let run = proc::stream(log, cmd, None, |line| {
        if let Some(pos) = line.find("Lightmass progress") {
            let pct = line[pos..].split_whitespace().find_map(|w| w.trim_end_matches('%').parse::<f32>().ok());
            log.progress(crate::log::Progress { percent: pct, label: line[pos..].to_string() });
        }
        true
    });

    // Put the read-only flag back on files the bake didn't change.
    for f in &made_writable {
        let unchanged = before.iter().any(|(bf, h)| bf == f && *h == sha1_file(&project.root.join(f)));
        if unchanged {
            set_writable(&project.root.join(f), false);
        }
    }
    let ok = matches!(run, Ok((true, _)));
    let log_text = std::fs::read_to_string(&log_file).unwrap_or_default();
    let failures: Vec<&str> = log_text
        .lines()
        .filter(|l| {
            ["Failed building lighting", "Failed to complete steps", "Skipping read-only", "Failed to save", "Failed to check out"]
                .iter()
                .any(|n| l.contains(n))
        })
        .collect();

    let others_changed: Vec<&String> = before
        .iter()
        .filter(|(f, h)| others_locked.contains(f) && *h != sha1_file(&project.root.join(f)))
        .map(|(f, _)| f)
        .collect();
    if !others_changed.is_empty() {
        log.info("The bake saved these maps although someone else has them locked. Don't commit them:");
        for f in &others_changed {
            log.info(format!("  {f}"));
        }
    }

    if !ok || !failures.is_empty() {
        // The bake may have left partial built data: put every original back over it.
        let restored = moved.is_empty() || restore_backup(&project.root, &backup, true);
        for f in failures.iter().take(20) {
            log.info(*f);
        }
        if !restored {
            return Err(format!(
                "The lighting bake failed, and the previous lighting data couldn't all be put back. It's safe in {}. Close Unreal Editor and anything else that may hold those files, then bake again: the hub puts it back first.",
                backup.display()
            ));
        }
        run?;
        return Err(format!("The lighting bake failed, so the previous lighting data was put back. The log is in {}.", log_file.display()));
    }
    // Success: keep the new built data, and restore the files the bake didn't recreate, such as
    // for levels without static lighting.
    restore_backup(&project.root, &backup, false);

    let dir = project.root.join("Saved/BuildLighting");
    let _ = std::fs::create_dir_all(&dir);
    for p in packages {
        if let Some(file) = maps::map_file(project, p) {
            let mut text = fingerprint(project, &file).join("\n");
            text.push('\n');
            let _ = std::fs::write(stamp_file(project, &file), text);
        }
    }
    log.info("Lighting bake finished.");
    Ok(())
}

fn level_files_dedup(project: &Project, persistent: &str, existing: &[String]) -> Vec<String> {
    level_files(project, persistent).into_iter().filter(|f| !existing.contains(f)).collect()
}

fn backup_dir(project: &Project) -> PathBuf {
    project.root.join("Saved/BuildLighting/BuiltDataBackup")
}

/// Moves the files in `backup` back to the same relative paths under `root`. With `overwrite`,
/// they replace whatever is there; otherwise only missing files are put back and the rest is
/// dropped. Deletes the backup only when nothing is left in it. Returns whether that worked.
fn restore_backup(root: &Path, backup: &Path, overwrite: bool) -> bool {
    fn files(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() { files(&p, out) } else { out.push(p) }
            }
        }
    }
    let mut all = vec![];
    files(backup, &mut all);
    let mut ok = true;
    for f in all {
        let Ok(rel) = f.strip_prefix(backup) else { continue };
        let dst = root.join(rel);
        if dst.exists() {
            if !overwrite {
                let _ = std::fs::remove_file(&f);
                continue;
            }
            set_writable(&dst, true);
            if std::fs::remove_file(&dst).is_err() {
                ok = false;
                continue;
            }
        }
        if let Some(dir) = dst.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::rename(&f, &dst).is_err() {
            ok = false;
        }
    }
    if ok {
        let _ = std::fs::remove_dir_all(backup);
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_whose_lfs_locks_are_whose() {
        let out = r#"{"ours":[{"id":"1","path":"Content/Mine.umap","owner":{"name":"me"}}],"theirs":[{"id":"2","path":"Content/Maps/Level_P.umap","owner":{"name":"someone"}}]}"#;
        assert_eq!(theirs_from_json(out).unwrap().into_iter().collect::<Vec<_>>(), vec!["Content/Maps/Level_P.umap"]);
        assert!(theirs_from_json("not json").is_none());
    }

    #[test]
    fn restores_built_data_over_partial_files() {
        let root = std::env::temp_dir().join(format!("ue-dev-hub-light-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let backup = root.join("Saved/BuildLighting/BuiltDataBackup");
        let rel = "Content/Maps/A_BuiltData.uasset";
        std::fs::create_dir_all(backup.join("Content/Maps")).unwrap();
        std::fs::create_dir_all(root.join("Content/Maps")).unwrap();
        std::fs::write(backup.join(rel), b"good").unwrap();
        std::fs::write(backup.join("Content/Maps/B_BuiltData.uasset"), b"good b").unwrap();
        std::fs::write(root.join(rel), b"partial").unwrap();

        // After a success, new files stay and only missing ones come back.
        assert!(restore_backup(&root, &backup, false));
        assert_eq!(std::fs::read(root.join(rel)).unwrap(), b"partial");
        assert_eq!(std::fs::read(root.join("Content/Maps/B_BuiltData.uasset")).unwrap(), b"good b");
        assert!(!backup.exists());

        // After a failure, the originals replace what the bake wrote.
        std::fs::create_dir_all(backup.join("Content/Maps")).unwrap();
        std::fs::write(backup.join(rel), b"good").unwrap();
        assert!(restore_backup(&root, &backup, true));
        assert_eq!(std::fs::read(root.join(rel)).unwrap(), b"good");
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
