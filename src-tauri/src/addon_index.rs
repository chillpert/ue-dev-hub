//! The official addons: a signed list (`index.json`) published by the addons repository, and
//! installing or updating addons from it.
//!
//! Each download is checked against its sha256 in the index and a signature made with the addons
//! key, whose public half is built into the hub. It's a different key from the hub's
//! update key, so a leaked addons key can't be used to push a hub update.

use crate::addons::{self, Manifest, Source};
use crate::project::Project;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Where the official list is. The addons repository's rolling "index" release.
const INDEX_URL: &str = "https://github.com/chillpert/ue-dev-hub-addons/releases/download/index/index.json";

/// The public half of the addons signing key (base64 of a minisign public key file).
pub const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IERFQUVDRUU2Qzk5QzkyRUUKUldUdWtweko1czZ1M2pCT2xIQkFyZU9tRlU1SkJNY1lGcHJKSlpZbDhpUklqRTdoc1dlUzIzaU0K";

/// For trying a list from a test server or a local folder: a URL or a path to an index.json.
const INDEX_ENV: &str = "UE_DEV_HUB_ADDON_INDEX";

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub addons: Vec<Entry>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub version: String,
    pub api: u32,
    pub min_hub: Option<String>,
    /// By platform, like "windows-x86_64".
    pub platforms: HashMap<String, Download>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    /// Absolute, or relative to the index.
    pub url: String,
    pub sha256: String,
    /// The `.sig` the Tauri signer writes for the zip.
    pub signature: String,
}

fn index_location() -> String {
    std::env::var(INDEX_ENV).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| INDEX_URL.to_string())
}

fn is_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

/// `url` relative to the index at `base`, unless it's absolute.
fn join(base: &str, url: &str) -> String {
    if is_url(url) || !is_url(base) && std::path::Path::new(url).is_absolute() {
        return url.to_string();
    }
    let dir = base.rsplit_once(['/', '\\']).map(|(d, _)| d).unwrap_or("");
    if dir.is_empty() { url.to_string() } else { format!("{dir}/{url}") }
}

fn fetch(location: &str, max: u64) -> Result<Vec<u8>, String> {
    if !is_url(location) {
        return std::fs::read(location).map_err(|e| format!("Couldn't read {location}: {e}"));
    }
    // The client only uses https, so an http:// address fails here.
    let res = crate::selfupdate::client()?.get(location).send().map_err(|e| format!("Couldn't reach the addons list: {e}"))?;
    match res.status().as_u16() {
        200 => crate::selfupdate::read_capped(res, max),
        404 => Err("The official addons can't be downloaded yet: their repository is still private.".into()),
        s => Err(format!("Downloading {location} failed (error {s}).")),
    }
}

static CACHE: Mutex<Option<(Instant, String, Index)>> = Mutex::new(None);

/// The official list, from memory if it was read in the last few minutes.
pub fn index(fresh: bool) -> Result<Index, String> {
    let location = index_location();
    {
        let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, loc, idx)) = cache.as_ref()
            && !fresh
            && *loc == location
            && at.elapsed() < Duration::from_secs(600)
        {
            return Ok(idx.clone());
        }
    }
    let idx: Index = serde_json::from_slice(&fetch(&location, crate::selfupdate::MAX_SMALL)?).map_err(|e| format!("The addons list couldn't be read: {e}"))?;
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), location, idx.clone()));
    Ok(idx)
}

impl Entry {
    /// Why this hub can't install it, if it can't.
    fn problem(&self, hub_version: &str) -> Option<String> {
        let m = Manifest {
            id: self.id.clone(),
            name: self.name.clone(),
            description: String::new(),
            version: self.version.clone(),
            api: self.api,
            min_hub: self.min_hub.clone(),
            pages: vec![],
            helper: None,
        };
        m.problem(hub_version).or_else(|| (!self.platforms.contains_key(&addons::platform())).then(|| format!("{} doesn't work on {}.", self.name, addons::platform())))
    }
}

/// Downloads and checks an addon's zip. Returns its bytes.
pub fn download_verified(index_at: &str, entry: &Entry, pubkey: &str) -> Result<Vec<u8>, String> {
    let dl = entry.platforms.get(&addons::platform()).ok_or_else(|| format!("{} doesn't work on {}.", entry.name, addons::platform()))?;
    let url = join(index_at, &dl.url);
    let data = fetch(&url, crate::selfupdate::MAX_DOWNLOAD)?;
    verify(&data, dl, &url, pubkey)?;
    Ok(data)
}

fn verify(data: &[u8], dl: &Download, url: &str, pubkey: &str) -> Result<(), String> {
    use sha2::Digest;
    let hash: String = sha2::Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect();
    if !hash.eq_ignore_ascii_case(dl.sha256.trim()) {
        return Err("The addon download is damaged (its checksum doesn't match). Nothing was installed.".into());
    }
    // The signature must be for this very file, so one signed addon can't stand in for another.
    let file = url.rsplit(['/', '\\']).next().unwrap_or(url);
    crate::selfupdate::verify_file(data, &dl.signature, pubkey, Some(&[file])).map_err(|e| format!("{e} Nothing was installed."))
}

/// Installs or updates an official addon. An older version than the installed one only goes in
/// with `downgrade`.
pub fn install(app: &AppHandle, id: &str, downgrade: bool) -> Result<Manifest, addons::InstallError> {
    let idx = index(false)?;
    let entry = idx.addons.iter().find(|e| e.id == id).ok_or_else(|| format!("There's no official addon '{id}'."))?;
    if let Some(problem) = entry.problem(&app.package_info().version.to_string()) {
        return Err(problem.into());
    }
    let data = download_verified(&index_location(), entry, PUBKEY)?;
    // A private folder, so nobody can swap the checked zip before it's extracted.
    let tmp = crate::tempdir::TempDir::new("ue-dev-hub-addon")?;
    let zip = tmp.path().join(format!("{id}.zip"));
    std::fs::write(&zip, &data).map_err(|e| format!("Couldn't save the download: {e}"))?;
    addons::install(app, &zip, Source::Official, addons::Expect { id: Some(id), version: Some(&entry.version), downgrade })
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Available {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    /// The installed version, if any.
    pub installed: Option<String>,
    /// A newer official version than the installed one.
    pub update: bool,
    pub problem: Option<String>,
}

pub fn available(app: &AppHandle, fresh: bool) -> Result<Vec<Available>, String> {
    let idx = index(fresh)?;
    let installed: HashMap<String, (String, Source)> = addons::installed_in(&addons::root(app)?)
        .into_iter()
        .map(|a| (a.manifest.id.clone(), (a.manifest.version.clone(), crate::settings::load(app).addons.get(&a.manifest.id).map(|r| r.source).unwrap_or(Source::Disk))))
        .collect();
    let version = app.package_info().version.to_string();
    Ok(idx
        .addons
        .into_iter()
        .filter(|e| addons::is_id(&e.id))
        .map(|e| {
            let inst = installed.get(&e.id);
            let newer = inst.is_some_and(|(v, _)| crate::config::parse_version(&e.version) > crate::config::parse_version(v));
            Available {
                problem: e.problem(&version),
                update: newer,
                installed: inst.map(|(v, _)| v.clone()),
                id: e.id,
                name: e.name,
                description: e.description,
                version: e.version,
            }
        })
        .collect())
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsResult {
    /// Names of addons that were installed just now.
    pub installed: Vec<String>,
    /// Ids the project lists that couldn't be installed, with why.
    pub missing: Vec<(String, String)>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsOffer {
    /// Official addons the project uses that can be installed, to ask the person about.
    pub offer: Vec<Offer>,
    /// Ids the project lists that can't be installed from the official list, with why.
    pub missing: Vec<(String, String)>,
}

/// The project's default addons that aren't installed yet and that the person didn't turn off
/// or decline. Nothing is installed without asking: a project's ue-dev-hub.toml comes from
/// whatever repository was cloned.
pub fn default_offers(app: &AppHandle, project: &Project) -> DefaultsOffer {
    let mut result = DefaultsOffer::default();
    let Ok(root) = addons::root(app) else { return result };
    let installed: Vec<String> = addons::installed_in(&root).into_iter().map(|a| a.manifest.id).collect();
    let prefs = crate::settings::load(app).prefs(&project.root);
    let wanted: Vec<&String> = project.config.addons.default.iter().filter(|id| !installed.contains(id) && prefs.addons.get(*id) != Some(&false)).collect();
    if wanted.is_empty() {
        return result;
    }
    let version = app.package_info().version.to_string();
    match index(false) {
        Ok(idx) => {
            for id in wanted {
                match idx.addons.iter().find(|e| e.id == *id) {
                    Some(e) => match e.problem(&version) {
                        Some(problem) => result.missing.push((id.clone(), problem)),
                        None => result.offer.push(Offer { id: e.id.clone(), name: e.name.clone(), description: e.description.clone() }),
                    },
                    None => result.missing.push((id.clone(), format!("There's no official addon '{id}'."))),
                }
            }
        }
        Err(e) => result.missing.extend(wanted.into_iter().map(|id| (id.clone(), e.clone()))),
    }
    result
}

/// Installs the default addons the person said yes to, or with `install` false remembers the
/// no, so they aren't asked again for this project. Only ids the project lists count.
pub fn answer_defaults(app: &AppHandle, project: &Project, ids: &[String], install_them: bool) -> Result<DefaultsResult, String> {
    let mut result = DefaultsResult::default();
    let ids: Vec<&String> = ids.iter().filter(|id| addons::is_default(project, id)).collect();
    if !install_them {
        crate::settings::update(app, |s| {
            let prefs = s.prefs_mut(&project.root);
            for id in &ids {
                prefs.addons.insert((*id).clone(), false);
            }
        })?;
        return Ok(result);
    }
    for id in ids {
        match install(app, id, false) {
            Ok(m) => result.installed.push(m.name),
            Err(e) => result.missing.push((id.clone(), e.to_string())),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_are_found_next_to_the_index() {
        assert_eq!(join("https://x.org/a/index.json", "git.zip"), "https://x.org/a/git.zip");
        assert_eq!(join("https://x.org/a/index.json", "https://y.org/git.zip"), "https://y.org/git.zip");
        assert_eq!(join(r"C:\addons\index.json", "git.zip"), r"C:\addons/git.zip");
        assert_eq!(join("index.json", "git.zip"), "git.zip");
    }

    #[test]
    fn refuses_a_damaged_or_unsigned_download() {
        let dl = Download { url: "git-1.0.0.zip".into(), sha256: "00".into(), signature: "x".into() };
        assert!(verify(b"zip", &dl, "git-1.0.0.zip", PUBKEY).unwrap_err().contains("checksum"));
        use sha2::Digest;
        let real: String = sha2::Sha256::digest(b"zip").iter().map(|b| format!("{b:02x}")).collect();
        let dl = Download { sha256: real, ..dl };
        assert!(verify(b"zip", &dl, "git-1.0.0.zip", PUBKEY).unwrap_err().contains("Nothing was installed"));
    }
}
