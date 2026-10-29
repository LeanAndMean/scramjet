use std::process::Command;

#[test]
fn command_readers_demo_reports_logical_completion_and_actual_joined_settlement() {
    let output = Command::new("timeout")
        .args([
            "20",
            env!("CARGO_BIN_EXE_scramjet-rs"),
            "command-readers-demo",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("children=3 attempts=3 active_overlap=2/2 pending=1 out_of_order=1-before-0")
    );
    assert!(
        text.contains("before_report=AwaitingCompletion explicit_report=Completed settled=true")
    );
    assert!(text.contains("fixture_joined=true reconstructed=3 no_replay=true"));
    assert!(text.contains("active=0 pending=0 handles=0 unresolved=0 obligations=0 debt=0"));
}

#[test]
fn command_coding_demo_reports_correlated_explicit_completion_and_joined_settlement() {
    let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .arg("command-coding-demo")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    for required in [
        "command=replace outcome=ReplaceVerified target=sample.txt input_bytes=44",
        "phase=Completed report=Some(CommandReportFact",
        "command_local_settled=true cancellation_requested=false",
        "worker_terminated=true worker_joined=true",
        "post_attestation: active=0 pending=0 handles=0 exposure=0 unknown_usage=0 obligations=0 debt=0",
    ] {
        assert!(text.contains(required), "{text}");
    }
    assert!(
        text.contains("turn=4 recorded=true")
            && !text.contains("turn=5 ")
            && !text.contains("journey=2 ")
    );
}

#[test]
fn stream_demo_reports_owned_overlap_and_reclamation() {
    let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .arg("stream-demo")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    for ceiling in [1, 2, 4] {
        let line = text
            .lines()
            .find(|line| line.starts_with(&format!("active_ceiling={ceiling} ")))
            .unwrap();
        assert!(line.contains(&format!("client_peak={ceiling} fixture_peak={ceiling}")));
        assert!(line.contains("completed=12 errors=0 cancelled=0"));
        assert!(line.contains("unresolved=0 retries=0"));
        assert!(line.ends_with("local_final=0"));
    }
    assert!(text.contains("errors=1 cancelled=1 slow_retained_bytes=8192"));
    assert!(output.stderr.is_empty());
}

#[test]
fn coding_demo_reports_correlated_verified_joined_result() {
    let output = Command::new("timeout")
        .arg("15")
        .arg(env!("CARGO_BIN_EXE_scramjet-rs"))
        .arg("coding-demo")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Exact disposable edit completed and verified."));
    assert!(text.contains("turns=4 admitted=4 edit_operation=1 verified=true synthetic_usage=4 worker=joined local_final=0 unresolved=0"));
}

#[test]
fn cooldown_demo_reports_timed_idle_recovery_and_no_retry() {
    let output = Command::new("timeout")
        .arg("30")
        .arg(env!("CARGO_BIN_EXE_scramjet-rs"))
        .arg("cooldown-demo")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("attempts=8 refused=1 completed=7 cancelled_queued=2 retries=0 overlap=2/2 pending_peak=8 commitments=8"));
    assert!(text.contains("joined=true local_final=0 unresolved=0"));
}

#[test]
fn pacing_demo_reports_separate_commitment_receipt_and_joined_evidence() {
    let output = Command::new("timeout")
        .arg("30")
        .arg(env!("CARGO_BIN_EXE_scramjet-rs"))
        .arg("pacing-demo")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("burst=2 interval_ms=250 commitments=8 attempts=8 receipts=8 completed=8 cancelled_queued=2 retries=0 overlap=2/2 pending_peak=8"));
    assert!(text.contains("commitment_ms=["));
    assert!(text.contains("receipt_ms=["));
    assert!(text.contains("joined=true local_final=0 unresolved=0"));
}

#[test]
fn adaptive_demo_reports_owned_fixed_comparison_pressure_and_recovery() {
    let output = Command::new("timeout")
        .arg("60")
        .arg(env!("CARGO_BIN_EXE_scramjet-rs"))
        .arg("adaptive-demo")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    for mode in ["Fixed", "Adaptive"] {
        let line = text
            .lines()
            .find(|l| l.starts_with(&format!("adaptive-demo: mode={mode} ")))
            .unwrap();
        for field in [
            "workload=48",
            "capacity=[4,1,1,4,4,4]",
            "commitments=48 attempts=48 receipts=48",
            "retries=0",
            "client_peak=4",
            "usage_peak=4 exposure_peak=4",
            "head_us_p50_p95_max=",
            "completion_us_p50_p95_max=",
            "receipt_logical_id_status=",
            "commitment_logical_id_order_ms=",
            "joined=true local_final=0 exposure_final=0 usage_final=0 debt_final=0",
        ] {
            assert!(line.contains(field), "{line}");
        }
    }
}

#[test]
fn command_demo_reports_explicit_completion_and_actual_policy_qualified_reads() {
    for option in [None, Some("--automatic"), Some("--resume")] {
        let automatic = option == Some("--automatic");
        let followed = option.is_some();
        let mut command = Command::new("timeout");
        command
            .arg("10")
            .arg(env!("CARGO_BIN_EXE_scramjet-rs"))
            .arg("command-demo");
        if let Some(option) = option {
            command.arg(option);
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("before_report=AwaitingCompletion reads=2"));
        assert_eq!(
            text.matches("explicit_report=Completed").count(),
            if followed { 2 } else { 1 }
        );
        assert_eq!(
            text.matches("captured_bytes=5").count(),
            if followed { 3 } else { 2 }
        );
        assert!(text.contains(if automatic {
            "host=Automatic continuation=Followed"
        } else if followed {
            "host=Pause continuation=Followed"
        } else {
            "host=Pause continuation=Paused"
        }));
        assert!(text.contains(if followed {
            "invocations=2 reads=3 retained_bytes=15 settled=true local_final=0"
        } else {
            "invocations=1 reads=2 retained_bytes=10 settled=true local_final=0"
        }));
        assert!(!text.as_bytes().contains(&27));
    }
}

#[test]
fn smoke_runs_the_headless_library() {
    let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .arg("smoke")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, b"scramjet-rs: offline bootstrap ready\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn help_and_no_arguments_show_usage() {
    for arguments in [vec![], vec!["--help"], vec!["-h"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .args(arguments)
            .output()
            .unwrap();

        assert!(output.status.success());
        assert_eq!(
            output.stdout,
            b"Usage: scramjet-rs smoke\n       scramjet-rs stream-demo\n       scramjet-rs coding-demo\n       scramjet-rs coding --owned-fixture --endpoint ADDR --project PATH --storage PATH --run ID --file NAME --file NAME --prompt-file PATH [--record-file PATH] [--approve-effects]\n       scramjet-rs parent-demo\n       scramjet-rs cooldown-demo\n       scramjet-rs pacing-demo\n       scramjet-rs token-demo\n       scramjet-rs adaptive-demo\n       scramjet-rs command-demo [--automatic | --resume]\n       scramjet-rs command-coding-demo\n       scramjet-rs command-readers-demo\n       scramjet-rs provider-demo --live|--fixture --storage PATH [--tool] [--retain-unknown-through N]\n       scramjet-rs haiku-demo --fixture --project PATH --storage PATH [--approve-effects] [--session ID]\n       scramjet-rs haiku-demo --history --storage PATH [--session ID]\n       scramjet-rs haiku-ui-demo --fixture --project PATH --storage PATH [--session ID]\n       scramjet-rs haiku-ui-demo --history --storage PATH [--session ID]\n       scramjet-rs project-demo --project PATH --storage PATH --request JSON [--approve-edit] [--verify-edited] [--second-request JSON] [--session ID]\n       scramjet-rs project-demo --history --storage PATH [--session ID]\n       scramjet-rs project-ui-demo --project PATH --storage PATH [--session ID] [--fixture golden|reader-failure|model-failure|unknown-usage]\n       scramjet-rs command-ui-demo [--automatic]\n       scramjet-rs ui-demo [--diagnostic]\n       scramjet-rs runtime-ui-demo\n       scramjet-rs reader-ui-demo [--command]\n       scramjet-rs coding-ui-demo [--follow-up | --command]\n       scramjet-rs --help\n"
        );
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn provider_demo_requires_explicit_network_selection_and_runs_offline_envelopes() {
    for arguments in [
        vec!["provider-demo"],
        vec!["provider-demo", "--storage", "unused"],
        vec!["provider-demo", "--live", "--storage", "unused", "--retry"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("requires --live or --fixture"));
    }
    for tool in [false, true] {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!(
            "provider-cli-{}-{}",
            std::process::id(),
            if tool { "call" } else { "text" }
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"));
        command
            .args(["provider-demo", "--fixture", "--storage"])
            .arg(&path);
        if tool {
            command.arg("--tool");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(
            "consumed=1 remaining=31 unresolved_remote=0 unknown_usage=0 local_active=0"
        ));
        assert!(text.contains(if tool {
            "validated function-call envelope; no tools executed"
        } else {
            "validated text envelope; no tools executed"
        }));
    }
}

#[test]
fn unsupported_arguments_fail_without_smoke_output() {
    for arguments in [
        vec!["unknown"],
        vec!["smoke", "extra"],
        vec!["--help", "extra"],
        vec!["--diagnostic"],
        vec!["--diagnostic", "ui-demo"],
        vec!["ui-demo", "unknown"],
        vec!["ui-demo", "--diagnostic", "--diagnostic"],
        vec!["ui-demo", "--diagnostic", "extra"],
        vec!["ui-demo", "extra", "--diagnostic"],
        vec!["smoke", "--diagnostic"],
        vec!["stream-demo", "--diagnostic"],
        vec!["coding-demo", "extra"],
        vec!["parent-demo", "extra"],
        vec!["cooldown-demo", "extra"],
        vec!["pacing-demo", "extra"],
        vec!["token-demo", "extra"],
        vec!["adaptive-demo", "extra"],
        vec!["command-ui-demo", "extra"],
        vec!["command-ui-demo", "--automatic", "extra"],
        vec!["command-ui-demo", "--diagnostic"],
        vec!["command-demo", "extra"],
        vec!["command-demo", "--automatic", "extra"],
        vec!["command-demo", "--automatic", "--automatic"],
        vec!["command-demo", "--diagnostic"],
        vec!["command-demo", "--resume", "--automatic"],
        vec!["command-demo", "--resume", "--resume"],
        vec!["smoke", "--automatic"],
        vec!["runtime-ui-demo", "extra"],
        vec!["runtime-ui-demo", "--diagnostic"],
        vec!["command-readers-demo", "extra"],
        vec!["reader-ui-demo", "--command", "extra"],
        vec!["reader-ui-demo", "--command", "--command"],
        vec!["reader-ui-demo", "extra"],
        vec!["reader-ui-demo", "--diagnostic"],
        vec!["coding-ui-demo", "extra"],
        vec!["coding-ui-demo", "--diagnostic"],
        vec!["coding-ui-demo", "--follow-up", "extra"],
        vec!["coding-ui-demo", "--follow-up", "--follow-up"],
        vec!["coding-ui-demo", "--follow-up", "--command"],
        vec!["coding-ui-demo", "--command", "extra"],
        vec!["command-coding-demo", "extra"],
        vec!["coding-demo", "--endpoint"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .args(arguments)
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            b"Usage: scramjet-rs smoke\n       scramjet-rs stream-demo\n       scramjet-rs coding-demo\n       scramjet-rs coding --owned-fixture --endpoint ADDR --project PATH --storage PATH --run ID --file NAME --file NAME --prompt-file PATH [--record-file PATH] [--approve-effects]\n       scramjet-rs parent-demo\n       scramjet-rs cooldown-demo\n       scramjet-rs pacing-demo\n       scramjet-rs token-demo\n       scramjet-rs adaptive-demo\n       scramjet-rs command-demo [--automatic | --resume]\n       scramjet-rs command-coding-demo\n       scramjet-rs command-readers-demo\n       scramjet-rs provider-demo --live|--fixture --storage PATH [--tool] [--retain-unknown-through N]\n       scramjet-rs haiku-demo --fixture --project PATH --storage PATH [--approve-effects] [--session ID]\n       scramjet-rs haiku-demo --history --storage PATH [--session ID]\n       scramjet-rs haiku-ui-demo --fixture --project PATH --storage PATH [--session ID]\n       scramjet-rs haiku-ui-demo --history --storage PATH [--session ID]\n       scramjet-rs project-demo --project PATH --storage PATH --request JSON [--approve-edit] [--verify-edited] [--second-request JSON] [--session ID]\n       scramjet-rs project-demo --history --storage PATH [--session ID]\n       scramjet-rs project-ui-demo --project PATH --storage PATH [--session ID] [--fixture golden|reader-failure|model-failure|unknown-usage]\n       scramjet-rs command-ui-demo [--automatic]\n       scramjet-rs ui-demo [--diagnostic]\n       scramjet-rs runtime-ui-demo\n       scramjet-rs reader-ui-demo [--command]\n       scramjet-rs coding-ui-demo [--follow-up | --command]\n       scramjet-rs --help\n"
        );
    }
}

#[test]
fn parent_demo_reports_actual_parallelism_and_joined_reconstruction() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .arg("parent-demo")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("active_overlap=2/2 pending=1"));
    assert!(text.contains("persistence=true runtime=true"));
    assert!(text.contains("reconstructed=3 no_replay=true"));
}

#[test]
fn token_demo_reports_conservative_charges_and_known_synthetic_usage() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .arg("token-demo")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    for field in [
        "capacity=8 quantum=2 interval_ms=250",
        "commitments=8 attempts=8 receipts=8",
        "synthetic_measured=8 conservative_charges=16",
        "overlap=2/2",
        "joined=true local_final=0 unresolved=0 usage_obligations=0 debt=0",
    ] {
        assert!(text.contains(field), "{text}");
    }
}
