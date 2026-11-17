//! Finding maps: the ones the build cooks, and every map for "Test a map".

use crate::project::Project;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MapInfo {
    pub name: String,
    /// Long package name, like /Game/Maps/Menu/MainMenu.
    pub package: String,
}

/// A long package name like /Game/Maps/Menu or /MyPlugin/Arena, or a bare map name like Menu.
/// Map names go on Unreal's command line, which Unreal splits again by itself, so anything else
/// (spaces, quotes, dots) could add switches of its own.
pub fn is_map_name(name: &str) -> bool {
    let part = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    match name.strip_prefix('/') {
        Some(rest) => rest.split('/').count() >= 2 && rest.split('/').all(part),
        None => part(name),
    }
}

/// Fails for a map name that isn't one, see `is_map_name`.
pub fn check_map_name(name: &str) -> Result<(), String> {
    if is_map_name(name) { Ok(()) } else { Err(format!("'{name}' isn't a map name like /Game/Maps/MyMap.")) }
}

/// Long package names of the maps in the build, from `package.maps` or the MapsToCook list.
pub fn maps_to_cook(project: &Project) -> Vec<String> {
    let c = &project.config.package;
    if !c.maps.is_empty() {
        return c.maps.clone();
    }
    let ini = std::fs::read_to_string(project.root.join(&c.maps_from)).unwrap_or_default();
    ini.lines()
        .filter(|l| l.trim_start().starts_with("+MapsToCook="))
        .filter_map(|l| {
            let start = l.find("FilePath=\"")? + "FilePath=\"".len();
            let end = l[start..].find('"')? + start;
            Some(l[start..end].to_string())
        })
        .filter(|m| is_map_name(m))
        .collect()
}

pub fn short_name(package: &str) -> String {
    package.rsplit('/').next().unwrap_or(package).to_string()
}

/// The .umap of a map, relative to the project, with forward slashes. Accepts a long package
/// name or a bare map name.
pub fn map_file(project: &Project, map: &str) -> Option<String> {
    if let Some(rest) = map.strip_prefix("/Game/") {
        return Some(format!("Content/{rest}.umap"));
    }
    if map.starts_with('/') {
        // A plugin map: /Plugin/Path/Map -> Plugins/**/Plugin/Content/Path/Map.umap
        let (plugin, rest) = map.trim_start_matches('/').split_once('/')?;
        
        
        return all_umaps(project)
            .into_iter()
            .find(|f| f.ends_with(&format!("{plugin}/Content/{rest}.umap")));
    }
    let wanted = format!("{map}.umap");
    all_umaps(project).into_iter().find(|f| f.starts_with("Content/") && f.rsplit('/').next() == Some(wanted.as_str()))
}

const SKIP_DIRS: &[&str] = &["Saved", "Intermediate", "Binaries", "DerivedDataCache", "Builds", "node_modules"];

/// Every .umap in the project, relative paths with forward slashes.
pub fn all_umaps(project: &Project) -> Vec<String> {
    let mut out = Vec::new();
    walk(&project.root, &project.root, &mut out);
    out.sort();
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(root, &path, out);
        } else if name.to_ascii_lowercase().ends_with(".umap")
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Content/Foo/Bar.umap -> /Game/Foo/Bar; Plugins/.../MyPlugin/Content/Bar.umap -> /MyPlugin/Bar.
pub fn package_name(file: &str) -> Option<String> {
    let no_ext = file.strip_suffix(".umap")?;
    if let Some(rest) = no_ext.strip_prefix("Content/") {
        return Some(format!("/Game/{rest}"));
    }
    let (plugin_root, rest) = no_ext.split_once("/Content/")?;
    let plugin = plugin_root.rsplit('/').next()?;
    Some(format!("/{plugin}/{rest}"))
}

pub fn list_all(project: &Project) -> Vec<MapInfo> {
    all_umaps(project)
        .iter()
        .filter_map(|f| package_name(f))
        .map(|p| MapInfo { name: short_name(&p), package: p })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_plain_map_names() {
        for good in ["/Game/Maps/Menu", "/MyPlugin/Test/Arena_P", "/Game/A-B", "Menu"] {
            assert!(is_map_name(good), "{good}");
        }
        for bad in ["", "/", "/Game", "/Game/", "/Game//X", "/Game/../X", "/Game/Map -execcmds=quit", "/Game/Map\"", "/Game/A.B", "Game/Map", "-run=x", "/Game/Map+/Game/Other"] {
            assert!(!is_map_name(bad), "{bad}");
        }
    }

    #[test]
    fn converts_files_to_package_names() {
        assert_eq!(package_name("Content/Maps/Menu.umap").as_deref(), Some("/Game/Maps/Menu"));
        assert_eq!(package_name("Plugins/Gameplay/MyPlugin/Content/Test/Arena.umap").as_deref(), Some("/MyPlugin/Test/Arena"));
        assert_eq!(package_name("Saved/Other.umap"), None);
    }
}
