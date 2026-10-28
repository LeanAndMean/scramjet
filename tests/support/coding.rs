use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

pub struct OwnedRoot {
    pub path: PathBuf,
}

impl OwnedRoot {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "scramjet-rs-coding-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        fs::write(path.join("sample.txt"), "before\n").unwrap();
        Self { path }
    }
}

impl Drop for OwnedRoot {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}
