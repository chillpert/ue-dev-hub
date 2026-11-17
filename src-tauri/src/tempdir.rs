//! Private temporary folders. On Linux /tmp is shared by every user, so a folder with a name
//! someone can guess could be made by them first, or have files swapped in it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A new, empty folder in the temp folder that only this user can open. Removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(prefix: &str) -> Result<TempDir, String> {
        Self::new_in(&std::env::temp_dir(), prefix)
    }

    pub fn new_in(parent: &Path, prefix: &str) -> Result<TempDir, String> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let mut last = None;
        for _ in 0..16 {
            let name = format!("{prefix}-{:016x}", random(COUNTER.fetch_add(1, Ordering::Relaxed)));
            let dir = parent.join(name);
            // `create_dir` fails if the folder exists, so nobody can have prepared it.
            match create_private(&dir) {
                Ok(()) => return Ok(TempDir(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(format!("Couldn't make a temporary folder: {e}")),
            }
        }
        Err(format!("Couldn't make a temporary folder: {}", last.map(|e| e.to_string()).unwrap_or_default()))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Keeps the folder after this is dropped, for files a program started later still needs.
    pub fn keep(self) -> PathBuf {
        let dir = self.0.clone();
        std::mem::forget(self);
        dir
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn create_private(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn create_private(dir: &Path) -> std::io::Result<()> {
    // %TEMP% is already private to the user on Windows.
    std::fs::create_dir(dir)
}

/// Hard to guess: the std hasher is seeded randomly per process.
fn random(n: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(n);
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.write_u32(std::process::id());
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_a_fresh_private_folder_and_removes_it() {
        let a = TempDir::new("ue-dev-hub-test").unwrap();
        let b = TempDir::new("ue-dev-hub-test").unwrap();
        assert_ne!(a.path(), b.path());
        assert!(a.path().is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(a.path()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let path = a.path().to_path_buf();
        std::fs::write(path.join("x"), b"x").unwrap();
        drop(a);
        assert!(!path.exists());
    }
}
