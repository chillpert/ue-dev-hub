//! Updating and installing the hub when it runs without being installed, from
//! `%LOCALAPPDATA%\UE Dev Hub Portable\` where Launch.sh puts it. It's kept outside the
//! project on purpose: Windows looks for programs and DLLs next to a running .exe first, so a copy
//! inside a repository could pick up files that the repository contains.
//!
//! Installed copies update with the Tauri updater. A copy that isn't installed downloads the new
//! `ue-dev-hub.exe` from the latest release instead, checks its signature with the updater's
//! public key, renames itself to `ue-dev-hub.old.exe` (Windows can't overwrite a running .exe, but
//! it can rename one), puts the new one in its place and restarts.

use base64::Engine as _;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// Release files with fixed names, published next to the versioned ones by the release workflow.
pub const PORTABLE_ASSET: &str = "ue-dev-hub.exe";
pub const SETUP_ASSET: &str = "ue-dev-hub-setup.exe";

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InstallState {
    pub installed: bool,
    pub exe: PathBuf,
    /// Where an install goes: %LOCALAPPDATA%\UE Dev Hub.
    pub install_dir: Option<PathBuf>,
    /// Installed with a Linux package (.deb), which updates through the package manager: replacing
    /// its files needs root.
    pub package_managed: bool,
}

pub fn install_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("UE Dev Hub"))
}

fn installed_exe() -> Option<PathBuf> {
    install_dir().map(|d| d.join("ue-dev-hub.exe")).filter(|p| p.is_file())
}

pub fn state() -> InstallState {
    let exe = std::env::current_exe().unwrap_or_default();
    // On Linux the AppImage or package is the hub. The Tauri updater updates the AppImage;
    // the AppImage runtime sets APPIMAGE to its file.
    if !cfg!(windows) {
        let package_managed = linux_package_managed(std::env::var_os("APPIMAGE").is_some());
        return InstallState { installed: true, exe, install_dir: None, package_managed };
    }
    let dir = install_dir();
    let installed = dir
        .as_ref()
        .zip(exe.parent())
        .is_some_and(|(d, p)| crate::settings::same_path(d, p));
    InstallState { installed, exe, install_dir: dir, package_managed: false }
}

/// On Linux, everything that isn't an AppImage came from a package (or was built by hand).
fn linux_package_managed(in_appimage: bool) -> bool {
    cfg!(target_os = "linux") && !in_appimage
}

/// Removes the previous version left behind by an update.
pub fn cleanup_after_update() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_exe(&exe));
    }
}

fn old_exe(exe: &Path) -> PathBuf {
    exe.with_file_name("ue-dev-hub.old.exe")
}

/// The `https://github.com/<owner>/<repo>` the updater uses, from tauri.conf.json.
fn release_base(app: &AppHandle) -> Result<String, String> {
    let endpoint = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("endpoints"))
        .and_then(|e| e.get(0))
        .and_then(|e| e.as_str())
        .ok_or("The hub has no update address configured.")?
        .to_string();
    let base = endpoint.split("/releases/").next().unwrap_or("").to_string();
    if base.starts_with("https://github.com/") { Ok(base) } else { Err(format!("Unexpected update address: {endpoint}")) }
}

fn pubkey(app: &AppHandle) -> Result<String, String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|k| k.as_str())
        .map(str::to_string)
        .ok_or("The hub has no update key configured.".into())
}

pub(crate) fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        // Also for redirects: a release download never goes over plain http.
        .https_only(true)
        .timeout(Duration::from_secs(600))
        .user_agent(concat!("UE Dev Hub/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// The most a response body may be, so a server can't fill the memory. Downloads of programs
/// are far below `MAX_DOWNLOAD`; lists and signatures far below `MAX_SMALL`.
pub(crate) const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
pub(crate) const MAX_SMALL: u64 = 4 * 1024 * 1024;

/// Reads a response body of at most `max` bytes.
pub(crate) fn read_capped(res: reqwest::blocking::Response, max: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let too_big = || "The download is much bigger than expected, so it wasn't used.".to_string();
    if res.content_length().is_some_and(|n| n > max) {
        return Err(too_big());
    }
    let mut data = Vec::new();
    res.take(max + 1).read_to_end(&mut data).map_err(|e| format!("The download stopped: {e}"))?;
    if data.len() as u64 > max {
        return Err(too_big());
    }
    Ok(data)
}

fn download(url: &str, max: u64) -> Result<Vec<u8>, String> {
    let res = client()?.get(url).send().map_err(|e| format!("Couldn't download {url}: {e}"))?;
    match res.status().as_u16() {
        200 => read_capped(res, max),
        404 => Err("There's no hub release to download yet, or its GitHub repository is private.".into()),
        s => Err(format!("Downloading {url} failed (error {s}).")),
    }
}

/// Checks `data` against a Tauri updater signature (base64 of a minisign signature file) with the
/// updater's public key (base64 of a minisign public key file).
#[cfg(test)]
pub fn verify(data: &[u8], sig_b64: &str, pubkey_b64: &str) -> Result<(), String> {
    verify_file(data, sig_b64, pubkey_b64, None)
}

/// Like `verify`, and also checks the signed (trusted) comment names exactly one of `files`, so a
/// signed installer can't be passed off as the hub, or one signed addon as another.
pub fn verify_file(data: &[u8], sig_b64: &str, pubkey_b64: &str, files: Option<&[&str]>) -> Result<(), String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let pk_text = String::from_utf8(b64.decode(pubkey_b64.trim()).map_err(|_| "The update key is broken.")?).map_err(|_| "The update key is broken.")?;
    let sig_text = String::from_utf8(b64.decode(sig_b64.trim()).map_err(|_| "The download's signature is broken.")?).map_err(|_| "The download's signature is broken.")?;
    let pk_line = pk_text.lines().find(|l| !l.starts_with("untrusted comment")).ok_or("The update key is broken.")?;
    let pk = minisign_verify::PublicKey::from_base64(pk_line.trim()).map_err(|_| "The update key is broken.")?;
    let sig = minisign_verify::Signature::decode(&sig_text).map_err(|_| "The download's signature is broken.")?;
    pk.verify(data, &sig, false).map_err(|_| "The download's signature doesn't match. It wasn't used.".to_string())?;
    if let Some(files) = files {
        let signed_for = sig.trusted_comment().split('\t').find_map(|p| p.strip_prefix("file:")).unwrap_or("");
        if !files.iter().any(|f| f.eq_ignore_ascii_case(signed_for)) {
            return Err(format!("The download was signed for '{signed_for}', not for {}. It wasn't used.", files.join(" or ")));
        }
    }
    Ok(())
}

/// `signed_as` are the file names the signature may be for.
fn download_verified(app: &AppHandle, asset: &str, signed_as: &[&str]) -> Result<Vec<u8>, String> {
    let base = release_base(app)?;
    let data = download(&format!("{base}/releases/latest/download/{asset}"), MAX_DOWNLOAD)?;
    let sig = String::from_utf8(download(&format!("{base}/releases/latest/download/{asset}.sig"), MAX_SMALL)?).map_err(|_| "The download's signature is broken.")?;
    verify_file(&data, &sig, &pubkey(app)?, Some(signed_as))?;
    Ok(data)
}

/// The newest released version, from the updater's latest.json.
fn latest_version(app: &AppHandle) -> Result<String, String> {
    let base = release_base(app)?;
    let json: serde_json::Value = serde_json::from_slice(&download(&format!("{base}/releases/latest/download/latest.json"), MAX_SMALL)?)
        .map_err(|_| "The release's latest.json couldn't be read.")?;
    json.get("version").and_then(|v| v.as_str()).map(|v| v.trim_start_matches('v').to_string()).ok_or("latest.json has no version.".into())
}

/// The newest released version if it's newer than this one, for installs the hub doesn't
/// update itself, like a .deb. The Tauri updater can't tell them: latest.json has no entry for them.
pub fn newer_release(app: &AppHandle) -> Result<Option<String>, String> {
    Ok(newer(&latest_version(app)?, &app.package_info().version.to_string()))
}

fn newer(latest: &str, current: &str) -> Option<String> {
    let parsed = crate::config::parse_version(latest)?;
    (Some(parsed) > crate::config::parse_version(current)).then(|| latest.to_string())
}

/// The FileVersion in a Windows exe's version resource, like "0.2.0".
fn exe_version(exe: &[u8]) -> Option<String> {
    let key: Vec<u8> = "FileVersion\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut from = 0;
    while let Some(at) = exe.get(from..)?.windows(key.len()).position(|w| w == key.as_slice()) {
        let mut i = from + at + key.len();
        from = i;
        // The value comes after zero padding.
        while exe.get(i..i + 2) == Some(&[0, 0]) {
            i += 2;
        }
        let mut value = String::new();
        while let Some(&[lo, hi]) = exe.get(i..i + 2) {
            let c = u16::from_le_bytes([lo, hi]);
            if c == 0 || value.len() > 32 {
                break;
            }
            value.push(char::from_u32(c.into())?);
            i += 2;
        }
        if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit() || c == '.') && crate::config::parse_version(&value).is_some() {
            return Some(value);
        }
    }
    None
}

/// Swaps the running exe for `new` and restarts with the same arguments.
fn replace_running_exe(new: &[u8]) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    replace_exe(&exe, new)?;
    Ok(exe)
}

/// Puts `new` at `exe`, keeping the previous file as ue-dev-hub.old.exe. Works while `exe` runs.
fn replace_exe(exe: &Path, new: &[u8]) -> Result<(), String> {
    let exe = exe.to_path_buf();
    let staged = exe.with_file_name("ue-dev-hub.new.exe");
    std::fs::write(&staged, new).map_err(|e| format!("Couldn't save the update next to the hub: {e}"))?;
    let old = old_exe(&exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("Couldn't make room for the update: {e}"))?;
    if let Err(e) = std::fs::rename(&staged, &exe) {
        // Put the running version back so the hub still starts next time.
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("Couldn't put the update in place: {e}"));
    }
    Ok(())
}

/// Updates a hub that isn't installed. Returns the new version, or None if it's current.
/// On success the app restarts itself.
pub fn update_portable(app: &AppHandle) -> Result<Option<String>, String> {
    if !cfg!(windows) {
        return Ok(None);
    }
    let latest = latest_version(app)?;
    let current = app.package_info().version.to_string();
    if crate::config::parse_version(&latest) <= crate::config::parse_version(&current) {
        return Ok(None);
    }
    // If the last start already updated to this version and we're still older, the release's
    // exe reports a wrong version. Don't download and restart on every start because of it.
    let marker = app.path().app_config_dir().map_err(|e| e.to_string())?.join("portable-update.txt");
    if std::fs::read_to_string(&marker).is_ok_and(|v| v.trim() == latest) {
        return Err(format!("The hub already updated to {latest} once, but still reports {current}, so the release is probably labelled wrong. Download the hub again from the release page."));
    }
    crate::jobs::set_flow(Some("Updating the hub".into()))?;
    let result = (|| {
        let data = download_verified(app, PORTABLE_ASSET, &[PORTABLE_ASSET])?;
        // latest.json isn't signed, and the signature doesn't name a version. The version inside
        // the signed exe can't be faked, so an old release can't be passed off as an update.
        match exe_version(&data) {
            Some(v) if crate::config::parse_version(&v) > crate::config::parse_version(&current) => {}
            Some(v) => return Err(format!("The downloaded hub is version {v}, which isn't newer than this one ({current}). It wasn't used.")),
            None => return Err("The downloaded hub's version couldn't be read, so it wasn't used. Download the hub again from the release page.".into()),
        }
        let exe = replace_running_exe(&data)?;
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&marker, &latest);
        let args: Vec<String> = std::env::args().skip(1).collect();
        restart_later(&exe, &args)
    })();
    let _ = crate::jobs::set_flow(None);
    result?;
    app.exit(0);
    Ok(Some(latest))
}

/// Starts the new `exe` with the same arguments. It waits for this process to exit first
/// (`--wait-pid`), so the single-instance check doesn't hand the start back to the closing hub.
fn restart_later(exe: &Path, args: &[String]) -> Result<(), String> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args).arg(WAIT_PID).arg(std::process::id().to_string());
    crate::proc::spawn_detached(cmd)
}

pub const WAIT_PID: &str = "--wait-pid";

/// Handles `--wait-pid <pid>` at startup: waits up to 30 seconds for that process to end.
/// Returns the arguments without it. Runs before anything else in `run`, so the single-instance
/// check can't hand this start to the hub that is closing.
pub fn wait_for_previous(args: Vec<String>) -> Vec<String> {
    let (rest, pid) = split_wait_pid(args);
    if let Some(pid) = pid {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline && pid_running(&pid) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    rest
}

/// The arguments without `--wait-pid <pid>`, and the pid if it was there.
pub fn split_wait_pid(args: Vec<String>) -> (Vec<String>, Option<String>) {
    let mut out = Vec::with_capacity(args.len());
    let mut pid = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == WAIT_PID {
            pid = it.next();
        } else {
            out.push(a);
        }
    }
    (out, pid)
}

fn pid_running(pid: &str) -> bool {
    pid.parse().is_ok_and(crate::proc::pid_running)
}

/// Downloads the installer and checks it, then runs it silently and closes the hub. The
/// installer starts the installed hub on `project` when it's done (`/R`, `/ARGS`).
pub fn install(app: &AppHandle, project: Option<&Path>) -> Result<(), String> {
    if !cfg!(windows) {
        return Err("On Linux, install the hub with its .deb package or use the AppImage.".into());
    }
    // The release copies the bundler's signature, made for "UE Dev Hub_<version>_x64-setup.exe".
    let latest = latest_version(app)?;
    let versioned = format!("{}_{latest}_x64-setup.exe", app.package_info().name);
    let data = download_verified(app, SETUP_ASSET, &[SETUP_ASSET, &versioned])?;
    // A fresh folder, so nothing else can sit next to the installer or swap it before it runs.
    let dir = crate::tempdir::TempDir::new("ue-dev-hub-install").map_err(|e| format!("Couldn't prepare the installer: {e}"))?.keep();
    let setup = dir.join("ue-dev-hub-setup.exe");
    std::fs::write(&setup, data).map_err(|e| format!("Couldn't save the installer: {e}"))?;
    // The installer reads everything after /ARGS as the hub's arguments.
    let project_arg = project.map(|p| format!(" --project \"{}\"", p.display())).unwrap_or_default();
    let installer_args = format!("/S /R /ARGS{project_arg}");
    #[cfg(windows)]
    let cmd = {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new(&setup);
        c.raw_arg(installer_args);
        c
    };
    #[cfg(not(windows))]
    let cmd = {
        let mut c = std::process::Command::new(&setup);
        c.arg(installer_args);
        c
    };
    crate::proc::spawn_detached(cmd)?;
    app.exit(0);
    Ok(())
}

/// Whether to show the first-run "Install UE Dev Hub?" question.
pub fn should_offer_install(app: &AppHandle) -> bool {
    // Dev builds run from target\debug and would ask on every start.
    !cfg!(debug_assertions)
        && !state().installed && installed_exe().is_none() && !crate::settings::load(app).never_offer_install
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway key pair made for this test with `tauri signer generate`, and its signature of
    // DATA made with `tauri signer sign`. Not the hub's release key.
    const PUB: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDA2QkUxNDQ2RDhEQjRDMzUKUldRMVROdllSaFMrQm91WC9jNUxPWmRhTXI0Q2tXcFNpNElPd1B5YWpTQms3cXRFUnNvRVJGcm0K";
    const SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRMVROdllSaFMrQm4zZGZnYWpscGJqOGxSbURya3pCYUIzSGdwR2hNOEZZU1dzQ1I3dDRPWlorVHJYNXZCcm5uWnZ2Tzlqakp6Y1lLb3BIalhHODBIMENFaEdBcVRTMHd3PQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0NDM0NjYxCWZpbGU6ZGF0YS50eHQKQzJKaXFCd1Vhc3NMemY5R1ZuRUhZSG1NemJ0djBsdHpWcVB1SktnNk4wM0pGeUtSQmZScS9mNDEzcFpnSWJDemMySUJIc1g4Q2lvdGFxNXZuVkRiQ1E9PQo=";
    const DATA: &[u8] = b"hello launcher
";

    #[test]
    fn accepts_a_signed_download_and_rejects_a_changed_one() {
        verify(DATA, SIG, PUB).unwrap();
        assert!(verify(b"hello launcher, tampered
", SIG, PUB).unwrap_err().contains("doesn't match"));
        assert!(verify(DATA, "%%%", PUB).is_err());
    }

    #[test]
    fn swaps_the_exe_and_keeps_the_old_one() {
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("ue-dev-hub.exe");
        std::fs::write(&exe, b"v1").unwrap();
        // Holding the file open like a running program does on Windows.
        let _running = std::fs::File::open(&exe).unwrap();
        replace_exe(&exe, b"v2").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"v2");
        assert_eq!(std::fs::read(dir.join("ue-dev-hub.old.exe")).unwrap(), b"v1");
        drop(_running);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checks_which_file_was_signed() {
        // The throwaway signature above was made for data.txt.
        verify_file(DATA, SIG, PUB, Some(&["data.txt"])).unwrap();
        verify_file(DATA, SIG, PUB, Some(&["other.txt", "DATA.txt"])).unwrap();
        assert!(verify_file(DATA, SIG, PUB, Some(&["ue-dev-hub.exe"])).unwrap_err().contains("signed for"));
        // Only the exact name counts, not a name that contains it or is contained in it.
        assert!(verify_file(DATA, SIG, PUB, Some(&["ata.txt"])).is_err());
        assert!(verify_file(DATA, SIG, PUB, Some(&["data.txt.exe"])).is_err());
    }

    #[test]
    fn reads_the_version_inside_an_exe() {
        let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
        let mut exe = b"MZ...".to_vec();
        // A string elsewhere that isn't the version, then the real entry.
        exe.extend(utf16("FileVersion\0not a version\0"));
        exe.extend(utf16("FileVersion\0"));
        exe.extend([0, 0]);
        exe.extend(utf16("0.2.1\0"));
        assert_eq!(exe_version(&exe).as_deref(), Some("0.2.1"));
        assert_eq!(exe_version(b"MZ no resource"), None);
    }

    #[test]
    fn tells_only_a_newer_release() {
        assert_eq!(newer("0.2.0", "0.1.0").as_deref(), Some("0.2.0"));
        assert_eq!(newer("0.1.0", "0.1.0"), None);
        assert_eq!(newer("0.0.9", "0.1.0"), None);
        assert_eq!(newer("garbage", "0.1.0"), None);
    }

    #[test]
    fn strips_the_wait_argument() {
        let args = vec!["ue-dev-hub.exe".to_string(), "--project".into(), r"C:\Game".into(), WAIT_PID.into(), "999999".into()];
        assert_eq!(wait_for_previous(args), vec!["ue-dev-hub.exe".to_string(), "--project".into(), r"C:\Game".into()]);
    }

    #[test]
    fn updates_only_the_appimage_on_linux() {
        assert!(!linux_package_managed(true));
        assert_eq!(linux_package_managed(false), cfg!(target_os = "linux"));
    }

    #[test]
    fn rejects_a_signature_from_another_key() {
        // The hub's real public key didn't sign DATA.
        let real = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDc2OENBMzE5QjJBN0Y3MUYKUldRZjk2ZXlHYU9NZHJ1QXNqc3NTQkRGOFRXRldoQjRzNXVmcEZ5b05HVnRtRk9kcW5vQVdUVjQK";
        assert!(verify(DATA, SIG, real).is_err());
    }

    /// Serves `body` once on localhost, with or without a Content-Length.
    fn serve_once(body: &'static [u8], length: bool) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.read(&mut [0u8; 4096]);
            let head = if length { format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()) } else { "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_string() };
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(body);
        });
        url
    }

    #[test]
    fn refuses_bodies_over_the_limit() {
        let get = |url: &str| reqwest::blocking::get(url).unwrap();
        assert_eq!(read_capped(get(&serve_once(b"0123456789", true)), 10).unwrap(), b"0123456789");
        assert!(read_capped(get(&serve_once(b"0123456789", true)), 9).is_err());
        // Without a Content-Length, reading stops at the limit.
        assert!(read_capped(get(&serve_once(b"0123456789", false)), 9).is_err());
        assert_eq!(read_capped(get(&serve_once(b"0123456789", false)), 10).unwrap(), b"0123456789");
    }

    #[test]
    fn release_downloads_refuse_plain_http() {
        let err = client().unwrap().get(serve_once(b"x", true)).send().unwrap_err();
        assert!(err.is_builder() || err.to_string().contains("http"), "{err}");
    }
}
