use super::*;
use std::time::{Duration, Instant};
fn loaded(policy: CommandPolicy) -> (fixture::Roots, CommandSession, Bridge) {
    let roots = fixture::Roots::new(fixture::AUTOMATIC_CATALOG, b"same", b"same").unwrap();
    let session = CommandSession::load(&roots.definitions, &roots.inputs, policy).unwrap();
    let bridge = Bridge::new(policy);
    bridge.attach(&session);
    (roots, session, bridge)
}
#[test]
fn only_painted_current_offer_is_admitted_once_and_stop_is_sticky() {
    let (_roots, mut session, bridge) = loaded(CommandPolicy::Pause);
    let offer = bridge.snapshot().offer;
    assert!(!bridge.invoke(offer));
    assert!(!bridge.invoke(None));
    bridge.painted(Some(Offer::Compare(session.observer().session() + 1)));
    assert!(!bridge.invoke(offer));
    bridge.painted(offer);
    bridge.painted(None);
    assert!(!bridge.invoke(offer));
    bridge.painted(offer);
    assert!(!bridge.invoke(Some(Offer::Compare(session.observer().session() + 1))));
    assert!(bridge.invoke(offer));
    assert!(!bridge.invoke(offer));
    execute_primary(&bridge, &mut session).unwrap();
    assert_eq!(session.snapshot().reads, 2);
    assert_eq!(
        bridge.snapshot().markers[0].unwrap().phase,
        CommandPhase::AwaitingCompletion
    );
    assert!(matches!(
        session.snapshot().continuation,
        CommandContinuation::Paused { .. }
    ));
    bridge.stop();
    bridge.painted(offer);
    assert!(!bridge.invoke(offer));
    assert_eq!(
        session.finalize().invocations[0].as_ref().unwrap().phase,
        CommandPhase::Completed
    );
}
#[test]
fn companion_offer_is_fresh_and_stop_does_not_release_terminal_ownership() {
    for resume in [false, true] {
        let bridge = Bridge::new(CommandPolicy::Pause);
        let owner = bridge.clone();
        let worker = std::thread::spawn(move || {
            journey(&owner, fixture::AUTOMATIC_CATALOG, b"same", b"same")
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        let wait = |predicate: &dyn Fn(&Projection) -> bool| {
            loop {
                let p = bridge.snapshot();
                if predicate(&p) {
                    return p;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        };
        let primary = wait(&|p| matches!(p.offer, Some(Offer::Compare(_)))).offer;
        bridge.painted(primary);
        assert!(bridge.invoke(primary));
        let paused = wait(&|p| matches!(p.offer, Some(Offer::Resume(_))));
        let root = paused.retained_root.unwrap();
        let first = paused.observation.unwrap().invocations[0].clone();
        assert!(!bridge.invoke(primary));
        assert!(!bridge.invoke(paused.offer));
        if resume {
            bridge.painted(paused.offer);
            assert!(!bridge.invoke(primary));
            assert!(bridge.invoke(paused.offer));
            assert!(!bridge.invoke(paused.offer));
            wait(&|p| {
                p.observation.as_ref().unwrap().invocations[1]
                    .as_ref()
                    .is_some_and(|f| f.report.is_some())
            });
        }
        bridge.stop();
        assert!(root.exists());
        assert!(!bridge.snapshot().observation.unwrap().settled);
        assert!(!worker.is_finished());
        bridge.terminal_returned();
        worker.join().unwrap().unwrap();
        let s = bridge.snapshot().observation.unwrap();
        assert_eq!(s.invocations[0], first);
        assert_eq!(s.reads, if resume { 3 } else { 2 });
        assert!(s.settled);
        assert!(!root.exists());
    }
}

#[test]
fn stop_before_attach_or_invocation_wakes_owner_without_reads() {
    for attached in [false, true] {
        let (_roots, mut session, bridge) = loaded(CommandPolicy::Pause);
        if !attached {
            let bridge = Bridge::new(CommandPolicy::Pause);
            bridge.stop();
            bridge.attach(&session);
            execute(&bridge, &mut session).unwrap();
            assert_eq!(session.finalize().reads, 0);
        } else {
            bridge.stop();
            execute(&bridge, &mut session).unwrap();
            assert_eq!(session.finalize().reads, 0);
        }
    }
}
#[test]
fn invocation_stop_races_never_admit_more_than_one() {
    for _ in 0..100 {
        let (_roots, mut session, bridge) = loaded(CommandPolicy::Automatic);
        let offer = bridge.snapshot().offer;
        bridge.painted(offer);
        let other = bridge.clone();
        let stop = std::thread::spawn(move || other.stop());
        bridge.invoke(offer);
        execute(&bridge, &mut session).unwrap();
        stop.join().unwrap();
        let final_state = session.finalize();
        assert!(final_state.reads <= 3);
        assert!(final_state.invocations.iter().flatten().count() <= 2);
        assert!(final_state.settled);
    }
}
#[test]
fn actual_journeys_retain_keyed_markers_reports_and_cleanup() {
    for (policy, catalog, right, phase, reads) in [
        (
            CommandPolicy::Pause,
            fixture::AUTOMATIC_CATALOG.to_owned(),
            b"same".as_slice(),
            CommandPhase::Completed,
            2,
        ),
        (
            CommandPolicy::Automatic,
            fixture::AUTOMATIC_CATALOG.to_owned(),
            b"same".as_slice(),
            CommandPhase::Completed,
            3,
        ),
        (
            CommandPolicy::Automatic,
            fixture::AUTOMATIC_CATALOG.replace("Automatic", "Pause"),
            b"same".as_slice(),
            CommandPhase::Completed,
            2,
        ),
        (
            CommandPolicy::Automatic,
            fixture::AUTOMATIC_CATALOG.to_owned(),
            b"different".as_slice(),
            CommandPhase::Failed,
            2,
        ),
    ] {
        let bridge = Bridge::new(policy);
        let worker_bridge = bridge.clone();
        let worker = std::thread::spawn(move || journey(&worker_bridge, &catalog, b"same", right));
        let deadline = Instant::now() + Duration::from_secs(3);
        while bridge.snapshot().offer.is_none() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let p = bridge.snapshot();
        assert_eq!(p.observation.as_ref().unwrap().reads, 0);
        bridge.painted(p.offer);
        assert!(bridge.invoke(p.offer));
        while bridge.snapshot().observation.as_ref().unwrap().invocations[0]
            .as_ref()
            .is_none_or(|f| f.report.is_none())
        {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        if reads == 3 {
            while bridge.snapshot().observation.as_ref().unwrap().invocations[1]
                .as_ref()
                .is_none_or(|f| f.report.is_none())
            {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        assert!(bridge.snapshot().retained_root.as_ref().unwrap().exists());
        assert!(!bridge.snapshot().observation.as_ref().unwrap().settled);
        bridge.stop();
        bridge.terminal_returned();
        worker.join().unwrap().unwrap();
        let p = bridge.snapshot();
        let s = p.observation.as_ref().unwrap();
        assert_eq!(s.reads, reads);
        assert!(s.settled);
        assert!(p.retained_root.is_none());
        assert_eq!(s.invocations[0].as_ref().unwrap().phase, phase);
        for fact in s.invocations.iter().flatten() {
            assert_eq!(
                p.markers[fact.id.ordinal() as usize - 1].unwrap().id,
                fact.id
            );
            assert!(fact.report.is_some());
        }
    }
}
#[test]
fn missing_report_and_returned_fact_cancellation_do_not_become_success() {
    let (_roots, mut session, bridge) = loaded(CommandPolicy::Automatic);
    let id = session.begin("compare").unwrap();
    session.execute(id).unwrap();
    bridge.mark(&session, id).unwrap();
    bridge.stop();
    report(&mut session, id).unwrap();
    let s = session.finalize();
    assert_eq!(s.reads, 2);
    assert_eq!(
        s.invocations[0].as_ref().unwrap().phase,
        CommandPhase::Cancelled
    );
    assert_eq!(s.continuation, CommandContinuation::None);
    let (_roots, mut session, bridge) = loaded(CommandPolicy::Pause);
    let id = session.begin("compare").unwrap();
    session.execute(id).unwrap();
    bridge.mark(&session, id).unwrap();
    let observer = session.observer();
    session.finalize();
    assert_eq!(
        observer.snapshot().invocations[0].as_ref().unwrap().phase,
        CommandPhase::Incomplete
    );
    assert_eq!(
        bridge.snapshot().markers[0].unwrap().phase,
        CommandPhase::AwaitingCompletion
    );
}
#[test]
fn partial_read_and_companion_failures_keep_prior_facts_without_retry() {
    for target in ["left.txt", "right.txt"] {
        let (roots, mut session, bridge) = loaded(CommandPolicy::Automatic);
        std::fs::remove_file(roots.inputs.join(target)).unwrap();
        let offer = bridge.snapshot().offer;
        bridge.painted(offer);
        bridge.invoke(offer);
        execute(&bridge, &mut session).unwrap();
        let s = session.finalize();
        assert_eq!(s.reads, if target == "left.txt" { 1 } else { 2 });
        assert_eq!(
            s.invocations[0].as_ref().unwrap().phase,
            CommandPhase::Failed
        );
        assert!(s.invocations[1].is_none());
        assert_eq!(s.continuation, CommandContinuation::None);
    }
    let catalog = fixture::AUTOMATIC_CATALOG.replace(
        "\"Readable\",\"steps\":[{\"read\":\"left.txt\"}]",
        "\"Readable\",\"steps\":[{\"read\":\"missing.txt\"}]",
    );
    let roots = fixture::Roots::new(&catalog, b"same", b"same").unwrap();
    let mut session =
        CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Automatic).unwrap();
    let bridge = Bridge::new(CommandPolicy::Automatic);
    bridge.attach(&session);
    let offer = bridge.snapshot().offer;
    bridge.painted(offer);
    bridge.invoke(offer);
    execute(&bridge, &mut session).unwrap();
    let s = session.finalize();
    assert_eq!(s.reads, 3);
    assert_eq!(
        s.invocations[0].as_ref().unwrap().phase,
        CommandPhase::Completed
    );
    assert_eq!(
        s.invocations[1].as_ref().unwrap().phase,
        CommandPhase::Failed
    );
    assert!(bridge.snapshot().markers.iter().all(Option::is_some));
}
#[test]
fn stop_after_primary_report_suppresses_next_admission_and_first_error_is_bounded() {
    let (_roots, mut session, bridge) = loaded(CommandPolicy::Automatic);
    let id = session.begin("compare").unwrap();
    session.execute(id).unwrap();
    bridge.mark(&session, id).unwrap();
    report(&mut session, id).unwrap();
    bridge.stop();
    assert!(session.follow().unwrap().is_none());
    let s = session.finalize();
    assert_eq!(s.reads, 2);
    assert_eq!(
        s.invocations[0].as_ref().unwrap().phase,
        CommandPhase::Completed
    );
    assert!(matches!(
        s.continuation,
        CommandContinuation::Suppressed { .. }
    ));
    let oversized = "界".repeat(1024 * 1024);
    bridge.error(&oversized);
    let before = bridge.snapshot().error.unwrap();
    assert_eq!(before.len(), 510);
    bridge.error("second error must not grow retained history");
    assert_eq!(bridge.snapshot().error.unwrap(), before);
}

pub(crate) fn maximum_projection() -> Projection {
    let catalog = r#"{"commands":[{"name":"compare","outcome":"Equal","steps":[{"read":"left.txt"},{"read":"right.txt"}],"next":{"command":"inspect","policy":"Automatic"}},{"name":"inspect","outcome":"Equal","steps":[{"read":"left.txt"},{"read":"right.txt"}]}]}"#;
    let primary = "c".repeat(32);
    let companion = "i".repeat(32);
    let left = "l".repeat(64);
    let right = "r".repeat(64);
    let catalog = catalog
        .replace("compare", &primary)
        .replace("inspect", &companion)
        .replace("left.txt", &left)
        .replace("right.txt", &right);
    let roots = fixture::Roots::new(&catalog, &[27; 1024], &[27; 1024]).unwrap();
    std::fs::rename(roots.inputs.join("left.txt"), roots.inputs.join(&left)).unwrap();
    std::fs::rename(roots.inputs.join("right.txt"), roots.inputs.join(&right)).unwrap();
    let mut session =
        CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause).unwrap();
    let bridge = Bridge::new(CommandPolicy::Pause);
    bridge.attach(&session);
    let id = session.begin(&primary).unwrap();
    session.execute(id).unwrap();
    bridge.mark(&session, id).unwrap();
    session
        .report(
            id,
            CommandCompletion::Completed,
            &"\\".repeat(256),
            Some(&companion),
        )
        .unwrap();
    let handle = session.paused_continuation().unwrap();
    let id = session.resume(handle).unwrap().unwrap();
    session.execute(id).unwrap();
    bridge.mark(&session, id).unwrap();
    session
        .report(id, CommandCompletion::Completed, &"\"".repeat(256), None)
        .unwrap();
    session.finalize();
    bridge.error(&"\u{1b}".repeat(512));
    bridge.snapshot()
}
