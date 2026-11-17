//! The `addon://` scheme the window loads addon pages from. `addon://localhost/<id>/<file>`
//! (`http://addon.localhost/<id>/<file>` on Windows) serves `<addons>/<id>/ui/<file>` and
//! nothing outside an addon's `ui` folder.

use std::path::{Path, PathBuf};
use tauri::http::{Request, Response, StatusCode};
use tauri::{AppHandle, Runtime};

pub const SCHEME: &str = "addon";

/// Decodes %XX escapes. The window's `convertFileSrc` escapes the whole path, slashes too.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The file a request path names: `<root>/<id>/ui/<rest>`, or None when the path tries to leave
/// that folder or the file doesn't exist.
pub fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    let path = percent_decode(path)?;
    let path = path.trim_start_matches('/');
    let (id, rest) = path.split_once('/')?;
    if !crate::addons::is_id(id) || !crate::config::is_inside(rest) || rest.contains('\\') {
        return None;
    }
    let ui = root.join(id).join("ui").canonicalize().ok()?;
    let file = ui.join(rest).canonicalize().ok()?;
    // Also catches links inside the folder that point outside it.
    (file.starts_with(&ui) && file.is_file()).then_some(file)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("js" | "mjs") => "text/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        Some("html") => "text/html",
        _ => "application/octet-stream",
    }
}

pub fn handle<R: Runtime>(app: &AppHandle<R>, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let file = crate::addons::root_for(app).ok().and_then(|root| resolve(&root, request.uri().path()));
    let builder = Response::builder()
        // The page is another origin than the addon files, and modules need this to load.
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "no-store");
    match file.and_then(|f| std::fs::read(&f).ok().map(|data| (f, data))) {
        Some((f, data)) => builder.header("Content-Type", content_type(&f)).body(data),
        None => builder.status(StatusCode::NOT_FOUND).body(Vec::new()),
    }
    .unwrap_or_else(|_| Response::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_only_files_in_an_addons_ui_folder() {
        let root = std::env::temp_dir().join(format!("ue-dev-hub-protocol-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("demo/ui/img")).unwrap();
        std::fs::write(root.join("demo/ui/index.js"), "x").unwrap();
        std::fs::write(root.join("demo/ui/img/a.svg"), "x").unwrap();
        std::fs::write(root.join("demo/addon.toml"), "x").unwrap();
        std::fs::write(root.join("secret.txt"), "x").unwrap();

        assert!(resolve(&root, "/demo/index.js").is_some());
        assert!(resolve(&root, "/demo%2Fimg%2Fa.svg").is_some());
        assert!(resolve(&root, "demo/img/a.svg").is_some());
        for bad in [
            "/demo/../addon.toml",
            "/demo/%2E%2E/addon.toml",
            "/demo/..%2F..%2Fsecret.txt",
            "/demo/img\\..\\..\\addon.toml",
            "/../secret.txt",
            "/demo/C:/Windows/win.ini",
            "/demo//etc/passwd",
            "/demo/",
            "/demo",
            "/Demo/index.js",
            "/demo/missing.js",
            "/demo/%zz",
        ] {
            assert!(resolve(&root, bad).is_none(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
