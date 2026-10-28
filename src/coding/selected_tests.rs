use super::tests::{PROCESS_GATE, support::OwnedRoot};
use super::*;
use std::fs;
use std::process::Command;

fn selected_root() -> OwnedRoot {
    use std::os::unix::fs::PermissionsExt;
    let root = OwnedRoot::new();
    fs::set_permissions(&root.path, fs::Permissions::from_mode(0o755)).unwrap();
    for name in ["sample.txt", "rules.txt", "notes.txt"] {
        if name != "sample.txt" {
            fs::write(root.path.join(name), name).unwrap();
        }
        fs::set_permissions(root.path.join(name), fs::Permissions::from_mode(0o644)).unwrap();
    }
    root
}

#[test]
fn pre_rename_recheck_preserves_changed_checkout_and_uncertain_stage() {
    let _gate = PROCESS_GATE.read().unwrap();
    for target in ["sample.txt", "notes.txt", ".scramjet-stage-rename-gate-1"] {
        let root = selected_root();
        let storage = OwnedRoot::new();
        let workspace = crate::ProjectWorkspace::open(&root.path, &storage.path).unwrap();
        let request = crate::ProjectRequest::parse(r#"{"operation":"replace_once","target":"sample.txt","supporting":["rules.txt","notes.txt"],"needle":"before","replacement":"after"}"#).unwrap();
        let mut session = workspace
            .coding_session(workspace.capture(request).unwrap(), "rename-gate", 1)
            .unwrap();
        let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
        let approval = session.approve_exact(&intent).unwrap();
        let cancel = EditCancellation::default();
        session.authorize(intent, approval, &cancel).unwrap();
        let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let entered = Arc::new(AtomicBool::new(false));
        session.checkpoint = Some(Checkpoint::Wait(
            CheckpointPhase::Rename,
            release.clone(),
            entered.clone(),
        ));
        let worker = std::thread::spawn(move || session.dispatch(&cancel).unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !entered.load(Ordering::Acquire) {
            assert!(
                std::time::Instant::now() < deadline,
                "pre-rename gate not reached"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        fs::write(root.path.join(target), "external change\n").unwrap();
        *release.0.lock().unwrap() = true;
        release.1.notify_all();
        let result = worker.join().unwrap();
        assert_eq!(result.state, OperationState::Uncertain);
        assert!(result.acknowledgement_error.is_some());
        assert_eq!(
            fs::read_to_string(root.path.join(target)).unwrap(),
            "external change\n"
        );
        assert_eq!(
            fs::read_to_string(root.path.join(".scramjet-stage-rename-gate-1")).unwrap(),
            if target == ".scramjet-stage-rename-gate-1" {
                "external change\n"
            } else {
                "after\n"
            }
        );
        assert_eq!(
            crate::ProjectWorkspace::coding_history(
                &storage.path,
                1,
                "rename-gate",
                workspace.root_identity()
            )
            .unwrap()
            .operations()[0]
                .state,
            OperationState::Uncertain
        );
    }
}

#[test]
fn post_rename_exit_recovers_without_project_or_replay_in_three_fresh_processes() {
    use std::os::unix::fs::MetadataExt;
    let _gate = PROCESS_GATE.write().unwrap();
    let root = selected_root();
    let storage = OwnedRoot::new();
    let metadata = fs::metadata(&root.path).unwrap();
    let run = |mode: &str| {
        Command::new("timeout")
            .arg("10")
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "coding::tests::restart_process_helper",
                "--nocapture",
            ])
            .env("SCRAMJET_CODING_TEST_ROOT", &root.path)
            .env("SCRAMJET_CODING_TEST_MODE", mode)
            .env("SCRAMJET_SELECTED_STORAGE", &storage.path)
            .env("SCRAMJET_SELECTED_DEVICE", metadata.dev().to_string())
            .env("SCRAMJET_SELECTED_INODE", metadata.ino().to_string())
            .output()
            .unwrap()
    };
    let result = run("selected-interrupt");
    assert_eq!(result.status.code(), Some(17), "{result:?}");
    let journal_path = storage.path.join("coding-1/.coding-journal");
    let bytes = fs::read(&journal_path).unwrap();
    let target = fs::read(root.path.join("sample.txt")).unwrap();
    assert_eq!(&target, b"after\n");
    assert_eq!(
        fs::metadata(root.path.join("sample.txt")).unwrap().mode() & 0o7777,
        0o644
    );
    let moved = root.path.with_extension("moved");
    fs::rename(&root.path, &moved).unwrap();
    for _ in 0..3 {
        let result = run("selected-recover");
        assert!(result.status.success(), "{result:?}");
        assert_eq!(fs::read(&journal_path).unwrap(), bytes);
        assert_eq!(fs::read(moved.join("sample.txt")).unwrap(), target);
        assert!(!root.path.exists());
    }
    fs::rename(moved, &root.path).unwrap();
}
