use super::*;
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Root(PathBuf, File);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "scramjet-capture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        let root = crate::project::open_directory(&path).unwrap();
        Self(path, root)
    }
    fn write(&self, path: &str, content: &[u8]) {
        std::fs::write(self.0.join(path), content).unwrap();
        std::fs::set_permissions(self.0.join(path), std::fs::Permissions::from_mode(0o600))
            .unwrap();
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn manifest_rejects_ambiguous_paths_and_internal_metadata_without_effects() {
    let root = Root::new();
    for bad in [
        "",
        ".",
        "..",
        "/absolute",
        "a/b",
        "a\0b",
        crate::PROJECT_LOCK_NAME,
        ".coding-stage",
        RUN_ANCHOR,
    ] {
        assert!(FileSlots::new([bad, "other"]).is_err());
    }
    assert!(FileSlots::new(["same", "same"]).is_err());
    assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 0);
}

#[test]
fn legal_basename_and_utf8_bytes_are_not_normalized() {
    for name in [
        "verse notes hé🙂.md",
        "line\n\u{1b}tab\t",
        "back\\slash",
        "é",
        "e\u{301}",
    ] {
        let root = Root::new();
        let slots = FileSlots::new([name, "reader alternate.py"]).unwrap();
        let content = "# Verse\r\nQuiet hé🙂\rFiles\0sing\n";
        slots.absent(&root.1, name).unwrap();
        let mut created = slots.create(&root.1, name).unwrap();
        created.write_all(content.as_bytes()).unwrap();
        created.sync_all().unwrap();
        root.1.sync_all().unwrap();
        let captured = slots.capture(&root.1, name).unwrap();
        assert_eq!(captured.path, name);
        assert_eq!(captured.content.as_bytes(), content.as_bytes());
        let stat = fstat(&created).unwrap();
        assert_eq!(captured.identity, [stat.st_dev, stat.st_ino]);
        assert!(matches!(
            slots.absent(&root.1, name),
            Err(CodingError::TargetMismatch)
        ));
        assert!(slots.create(&root.1, name).is_err());
        assert_eq!(
            std::fs::read(root.0.join(name)).unwrap(),
            content.as_bytes()
        );
    }
}

#[test]
fn file_budget_is_bytes_not_characters_and_preserves_boundary_material() {
    let root = Root::new();
    let slots = FileSlots::new(["input", "other"]).unwrap();
    for content in [String::new(), "\0".repeat(512), "é".repeat(256)] {
        root.write("input", content.as_bytes());
        assert_eq!(slots.capture(&root.1, "input").unwrap().content, content);
    }
    for content in ["x".repeat(513), "é".repeat(257)] {
        root.write("input", content.as_bytes());
        assert!(matches!(
            slots.capture(&root.1, "input"),
            Err(CodingError::ResourceLimit(_))
        ));
        assert_eq!(
            std::fs::read(root.0.join("input")).unwrap(),
            content.as_bytes()
        );
    }
    root.write("input", &[0xff]);
    assert!(matches!(
        slots.capture(&root.1, "input"),
        Err(CodingError::Invalid(_))
    ));
    assert_eq!(std::fs::read(root.0.join("input")).unwrap(), [0xff]);
}

#[test]
fn unselected_names_never_open_read_or_create() {
    let root = Root::new();
    let slots = FileSlots::new(["one", "two"]).unwrap();
    root.write("foreign", b"untouched");
    for path in ["foreign", "absent", "../escape", "/etc/passwd"] {
        assert!(matches!(
            slots.absent(&root.1, path),
            Err(CodingError::Unsupported(_))
        ));
        assert!(matches!(
            slots.capture(&root.1, path),
            Err(CodingError::Unsupported(_))
        ));
        assert!(matches!(
            slots.create(&root.1, path),
            Err(CodingError::Unsupported(_))
        ));
    }
    assert_eq!(std::fs::read(root.0.join("foreign")).unwrap(), b"untouched");
    assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 1);
}

#[test]
fn appeared_symlink_multilink_directory_fifo_and_wrong_mode_refuse_intact() {
    for case in 0..6 {
        let root = Root::new();
        let slots = FileSlots::new(["input", "other"]).unwrap();
        slots.absent(&root.1, "input").unwrap();
        match case {
            0 => std::os::unix::fs::symlink("missing", root.0.join("input")).unwrap(),
            1 => {
                root.write("input", b"keep");
                std::fs::hard_link(root.0.join("input"), root.0.join("other")).unwrap();
            }
            2 => std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.0.join("input"))
                .unwrap(),
            3 => rustix::fs::mknodat(
                &root.1,
                "input",
                rustix::fs::FileType::Fifo,
                Mode::RUSR | Mode::WUSR,
                0,
            )
            .unwrap(),
            4 => {
                root.write("input", b"keep");
                std::fs::set_permissions(
                    root.0.join("input"),
                    std::fs::Permissions::from_mode(0o644),
                )
                .unwrap();
            }
            _ => root.write("input", b"keep"),
        }
        let metadata = std::fs::symlink_metadata(root.0.join("input")).unwrap();
        assert!(slots.absent(&root.1, "input").is_err());
        assert!(slots.create(&root.1, "input").is_err());
        if case != 5 {
            assert!(slots.capture(&root.1, "input").is_err());
        } else {
            assert_eq!(slots.capture(&root.1, "input").unwrap().content, "keep");
        }
        use std::os::unix::fs::MetadataExt;
        let after = std::fs::symlink_metadata(root.0.join("input")).unwrap();
        assert_eq!(
            (
                metadata.dev(),
                metadata.ino(),
                metadata.mode(),
                metadata.nlink(),
                metadata.len()
            ),
            (
                after.dev(),
                after.ino(),
                after.mode(),
                after.nlink(),
                after.len()
            )
        );
    }
}

#[test]
fn capture_identity_and_bytes_detect_replacement_and_in_place_mutation() {
    let root = Root::new();
    let slots = FileSlots::new(["input", "other"]).unwrap();
    root.write("input", b"original");
    let original = slots.capture(&root.1, "input").unwrap();
    root.write("input", b"changed");
    assert_ne!(slots.capture(&root.1, "input").unwrap(), original);
    std::fs::rename(root.0.join("input"), root.0.join("other")).unwrap();
    root.write("input", b"original");
    let replaced = slots.capture(&root.1, "input").unwrap();
    assert_eq!(replaced.content, original.content);
    assert_ne!(replaced.identity, original.identity);
    assert_eq!(slots.capture(&root.1, "other").unwrap().content, "changed");
}
