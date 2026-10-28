use std::{
    fs, io,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
pub const AUTOMATIC_CATALOG: &str = r#"{"commands":[{"name":"compare","outcome":"Equal","steps":[{"read":"left.txt"},{"read":"right.txt"}],"next":{"command":"inspect","policy":"Automatic"}},{"name":"inspect","outcome":"Readable","steps":[{"read":"left.txt"}]}]}"#;

pub struct Roots {
    pub definitions: PathBuf,
    pub inputs: PathBuf,
    base: PathBuf,
    cleanup_on_drop: bool,
}
impl Roots {
    #[allow(dead_code)]
    pub fn new(catalog: &str, left: &[u8], right: &[u8]) -> io::Result<Self> {
        Self::create(catalog, left, right, true, |_| {})
    }
    #[allow(dead_code)]
    pub fn new_retained(
        catalog: &str,
        left: &[u8],
        right: &[u8],
        created: impl FnOnce(&std::path::Path),
    ) -> io::Result<Self> {
        Self::create(catalog, left, right, false, created)
    }
    fn create(
        catalog: &str,
        left: &[u8],
        right: &[u8],
        cleanup_on_drop: bool,
        created: impl FnOnce(&std::path::Path),
    ) -> io::Result<Self> {
        let base = std::env::temp_dir().join(format!(
            "scramjet-commands-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base)?;
        created(&base);
        let roots = Self {
            definitions: base.join("definitions"),
            inputs: base.join("inputs"),
            base,
            cleanup_on_drop,
        };
        fs::set_permissions(&roots.base, fs::Permissions::from_mode(0o700))?;
        for path in [&roots.definitions, &roots.inputs] {
            fs::create_dir(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        fs::write(roots.definitions.join("commands.json"), catalog)?;
        fs::write(roots.inputs.join("left.txt"), left)?;
        fs::write(roots.inputs.join("right.txt"), right)?;
        Ok(roots)
    }
    #[allow(dead_code)]
    pub fn cleanup(&mut self) -> io::Result<()> {
        self.cleanup_on_drop = false;
        fs::remove_dir_all(&self.base)
    }
}
impl Drop for Roots {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            let _ = fs::remove_dir_all(&self.base);
        }
    }
}
