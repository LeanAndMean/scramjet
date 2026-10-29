use super::*;
use std::os::unix::fs::DirBuilderExt;

#[cfg(target_os = "linux")]
#[test]
fn accounting_observation_never_waits_for_runtime_or_ledger_locks() {
    let output = std::process::Command::new("timeout")
        .arg("10")
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runtime::provider_observation_tests::observation_child",
            "--nocapture",
        ])
        .env("S414C_OBSERVATION_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[cfg(target_os = "linux")]
#[test]
fn observation_child() {
    if std::env::var_os("S414C_OBSERVATION_CHILD").is_none() {
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let directory =
                std::env::temp_dir().join(format!("s414c-observer-{}", std::process::id()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            let runtime =
                Runtime::new_provider("127.0.0.1:1".parse().unwrap(), &directory).unwrap();
            assert_eq!(
                runtime.try_provider_history().unwrap().unwrap().consumed(),
                0
            );
            let provider = {
                let state = runtime.shared.state.lock().unwrap();
                assert!(runtime.try_provider_history().unwrap().is_none());
                state.provider.as_ref().unwrap().clone()
            };
            {
                let _held = provider.lock().unwrap();
                assert!(runtime.try_provider_history().unwrap().is_none());
            }
            assert_eq!(
                runtime.try_provider_history().unwrap().unwrap().consumed(),
                0
            );
            let poisoning = provider.clone();
            assert!(
                std::thread::spawn(move || {
                    let _held = poisoning.lock().unwrap();
                    panic!("synthetic accounting poison");
                })
                .join()
                .is_err()
            );
            assert_eq!(
                runtime.try_provider_history().unwrap_err(),
                "provider accounting ledger poisoned"
            );
            let local = runtime.shutdown().await;
            assert_eq!(local.active, 0);
            std::fs::remove_dir_all(directory).unwrap();
        });
}
