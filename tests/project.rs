use scramjet_rs::{
    CodingError, EditCancellation, OperationState, PROJECT_LOCK_NAME, ProjectCapture,
    ProjectRequest, ProjectWorkspace,
};
use std::fs::{self, File};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    parent: PathBuf,
    project: PathBuf,
    storage: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let parent = std::env::temp_dir().join(format!(
            "scramjet-project-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let project = parent.join("project");
        let storage = parent.join("storage");
        fs::create_dir(&project).unwrap();
        fs::create_dir(&storage).unwrap();
        fs::set_permissions(&project, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&storage, fs::Permissions::from_mode(0o700)).unwrap();
        for (name, text) in [
            ("main.txt", "header\nbody OLD text\nfooter\n"),
            ("rules.txt", "keep header and footer\n"),
            ("notes.txt", "change only the unique marker\n"),
            ("unrelated.txt", "never touch\n"),
        ] {
            fs::write(project.join(name), text).unwrap();
            fs::set_permissions(project.join(name), fs::Permissions::from_mode(0o644)).unwrap();
        }
        Self {
            parent,
            project,
            storage,
        }
    }
    fn open(&self) -> ProjectWorkspace {
        ProjectWorkspace::open(&self.project, &self.storage).unwrap()
    }
    fn capture(&self, workspace: &ProjectWorkspace) -> ProjectCapture {
        workspace.capture(request()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.parent).unwrap();
    }
}
fn request() -> ProjectRequest {
    ProjectRequest::parse(r#"{"operation":"replace_once","target":"main.txt","supporting":["rules.txt","notes.txt"],"needle":"OLD","replacement":"NEW"}"#).unwrap()
}
fn raw(needle: &str, replacement: &str) -> String {
    serde_json::json!({"operation":"replace_once", "target":"main.txt", "supporting":["rules.txt", "notes.txt"], "needle":needle, "replacement":replacement}).to_string()
}

#[test]
fn strict_request_rejects_unsupported_duplicate_positional_and_oversized_inputs() {
    let valid = serde_json::to_string(&request()).unwrap();
    assert_eq!(ProjectRequest::parse(&valid).unwrap(), request());
    let invalid = [
        valid.replace("replace_once", "shell"),
        valid.replace("main.txt", "../main.txt"),
        valid.replace("notes.txt", "rules.txt"),
        valid.replace("main.txt", PROJECT_LOCK_NAME),
        valid.replace("main.txt", ".scramjet-stage-foreign"),
        valid.replace("\"operation\":", "\"unknown\":0,\"operation\":"),
        valid.replace(
            "\"operation\":",
            "\"operation\":\"replace_once\",\"operation\":",
        ),
        r#"["replace_once","main.txt",["rules.txt","notes.txt"],"OLD","NEW"]"#.into(),
        r#"{"operation":"replace_once"}"#.into(),
        raw("", "NEW"),
        raw("OLD", ""),
        raw("OLD", "OLD"),
        raw("\u{1b}", "NEW"),
        raw(&"x".repeat(257), "NEW"),
        raw("OLD", &"é".repeat(129)),
        valid.clone() + &" ".repeat(1024),
        valid.replace("notes.txt", &"x".repeat(65)),
    ];
    for value in invalid {
        assert!(ProjectRequest::parse(&value).is_err(), "accepted {value:?}");
    }
    let expanded = raw(&"\n".repeat(256), &"\t".repeat(256));
    assert!(serde_json::from_str::<ProjectRequest>(&expanded).is_err());
}

#[test]
fn capture_derives_only_unique_fragment_and_preserves_complete_inputs() {
    let fixture = Fixture::new();
    let workspace = fixture.open();
    let capture = fixture.capture(&workspace);
    assert_eq!(capture.desired_target(), "header\nbody NEW text\nfooter\n");
    assert_eq!(
        capture.files()[0].content,
        "header\nbody OLD text\nfooter\n"
    );
    assert_eq!(capture.files()[1].content, "keep header and footer\n");
    assert_eq!(
        capture.files()[2].content,
        "change only the unique marker\n"
    );
    capture.recheck().unwrap();
    for text in ["absent", "OLD and OLD"] {
        fs::write(fixture.project.join("main.txt"), text).unwrap();
        assert!(workspace.capture(request()).is_err());
    }
    fs::write(fixture.project.join("main.txt"), "aaa").unwrap();
    assert!(
        workspace
            .capture(ProjectRequest::parse(&raw("aa", "z")).unwrap())
            .is_err()
    );
    fs::write(
        fixture.project.join("main.txt"),
        format!("{}OLD", "x".repeat(1021)),
    )
    .unwrap();
    assert!(
        workspace
            .capture(ProjectRequest::parse(&raw("OLD", "LONGER")).unwrap())
            .is_err()
    );
}

#[test]
fn selected_edit_preserves_modes_project_and_storage_after_close_and_recovery() {
    let fixture = Fixture::new();
    let workspace = fixture.open();
    let id = workspace.root_identity();
    let capture = fixture.capture(&workspace);
    let desired = capture.desired_target().to_owned();
    let mut coding = workspace
        .coding_session(capture, "project-first", 1)
        .unwrap();
    assert!(
        coding
            .intent(
                "main.txt",
                "header\nbody OLD text\nfooter\n",
                "unrelated replacement"
            )
            .is_err()
    );
    let intent = coding
        .intent("main.txt", "header\nbody OLD text\nfooter\n", &desired)
        .unwrap();
    let approval = coding.approve_exact(&intent).unwrap();
    coding
        .authorize(intent, approval, &EditCancellation::default())
        .unwrap();
    assert_eq!(
        coding.dispatch(&EditCancellation::default()).unwrap().state,
        OperationState::Completed
    );
    assert_eq!(coding.read("main.txt").unwrap(), desired);
    assert!(coding.intent("main.txt", &desired, "third").is_err());
    assert!(ProjectWorkspace::coding_history(&fixture.storage, 1, "project-first", id).is_err());
    drop(coding);
    drop(workspace);
    let history =
        ProjectWorkspace::coding_history(&fixture.storage, 1, "project-first", id).unwrap();
    assert_eq!(history.root(), id);
    assert_eq!(history.operations()[0].state, OperationState::Completed);
    assert_eq!(
        fs::metadata(fixture.project.join("main.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o644
    );
    assert_eq!(
        fs::metadata(&fixture.project).unwrap().permissions().mode() & 0o7777,
        0o755
    );
    assert_eq!(
        fs::read_to_string(fixture.project.join("unrelated.txt")).unwrap(),
        "never touch\n"
    );
    assert!(!fixture.project.join(".coding-journal").exists());
    assert!(fixture.project.join(PROJECT_LOCK_NAME).exists());
    assert!(fixture.storage.join("coding-1/.coding-journal").exists());
    let moved = fixture.parent.join("moved-project");
    fs::rename(&fixture.project, moved).unwrap();
    assert_eq!(
        ProjectWorkspace::coding_history(&fixture.storage, 1, "project-first", id)
            .unwrap()
            .operations()[0]
            .state,
        OperationState::Completed
    );
    assert!(ProjectWorkspace::coding_history(&fixture.storage, 1, "foreign", id).is_err());
    let foreign = scramjet_rs::RootIdentity {
        inode: id.inode + 1,
        ..id
    };
    assert!(
        ProjectWorkspace::coding_history(&fixture.storage, 1, "project-first", foreign).is_err()
    );
}

#[test]
fn selected_recovery_rejects_bad_metadata_and_contradictory_or_truncated_facts_read_only() {
    let fixture = Fixture::new();
    let workspace = fixture.open();
    let capture = fixture.capture(&workspace);
    let desired = capture.desired_target().to_owned();
    let mut coding = workspace.coding_session(capture, "recovery", 1).unwrap();
    let intent = coding
        .intent("main.txt", "header\nbody OLD text\nfooter\n", &desired)
        .unwrap();
    let approval = coding.approve_exact(&intent).unwrap();
    coding
        .authorize(intent, approval, &EditCancellation::default())
        .unwrap();
    coding.dispatch(&EditCancellation::default()).unwrap();
    drop(coding);
    let journal = fixture.storage.join("coding-1/.coding-journal");
    let original = fs::read(&journal).unwrap();
    let recover = || {
        ProjectWorkspace::coding_history(&fixture.storage, 1, "recovery", workspace.root_identity())
    };
    fs::set_permissions(&journal, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(recover().is_err());
    fs::set_permissions(&journal, fs::Permissions::from_mode(0o600)).unwrap();
    let file = File::open(&journal).unwrap();
    rustix::fs::fsetxattr(
        &file,
        "user.synthetic",
        b"keep",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    assert!(recover().is_err());
    rustix::fs::fremovexattr(&file, "user.synthetic").unwrap();
    let lines: Vec<_> = std::str::from_utf8(&original).unwrap().lines().collect();
    let contradictory = lines
        .iter()
        .map(|line| {
            if line.contains("Dispatched") {
                line.replace("Dispatched", "Completed")
            } else {
                (*line).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    for malformed in [
        original[..original.len() - 1].to_vec(),
        contradictory.into_bytes(),
        (lines[0].to_owned() + "\n" + lines[0] + "\n").into_bytes(),
    ] {
        fs::write(&journal, &malformed).unwrap();
        assert!(recover().is_err());
        assert_eq!(fs::read(&journal).unwrap(), malformed);
    }
    fs::write(&journal, original).unwrap();
    assert_eq!(
        recover().unwrap().operations()[0].state,
        OperationState::Completed
    );
    assert_eq!(
        fs::read_to_string(fixture.project.join("main.txt")).unwrap(),
        desired
    );
}

#[test]
fn persistent_prior_open_contender_uses_the_same_inode_and_excludes_reopen() {
    use rustix::fs::{FlockOperation, flock, fstat};
    let fixture = Fixture::new();
    let workspace = fixture.open();
    let contender = File::open(fixture.project.join(PROJECT_LOCK_NAME)).unwrap();
    let id = fstat(&contender).unwrap();
    assert!(flock(&contender, FlockOperation::NonBlockingLockExclusive).is_err());
    assert!(ProjectWorkspace::open(&fixture.project, &fixture.storage).is_err());
    let capture = fixture.capture(&workspace);
    drop(workspace);
    assert!(flock(&contender, FlockOperation::NonBlockingLockExclusive).is_err());
    drop(capture);
    flock(&contender, FlockOperation::NonBlockingLockExclusive).unwrap();
    let reopened = File::open(fixture.project.join(PROJECT_LOCK_NAME)).unwrap();
    assert_eq!(fstat(&reopened).unwrap().st_ino, id.st_ino);
    assert!(ProjectWorkspace::open(&fixture.project, &fixture.storage).is_err());
    drop(contender);
    let workspace = fixture.open();
    assert!(flock(&reopened, FlockOperation::NonBlockingLockExclusive).is_err());
    drop(workspace);
    flock(&reopened, FlockOperation::NonBlockingLockExclusive).unwrap();
    assert_eq!(fstat(&reopened).unwrap().st_ino, id.st_ino);
}

#[test]
fn changed_inputs_and_root_are_refused_at_capture_proposal_authorization_dispatch() {
    for phase in 0..4 {
        let fixture = Fixture::new();
        let workspace = fixture.open();
        let capture = fixture.capture(&workspace);
        if phase == 0 {
            fs::write(fixture.project.join("notes.txt"), "changed").unwrap();
            assert!(capture.recheck().is_err());
            assert!(workspace.coding_session(capture, "change", 1).is_err());
            continue;
        }
        let desired = capture.desired_target().to_owned();
        let mut coding = workspace.coding_session(capture, "change", 1).unwrap();
        let intent = coding
            .intent("main.txt", "header\nbody OLD text\nfooter\n", &desired)
            .unwrap();
        if phase == 1 {
            fs::write(fixture.project.join("rules.txt"), "changed").unwrap();
            assert!(coding.approve_exact(&intent).is_err());
        } else {
            let approval = coding.approve_exact(&intent).unwrap();
            if phase == 2 {
                fs::write(fixture.project.join("notes.txt"), "changed").unwrap();
                assert!(
                    coding
                        .authorize(intent, approval, &EditCancellation::default())
                        .is_err()
                );
            } else {
                coding
                    .authorize(intent, approval, &EditCancellation::default())
                    .unwrap();
                fs::write(fixture.project.join("main.txt"), "changed").unwrap();
                assert!(coding.dispatch(&EditCancellation::default()).is_err());
            }
        }
        assert!(!fixture.project.join(".scramjet-stage-change-1").exists());
    }
    let fixture = Fixture::new();
    let workspace = fixture.open();
    let capture = fixture.capture(&workspace);
    fs::rename(&fixture.project, fixture.parent.join("original")).unwrap();
    fs::create_dir(&fixture.project).unwrap();
    assert!(capture.recheck().is_err());
}

#[test]
fn symlink_components_containment_and_unsupported_storage_permissions_reject() {
    let fixture = Fixture::new();
    let alias = fixture.parent.join("alias");
    symlink(&fixture.project, &alias).unwrap();
    assert!(ProjectWorkspace::open(&alias, &fixture.storage).is_err());
    let parent_alias = fixture.parent.with_extension("alias");
    symlink(&fixture.parent, &parent_alias).unwrap();
    assert!(ProjectWorkspace::open(&parent_alias.join("project"), &fixture.storage).is_err());
    fs::remove_file(parent_alias).unwrap();
    assert!(ProjectWorkspace::open(&fixture.project, &fixture.project).is_err());
    let nested = fixture.project.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::set_permissions(&nested, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(ProjectWorkspace::open(&fixture.project, &nested).is_err());
    assert!(ProjectWorkspace::open(&fixture.parent, &fixture.storage).is_err());
    fs::set_permissions(&fixture.storage, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(ProjectWorkspace::open(&fixture.project, &fixture.storage).is_err());
    fs::set_permissions(&fixture.storage, fs::Permissions::from_mode(0o700)).unwrap();
    for mode in [0o777, 0o775, 0o2755, 0o1755] {
        fs::set_permissions(&fixture.project, fs::Permissions::from_mode(mode)).unwrap();
        assert!(ProjectWorkspace::open(&fixture.project, &fixture.storage).is_err());
    }
    assert!(ProjectWorkspace::open(std::path::Path::new("relative"), &fixture.storage).is_err());
}

#[test]
fn unsupported_file_types_metadata_and_content_are_refused() {
    for mode in [0o666, 0o755, 0o640, 0o2600] {
        let fixture = Fixture::new();
        let workspace = fixture.open();
        fs::set_permissions(
            fixture.project.join("main.txt"),
            fs::Permissions::from_mode(mode),
        )
        .unwrap();
        assert!(workspace.capture(request()).is_err());
    }
    for kind in 0..6 {
        let fixture = Fixture::new();
        let workspace = fixture.open();
        let target = fixture.project.join("main.txt");
        match kind {
            0 => {
                fs::remove_file(&target).unwrap();
                symlink("rules.txt", &target).unwrap();
            }
            1 => {
                fs::hard_link(&target, fixture.project.join("link")).unwrap();
            }
            2 => {
                fs::remove_file(&target).unwrap();
                fs::create_dir(&target).unwrap();
            }
            3 => {
                fs::write(&target, [0xff]).unwrap();
            }
            4 => {
                fs::write(&target, "x".repeat(1025)).unwrap();
            }
            5 => {
                fs::write(&target, b"OLD\x1b").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(workspace.capture(request()).is_err());
    }
    let fixture = Fixture::new();
    let workspace = fixture.open();
    let target = fixture.project.join("main.txt");
    fs::remove_file(&target).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &target,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    assert!(workspace.capture(request()).is_err());
}

#[test]
fn attributes_and_acl_metadata_are_rejected_without_discarding_them() {
    let fixture = Fixture::new();
    let workspace = fixture.open();
    let file = File::open(fixture.project.join("main.txt")).unwrap();
    rustix::fs::fsetxattr(
        &file,
        "user.synthetic",
        b"keep",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    assert!(workspace.capture(request()).is_err());
    let mut data = [0; 4];
    assert_eq!(
        rustix::fs::fgetxattr(&file, "user.synthetic", &mut data[..]).unwrap(),
        4
    );
    assert_eq!(&data, b"keep");
}

#[test]
fn invalid_lock_entries_never_get_replaced_or_unlinked() {
    for kind in 0..4 {
        let fixture = Fixture::new();
        let lock = fixture.project.join(PROJECT_LOCK_NAME);
        match kind {
            0 => {
                symlink("rules.txt", &lock).unwrap();
            }
            1 => {
                fs::create_dir(&lock).unwrap();
            }
            2 => {
                fs::write(&lock, "foreign").unwrap();
                fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
            }
            3 => {
                fs::write(&lock, "foreign").unwrap();
                fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
                fs::hard_link(&lock, fixture.project.join("lock-alias")).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(ProjectWorkspace::open(&fixture.project, &fixture.storage).is_err());
        assert!(fs::symlink_metadata(&lock).is_ok());
    }
}

#[test]
fn exact_approval_is_bound_to_the_actual_coding_owner_even_with_identical_journey_payloads() {
    let fixture = Fixture::new();
    let workspace = fixture.open();
    let first_capture = fixture.capture(&workspace);
    let desired = first_capture.desired_target().to_owned();
    let first = workspace
        .coding_session(first_capture, "same-session", 1)
        .unwrap();
    let intent = first
        .intent("main.txt", "header\nbody OLD text\nfooter\n", &desired)
        .unwrap();
    let old_approval = first.approve_exact(&intent).unwrap();
    drop(first);
    let mut second = workspace
        .coding_session(fixture.capture(&workspace), "same-session", 2)
        .unwrap();
    let second_intent = second
        .intent("main.txt", "header\nbody OLD text\nfooter\n", &desired)
        .unwrap();
    assert_eq!(intent, second_intent);
    assert!(matches!(
        second.authorize(
            second_intent.clone(),
            old_approval,
            &EditCancellation::default()
        ),
        Err(CodingError::Stale)
    ));
    let fresh = second.approve_exact(&second_intent).unwrap();
    second
        .authorize(second_intent, fresh, &EditCancellation::default())
        .unwrap();
    assert_eq!(
        second.dispatch(&EditCancellation::default()).unwrap().state,
        OperationState::Completed
    );
}

#[test]
#[cfg(target_os = "linux")]
fn project_default_acl_is_rejected_before_it_can_contaminate_a_stage() {
    let fixture = Fixture::new();
    let workspace = fixture.open();
    drop(workspace);
    let directory = File::open(&fixture.project).unwrap();
    let mut acl = 2u32.to_le_bytes().to_vec();
    for (tag, permissions, uid) in [
        (1u16, 7u16, u32::MAX),
        (2, 7, rustix::process::geteuid().as_raw() + 1),
        (4, 5, u32::MAX),
        (16, 5, u32::MAX),
        (32, 5, u32::MAX),
    ] {
        acl.extend_from_slice(&tag.to_le_bytes());
        acl.extend_from_slice(&permissions.to_le_bytes());
        acl.extend_from_slice(&uid.to_le_bytes());
    }
    rustix::fs::fsetxattr(
        &directory,
        "system.posix_acl_default",
        &acl,
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    assert!(ProjectWorkspace::open(&fixture.project, &fixture.storage).is_err());
    assert_eq!(
        fs::read_to_string(fixture.project.join("main.txt")).unwrap(),
        "header\nbody OLD text\nfooter\n"
    );
    assert!(fixture.project.join(PROJECT_LOCK_NAME).exists());
    assert!(!fixture.storage.join("coding-1").exists());
}

#[test]
fn stage_collision_retains_foreign_entry_and_uncertain_history_without_retry() {
    let fixture = Fixture::new();
    let workspace = fixture.open();
    let capture = fixture.capture(&workspace);
    let desired = capture.desired_target().to_owned();
    let mut coding = workspace.coding_session(capture, "collision", 1).unwrap();
    let intent = coding
        .intent("main.txt", "header\nbody OLD text\nfooter\n", &desired)
        .unwrap();
    let approval = coding.approve_exact(&intent).unwrap();
    coding
        .authorize(intent, approval, &EditCancellation::default())
        .unwrap();
    let stage = fixture.project.join(".scramjet-stage-collision-1");
    let mut options = fs::OpenOptions::new();
    let _foreign = options
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&stage)
        .unwrap();
    fs::write(&stage, "foreign").unwrap();
    let result = coding.dispatch(&EditCancellation::default()).unwrap();
    assert_eq!(result.state, OperationState::Uncertain);
    assert!(result.acknowledgement_error.is_some());
    assert!(matches!(
        coding.dispatch(&EditCancellation::default()),
        Err(CodingError::Poisoned)
    ));
    drop(coding);
    assert_eq!(fs::read_to_string(&stage).unwrap(), "foreign");
    assert_eq!(
        ProjectWorkspace::coding_history(
            &fixture.storage,
            1,
            "collision",
            workspace.root_identity()
        )
        .unwrap()
        .operations()[0]
            .state,
        OperationState::Uncertain
    );
    assert_eq!(
        fs::read_to_string(fixture.project.join("main.txt")).unwrap(),
        "header\nbody OLD text\nfooter\n"
    );
}

#[test]
fn foreign_capture_exclusive_journey_storage_and_denied_or_cancelled_authority() {
    let fixture = Fixture::new();
    let other = Fixture::new();
    let workspace = fixture.open();
    let foreign = other.open();
    assert!(
        workspace
            .coding_session(other.capture(&foreign), "foreign", 1)
            .is_err()
    );
    assert!(
        workspace
            .coding_session(fixture.capture(&workspace), "invalid", 3)
            .is_err()
    );
    let capture = fixture.capture(&workspace);
    let desired = capture.desired_target().to_owned();
    let mut coding = workspace.coding_session(capture, "cancel", 1).unwrap();
    let intent = coding
        .intent("main.txt", "header\nbody OLD text\nfooter\n", &desired)
        .unwrap();
    let approval = coding.approve_exact(&intent).unwrap();
    let cancel = EditCancellation::default();
    cancel.cancel();
    assert!(matches!(
        coding.authorize(intent, approval, &cancel),
        Err(CodingError::Cancelled)
    ));
    drop(coding);
    assert!(
        workspace
            .coding_session(fixture.capture(&workspace), "reuse", 1)
            .is_err()
    );
    let coding = workspace
        .coding_session(fixture.capture(&workspace), "second", 2)
        .unwrap();
    drop(coding);
    assert_eq!(
        fs::read_to_string(fixture.project.join("main.txt")).unwrap(),
        "header\nbody OLD text\nfooter\n"
    );
}
