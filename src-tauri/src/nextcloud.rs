//! Uploading builds to the team's Nextcloud.
//!
//! Each person connects with their own account through Nextcloud's Login Flow v2: the browser
//! opens, they sign in as usual (through single sign-on if the server uses it), and Nextcloud hands the
//! hub an app password that can be revoked in Nextcloud's security settings. If Cloudflare
//! Access protects the server, each person also signs in with `cloudflared access login`, and
//! their personal token goes with every request. Secrets live only in the system's
//! keyring: Windows Credential Manager, or the Secret Service (GNOME Keyring, KWallet) on Linux.
//!
//! Files go up with WebDAV chunked upload v2 in 50 MB pieces (Cloudflare refuses request bodies
//! over 100 MB), and an interrupted upload continues where it stopped.

use crate::log::{Log, Progress};
use crate::proc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const CHUNK: u64 = 50 * 1024 * 1024;
const KEYRING_SERVICE: &str = "UE Dev Hub Nextcloud";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct Credentials {
    login_name: String,
    app_password: String,
    /// The account's user id, which WebDAV paths use. It can differ from the login name (LDAP,
    /// single sign-on, email logins). Empty for logins saved before it was stored.
    #[serde(default)]
    user_id: String,
}

impl Credentials {
    fn dav_user(&self) -> &str {
        if self.user_id.is_empty() { &self.login_name } else { &self.user_id }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub configured: bool,
    pub url: Option<String>,
    pub folder: Option<String>,
    /// Who's connected, if anyone.
    pub login_name: Option<String>,
    /// Whether the server is behind Cloudflare Access (None: couldn't tell).
    pub cloudflare_access: Option<bool>,
    pub cloudflared_installed: bool,
    /// Where the uploaded builds can be seen in the browser.
    pub web_folder_url: Option<String>,
}

/// The server's address without a trailing slash, from `upload.nextcloud_url`.
pub struct Server {
    pub url: String,
    pub folder: String,
}

fn host_of(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or(url).to_lowercase()
}

/// Where logins are kept, for messages.
const KEYRING: &str = if cfg!(windows) { "Windows Credential Manager" } else { "your system keyring" };

/// What to do when the keyring can't be used. On Linux a Secret Service has to be running.
const KEYRING_HELP: &str = if cfg!(windows) {
    ""
} else {
    " Logins are kept in a Secret Service keyring, such as GNOME Keyring or KWallet. Make sure one is installed and running, and unlocked."
};

fn keyring_entry(url: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, &host_of(url)).map_err(|e| format!("{KEYRING} isn't available: {e}.{KEYRING_HELP}"))
}

fn load_credentials(url: &str) -> Option<Credentials> {
    let secret = keyring_entry(url).ok()?.get_password().ok()?;
    serde_json::from_str(&secret).ok()
}

fn save_credentials(url: &str, c: &Credentials) -> Result<(), String> {
    let json = serde_json::to_string(c).map_err(|e| e.to_string())?;
    keyring_entry(url)?.set_password(&json).map_err(|e| format!("Couldn't save the login in {KEYRING}: {e}.{KEYRING_HELP}"))
}

fn delete_credentials(url: &str) {
    if let Ok(e) = keyring_entry(url) {
        let _ = e.delete_credential();
    }
}

fn enc(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn enc_path(path: &str) -> String {
    path.split('/').filter(|s| !s.is_empty()).map(enc).collect::<Vec<_>>().join("/")
}

// ---------- Cloudflare Access ----------

pub fn cloudflared() -> Option<PathBuf> {
    if proc::exists("cloudflared", "--version") {
        return Some(PathBuf::from("cloudflared"));
    }
    ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
        .iter()
        .filter_map(std::env::var_os)
        .flat_map(|p| [PathBuf::from(&p).join(r"cloudflared\cloudflared.exe"), PathBuf::from(&p).join(r"Cloudflare\cloudflared\cloudflared.exe")])
        .find(|p| p.is_file())
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        // The address must be https (see config.rs); the login and uploads never go over http.
        .https_only(true)
        // Redirects to a sign-in page mean "not allowed", so look at them instead of following.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(600))
        .user_agent(concat!("UE Dev Hub/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// Whether a response is a sign-in page instead of the server.
#[derive(Debug, PartialEq)]
enum Gate {
    Open,
    CloudflareAccess,
    /// Another sign-in page (an authenticating proxy) in front of the app's addresses.
    OtherLogin(String),
}

fn gate_of(res: &reqwest::blocking::Response) -> Gate {
    let location = res.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    let status = res.status().as_u16();
    if location.contains("cloudflareaccess.com") || res.headers().contains_key("cf-access-domain") {
        return Gate::CloudflareAccess;
    }
    if (300..400).contains(&status) && !location.is_empty() {
        return Gate::OtherLogin(location);
    }
    Gate::Open
}

/// Asks the server something harmless to learn what's in front of it.
fn probe(url: &str, cf_token: Option<&str>) -> Result<Gate, String> {
    let mut req = client()?.get(format!("{url}/status.php"));
    if let Some(t) = cf_token {
        req = req.header("cf-access-token", t);
    }
    let res = req.send().map_err(|e| format!("Couldn't reach {url}: {}", e.without_url()))?;
    Ok(gate_of(&res))
}

/// This person's Cloudflare Access token for the server, if they signed in before and it's valid.
fn cf_token(url: &str) -> Option<String> {
    let exe = cloudflared()?;
    let o = proc::run_timeout(&exe.to_string_lossy(), &["access", "token", &format!("-app={url}")], None, 30).ok()?;
    let t = o.stdout.trim().to_string();
    (o.ok && t.split('.').count() == 3).then_some(t)
}

/// Opens the browser for the Cloudflare Access sign-in and waits for it, until `cancelled`.
fn cf_login(log: &Log, url: &str, cancelled: &dyn Fn() -> bool) -> Result<String, String> {
    let how = if cfg!(windows) { "winget install Cloudflare.cloudflared" } else { "see Cloudflare's downloads page, or your package manager" };
    let exe = cloudflared()
        .ok_or(format!("This server is behind Cloudflare Access, and signing in to it needs cloudflared. Install it ({how}), then connect again."))?;
    log.info("Opening the browser to sign in to Cloudflare Access ...");
    let o = proc::run_cancellable(&exe.to_string_lossy(), &["access", "login", url], None, 300, cancelled)?;
    if !o.ok {
        return Err(format!("The Cloudflare Access sign-in didn't finish: {}", o.message()));
    }
    cf_token(url).ok_or_else(|| "Signed in to Cloudflare Access, but no token came back. Try connecting again.".into())
}

/// The headers every request needs: the Cloudflare token when the server is behind Access.
struct Session {
    url: String,
    cf: Option<String>,
    creds: Credentials,
    client: reqwest::blocking::Client,
}

impl Session {
    fn req(&self, method: &str, url: &str) -> reqwest::blocking::RequestBuilder {
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut r = self.client.request(m, url).basic_auth(&self.creds.login_name, Some(&self.creds.app_password));
        if let Some(t) = &self.cf {
            r = r.header("cf-access-token", t);
        }
        r
    }

    fn check(&self, res: reqwest::blocking::Response, what: &str) -> Result<reqwest::blocking::Response, String> {
        match gate_of(&res) {
            Gate::CloudflareAccess => return Err("Your Cloudflare Access sign-in expired. Connect to Nextcloud again in Settings.".into()),
            Gate::OtherLogin(to) => return Err(blocked_message(&to)),
            Gate::Open => {}
        }
        let status = res.status().as_u16();
        if status == 401 {
            return Err("Nextcloud doesn't accept the hub's login anymore (it may have been revoked). Connect again in Settings.".into());
        }
        if status == 507 {
            return Err("The Nextcloud storage is full. Delete old builds from the upload folder in Nextcloud, then upload again.".into());
        }
        if !res.status().is_success() {
            let body: String = res.text().unwrap_or_default().chars().take(300).collect();
            return Err(format!("{what} failed (error {status}).\n\n{body}"));
        }
        Ok(res)
    }
}

fn blocked_message(to: &str) -> String {
    format!(
        "A sign-in page in front of Nextcloud blocks the hub (it redirects to {}). The server needs to let the app addresses through: /remote.php/dav and /index.php/login/v2. Whoever runs the server has to allow them.",
        to.split('?').next().unwrap_or(to)
    )
}

pub fn server(project: &crate::project::Project) -> Option<Server> {
    let u = project.config.upload.nextcloud_url.clone()?;
    let folder = expand_folder(project.config.upload.folder.trim_matches('/'), chrono::Local::now().date_naive());
    Some(Server { url: u.trim_end_matches('/').to_string(), folder })
}

/// Fills in `{year}` and `{month}`, like Team/Builds/{year} -> Team/Builds/2026. Missing
/// folders below the top one are created when uploading.
fn expand_folder(folder: &str, date: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    folder.replace("{year}", &date.year().to_string()).replace("{month}", &format!("{:02}", date.month()))
}

pub fn status(project: &crate::project::Project, probe_server: bool) -> Status {
    let Some(s) = server(project) else {
        return Status {
            configured: false,
            url: None,
            folder: None,
            login_name: None,
            cloudflare_access: None,
            cloudflared_installed: cloudflared().is_some(),
            web_folder_url: None,
        };
    };
    let cloudflare_access = if probe_server { probe(&s.url, None).ok().map(|g| g == Gate::CloudflareAccess) } else { None };
    Status {
        configured: true,
        login_name: load_credentials(&s.url).map(|c| c.login_name),
        web_folder_url: Some(format!("{}/apps/files/?dir=/{}", s.url, enc_path(&s.folder))),
        url: Some(s.url),
        folder: Some(s.folder),
        cloudflare_access,
        cloudflared_installed: cloudflared().is_some(),
    }
}

// ---------- Connecting ----------

#[derive(Deserialize)]
struct LoginStart {
    poll: LoginPoll,
    login: String,
}

#[derive(Deserialize)]
struct LoginPoll {
    token: String,
    endpoint: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginResult {
    login_name: String,
    app_password: String,
}

/// The sign-in that's waiting for the browser, if any, with its cancel flag.
static CONNECTING: std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> = std::sync::Mutex::new(None);

/// Claims the one sign-in that may run; releases it when dropped.
#[derive(Debug)]
struct Connecting(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Connecting {
    fn claim() -> Result<Connecting, String> {
        let mut slot = CONNECTING.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Err("Signing in to Nextcloud is already in progress. Finish it in your browser, or cancel it first.".into());
        }
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        *slot = Some(flag.clone());
        Ok(Connecting(flag))
    }

    fn check_cancel(&self) -> Result<(), String> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) { Err(crate::jobs::CANCELLED.into()) } else { Ok(()) }
    }
}

impl Drop for Connecting {
    fn drop(&mut self) {
        *CONNECTING.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Stops a sign-in that's waiting for the browser.
pub fn cancel_connect() {
    if let Some(flag) = CONNECTING.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Signs in through the browser with Nextcloud's Login Flow v2 and stores the app password.
/// `open_browser` opens a URL in the person's browser. Only one sign-in runs at a time, and
/// `cancel_connect` stops it.
pub fn connect(log: &Log, project: &crate::project::Project, open_browser: impl Fn(&str) -> Result<(), String>) -> Result<String, String> {
    let s = server(project).ok_or("This project has no Nextcloud set up (upload.nextcloud_url in ue-dev-hub.toml).")?;
    let connecting = Connecting::claim()?;
    let mut cf = None;
    match probe(&s.url, None)? {
        Gate::Open => {}
        Gate::CloudflareAccess => {
            cf = Some(match cf_token(&s.url) {
                Some(t) => t,
                None => cf_login(log, &s.url, &|| connecting.check_cancel().is_err() || log.check_cancel().is_err())?,
            });
            if let Gate::OtherLogin(to) = probe(&s.url, cf.as_deref())? {
                return Err(blocked_message(&to));
            }
        }
        Gate::OtherLogin(to) => return Err(blocked_message(&to)),
    }

    let client = client()?;
    let with_cf = |r: reqwest::blocking::RequestBuilder| match &cf {
        Some(t) => r.header("cf-access-token", t),
        None => r,
    };
    let res = with_cf(client.post(format!("{}/index.php/login/v2", s.url))).send().map_err(|e| format!("Couldn't reach Nextcloud: {}", e.without_url()))?;
    if let Gate::OtherLogin(to) = gate_of(&res) {
        return Err(blocked_message(&to));
    }
    if !res.status().is_success() {
        return Err(format!("Nextcloud didn't start the sign-in (error {}).", res.status().as_u16()));
    }
    let start: LoginStart = res.json().map_err(|e| format!("Nextcloud's answer wasn't understood: {e}"))?;
    for u in [&start.login, &start.poll.endpoint] {
        if !u.to_ascii_lowercase().starts_with("https://") || host_of(u) != host_of(&s.url) {
            return Err(format!("Nextcloud sent a sign-in address on another server ({}), so the hub stopped. Check that upload.nextcloud_url in ue-dev-hub.toml is the right address.", host_of(u)));
        }
    }
    log.info("Opening the browser to sign in to Nextcloud ...");
    open_browser(&start.login)?;

    // Wait for the person to approve the hub in the browser.
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    let result: LoginResult = loop {
        if std::time::Instant::now() > deadline {
            return Err("The Nextcloud sign-in wasn't finished within 10 minutes. Try again.".into());
        }
        log.check_cancel()?;
        for _ in 0..8 {
            connecting.check_cancel()?;
            std::thread::sleep(Duration::from_millis(250));
        }
        connecting.check_cancel()?;
        let res = with_cf(client.post(&start.poll.endpoint))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!("token={}", enc(&start.poll.token)))
            .send();
        match res {
            Ok(r) if r.status().is_success() => break r.json().map_err(|e| format!("Nextcloud's answer wasn't understood: {e}"))?,
            Ok(r) if r.status().as_u16() == 404 => continue,
            Ok(r) => {
                if let Gate::OtherLogin(to) = gate_of(&r) {
                    return Err(blocked_message(&to));
                }
                continue;
            }
            Err(_) => continue,
        }
    };
    let mut creds = Credentials { login_name: result.login_name.clone(), app_password: result.app_password, user_id: String::new() };
    creds.user_id = user_id(&client, &s.url, cf.as_deref(), &creds).unwrap_or_default();
    save_credentials(&s.url, &creds)?;
    Ok(result.login_name)
}

/// The account's user id from the OCS API.
fn user_id(client: &reqwest::blocking::Client, url: &str, cf: Option<&str>, creds: &Credentials) -> Option<String> {
    let mut r = client
        .get(format!("{url}/ocs/v2.php/cloud/user?format=json"))
        .basic_auth(&creds.login_name, Some(&creds.app_password))
        .header("OCS-APIRequest", "true");
    if let Some(t) = cf {
        r = r.header("cf-access-token", t);
    }
    let v: serde_json::Value = r.send().ok()?.json().ok()?;
    v["ocs"]["data"]["id"].as_str().map(str::to_string).filter(|s| !s.is_empty())
}

/// Forgets the login on this computer and asks Nextcloud to revoke the app password.
pub fn disconnect(project: &crate::project::Project) -> Result<(), String> {
    let s = server(project).ok_or("This project has no Nextcloud set up.")?;
    if let Some(creds) = load_credentials(&s.url) {
        let session = Session { url: s.url.clone(), cf: cf_token(&s.url), creds, client: client()? };
        // Best effort: the login is forgotten here either way.
        let _ = session.req("DELETE", &format!("{}/ocs/v2.php/core/apppassword", session.url)).header("OCS-APIRequest", "true").send();
    }
    delete_credentials(&s.url);
    Ok(())
}

// ---------- Uploading ----------

/// Upload ids per file, so an interrupted upload continues instead of starting over.
fn state_file(dir: &Path) -> PathBuf {
    dir.join("uploads.json")
}

fn upload_key(file: &Path) -> String {
    let meta = std::fs::metadata(file).ok();
    let size = meta.as_ref().map_or(0, |m| m.len());
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    format!("{}|{size}|{mtime}", file.display())
}

fn load_state(dir: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(state_file(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_state(dir: &Path, s: &HashMap<String, String>) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(state_file(dir), serde_json::to_string_pretty(s).unwrap_or_default());
}

/// Chunks already on the server for this upload: name -> size.
fn existing_chunks(session: &Session, upload_url: &str) -> HashMap<String, u64> {
    let body = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:getcontentlength/></d:prop></d:propfind>"#;
    let Ok(res) = session.req("PROPFIND", upload_url).header("Depth", "1").header("Content-Type", "application/xml").body(body).send() else {
        return HashMap::new();
    };
    if res.status().as_u16() != 207 {
        return HashMap::new();
    }
    parse_chunks(&res.text().unwrap_or_default())
}

/// Chunk name -> size from a PROPFIND answer. Each <d:response> has an href ending in the
/// chunk's name, and its length. Namespace prefixes are stripped first.
fn parse_chunks(xml: &str) -> HashMap<String, u64> {
    let xml = strip_xml_prefixes(xml);
    let xml = xml.as_str();
    let mut out = HashMap::new();
    for part in xml.split("<response>").skip(1) {
        let href = part.split("<href>").nth(1).and_then(|h| h.split("</href>").next()).unwrap_or("");
        let name = href.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
        let len = part
            .split("<getcontentlength>")
            .nth(1)
            .and_then(|l| l.split("</getcontentlength>").next())
            .and_then(|l| l.trim().parse::<u64>().ok());
        if let Some(len) = len
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_digit())
        {
            out.insert(name, len);
        }
    }
    out
}

/// `<d:href>` and `<D:href>` both become `<href>`: servers pick their own namespace prefixes.
fn strip_xml_prefixes(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        let (slash, after) = match rest.strip_prefix('/') {
            Some(r) => ("/", r),
            None => ("", rest),
        };
        let name_end = after.find(|c: char| c == '>' || c.is_whitespace() || c == '/').unwrap_or(after.len());
        let name = &after[..name_end];
        let local = name.rsplit(':').next().unwrap_or(name);
        out.push('<');
        out.push_str(slash);
        out.push_str(local);
        rest = &after[name_end..];
    }
    out.push_str(rest);
    out
}

/// The size of a file on Nextcloud, if it exists.
fn remote_size(session: &Session, url: &str) -> Option<u64> {
    let body = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:getcontentlength/></d:prop></d:propfind>"#;
    let res = session.req("PROPFIND", url).header("Depth", "0").header("Content-Type", "application/xml").body(body).send().ok()?;
    if res.status().as_u16() != 207 {
        return None;
    }
    let xml = strip_xml_prefixes(&res.text().ok()?);
    xml.split("<getcontentlength>").nth(1)?.split("</getcontentlength>").next()?.trim().parse().ok()
}

/// Creates every folder of `path` under the person's files, ignoring ones that exist.
fn ensure_folders(session: &Session, path: &str) -> Result<(), String> {
    let base = format!("{}/remote.php/dav/files/{}", session.url, enc(session.creds.dav_user()));
    // The top folder is usually shared with the team. Creating it here would put builds in the
    // person's private files, where nobody else sees them.
    if let Some(top) = path.split('/').find(|p| !p.is_empty()) {
        let res = session.req("PROPFIND", &format!("{base}/{}", enc(top))).header("Depth", "0").send().map_err(|e| format!("Couldn't reach Nextcloud: {}", e.without_url()))?;
        if res.status().as_u16() == 404 {
            return Err(format!(
                "The folder '{top}' isn't in your Nextcloud files. If it's shared with the team, accept the share in Nextcloud first, then try again."
            ));
        }
        session.check(res, "Checking the upload folder")?;
    }
    let mut cur = String::new();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        cur.push('/');
        cur.push_str(&enc(part));
        let res = session.req("MKCOL", &format!("{base}{cur}")).send().map_err(|e| format!("Couldn't reach Nextcloud: {}", e.without_url()))?;
        // 201 created, 405 already exists.
        if ![201, 405].contains(&res.status().as_u16()) {
            session.check(res, "Creating the folder on Nextcloud")?;
        }
    }
    Ok(())
}

/// Uploads `file` into the project's Nextcloud folder, under `subfolder`. Continues an earlier,
/// interrupted upload of the same file. Returns the path on Nextcloud.
/// A finished upload: where the file is on Nextcloud, and how it went.
#[derive(Debug)]
pub struct Uploaded {
    pub path: String,
    /// Size and average speed, like "540 MB at 23.4 MB/s".
    pub summary: String,
}

impl Uploaded {
    /// For a step's note: the summary first, so a cut-off path doesn't hide it.
    pub fn note(&self) -> String {
        format!("{} \u{2192} {}", self.summary, self.path)
    }
}

/// Upload progress, measured as the connection takes the bytes. Speed is taken over the last few
/// seconds, so it follows the connection instead of averaging the whole upload.
struct Meter {
    log: Log,
    size: u64,
    /// Bytes of the file that are on the server or on their way.
    position: u64,
    /// Bytes sent in this run, retries included.
    sent: u64,
    started: Instant,
    samples: VecDeque<(Instant, u64)>,
    last_report: Option<Instant>,
}

const SPEED_WINDOW: Duration = Duration::from_secs(5);

impl Meter {
    fn new(log: &Log, size: u64, position: u64) -> Self {
        Self { log: log.clone(), size, position, sent: 0, started: Instant::now(), samples: VecDeque::new(), last_report: None }
    }

    fn advance(&mut self, n: u64) {
        self.position = (self.position + n).min(self.size);
        self.sent += n;
        let now = Instant::now();
        if self.last_report.is_none_or(|t| now - t >= Duration::from_millis(250)) {
            self.report(now);
        }
    }

    /// Bytes per second over the last few seconds, once there's enough to tell.
    fn speed(&mut self, now: Instant) -> Option<f64> {
        self.samples.push_back((now, self.sent));
        while self.samples.len() > 2 && now - self.samples[1].0 >= SPEED_WINDOW {
            self.samples.pop_front();
        }
        let (t0, b0) = *self.samples.front()?;
        let secs = (now - t0).as_secs_f64();
        (secs >= 0.5).then(|| (self.sent - b0) as f64 / secs)
    }

    fn report(&mut self, now: Instant) {
        self.last_report = Some(now);
        let speed = self.speed(now);
        let mut label = format!("{} of {}", mb(self.position), mb(self.size));
        if let Some(bps) = speed {
            label += &format!("  \u{b7}  {:.1} MB/s", bps / 1e6);
            if bps > 0.0 && self.position < self.size {
                let left = (self.size - self.position) as f64 / bps;
                label += &format!("  \u{b7}  about {} left", if left > 90.0 { format!("{:.0} min", left / 60.0) } else { format!("{left:.0} s") });
            }
        }
        self.log.progress(Progress { percent: Some(self.position as f32 / self.size.max(1) as f32 * 100.0), label });
    }

    fn summary(&self) -> String {
        let secs = self.started.elapsed().as_secs_f64();
        if self.sent == 0 || secs < 0.1 {
            return mb(self.size);
        }
        format!("{} at {:.1} MB/s", mb(self.size), self.sent as f64 / secs / 1e6)
    }
}

fn mb(bytes: u64) -> String {
    if bytes >= 10_000_000_000 { format!("{:.1} GB", bytes as f64 / 1e9) } else { format!("{:.0} MB", bytes as f64 / 1e6) }
}

/// A piece of the file as a request body that counts bytes as the connection takes them.
struct Counting {
    data: std::io::Cursor<Vec<u8>>,
    meter: Arc<Mutex<Meter>>,
}

impl Read for Counting {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Small reads, so progress moves smoothly instead of in big jumps.
        let max = buf.len().min(256 * 1024);
        let n = self.data.read(&mut buf[..max])?;
        if n > 0
            && let Ok(mut m) = self.meter.lock()
        {
            m.advance(n as u64);
        }
        Ok(n)
    }
}

pub fn upload(log: &Log, project: &crate::project::Project, file: &Path, subfolder: &str, state_dir: &Path) -> Result<Uploaded, String> {
    let s = server(project).ok_or("This project has no Nextcloud set up (upload.nextcloud_url in ue-dev-hub.toml).")?;
    let creds = load_credentials(&s.url).ok_or("Not connected to Nextcloud yet. Connect in Settings first.")?;
    let cf = match probe(&s.url, None)? {
        Gate::CloudflareAccess => Some(cf_token(&s.url).ok_or("Your Cloudflare Access sign-in expired. Connect to Nextcloud again in Settings.")?),
        Gate::OtherLogin(to) => return Err(blocked_message(&to)),
        Gate::Open => None,
    };
    let mut session = Session { url: s.url.clone(), cf, creds, client: client()? };
    upload_with(log, &mut session, &s.folder, file, subfolder, state_dir, CHUNK)
}

fn upload_with(log: &Log, session: &mut Session, base_folder: &str, file: &Path, subfolder: &str, state_dir: &Path, chunk: u64) -> Result<Uploaded, String> {
    let s = Server { url: session.url.clone(), folder: base_folder.to_string() };
    let name = file.file_name().ok_or("That isn't a file.")?.to_string_lossy().to_string();
    let size = std::fs::metadata(file).map_err(|e| format!("Couldn't read {}: {e}", file.display()))?.len();
    let folder = [s.folder.as_str(), subfolder].iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join("/");
    ensure_folders(session, &folder)?;
    let user = enc(session.creds.dav_user());
    let destination = format!("{}/remote.php/dav/files/{user}/{}/{}", s.url, enc_path(&folder), enc(&name));

    let key = upload_key(file);
    let mut state = load_state(state_dir);
    let id = state.entry(key.clone()).or_insert_with(|| format!("ue-dev-hub-{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"))).clone();
    save_state(state_dir, &state);
    let upload_url = format!("{}/remote.php/dav/uploads/{user}/{id}", s.url);

    let res = session
        .req("MKCOL", &upload_url)
        .header("Destination", &destination)
        .send()
        .map_err(|e| format!("Couldn't reach Nextcloud: {}", e.without_url()))?;
    if ![201, 405].contains(&res.status().as_u16()) {
        session.check(res, "Starting the upload")?;
    }
    let done = existing_chunks(session, &upload_url);
    if !done.is_empty() {
        log.info(format!("Continuing an earlier upload: {} piece(s) are already on Nextcloud.", done.len()));
    }

    let chunks = size.div_ceil(chunk).max(1);
    let piece_len = |i: u64| chunk.min(size - i * chunk);
    let already: u64 = (0..chunks).filter(|&i| done.get(&format!("{:05}", i + 1)) == Some(&piece_len(i))).map(piece_len).sum();
    let mut f = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let meter = Arc::new(Mutex::new(Meter::new(log, size, already)));
    meter.lock().unwrap().report(Instant::now());
    for i in 0..chunks {
        log.check_cancel()?;
        let offset = i * chunk;
        let len = chunk.min(size - offset);
        // Nextcloud numbers chunks from 1; zero-padded names keep them in order.
        let chunk_name = format!("{:05}", i + 1);
        if done.get(&chunk_name) == Some(&len) {
            continue;
        }
        f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf).map_err(|e| e.to_string())?;
        let before = meter.lock().unwrap().position;

        // Waits between tries add up to about 5 minutes, enough for a server restart.
        const WAITS: [u64; 8] = [2, 4, 8, 15, 30, 60, 90, 90];
        let mut attempt = 0;
        loop {
            log.check_cancel()?;
            let res = session
                .req("PUT", &format!("{upload_url}/{chunk_name}"))
                .header("Destination", &destination)
                .header("OC-Total-Length", size.to_string())
                .body(reqwest::blocking::Body::sized(Counting { data: std::io::Cursor::new(buf.clone()), meter: meter.clone() }, len))
                .send();
            let retry = match res {
                Ok(r) if r.status().is_success() => break,
                Ok(r) if gate_of(&r) == Gate::CloudflareAccess && session.cf.is_some() => {
                    // The Cloudflare sign-in expired during a long upload: get a fresh token.
                    match cf_token(&session.url) {
                        Some(t) if attempt < WAITS.len() => {
                            session.cf = Some(t);
                            true
                        }
                        _ => return session.check(r, "Uploading").map(|_| unreachable!()),
                    }
                }
                Ok(r) if [401, 403, 507].contains(&r.status().as_u16()) || attempt >= WAITS.len() => {
                    if r.status().is_server_error() || r.status().as_u16() == 429 {
                        return Err(format!("Nextcloud kept failing (error {}). Start the upload again later; it continues where it stopped.", r.status().as_u16()));
                    }
                    session.check(r, "Uploading")?;
                    break;
                }
                Ok(_) => true,
                Err(e) if attempt >= WAITS.len() => return Err(format!("The upload stopped: {}. Start it again to continue where it stopped.", e.without_url())),
                Err(_) => true,
            };
            // What the failed try sent of this piece isn't on the server.
            meter.lock().unwrap().position = before;
            if retry {
                let wait = WAITS[attempt.min(WAITS.len() - 1)];
                attempt += 1;
                log.info(format!("Piece {} of {chunks} didn't go through, trying again in {wait} s ({attempt}/{}) ...", i + 1, WAITS.len()));
                std::thread::sleep(if cfg!(test) { Duration::from_millis(20) } else { Duration::from_secs(wait) });
            }
        }
        let mut m = meter.lock().unwrap();
        m.position = before + len;
        m.report(Instant::now());
    }
    let summary = meter.lock().unwrap().summary();
    log.progress(Progress { percent: None, label: "Nextcloud is putting the pieces together".into() });

    log.info("Putting the pieces together on Nextcloud ...");
    let mut last_error = String::new();
    let mut finished = false;
    for attempt in 0..3 {
        let res = session
            .req("MOVE", &format!("{upload_url}/.file"))
            .header("Destination", &destination)
            .header("OC-Total-Length", size.to_string())
            .header("Overwrite", "T")
            .send();
        match res.map_err(|e| e.without_url().to_string()).and_then(|r| session.check(r, "Finishing the upload").map(|_| ())) {
            Ok(()) => {
                finished = true;
                break;
            }
            Err(e) => last_error = e,
        }
        // A proxy may have given up while Nextcloud kept assembling: check whether it's there.
        for _ in 0..12 {
            if remote_size(session, &destination) == Some(size) {
                finished = true;
                break;
            }
            std::thread::sleep(if cfg!(test) { Duration::from_millis(10) } else { Duration::from_secs(10) });
        }
        if finished {
            break;
        }
        log.info(format!("Finishing the upload didn't work yet, trying again ({}/3) ...", attempt + 1));
    }
    if !finished {
        return Err(last_error);
    }

    state.remove(&key);
    save_state(state_dir, &state);
    Ok(Uploaded { path: format!("{folder}/{name}"), summary })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_sign_in_runs_and_it_can_be_cancelled() {
        let first = Connecting::claim().unwrap();
        assert!(Connecting::claim().unwrap_err().contains("already in progress"));
        assert!(first.check_cancel().is_ok());
        cancel_connect();
        assert_eq!(first.check_cancel().unwrap_err(), crate::jobs::CANCELLED);
        drop(first);
        let again = Connecting::claim().unwrap();
        assert!(again.check_cancel().is_ok());
    }

    #[test]
    fn measures_speed_over_the_last_seconds() {
        let mut m = Meter::new(&Log::none(), 100_000_000, 0);
        let t = Instant::now();
        assert_eq!(m.speed(t), None, "nothing to tell yet");
        // 10 MB/s for 10 seconds, then 2 MB/s: the speed follows the recent rate.
        for s in 1..=10 {
            m.sent += 10_000_000;
            m.speed(t + Duration::from_secs(s));
        }
        for s in 11..=20 {
            m.sent += 2_000_000;
            m.speed(t + Duration::from_secs(s));
        }
        let bps = m.speed(t + Duration::from_secs(20)).unwrap();
        assert!((1.9e6..2.6e6).contains(&bps), "{bps}");
        assert_eq!(mb(540_000_000), "540 MB");
        assert_eq!(mb(12_300_000_000), "12.3 GB");
    }

    #[test]
    fn fills_in_the_year_and_month() {
        let d = chrono::NaiveDate::from_ymd_opt(2026, 3, 9).unwrap();
        assert_eq!(super::expand_folder("Team/Builds/{year}/{month}", d), "Team/Builds/2026/03");
        assert_eq!(super::expand_folder("Team/Builds", d), "Team/Builds");
    }

    #[test]
    fn encodes_paths_for_webdav() {
        assert_eq!(enc_path("/Starfall/Builds/"), "Starfall/Builds");
        assert_eq!(enc("InternalOnly-Starfall 26.11.10.zip"), "InternalOnly-Starfall%2026.11.10.zip");
        assert_eq!(host_of("https://Cloud.Example.com/nextcloud"), "cloud.example.com");
    }

    #[test]
    fn reads_chunk_listings() {
        let xml = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">
<d:response><d:href>/remote.php/dav/uploads/jo/ue-dev-hub-1/</d:href><d:propstat><d:prop/></d:propstat></d:response>
<d:response><d:href>/remote.php/dav/uploads/jo/ue-dev-hub-1/00001</d:href><d:propstat><d:prop><d:getcontentlength>52428800</d:getcontentlength></d:prop></d:propstat></d:response>
<d:response><d:href>/remote.php/dav/uploads/jo/ue-dev-hub-1/00002</d:href><d:propstat><d:prop><d:getcontentlength>1000</d:getcontentlength></d:prop></d:propstat></d:response>
</d:multistatus>"#;
        let out = parse_chunks(xml);
        assert_eq!(parse_chunks(&xml.replace("d:", "D:")).len(), 2, "any namespace prefix works");
        assert_eq!(out.get("00001"), Some(&CHUNK));
        assert_eq!(out.get("00002"), Some(&1000));
        assert_eq!(out.len(), 2);
    }

    use std::io::{BufRead, BufReader, Write};
    use std::sync::{Arc, Mutex};

    /// A tiny WebDAV server that stores chunks and assembles them on MOVE, like Nextcloud's
    /// chunked upload v2. `fail_puts` lists PUTs (counted from 1) that drop the connection.
    #[derive(Default)]
    struct Fake {
        chunks: HashMap<String, Vec<u8>>,
        files: HashMap<String, Vec<u8>>,
        puts: usize,
        fail_puts: Vec<usize>,
        log: Vec<String>,
    }

    fn serve(fake: Arc<Mutex<Fake>>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let fake = fake.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut out = stream;
                    loop {
                        let mut first = String::new();
                        if reader.read_line(&mut first).unwrap_or(0) == 0 {
                            return;
                        }
                        let mut parts = first.split_whitespace();
                        let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
                        let mut len = 0usize;
                        let mut auth = false;
                        loop {
                            let mut h = String::new();
                            reader.read_line(&mut h).unwrap();
                            let lower = h.to_ascii_lowercase();
                            if let Some(v) = lower.strip_prefix("content-length:") {
                                len = v.trim().parse().unwrap();
                            }
                            if lower.starts_with("authorization: basic") {
                                auth = true;
                            }
                            if h == "\r\n" {
                                break;
                            }
                        }
                        let mut body = vec![0u8; len];
                        reader.read_exact(&mut body).unwrap();
                        let mut f = fake.lock().unwrap();
                        f.log.push(format!("{method} {path}"));
                        let (status, text): (u16, String) = if !auth {
                            (401, String::new())
                        } else {
                            match method.as_str() {
                                "MKCOL" => (201, String::new()),
                                "PROPFIND" => {
                                    let mut xml = String::from("<d:multistatus>");
                                    for (k, v) in f.chunks.iter().filter(|(k, _)| k.starts_with(&path)) {
                                        xml += &format!("<d:response><d:href>{k}</d:href><d:getcontentlength>{}</d:getcontentlength></d:response>", v.len());
                                    }
                                    (207, xml + "</d:multistatus>")
                                }
                                "PUT" => {
                                    f.puts += 1;
                                    if f.fail_puts.contains(&f.puts) {
                                        // Simulate the connection dropping mid-upload.
                                        return;
                                    }
                                    f.chunks.insert(path.clone(), body);
                                    (201, String::new())
                                }
                                "MOVE" => {
                                    let dir = path.trim_end_matches("/.file").to_string();
                                    let mut names: Vec<String> = f.chunks.keys().filter(|k| k.starts_with(&format!("{dir}/"))).cloned().collect();
                                    names.sort();
                                    let mut whole = vec![];
                                    for n in &names {
                                        whole.extend(f.chunks.remove(n).unwrap());
                                    }
                                    f.files.insert(dir, whole);
                                    (201, String::new())
                                }
                                _ => (405, String::new()),
                            }
                        };
                        drop(f);
                        let _ = write!(out, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n\r\n{text}", text.len());
                    }
                });
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn uploads_in_pieces_and_continues_after_an_interruption() {
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-nc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Game Build.zip");
        let data: Vec<u8> = (0..2500u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&file, &data).unwrap();

        // 3 pieces of 1000 bytes. The 2nd piece's first try and all 8 retries fail.
        let fake = Arc::new(Mutex::new(Fake { fail_puts: (2..=10).collect(), ..Default::default() }));
        let mut session = Session {
            url: serve(fake.clone()),
            cf: None,
            creds: Credentials { login_name: "jo".into(), app_password: "secret".into(), user_id: String::new() },
            client: reqwest::blocking::Client::builder().timeout(Duration::from_secs(5)).build().unwrap(),
        };
        let log = Log::default();
        let err = upload_with(&log, &mut session, "Starfall/Builds", &file, "", &dir, 1000).unwrap_err();
        assert!(err.contains("continue where it stopped"), "{err}");
        assert!(fake.lock().unwrap().files.is_empty());
        assert_eq!(load_state(&dir).len(), 1, "the upload id is kept for the next try");

        // The next try sends only pieces 2 and 3, then assembles the file.
        upload_with(&log, &mut session, "Starfall/Builds", &file, "", &dir, 1000).unwrap();
        let f = fake.lock().unwrap();
        assert_eq!(f.puts, 12, "10 tries in the first run, then only pieces 2 and 3");
        assert_eq!(f.files.len(), 1);
        assert_eq!(f.files.values().next().unwrap(), &data);
        assert!(f.log.iter().any(|l| l.starts_with("MKCOL /remote.php/dav/files/jo/Starfall")));
        drop(f);
        assert!(load_state(&dir).is_empty(), "forgotten once complete");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rides_out_a_short_outage() {
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-nc-outage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("c.zip");
        let data: Vec<u8> = (0..1500u32).map(|i| (i % 7) as u8).collect();
        std::fs::write(&file, &data).unwrap();
        // The first piece fails 4 times, then the server is back.
        let fake = Arc::new(Mutex::new(Fake { fail_puts: vec![1, 2, 3, 4], ..Default::default() }));
        let mut session = Session {
            url: serve(fake.clone()),
            cf: None,
            creds: Credentials { login_name: "jo".into(), app_password: "secret".into(), user_id: "jo-id".into() },
            client: reqwest::blocking::Client::builder().timeout(Duration::from_secs(5)).build().unwrap(),
        };
        upload_with(&Log::default(), &mut session, "Builds", &file, "", &dir, 1000).unwrap();
        let f = fake.lock().unwrap();
        assert_eq!(f.files.values().next().unwrap(), &data);
        // Paths use the user id, not the login name.
        assert!(f.log.iter().any(|l| l.contains("/remote.php/dav/uploads/jo-id/")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resumes_with_the_pieces_already_uploaded() {
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-nc-resume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("b.zip");
        let data: Vec<u8> = (0..3000u32).map(|i| (i % 253) as u8).collect();
        std::fs::write(&file, &data).unwrap();

        // An earlier run uploaded pieces 1 and 2 under a known id, then stopped.
        let id = "ue-dev-hub-earlier";
        let mut state = HashMap::new();
        state.insert(upload_key(&file), id.to_string());
        save_state(&dir, &state);
        let fake = Arc::new(Mutex::new(Fake::default()));
        fake.lock().unwrap().chunks.insert(format!("/remote.php/dav/uploads/jo/{id}/00001"), data[..1000].to_vec());
        fake.lock().unwrap().chunks.insert(format!("/remote.php/dav/uploads/jo/{id}/00002"), data[1000..2000].to_vec());
        let url = serve(fake.clone());
        let mut session = Session {
            url,
            cf: None,
            creds: Credentials { login_name: "jo".into(), app_password: "secret".into(), user_id: String::new() },
            client: reqwest::blocking::Client::builder().timeout(Duration::from_secs(5)).build().unwrap(),
        };
        upload_with(&Log::default(), &mut session, "Builds", &file, "", &dir, 1000).unwrap();
        let f = fake.lock().unwrap();
        // Only the missing third piece was sent again.
        assert_eq!(f.puts, 1);
        assert_eq!(f.files.values().next().unwrap(), &data);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
