#[allow(dead_code)]
#[path = "support/coding_ui_fixture.rs"]
mod fixture;
#[path = "support/coding.rs"]
mod roots;
use scramjet_rs::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
static GATE: Mutex<()> = Mutex::new(());
const SESSION: &str = "same-owner";
const FIRST: &str = "first 界\nline e\u{301}";
const SECOND: &str = "follow 🙂\nsecond line";
fn executor() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
async fn setup() -> (roots::OwnedRoot, fixture::Fixture, Runtime, CodingAgent) {
    let root = roots::OwnedRoot::new();
    let fixture = fixture::Fixture::consuming_follow_up(Arc::from(SESSION))
        .await
        .unwrap();
    let runtime = Runtime::new(
        fixture.address,
        1,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Fixed,
    )
    .unwrap();
    let session =
        CodingSession::create(&root.path, SESSION, CodingCapability::ControlledEdit).unwrap();
    let agent = CodingAgent::start_follow_up(session, EditCancellation::default(), FIRST).unwrap();
    (root, fixture, runtime, agent)
}
async fn run(
    agent: &mut CodingAgent,
    runtime: &Runtime,
    decision: CodingDecision,
) -> Result<CodingRun, CodingError> {
    let observer = agent.observer();
    let control = agent.control();
    let generation = observer.snapshot().invocation;
    let decide = async {
        loop {
            let observation = observer.snapshot();
            if let Some(proposal) = &observation.proposal
                && proposal.invocation == generation
            {
                control.decide(proposal, decision).unwrap();
                assert!(control.decide(proposal, decision).is_err());
                break;
            }
            if observation.run_ended {
                break;
            }
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(agent.run_unjoined(runtime), decide).0
    })
    .await
    .unwrap()
}
async fn settle(agent: CodingAgent, runtime: Runtime, fixture: fixture::Fixture) {
    let observer = agent.observer();
    agent.finish().unwrap();
    let state = runtime.shutdown().await;
    assert_eq!(state.active + state.pending + state.handles, 0);
    let report = fixture.shutdown().await.unwrap();
    fixture::reconcile(&observer.snapshot(), &report).unwrap();
    let post = Runtime::process_snapshot();
    assert_eq!(
        post.unresolved + post.usage_obligations + post.potential + post.token_debt as usize,
        0
    );
    assert!(observer.snapshot().worker_joined);
    tokio::time::sleep(Duration::from_millis(5)).await;
}
fn rewrite(root: &roots::OwnedRoot, records: &[serde_json::Value]) {
    let text: String = records
        .iter()
        .enumerate()
        .map(|(sequence, r)| {
            let mut r = r.clone();
            r["sequence"] = sequence.into();
            format!("{r}\n")
        })
        .collect();
    std::fs::write(root.path.join(".coding-journal"), text).unwrap();
}
#[test]
fn same_owner_context_two_fresh_approvals_and_strict_partial_history() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let (root, fixture, runtime, mut agent) = setup().await;
        let observer = agent.observer();
        let old_control = agent.control();
        assert!(agent.follow_up(&runtime, SECOND).await.is_err());
        let first = run(&mut agent, &runtime, CodingDecision::Approve)
            .await
            .unwrap();
        assert_eq!((first.turns, first.edit_operation), (4, 1));
        let old_proposal = observer.snapshot().proposal.clone().unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            FIRST
        );
        assert!(reconstruct_coding_history(&root.path, SESSION).is_err());
        let checkpoint = tokio::time::timeout(Duration::from_secs(5), fixture.checkpoint())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint.accepts, 4);
        assert!(checkpoint.supervisor_joined[..4].iter().all(|v| *v));
        let identity = std::fs::metadata(&root.path).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(runtime.snapshot().attempts, 4);
        assert!(agent.validate_follow_up(&"x".repeat(1025)).is_err());
        agent.follow_up(&runtime, SECOND).await.unwrap();
        assert_eq!(observer.snapshot().owner, old_proposal.owner);
        assert_eq!(observer.snapshot().invocation, 2);
        assert_eq!(runtime.snapshot().attempts, 4);
        assert!(!observer.snapshot().worker_terminated);
        old_control.stop();
        assert!(
            old_control
                .decide(&old_proposal, CodingDecision::Approve)
                .is_err()
        );
        let current_control = agent.control();
        let approve = async {
            loop {
                if let Some(proposal) = &observer.snapshot().proposal
                    && proposal.invocation == 2
                {
                    assert_eq!((proposal.turn, proposal.intent.operation), (6, 2));
                    assert_eq!(proposal.intent.expected, FIRST);
                    assert_eq!(proposal.intent.replacement, SECOND);
                    assert_eq!(
                        std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),
                        FIRST
                    );
                    assert!(
                        old_control
                            .decide(proposal, CodingDecision::Approve)
                            .is_err()
                    );
                    assert!(
                        current_control
                            .decide(&old_proposal, CodingDecision::Approve)
                            .is_err()
                    );
                    current_control
                        .decide(proposal, CodingDecision::Approve)
                        .unwrap();
                    break;
                }
                tokio::task::yield_now().await;
            }
        };
        let second = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(agent.run_unjoined(&runtime), approve).0
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!((second.turns, second.edit_operation), (8, 2));
        assert!(
            first
                .request_ids
                .iter()
                .all(|id| !second.request_ids.contains(id))
        );
        assert_eq!(std::fs::metadata(&root.path).unwrap().ino(), identity.ino());
        assert_eq!(
            std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            SECOND
        );
        assert!(agent.follow_up(&runtime, "third").await.is_err());
        assert_eq!(runtime.snapshot().attempts, 8);
        settle(agent, runtime, fixture).await;
        let history = reconstruct_coding_history(&root.path, SESSION).unwrap();
        assert_eq!(history.ending(), CodingHistoryEnding::RecordedFinal);
        assert_eq!(history.operations().len(), 2);
        assert!(
            history
                .operations()
                .iter()
                .all(|f| f.state == OperationState::Completed)
        );
        assert_eq!(
            history
                .records()
                .iter()
                .filter(|r| matches!(r.entry, CodingHistoryEntry::FollowUp { .. }))
                .count(),
            1
        );
        assert!(history.records().len() < 64);
        let text = std::fs::read_to_string(root.path.join(".coding-journal")).unwrap();
        let records: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let marker = records
            .iter()
            .position(|r| r["record"]["kind"] == "FollowUp")
            .unwrap();
        for (end, expected) in [
            (marker, CodingHistoryEnding::RecordedFinal),
            (marker + 1, CodingHistoryEnding::Incomplete),
            (marker + 2, CodingHistoryEnding::Incomplete),
            (records.len() - 1, CodingHistoryEnding::Incomplete),
        ] {
            rewrite(&root, &records[..end]);
            let partial = reconstruct_coding_history(&root.path, SESSION).unwrap();
            assert_eq!(partial.ending(), expected);
            assert!(
                partial
                    .operations()
                    .iter()
                    .all(|f| f.state == OperationState::Completed)
            );
        }
        for field in ["invocation", "after_turn"] {
            let mut changed = records.clone();
            changed[marker]["record"][field] = 3.into();
            rewrite(&root, &changed);
            assert!(reconstruct_coding_history(&root.path, SESSION).is_err());
        }
        let mut changed = records.clone();
        changed.insert(marker + 1, changed[marker].clone());
        rewrite(&root, &changed);
        assert!(reconstruct_coding_history(&root.path, SESSION).is_err());
        let mut changed = records.clone();
        changed[marker + 1]["record"]["entry"]["text"] =
            changed[1]["record"]["entry"]["text"].clone();
        rewrite(&root, &changed);
        assert!(reconstruct_coding_history(&root.path, SESSION).is_err());
        rewrite(&root, &records);
    });
}
#[test]
fn duplicate_run_invalidates_eligibility_without_appending_a_marker() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let (root, fixture, runtime, mut agent) = setup().await;
        run(&mut agent, &runtime, CodingDecision::Approve)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), fixture.checkpoint())
            .await
            .unwrap()
            .unwrap();
        let before = std::fs::read(root.path.join(".coding-journal")).unwrap();
        assert!(matches!(
            agent.run_unjoined(&runtime).await,
            Err(CodingError::Stale)
        ));
        assert!(agent.follow_up(&runtime, SECOND).await.is_err());
        assert_eq!(runtime.snapshot().attempts, 4);
        assert_eq!(
            before,
            std::fs::read(root.path.join(".coding-journal")).unwrap()
        );
        settle(agent, runtime, fixture).await;
        assert_eq!(
            reconstruct_coding_history(&root.path, SESSION)
                .unwrap()
                .ending(),
            CodingHistoryEnding::RecordedFinal
        );
    });
}

#[test]
fn denied_first_or_second_and_unused_follow_up_never_replay() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for mode in 0..3 {
            let (root, fixture, runtime, mut agent) = setup().await;
            let first_decision = if mode == 0 {
                CodingDecision::Deny
            } else {
                CodingDecision::Approve
            };
            let first = run(&mut agent, &runtime, first_decision).await;
            if mode == 0 {
                assert!(matches!(first, Err(CodingError::Denied)));
                assert!(agent.follow_up(&runtime, SECOND).await.is_err());
            } else {
                first.unwrap();
                tokio::time::timeout(Duration::from_secs(5), fixture.checkpoint())
                    .await
                    .unwrap()
                    .unwrap();
                agent.follow_up(&runtime, SECOND).await.unwrap();
                if mode == 1 {
                    assert!(matches!(
                        run(&mut agent, &runtime, CodingDecision::Deny).await,
                        Err(CodingError::Denied)
                    ));
                }
            }
            settle(agent, runtime, fixture).await;
            let h = reconstruct_coding_history(&root.path, SESSION).unwrap();
            if mode == 0 {
                assert!(h.operations().is_empty());
                assert_eq!(
                    std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),
                    fixture::BEFORE
                );
            } else {
                assert_eq!(h.operations().len(), 1);
                assert_eq!(h.operations()[0].state, OperationState::Completed);
                assert_eq!(
                    std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),
                    FIRST
                );
            }
            assert_eq!(
                h.ending(),
                if mode == 2 {
                    CodingHistoryEnding::Incomplete
                } else {
                    CodingHistoryEnding::RecordedToolError
                }
            );
        }
    });
}
