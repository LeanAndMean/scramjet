use std::process::Command;

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
            b"Usage: scramjet-rs smoke\n       scramjet-rs --help\n"
        );
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn unsupported_arguments_fail_without_smoke_output() {
    for arguments in [
        vec!["unknown"],
        vec!["smoke", "extra"],
        vec!["--help", "extra"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .args(arguments)
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            b"Usage: scramjet-rs smoke\n       scramjet-rs --help\n"
        );
    }
}
