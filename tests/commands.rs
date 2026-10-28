#[path = "support/commands.rs"]
mod fixture;
use fixture::{AUTOMATIC_CATALOG, Roots};
use scramjet_rs::*;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::Path,
};

fn load(roots: &Roots, policy: CommandPolicy) -> CommandSession {
    CommandSession::load(&roots.definitions, &roots.inputs, policy).unwrap()
}
fn executed(session: &mut CommandSession) -> CommandInvocation {
    let id = session.begin("compare").unwrap();
    session.execute(id).unwrap();
    id
}
fn complete(session: &mut CommandSession, id: CommandInvocation) {
    session
        .report(id, CommandCompletion::Completed, "equal", Some("inspect"))
        .unwrap();
}

#[test]
fn explicit_resume_is_live_keyed_admission_only_and_requires_own_report() {
    for (host, edge) in [
        (CommandPolicy::Pause, "Automatic"),
        (CommandPolicy::Automatic, "Pause"),
    ] {
        let roots = Roots::new(
            &AUTOMATIC_CATALOG.replace("Automatic", edge),
            b"same",
            b"same",
        )
        .unwrap();
        let mut session = load(&roots, host);
        assert!(session.paused_continuation().is_none());
        let primary = executed(&mut session);
        assert!(session.paused_continuation().is_none());
        complete(&mut session, primary);
        let handle = session.paused_continuation().unwrap();
        let paused = session.snapshot();
        let mut foreign = load(&roots, CommandPolicy::Pause);
        let id = executed(&mut foreign);
        complete(&mut foreign, id);
        let before = foreign.snapshot();
        assert!(foreign.resume(handle).is_err());
        assert_eq!(foreign.snapshot(), before);
        assert_eq!(session.snapshot(), paused);
        let companion = session.resume(handle).unwrap().unwrap();
        assert_eq!(companion.session(), primary.session());
        assert_eq!(companion.ordinal(), 2);
        let admitted = session.snapshot();
        assert_eq!(admitted.reads, 2);
        assert!(!admitted.settled);
        assert_eq!(
            admitted.invocations[1].as_ref().unwrap().phase,
            CommandPhase::Ready
        );
        assert!(session.paused_continuation().is_none());
        assert!(session.resume(handle).is_err());
        assert_eq!(session.snapshot(), admitted);
        assert!(
            session
                .report(
                    companion,
                    CommandCompletion::Completed,
                    "not executed",
                    None
                )
                .is_err()
        );
        session.execute(companion).unwrap();
        assert_eq!(
            session.snapshot().invocations[1].as_ref().unwrap().phase,
            CommandPhase::AwaitingCompletion
        );
        assert!(
            session.snapshot().invocations[1]
                .as_ref()
                .unwrap()
                .report
                .is_none()
        );
        session
            .report(companion, CommandCompletion::Completed, "readable", None)
            .unwrap();
        assert_eq!(session.snapshot().invocations[0], paused.invocations[0]);
        let final_state = session.finalize();
        assert_eq!(final_state.reads, 3);
        assert!(final_state.settled);
        assert_eq!(fs::read(roots.inputs.join("left.txt")).unwrap(), b"same");
        assert_eq!(fs::read_dir(&roots.inputs).unwrap().count(), 2);
    }
}

#[test]
fn paused_handle_cancellation_and_missing_companion_report_preserve_first_success() {
    for admitted in [false, true] {
        let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
        let mut session = load(&roots, CommandPolicy::Pause);
        let primary = executed(&mut session);
        complete(&mut session, primary);
        let first = session.snapshot().invocations[0].clone();
        let handle = session.paused_continuation().unwrap();
        if admitted {
            let id = session.resume(handle).unwrap().unwrap();
            session.control().cancel();
            session.execute(id).unwrap();
            assert!(
                session
                    .report(id, CommandCompletion::Completed, "no", None)
                    .is_err()
            );
        } else {
            session.control().cancel();
            assert!(session.paused_continuation().is_none());
            assert!(session.resume(handle).unwrap().is_none());
        }
        let state = session.finalize();
        assert_eq!(state.invocations[0], first);
        assert_eq!(state.reads, 2);
        if admitted {
            assert_eq!(
                state.invocations[1].as_ref().unwrap().phase,
                CommandPhase::Incomplete
            );
        } else {
            assert!(matches!(
                state.continuation,
                CommandContinuation::Suppressed { .. }
            ));
        }
    }
}

#[test]
fn equal_reads_need_explicit_report_and_default_pause_never_dispatches_companion() {
    let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
    let mut session = load(&roots, CommandPolicy::default());
    session.discover("compare").unwrap();
    assert!(session.discover("absent").is_err());
    let id = executed(&mut session);
    let state = session.snapshot();
    assert_eq!(
        state.invocations[0].as_ref().unwrap().phase,
        CommandPhase::AwaitingCompletion
    );
    assert!(state.invocations[0].as_ref().unwrap().report.is_none());
    assert_eq!(state.reads, 2);
    complete(&mut session, id);
    assert_eq!(
        session.snapshot().continuation,
        CommandContinuation::Paused {
            source: id,
            target: "inspect".into()
        }
    );
    assert!(session.follow().is_err());
    let state = session.finalize();
    assert!(state.settled);
    assert_eq!(state.reads, 2);
    assert!(state.invocations[1].is_none());
    assert_eq!(
        state.invocations[0].as_ref().unwrap().phase,
        CommandPhase::Completed
    );
}

#[test]
fn automatic_requires_both_policies_and_companion_own_completion() {
    for (edge, expected_reads) in [("Automatic", 3), ("Pause", 2)] {
        let roots = Roots::new(
            &AUTOMATIC_CATALOG.replace("Automatic", edge),
            b"same",
            b"same",
        )
        .unwrap();
        let mut session = load(&roots, CommandPolicy::Automatic);
        let id = executed(&mut session);
        complete(&mut session, id);
        if edge == "Automatic" {
            let companion = session.follow().unwrap().unwrap();
            assert_ne!(id, companion);
            assert_eq!(id.session(), companion.session());
            assert_eq!(companion.ordinal(), 2);
            assert_eq!(
                session.snapshot().invocations[1].as_ref().unwrap().phase,
                CommandPhase::AwaitingCompletion
            );
            assert!(
                session
                    .report(
                        companion,
                        CommandCompletion::Completed,
                        "ok",
                        Some("compare")
                    )
                    .is_err()
            );
            session
                .report(companion, CommandCompletion::Completed, "readable", None)
                .unwrap();
            assert!(session.follow().is_err());
            assert!(session.begin("inspect").is_err());
        } else {
            assert!(session.follow().is_err());
        }
        let state = session.finalize();
        assert_eq!(state.reads, expected_reads);
        assert!(
            state
                .invocations
                .iter()
                .flatten()
                .all(|f| f.phase == CommandPhase::Completed)
        );
    }
}

#[test]
fn missing_reports_are_incomplete_even_after_all_successful_tools() {
    let roots = Roots::new(AUTOMATIC_CATALOG, b"", b"").unwrap();
    let mut session = load(&roots, CommandPolicy::Automatic);
    executed(&mut session);
    let state = session.finalize();
    assert_eq!(
        state.invocations[0].as_ref().unwrap().phase,
        CommandPhase::Incomplete
    );
    assert!(state.invocations[0].as_ref().unwrap().report.is_none());
    assert_eq!(state.reads, 2);
    let mut session = load(&roots, CommandPolicy::Automatic);
    let id = executed(&mut session);
    complete(&mut session, id);
    session.follow().unwrap();
    let state = session.finalize();
    assert_eq!(
        state.invocations[0].as_ref().unwrap().phase,
        CommandPhase::Completed
    );
    assert_eq!(
        state.invocations[1].as_ref().unwrap().phase,
        CommandPhase::Incomplete
    );
}

#[test]
fn mismatch_and_read_failure_preserve_earlier_facts_without_success_or_edge() {
    for failed in [false, true] {
        let roots = Roots::new(AUTOMATIC_CATALOG, b"left", b"right").unwrap();
        if failed {
            fs::remove_file(roots.inputs.join("right.txt")).unwrap();
        }
        let mut session = load(&roots, CommandPolicy::Automatic);
        let id = executed(&mut session);
        assert!(
            session
                .report(id, CommandCompletion::Completed, "lie", Some("inspect"))
                .is_err()
        );
        let fact = session.snapshot().invocations[0].take().unwrap();
        assert_eq!(
            fact.steps[0].as_ref().unwrap().result,
            CommandReadResult::Read("left".into())
        );
        assert_eq!(
            matches!(
                fact.steps[1].as_ref().unwrap().result,
                CommandReadResult::Failed(_)
            ),
            failed
        );
        session
            .report(id, CommandCompletion::Failed, "comparison failed", None)
            .unwrap();
        assert!(session.follow().is_err());
        let state = session.finalize();
        assert_eq!(
            state.invocations[0].as_ref().unwrap().phase,
            CommandPhase::Failed
        );
        assert!(state.invocations[1].is_none());
    }
}

#[test]
fn rejected_reports_preserve_pending_facts_and_dispatch_nothing() {
    let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
    let mut session = load(&roots, CommandPolicy::Automatic);
    let id = session.begin("compare").unwrap();
    let before = session.snapshot();
    assert!(
        session
            .report(id, CommandCompletion::Completed, "premature", None)
            .is_err()
    );
    assert_eq!(session.snapshot(), before);
    session.execute(id).unwrap();
    let mut foreign = load(&roots, CommandPolicy::Automatic);
    let other = executed(&mut foreign);
    assert_ne!(id.session(), other.session());
    let before = session.snapshot();
    for (key, status, next) in [
        (other, CommandCompletion::Completed, Some("inspect")),
        (id, CommandCompletion::Completed, Some("missing")),
        (id, CommandCompletion::Completed, Some("compare")),
        (id, CommandCompletion::Failed, None),
        (id, CommandCompletion::Cancelled, None),
        (id, CommandCompletion::Incomplete, Some("inspect")),
    ] {
        assert!(session.report(key, status, "bad", next).is_err());
        assert_eq!(session.snapshot(), before);
    }
    complete(&mut session, id);
    let accepted = session.snapshot();
    assert!(
        session
            .report(
                id,
                CommandCompletion::Completed,
                "duplicate",
                Some("inspect")
            )
            .is_err()
    );
    assert!(session.execute(id).is_err());
    assert_eq!(session.snapshot(), accepted);
    let companion = session.follow().unwrap().unwrap();
    assert!(
        session
            .report(id, CommandCompletion::Completed, "stale", None)
            .is_err()
    );
    session
        .report(companion, CommandCompletion::Completed, "ok", None)
        .unwrap();
}

#[test]
fn cancellation_is_sticky_instance_bound_and_does_not_rewrite_prior_completion() {
    let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
    let mut session = load(&roots, CommandPolicy::Automatic);
    let mut other = load(&roots, CommandPolicy::Automatic);
    let id = session.begin("compare").unwrap();
    assert!(session.control().cancel());
    assert!(session.control().cancel());
    session.execute(id).unwrap();
    assert_eq!(session.snapshot().reads, 0);
    assert!(
        session
            .report(id, CommandCompletion::Completed, "no", None)
            .is_err()
    );
    session
        .report(
            id,
            CommandCompletion::Cancelled,
            "cancelled before read",
            None,
        )
        .unwrap();
    let other_id = executed(&mut other);
    complete(&mut other, other_id);
    assert!(other.control().cancel());
    assert_eq!(other.follow().unwrap(), None);
    let state = other.finalize();
    assert_eq!(
        state.invocations[0].as_ref().unwrap().phase,
        CommandPhase::Completed
    );
    assert_eq!(
        state.continuation,
        CommandContinuation::Suppressed { source: other_id }
    );
    let control = session.control();
    let state = session.finalize();
    assert_eq!(
        state.invocations[0].as_ref().unwrap().phase,
        CommandPhase::Cancelled
    );
    assert!(!control.cancel());
}

#[test]
fn cancellation_after_reads_accepts_only_non_success_and_missing_report_is_incomplete() {
    let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
    for report in [true, false] {
        let mut session = load(&roots, CommandPolicy::Automatic);
        let id = executed(&mut session);
        session.control().cancel();
        assert!(
            session
                .report(
                    id,
                    CommandCompletion::Completed,
                    "late lie",
                    Some("inspect")
                )
                .is_err()
        );
        if report {
            session
                .report(id, CommandCompletion::Cancelled, "stopped", None)
                .unwrap();
        }
        let state = session.finalize();
        assert_eq!(state.reads, 2);
        assert_eq!(
            state.invocations[0].as_ref().unwrap().phase,
            if report {
                CommandPhase::Cancelled
            } else {
                CommandPhase::Incomplete
            }
        );
    }
}

#[test]
fn loaded_definitions_and_root_descriptors_are_bound_to_invocations() {
    let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
    let mut session = load(&roots, CommandPolicy::Automatic);
    fs::write(roots.definitions.join("commands.json"), "not a catalog").unwrap();
    let old_inputs = roots.inputs.with_extension("original");
    fs::rename(&roots.inputs, &old_inputs).unwrap();
    fs::create_dir(&roots.inputs).unwrap();
    fs::write(roots.inputs.join("left.txt"), "changed").unwrap();
    let id = executed(&mut session);
    complete(&mut session, id);
    let companion = session.follow().unwrap().unwrap();
    session
        .report(companion, CommandCompletion::Completed, "readable", None)
        .unwrap();
    assert_eq!(session.finalize().reads, 3);
}

#[test]
fn exact_and_over_byte_and_retained_text_limits_reject_without_truncation() {
    let text = vec![b'x'; COMMAND_INPUT_BYTES];
    let roots = Roots::new(AUTOMATIC_CATALOG, &text, &text).unwrap();
    let mut session = load(&roots, CommandPolicy::Automatic);
    let id = executed(&mut session);
    let before = session.snapshot();
    assert!(
        session
            .report(id, CommandCompletion::Completed, &"x".repeat(513), None)
            .is_err()
    );
    assert!(
        session
            .report(id, CommandCompletion::Completed, "\x1b", None)
            .is_err()
    );
    assert_eq!(session.snapshot(), before);
    session
        .report(
            id,
            CommandCompletion::Completed,
            &"x".repeat(512),
            Some("inspect"),
        )
        .unwrap();
    let companion = session.follow().unwrap().unwrap();
    assert!(
        session
            .report(companion, CommandCompletion::Completed, "x", None)
            .is_err()
    );
    session
        .report(companion, CommandCompletion::Completed, "", None)
        .unwrap();
    let state = session.finalize();
    assert_eq!(state.retained_bytes, 3 * COMMAND_INPUT_BYTES);
    assert_eq!(state.retained_text_bytes, COMMAND_TEXT_BYTES);
    fs::write(
        roots.inputs.join("left.txt"),
        vec![b'x'; COMMAND_INPUT_BYTES + 1],
    )
    .unwrap();
    let mut session = load(&roots, CommandPolicy::Pause);
    let id = executed(&mut session);
    let state = session.snapshot();
    assert_eq!(state.reads, 1);
    assert_eq!(
        state.invocations[0].as_ref().unwrap().steps[0]
            .as_ref()
            .unwrap()
            .result,
        CommandReadResult::Failed(CommandReadFailure::Oversized)
    );
    session
        .report(id, CommandCompletion::Failed, "too large", None)
        .unwrap();
    let padded = format!(
        "{AUTOMATIC_CATALOG}{}",
        " ".repeat(COMMAND_CATALOG_BYTES - AUTOMATIC_CATALOG.len())
    );
    fs::write(roots.definitions.join("commands.json"), &padded).unwrap();
    assert!(CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause).is_ok());
    fs::write(roots.definitions.join("commands.json"), padded + " ").unwrap();
    assert!(CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause).is_err());
}

#[test]
fn hostile_files_fail_closed_for_catalog_and_inputs_without_blocking_fifo() {
    for catalog in [false, true] {
        for kind in [
            "symlink",
            "hardlink",
            "fifo",
            "directory",
            "non_utf8",
            "missing",
        ] {
            let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
            let path = if catalog {
                roots.definitions.join("commands.json")
            } else {
                roots.inputs.join("left.txt")
            };
            fs::remove_file(&path).unwrap();
            match kind {
                "symlink" => symlink(roots.inputs.join("right.txt"), &path).unwrap(),
                "hardlink" => fs::hard_link(roots.inputs.join("right.txt"), &path).unwrap(),
                "fifo" => rustix::fs::mknodat(
                    rustix::fs::CWD,
                    &path,
                    rustix::fs::FileType::Fifo,
                    rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
                    0,
                )
                .unwrap(),
                "directory" => fs::create_dir(&path).unwrap(),
                "non_utf8" => fs::write(&path, [255]).unwrap(),
                _ => (),
            }
            if catalog {
                assert!(
                    CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause)
                        .is_err(),
                    "{kind}"
                );
            } else {
                let mut session = load(&roots, CommandPolicy::Pause);
                let id = executed(&mut session);
                let state = session.snapshot();
                assert_eq!(state.reads, 1, "{kind}");
                assert!(
                    matches!(
                        state.invocations[0].as_ref().unwrap().steps[0]
                            .as_ref()
                            .unwrap()
                            .result,
                        CommandReadResult::Failed(_)
                    ),
                    "{kind}"
                );
                session
                    .report(id, CommandCompletion::Failed, "read failed", None)
                    .unwrap();
            }
        }
    }
}

#[test]
fn root_permission_alias_and_symlink_boundaries_fail_closed() {
    let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
    assert!(CommandSession::load(&roots.inputs, &roots.inputs, CommandPolicy::Pause).is_err());
    for path in [&roots.definitions, &roots.inputs] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause).is_err()
        );
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let link = roots.inputs.with_extension("link");
    symlink(&roots.inputs, &link).unwrap();
    assert!(CommandSession::load(&roots.definitions, &link, CommandPolicy::Pause).is_err());
}

#[test]
fn malformed_definitions_reject_before_any_execution() {
    let cases = [
        "[]".into(), "{}".into(), "{\"commands\":[]}".into(),
        AUTOMATIC_CATALOG.replace("\"commands\":", "\"unknown\":"),
        AUTOMATIC_CATALOG.replace("\"name\":\"compare\"", "\"name\":\"compare\",\"name\":\"compare\""),
        AUTOMATIC_CATALOG.replace("\"read\":\"left.txt\"", "\"shell\":\"true\""),
        AUTOMATIC_CATALOG.replace("\"outcome\":\"Equal\"", "\"outcome\":\"Unknown\""),
        AUTOMATIC_CATALOG.replace("\"policy\":\"Automatic\"", "\"policy\":\"Unknown\""),
        AUTOMATIC_CATALOG.replace("\"command\":\"inspect\"", "\"command\":\"missing\""),
        AUTOMATIC_CATALOG.replace("\"command\":\"inspect\"", "\"command\":\"compare\""),
        AUTOMATIC_CATALOG.replace("\"name\":\"inspect\"", "\"name\":\"compare\""),
        AUTOMATIC_CATALOG.replace("\"read\":\"right.txt\"", "\"read\":\"../right.txt\""),
        AUTOMATIC_CATALOG.replace("\"read\":\"right.txt\"", "\"read\":\"...\""),
        AUTOMATIC_CATALOG.replace("\"read\":\"right.txt\"", "\"read\":\"/right.txt\""),
        AUTOMATIC_CATALOG.replace("\"name\":\"compare\"", "\"name\":\"bad.name\""),
        AUTOMATIC_CATALOG.replace("{\"read\":\"right.txt\"}", "{\"read\":\"right.txt\"},{\"read\":\"left.txt\"}"),
        AUTOMATIC_CATALOG.replace("\"Equal\"", "\"Readable\""),
        AUTOMATIC_CATALOG.replace("{\"read\":\"right.txt\"}", "[\"right.txt\"]"),
        AUTOMATIC_CATALOG.replace("{\"command\":\"inspect\",\"policy\":\"Automatic\"}", "[\"inspect\",\"Automatic\"]"),
        r#"{"commands":[["compare","Equal",[{"read":"left.txt"},{"read":"right.txt"}]]]}"#.into(),
        r#"{"commands":[{"name":"a","outcome":"Readable","steps":[{"read":"left.txt"}],"next":{"command":"b","policy":"Automatic"}},{"name":"b","outcome":"Readable","steps":[{"read":"left.txt"}],"next":{"command":"a","policy":"Pause"}}]}"#.into(),
    ];
    for catalog in cases {
        let roots = Roots::new(&catalog, b"same", b"same").unwrap();
        assert!(
            CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Automatic)
                .is_err(),
            "{catalog}"
        );
    }
}

#[test]
fn failed_companion_retains_primary_completion_and_never_retries() {
    let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
    let mut session = load(&roots, CommandPolicy::Automatic);
    let id = executed(&mut session);
    complete(&mut session, id);
    fs::remove_file(roots.inputs.join("left.txt")).unwrap();
    let companion = session.follow().unwrap().unwrap();
    session
        .report(companion, CommandCompletion::Failed, "read failed", None)
        .unwrap();
    assert!(session.follow().is_err());
    assert!(session.execute(companion).is_err());
    let state = session.finalize();
    assert_eq!(state.reads, 3);
    assert_eq!(
        state.invocations[0].as_ref().unwrap().phase,
        CommandPhase::Completed
    );
    assert_eq!(
        state.invocations[1].as_ref().unwrap().phase,
        CommandPhase::Failed
    );
}

#[test]
fn finalize_closes_unstarted_work_and_dropped_controls_do_not_cancel() {
    let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
    for policy in [CommandPolicy::Automatic, CommandPolicy::Pause] {
        let mut session = load(&roots, policy);
        drop(session.control());
        let id = executed(&mut session);
        complete(&mut session, id);
        let control = session.control();
        if policy == CommandPolicy::Pause {
            control.cancel();
        }
        let state = session.finalize();
        assert_eq!(
            state.invocations[0].as_ref().unwrap().phase,
            CommandPhase::Completed
        );
        assert_eq!(
            state.continuation,
            CommandContinuation::Suppressed { source: id }
        );
        assert_eq!(state.reads, 2);
        assert!(state.invocations[1].is_none());
        assert!(!control.cancel());
    }
    let session = load(&roots, CommandPolicy::Automatic);
    let control = session.control();
    drop(session);
    assert!(!control.cancel());
}

fn contents(path: &Path) -> Vec<(String, u64, Vec<u8>)> {
    let mut entries: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                entry.metadata().unwrap().ino(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    entries.sort();
    entries
}
#[test]
fn command_execution_creates_or_changes_no_definition_or_input_entries() {
    let roots = Roots::new(
        AUTOMATIC_CATALOG,
        b"\x1b[31m\n\t\xe2\x80\xae",
        b"\x1b[31m\n\t\xe2\x80\xae",
    )
    .unwrap();
    let before = (contents(&roots.definitions), contents(&roots.inputs));
    let cwd = std::env::current_dir().unwrap();
    let mut session = load(&roots, CommandPolicy::Automatic);
    let id = executed(&mut session);
    complete(&mut session, id);
    let companion = session.follow().unwrap().unwrap();
    session
        .report(companion, CommandCompletion::Completed, "readable", None)
        .unwrap();
    assert!(session.finalize().settled);
    assert_eq!(
        (contents(&roots.definitions), contents(&roots.inputs)),
        before
    );
    assert_eq!(std::env::current_dir().unwrap(), cwd);
}
