//! GitHub profile pictures for commit authors, looked up by their commit email.
//!
//! Pictures come back as data URLs so the page needs no network access of its own. Authors
//! without a GitHub picture get `None`, and the screen shows their initials instead.

use base64::Engine;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const SIZE: u32 = 64;

/// Pictures already looked up this session, by email. `None` means GitHub has none.
static CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/// One batch of downloads at a time: a second call waits, then finds the first one's answers.
static LOOKING_UP: Mutex<()> = Mutex::new(());

/// At most this many downloads run at once.
const WORKERS: usize = 6;

/// What GitHub sends for an email it doesn't know, to tell it apart from a real picture.
static PLACEHOLDER: OnceLock<Option<Vec<u8>>> = OnceLock::new();

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .https_only(true)
        .user_agent("ue-dev-hub")
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
}

/// Where GitHub serves the picture for a commit email.
fn avatar_url(email: &str) -> String {
    let email = email.trim().to_lowercase();
    if let Some(local) = email.strip_suffix("@users.noreply.github.com") {
        // "12345+name@users.noreply.github.com" carries the account id.
        if let Some((id, _)) = local.split_once('+').filter(|(id, _)| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())) {
            return format!("https://avatars.githubusercontent.com/u/{id}?s={SIZE}");
        }
        // Older addresses are just "name@users.noreply.github.com".
        return format!("https://github.com/{local}.png?size={SIZE}");
    }
    let mut query = String::new();
    for b in email.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            query.push(b as char);
        } else {
            query.push_str(&format!("%{b:02X}"));
        }
    }
    format!("https://avatars.githubusercontent.com/u/e?email={query}&s={SIZE}")
}

/// Downloads a picture. `Ok(None)` when GitHub has none for this author.
fn fetch(client: &reqwest::blocking::Client, url: &str) -> Result<Option<(Vec<u8>, String)>, String> {
    let resp = client.get(url).send().map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub answered {}", resp.status()));
    }
    let mime = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("image/png").to_string();
    if !mime.starts_with("image/") {
        return Ok(None);
    }
    // Pictures are a few KB; anything much bigger isn't one.
    let bytes = crate::selfupdate::read_capped(resp, 1024 * 1024)?;
    Ok(Some((bytes, mime)))
}

fn placeholder(client: &reqwest::blocking::Client) -> Option<&'static Vec<u8>> {
    PLACEHOLDER
        .get_or_init(|| fetch(client, &avatar_url("no-such-author@ue-dev-hub.invalid")).ok().flatten().map(|(b, _)| b))
        .as_ref()
}

fn lookup(client: &reqwest::blocking::Client, email: &str) -> Result<Option<String>, String> {
    let url = avatar_url(email);
    let Some((bytes, mime)) = fetch(client, &url)? else { return Ok(None) };
    // Lookups by email give a generic picture when no account matches.
    if url.contains("/u/e?") && placeholder(client) == Some(&bytes) {
        return Ok(None);
    }
    Ok(Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))))
}

/// Pictures for these commit emails. Emails that couldn't be checked, for example without
/// internet, are left out so a later call tries again.
pub fn avatars(emails: Vec<String>) -> HashMap<String, Option<String>> {
    let _one_batch = LOOKING_UP.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = HashMap::new();
    let mut missing = Vec::new();
    {
        let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        for email in emails {
            match cache.as_ref().and_then(|c| c.get(&email)) {
                Some(hit) => {
                    out.insert(email, hit.clone());
                }
                None if !missing.contains(&email) && !email.is_empty() => missing.push(email),
                None => {}
            }
        }
    }
    if missing.is_empty() {
        return out;
    }
    let Ok(client) = client() else { return out };
    let found = in_parallel(missing, WORKERS, |email| lookup(&client, &email).ok().map(|r| (email, r)));
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    for (email, picture) in found {
        cache.insert(email.clone(), picture.clone());
        out.insert(email, picture);
    }
    out
}

/// Runs `work` on every item with at most `workers` threads, and collects the answers.
fn in_parallel<T: Send, R: Send>(items: Vec<T>, workers: usize, work: impl Fn(T) -> Option<R> + Sync) -> Vec<R> {
    let queue = Mutex::new(items);
    let next = || queue.lock().unwrap_or_else(|e| e.into_inner()).pop();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut done = vec![];
                    while let Some(item) = next() {
                        done.extend(work(item));
                    }
                    done
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).flatten().collect()
    })
}

#[cfg(test)]
mod tests {
    use super::{avatar_url, in_parallel};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn never_runs_more_lookups_at_once_than_allowed() {
        let (now, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let mut out = in_parallel((0..40).collect(), 6, |i: u32| {
            let n = now.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(n, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(5));
            now.fetch_sub(1, Ordering::SeqCst);
            Some(i)
        });
        out.sort();
        assert_eq!(out, (0..40).collect::<Vec<_>>());
        assert!(most.load(Ordering::SeqCst) <= 6);
    }

    #[test]
    fn builds_avatar_urls() {
        assert_eq!(avatar_url("12345+someone@users.noreply.github.com"), "https://avatars.githubusercontent.com/u/12345?s=64");
        assert_eq!(avatar_url("Someone@users.noreply.github.com"), "https://github.com/someone.png?size=64");
        assert_eq!(avatar_url("a.b+c@mail.example"), "https://avatars.githubusercontent.com/u/e?email=a.b%2Bc%40mail.example&s=64");
    }
}
