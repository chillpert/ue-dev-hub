//! "Report a problem": posts the report with its logs and screenshots to the team's Discord
//! channel through a webhook, or gives it as Markdown to paste into a task tracker.

use crate::build;
use crate::proc;
use crate::project::Project;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub path: PathBuf,
    pub exists: bool,
    /// Like "Today, 12:31".
    pub modified: Option<String>,
    /// For the build log: whether the last compile failed.
    pub failed: Option<bool>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub branch: String,
    pub commit: String,
    pub commit_subject: String,
    pub commit_url: Option<String>,
    pub engine_log: Attachment,
    pub build_log: Attachment,
    /// The hub's own log of this run.
    pub hub_log: Attachment,
    /// Git's user.name, shown as who reported it.
    pub reporter: String,
    pub has_webhook: bool,
    pub max_upload_mb: u64,
}

/// The web address of the repository, like https://github.com/owner/repo, from origin's URL.
pub fn web_url(project: &Project) -> Option<String> {
    let remote = project.git(&["remote", "get-url", "origin"])?;
    let remote = remote.trim().trim_end_matches(".git").trim_end_matches('/');
    let url = if let Some(rest) = remote.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        format!("https://{host}/{path}")
    } else if let Some(rest) = remote.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/')?;
        let host = host.split(':').next()?;
        format!("https://{host}/{path}")
    } else if remote.starts_with("https://") || remote.starts_with("http://") {
        // Drop credentials like https://user:token@host.
        let (scheme, rest) = remote.split_once("://")?;
        let rest = rest.split_once('@').map_or(rest, |(_, r)| r);
        format!("{scheme}://{rest}")
    } else {
        return None;
    };
    Some(url)
}

pub fn friendly_time(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let t: chrono::DateTime<chrono::Local> = modified.into();
    let today = chrono::Local::now().date_naive();
    Some(if t.date_naive() == today {
        format!("Today, {}", t.format("%H:%M"))
    } else if Some(t.date_naive()) == today.pred_opt() {
        format!("Yesterday, {}", t.format("%H:%M"))
    } else {
        t.format("%b %-d, %H:%M").to_string()
    })
}

fn attachment(path: PathBuf) -> Attachment {
    Attachment { exists: path.is_file(), modified: friendly_time(&path), failed: None, path }
}

pub fn engine_log_path(project: &Project) -> PathBuf {
    project.logs_dir().join(format!("{}.log", project.name))
}

/// Whether the last compile failed, read from the end of the build log.
pub fn last_build_failed(path: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(path).ok()?;
    let tail: Vec<&str> = text.lines().rev().take(40).collect();
    if tail.iter().any(|l| l.contains("Result: Succeeded") || l.contains("Target is up to date")) {
        Some(false)
    } else if tail.iter().any(|l| l.contains("Result: Failed") || l.contains("error")) {
        Some(true)
    } else {
        None
    }
}

pub fn context(project: &Project) -> Context {
    let commit = project.git(&["rev-parse", "HEAD"]).unwrap_or_default();
    let mut build_log = attachment(build::build_log_path(project));
    build_log.failed = last_build_failed(&build_log.path);
    Context {
        branch: project.current_branch().unwrap_or_default(),
        commit_subject: project.git(&["log", "-1", "--format=%s"]).unwrap_or_default(),
        commit_url: web_url(project).filter(|_| !commit.is_empty()).map(|u| format!("{u}/commit/{commit}")),
        commit: commit.chars().take(9).collect(),
        engine_log: attachment(engine_log_path(project)),
        build_log,
        hub_log: attachment(crate::log::hub_log_path().unwrap_or_default()),
        reporter: project.git(&["config", "user.name"]).unwrap_or_default(),
        has_webhook: project.config.report.discord_webhook.is_some(),
        max_upload_mb: project.config.report.max_upload_mb,
    }
}

/// What the person filled in on the Report screen.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReportInput {
    pub title: String,
    pub steps: String,
    pub happened: String,
    pub expected: String,
    pub include_commit: bool,
    /// The Setup screen's checks as text, or empty to leave it out.
    pub setup: String,
    pub logs: Vec<PathBuf>,
    /// Screenshots and videos.
    pub media: Vec<PathBuf>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    /// Files left out because they were too big for Discord.
    pub skipped: Vec<String>,
    /// Logs that were cut to their last part to fit.
    pub trimmed: Vec<String>,
}

const MB: u64 = 1024 * 1024;
/// Room for the message itself, so files plus text stay under the limit.
const PAYLOAD_SLACK: u64 = 256 * 1024;

fn cut(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The Discord message: one embed with the report, plus the attachments' names.
pub fn payload(project: &Project, ctx: &Context, input: &Report, files: &[String]) -> serde_json::Value {
    let mut fields: Vec<serde_json::Value> = vec![];
    let mut field = |name: &str, value: &str, max: usize| {
        if !value.trim().is_empty() {
            fields.push(serde_json::json!({ "name": name, "value": cut(value, max) }));
        }
    };
    field("Steps to reproduce", &input.input.steps, 1024);
    field("What should happen", &input.input.expected, 1024);
    if input.input.include_commit {
        let commit = match &ctx.commit_url {
            Some(url) => format!("`{}` @ [{}]({url}) {}", ctx.branch, ctx.commit, ctx.commit_subject),
            None => format!("`{}` @ {} {}", ctx.branch, ctx.commit, ctx.commit_subject),
        };
        field("Branch and commit", &commit, 1024);
    }
    field("Setup", &input.input.setup, 1000);
    if !input.sent.skipped.is_empty() {
        field("Too big to attach", &input.sent.skipped.join("\n"), 1024);
    }
    // Discord refuses embeds over 6000 characters in total; shorten the longest parts until it fits.
    let fixed = 250 + 200 + fields.iter().map(|f| f["name"].as_str().unwrap_or("").len()).sum::<usize>();
    let mut description = cut(&input.input.happened, 2500);
    let total = |d: &str, fs: &[serde_json::Value]| fixed + d.chars().count() + fs.iter().map(|f| f["value"].as_str().unwrap_or("").chars().count()).sum::<usize>();
    while total(&description, &fields) > 5800 {
        let longest = fields.iter().enumerate().max_by_key(|(_, f)| f["value"].as_str().unwrap_or("").chars().count()).map(|(i, f)| (i, f["value"].as_str().unwrap_or("").chars().count()));
        match longest {
            Some((i, n)) if n > description.chars().count() && n > 100 => {
                let v = cut(fields[i]["value"].as_str().unwrap_or(""), n / 2);
                fields[i]["value"] = serde_json::Value::String(v);
            }
            _ if description.chars().count() > 100 => description = cut(&description, description.chars().count() / 2),
            _ => break,
        }
    }
    let reporter = if ctx.reporter.is_empty() { "someone".to_string() } else { ctx.reporter.clone() };
    let attachments: Vec<_> = files.iter().enumerate().map(|(i, f)| serde_json::json!({ "id": i, "filename": f })).collect();
    serde_json::json!({
        "username": "UE Dev Hub",
        // Never ping anyone from text people typed.
        "allowed_mentions": { "parse": [] },
        "embeds": [{
            "title": cut(if input.input.title.trim().is_empty() { "Problem report" } else { &input.input.title }, 250),
            "description": description,
            "color": 0xE5484D,
            "fields": fields,
            "footer": { "text": cut(&format!("{} · reported by {reporter}", project.display_name()), 200) },
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }],
        "attachments": attachments,
    })
}

/// A report with what happened to its files.
pub struct Report {
    pub input: ReportInput,
    pub sent: Sent,
}

/// Copies the last `max` bytes of `src` to `dst`. Returns whether it had to cut.
fn tail_copy(src: &Path, dst: &Path, max: u64) -> Result<bool, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(src).map_err(|e| format!("Couldn't read {}: {e}", src.display()))?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let cut = len > max;
    if cut {
        f.seek(SeekFrom::Start(len - max)).map_err(|e| e.to_string())?;
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if cut {
        // Start at a full line.
        if let Some(nl) = buf.iter().position(|b| *b == b'\n') {
            buf.drain(..=nl);
        }
        let mut head = b"[UE Dev Hub: the start of this log was cut to fit Discord's size limit]\n".to_vec();
        head.extend(buf);
        buf = head;
    }
    std::fs::write(dst, buf).map_err(|e| e.to_string())?;
    Ok(cut)
}

/// Zips `files` (inside `dir`) to `dir/zip_name`.
fn zip(dir: &Path, files: &[String], zip_name: &str) -> Result<PathBuf, String> {
    let out = dir.join(zip_name);
    let _ = std::fs::remove_file(&out);
    let items: Vec<&str> = files.iter().map(String::as_str).collect();
    let r = proc::zip_command(dir, &out, &items).output().map_err(|e| format!("Couldn't zip the logs: {e}"))?;
    if r.status.success() && out.is_file() { Ok(out) } else { Err(format!("Couldn't zip the logs: {}", String::from_utf8_lossy(&r.stderr))) }
}

fn size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Packs the logs into one zip and picks which media fit under `limit` bytes in total.
/// Logs that are too big keep only their end, where crashes and errors are.
fn prepare_files(dir: &Path, input: &ReportInput, limit: u64) -> Result<(Vec<PathBuf>, Sent), String> {
    let budget = limit.saturating_sub(PAYLOAD_SLACK);
    let mut sent = Sent { skipped: vec![], trimmed: vec![] };
    let mut files = vec![];
    let logs: Vec<&PathBuf> = input.logs.iter().filter(|p| p.is_file()).collect();

    if !logs.is_empty() {
        // Text logs compress about 10x; shrink the tails until the zip fits.
        let mut tail = 60 * MB;
        loop {
            let mut names = vec![];
            let mut trimmed = vec![];
            for (i, log) in logs.iter().enumerate() {
                let name = format!("{}-{}", i + 1, log.file_name().unwrap_or_default().to_string_lossy());
                if tail_copy(log, &dir.join(&name), tail)? {
                    trimmed.push(log.file_name().unwrap_or_default().to_string_lossy().to_string());
                }
                names.push(name);
            }
            let z = zip(dir, &names, "logs.zip")?;
            if size(&z) <= budget / 2 || tail <= MB / 4 {
                if size(&z) <= budget {
                    sent.trimmed = trimmed;
                    files.push(z);
                } else {
                    sent.skipped.push("logs.zip".into());
                }
                break;
            }
            tail /= 2;
        }
    }

    let mut used: u64 = files.iter().map(|f| size(f)).sum();
    for m in &input.media {
        let name = m.file_name().unwrap_or_default().to_string_lossy().to_string();
        let s = size(m);
        if !m.is_file() {
            sent.skipped.push(format!("{name} (not found)"));
        } else if used + s > budget || files.len() >= 10 {
            sent.skipped.push(format!("{name} ({:.1} MB)", s as f64 / MB as f64));
        } else {
            used += s;
            files.push(m.clone());
        }
    }
    Ok((files, sent))
}

/// The screenshots and videos a report may attach.
pub const MEDIA_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "mp4", "mov", "webm"];

/// Screenshots and videos the person picked in the hub's file dialog this session.
static PICKED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub fn remember_picked(files: &[PathBuf]) {
    let mut picked = PICKED.lock().unwrap_or_else(|e| e.into_inner());
    picked.extend(files.iter().filter_map(|f| f.canonicalize().ok()));
}

/// A report only sends the project's own logs, and screenshots or videos the person picked or
/// that are in the project, so a page can't make the hub post any file on the computer.
pub fn check_files(project: &Project, input: &ReportInput) -> Result<(), String> {
    let name = |p: &Path| p.file_name().unwrap_or(p.as_os_str()).to_string_lossy().to_string();
    for log in input.logs.iter().filter(|p| p.exists()) {
        let project_log = crate::inside_any(log, &[project.logs_dir()]).is_some_and(|p| p.is_file());
        if !project_log && !is_hub_log(log, crate::log::log_dir().as_deref()) {
            return Err(format!("{} isn't one of the project's logs, so it can't be attached.", name(log)));
        }
    }
    let picked = PICKED.lock().unwrap_or_else(|e| e.into_inner());
    for m in input.media.iter().filter(|p| p.exists()) {
        let is_media = m.extension().is_some_and(|e| MEDIA_EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()));
        let allowed = m.canonicalize().is_ok_and(|c| picked.contains(&c)) || crate::inside_any(m, std::slice::from_ref(&project.root)).is_some();
        if !is_media || !allowed {
            return Err(format!("{} can't be attached. Add screenshots and videos with the button on this screen.", name(m)));
        }
    }
    Ok(())
}

/// The hub's own logs, which a report may attach too.
const HUB_LOGS: &[&str] = &["hub.log", "hub.old.log", "crash.log"];

/// Whether `path` is one of the hub's own logs in its log folder `dir`.
fn is_hub_log(path: &Path, dir: Option<&Path>) -> bool {
    let Some(dir) = dir else { return false };
    let Ok(path) = path.canonicalize() else { return false };
    path.is_file() && HUB_LOGS.iter().any(|name| dir.join(name).canonicalize().is_ok_and(|l| l == path))
}

/// Posts the report to the project's Discord webhook.
pub fn send(project: &Project, input: ReportInput) -> Result<Sent, String> {
    check_files(project, &input)?;
    let url = project
        .config
        .report
        .discord_webhook
        .clone()
        .ok_or("This project has no Discord webhook yet. Add report.discord_webhook to ue-dev-hub.toml.")?;
    send_to(project, &url, input)
}

pub fn send_to(project: &Project, url: &str, input: ReportInput) -> Result<Sent, String> {
    let ctx = context(project);
    let limit = project.config.report.max_upload_mb * MB;
    // A private folder: the logs are copied there, and on Linux /tmp is shared.
    let tmp = crate::tempdir::TempDir::new("ue-dev-hub-report")?;
    let dir = tmp.path();
    (|| {
        let (files, sent) = prepare_files(dir, &input, limit)?;
        let names: Vec<String> = files.iter().map(|f| f.file_name().unwrap_or_default().to_string_lossy().to_string()).collect();
        let report = Report { input, sent };
        let json = payload(project, &ctx, &report, &names);

        let mut form = reqwest::blocking::multipart::Form::new().text("payload_json", json.to_string());
        for (i, f) in files.iter().enumerate() {
            let bytes = std::fs::read(f).map_err(|e| format!("Couldn't read {}: {e}", f.display()))?;
            let part = reqwest::blocking::multipart::Part::bytes(bytes).file_name(names[i].clone());
            form = form.part(format!("files[{i}]"), part);
        }
        let client = reqwest::blocking::Client::builder()
            // Tests post to a fake webhook on localhost.
            .https_only(!cfg!(test))
            .timeout(std::time::Duration::from_secs(180))
            .build()
            .map_err(|e| e.to_string())?;
        let res = client.post(url).multipart(form).send().map_err(|e| {
            if e.is_timeout() {
                "Sending took too long. Check your internet connection and try again.".to_string()
            } else {
                // Without the address: it contains the webhook's secret token.
                format!("Couldn't reach Discord. Check your internet connection and try again.\n\n{}", e.without_url())
            }
        })?;
        let status = res.status().as_u16();
        let body = String::from_utf8_lossy(&crate::selfupdate::read_capped(res, 64 * 1024).unwrap_or_default()).to_string();
        match status {
            200..=299 => Ok(report.sent),
            401 | 403 | 404 => Err("The Discord webhook in ue-dev-hub.toml doesn't work anymore. Make a new webhook in the Discord channel's settings (Integrations) and put it in report.discord_webhook.".into()),
            413 => Err(format!(
                "Discord says the files are too big for this channel. Remove a screenshot or video and try again. If the server allows bigger uploads, raise report.max_upload_mb (now {}).",
                project.config.report.max_upload_mb
            )),
            429 => Err("Discord is getting too many messages right now. Wait a minute and try again.".into()),
            _ => Err(format!("Discord didn't accept the report (error {status}).\n\n{}", cut(&body, 400))),
        }
    })()
}

/// The report as Markdown, for pasting into a task tracker or anywhere else.
pub fn markdown(ctx: &Context, input: &ReportInput) -> String {
    let section = |t: &str, b: &str| if b.trim().is_empty() { String::new() } else { format!("### {t}\n{}\n\n", b.trim()) };
    let mut s = format!("## {}\n\n", if input.title.trim().is_empty() { "Problem report" } else { input.title.trim() });
    s += &section("Steps to reproduce", &input.steps);
    s += &section("What happened", &input.happened);
    s += &section("What should happen", &input.expected);
    if input.include_commit {
        let link = ctx.commit_url.as_deref().map(|u| format!("\n{u}")).unwrap_or_default();
        s += &section("Branch and commit", &format!("{} @ {} {}{link}", ctx.branch, ctx.commit, ctx.commit_subject));
    }
    s += &section("Setup", &input.setup);
    s.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};

    fn input(logs: Vec<PathBuf>, media: Vec<PathBuf>) -> ReportInput {
        ReportInput {
            title: "Boss projectile goes through walls @everyone".into(),
            steps: "1. Open the harbor level".into(),
            happened: "x".repeat(5000),
            expected: String::new(),
            include_commit: true,
            setup: "- Git: Installed".into(),
            logs,
            media,
        }
    }

    fn project(dir: &Path, extra: &str) -> Project {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("Game.uproject"), br#"{"EngineAssociation":"5.8"}"#).unwrap();
        std::fs::write(dir.join("ue-dev-hub.toml"), format!("[report]\n{extra}")).unwrap();
        Project::load(dir).unwrap()
    }

    #[test]
    fn trims_big_logs_and_skips_media_that_doesnt_fit() {
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-report-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("work")).unwrap();
        // A 3 MB log of random-ish text that doesn't compress well, and two media files.
        let mut log = String::new();
        let mut x: u64 = 7;
        while log.len() < 3 * MB as usize {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            log.push_str(&format!("LogTemp: {x:x}\n"));
        }
        log.push_str("LogTemp: Error: the crash at the very end\n");
        std::fs::write(dir.join("Game.log"), &log).unwrap();
        std::fs::write(dir.join("small.png"), vec![1u8; 100 * 1024]).unwrap();
        std::fs::write(dir.join("huge.mp4"), vec![2u8; 3 * MB as usize]).unwrap();

        let (files, sent) = prepare_files(&dir.join("work"), &input(vec![dir.join("Game.log")], vec![dir.join("small.png"), dir.join("huge.mp4")]), 2 * MB).unwrap();
        let total: u64 = files.iter().map(|f| size(f)).sum();
        assert!(total <= 2 * MB - PAYLOAD_SLACK, "{total}");
        assert_eq!(sent.trimmed, vec!["Game.log".to_string()]);
        assert!(files.iter().any(|f| f.ends_with("small.png")));
        assert!(sent.skipped.iter().any(|s| s.starts_with("huge.mp4")));
        // The end of the log, where the error is, survives the cut.
        let kept = std::fs::read_to_string(dir.join("work/1-Game.log")).unwrap();
        assert!(kept.contains("the crash at the very end"));
        assert!(kept.starts_with("[UE Dev Hub: the start of this log was cut"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_webhooks_that_arent_discord() {
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-report-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = project(&dir, "discord_webhook = \"https://evil.example.com/hook\"\n");
        assert!(p.config.report.discord_webhook.is_none());
        assert!(p.config_issues.iter().any(|i| i.message.contains("discord_webhook")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Posts to a fake webhook on localhost and checks what arrives.
    #[test]
    fn posts_the_report_with_attachments() {
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-report-post-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = project(&dir, "");
        std::fs::write(dir.join("Game.log"), "LogTemp: hello\n").unwrap();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api/webhooks/1/abc", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).unwrap();
            let mut s = stream;
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}").unwrap();
            String::from_utf8_lossy(&body).to_string()
        });

        let sent = send_to(&p, &url, input(vec![dir.join("Game.log")], vec![])).unwrap();
        assert!(sent.skipped.is_empty());
        let body = server.join().unwrap();
        assert!(body.contains("name=\"payload_json\""));
        assert!(body.contains("name=\"files[0]\"; filename=\"logs.zip\""));
        assert!(body.contains("\"allowed_mentions\":{\"parse\":[]}"));
        assert!(body.contains("Boss projectile"));
        // The 5000-character description was cut to Discord's limits.
        assert!(!body.contains(&"x".repeat(2600)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attaches_only_project_logs_and_picked_media() {
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-report-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = project(&dir.join("Game"), "");
        std::fs::create_dir_all(p.logs_dir()).unwrap();
        std::fs::write(p.logs_dir().join("Game.log"), "x").unwrap();
        std::fs::write(dir.join("secret.txt"), "x").unwrap();
        std::fs::write(dir.join("shot.png"), "x").unwrap();
        std::fs::write(dir.join("Game/inside.png"), "x").unwrap();

        assert!(check_files(&p, &input(vec![p.logs_dir().join("Game.log")], vec![dir.join("Game/inside.png")])).is_ok());
        assert!(check_files(&p, &input(vec![dir.join("secret.txt")], vec![])).is_err());
        assert!(check_files(&p, &input(vec![p.logs_dir().join("../../../secret.txt")], vec![])).is_err());
        assert!(check_files(&p, &input(vec![], vec![dir.join("secret.txt")])).is_err());
        // A screenshot outside the project only after it was picked in the dialog.
        assert!(check_files(&p, &input(vec![], vec![dir.join("shot.png")])).is_err());
        remember_picked(&[dir.join("shot.png")]);
        assert!(check_files(&p, &input(vec![], vec![dir.join("shot.png")])).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attaches_the_hubs_own_logs() {
        let dir = std::env::temp_dir().join(format!("ue-dev-hub-report-own-logs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["hub.log", "crash.log", "settings.json"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        assert!(is_hub_log(&dir.join("hub.log"), Some(&dir)));
        assert!(is_hub_log(&dir.join("crash.log"), Some(&dir)));
        assert!(!is_hub_log(&dir.join("settings.json"), Some(&dir)));
        assert!(!is_hub_log(&dir.join("hub.log"), None));
        assert!(!is_hub_log(&dir.join("sub/../hub.old.log"), Some(&dir)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn formats_markdown_for_a_task_tracker() {
        let ctx = Context {
            branch: "feature/boss-arena".into(),
            commit: "1a2b3c4d5".into(),
            commit_subject: "Fix".into(),
            commit_url: Some("https://github.com/o/r/commit/1a2b3c4d5".into()),
            engine_log: attachment(PathBuf::from("none")),
            build_log: attachment(PathBuf::from("none")),
            hub_log: attachment(PathBuf::from("none")),
            reporter: "Jo".into(),
            has_webhook: false,
            max_upload_mb: 10,
        };
        let md = markdown(&ctx, &input(vec![], vec![]));
        assert!(md.starts_with("## Boss projectile"));
        assert!(md.contains("### Branch and commit\nfeature/boss-arena @ 1a2b3c4d5 Fix\nhttps://github.com/o/r/commit/1a2b3c4d5"));
        assert!(!md.contains("What should happen"));
    }
}
