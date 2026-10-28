#[path = "support/coding_fixture.rs"]
mod fixture;
#[path = "support/coding.rs"]
mod roots;
use fixture::{ModelFixture, Reply, golden};
use roots::OwnedRoot;
use scramjet_rs::{
    CodingAgent, CodingCapability, CodingSession, EditCancellation, OperationState, RequestId,
    Runtime, reconstruct_coding,
};
use std::fs;
use std::sync::Mutex;
static GATE: Mutex<()> = Mutex::new(());

fn executor() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

async fn journey(
    replies: Vec<Reply>,
    capability: CodingCapability,
    approve: bool,
    cancel: EditCancellation,
) -> (
    OwnedRoot,
    Result<scramjet_rs::CodingRun, scramjet_rs::CodingError>,
    Vec<String>,
) {
    journey_in_mode(
        replies,
        capability,
        approve,
        cancel,
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .await
}

async fn journey_in_mode(
    replies: Vec<Reply>,
    capability: CodingCapability,
    approve: bool,
    cancel: EditCancellation,
    mode: scramjet_rs::ConcurrencyMode,
) -> (
    OwnedRoot,
    Result<scramjet_rs::CodingRun, scramjet_rs::CodingError>,
    Vec<String>,
) {
    let root = OwnedRoot::new();
    let session = CodingSession::create(&root.path, "loop-test", capability).unwrap();
    let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
    let approval = if approve {
        Some(session.approve_exact(&intent).unwrap())
    } else {
        None
    };
    let agent = CodingAgent::start(session, approval, cancel, "").unwrap();
    let fixture = ModelFixture::start(replies).await;
    let runtime = Runtime::new(
        fixture.endpoint,
        1,
        scramjet_rs::RequestRate::new(1, std::time::Duration::from_millis(20)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        mode,
    )
    .unwrap();
    let started = std::time::Instant::now();
    let result = agent.run(&runtime).await;
    if result.is_ok() {
        assert!(started.elapsed() >= std::time::Duration::from_millis(60));
    }
    let attempts = runtime.snapshot().attempts;
    let snapshot = runtime.shutdown().await;
    fixture.settle(attempts).await;
    let requests = fixture.requests();
    fixture.stop().await;
    tokio::time::sleep(snapshot.rate_full_refill).await;
    assert_eq!(
        snapshot.active
            + snapshot.pending
            + snapshot.handles
            + snapshot.input_bytes
            + snapshot.delta_items,
        0
    );
    assert_eq!(Runtime::process_snapshot().unresolved, 0);
    (root, result, requests)
}

#[test]
fn mixed_synthetic_usage_and_overrun_never_fabricate_usage_or_execute_tools() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for overrun in [false, true] {
            let root = OwnedRoot::new();
            let session =
                CodingSession::create(&root.path, "token-code", CodingCapability::ControlledEdit)
                    .unwrap();
            let approval = session
                .approve_exact(&session.intent("sample.txt", "before\n", "after\n").unwrap())
                .unwrap();
            let agent =
                CodingAgent::start(session, Some(approval), EditCancellation::default(), "")
                    .unwrap();
            let mut usage = [Some(1); 8];
            if overrun {
                usage[0] = Some(3);
            } else {
                usage[1] = None;
            }
            let fixture = ModelFixture::start_with_usage(golden(), None, usage).await;
            let runtime = Runtime::new(
                fixture.endpoint,
                1,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap();
            let result = agent.run(&runtime).await;
            if overrun {
                assert!(result.is_err());
                assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), b"before\n");
                let journal = fs::read_to_string(root.path.join(".coding-journal")).unwrap();
                assert!(!journal.contains("Authorized"));
                assert!(!journal.contains("\"status\":\"Read\""));
                assert_eq!(
                    (
                        runtime.snapshot().synthetic_measured,
                        runtime.snapshot().synthetic_charges
                    ),
                    (Some(3), Some(3))
                );
            } else {
                assert_eq!(result.unwrap().usage_tokens, None);
                assert_eq!(runtime.snapshot().unknown_usage, 1);
            }
            let attempts = runtime.snapshot().attempts;
            runtime.shutdown().await;
            fixture.settle(attempts).await;
            fixture.stop().await;
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    });
}

#[test]
fn cooldown_refusal_is_failed_durable_attempt_without_tools_or_replay() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let root = OwnedRoot::new();
        let session = CodingSession::create(&root.path, "cooldown-code", CodingCapability::ControlledEdit).unwrap();
        let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
        let approval = session.approve_exact(&intent).unwrap();
        let agent = CodingAgent::start(session, Some(approval), EditCancellation::default(), "").unwrap();
        let mut reply = Reply::success(r#"{"kind":"Replace","op":1,"target":"sample.txt","expected":"before\n","replacement":"after\n"}"#);
        reply.status = 429;
        let fixture = ModelFixture::start_with_retry_after(vec![reply], Some(1)).await;
        let runtime = Runtime::new(fixture.endpoint, 1, scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(), scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1)).unwrap(), scramjet_rs::ConcurrencyMode::Fixed,
).unwrap();
        let result = agent.run(&runtime).await;
        assert!(matches!(result, Err(scramjet_rs::CodingError::Invalid("model response did not complete successfully"))));
        let state = runtime.shutdown().await;
        assert_eq!((state.attempts, state.cooldown_signals, state.unresolved, state.handles), (1, 1, 0, 0));
        assert!(state.cooldown_remaining > std::time::Duration::ZERO);
        assert!(Runtime::new(fixture.endpoint, 1, scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(), scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1)).unwrap(), scramjet_rs::ConcurrencyMode::Fixed,
).is_err());
        fixture.settle(1).await;
        assert_eq!(fixture.requests().len(), 1);
        fixture.stop().await;
        let journal = fs::read(root.path.join(".coding-journal")).unwrap();
        let text = std::str::from_utf8(&journal).unwrap();
        assert!(text.contains("Failed"));
        assert!(!text.contains("Authorized"));
        assert!(!text.contains("ResponsePart"));
        assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), b"before\n");
        for _ in 0..3 {
            assert!(reconstruct_coding(&root.path, "cooldown-code").unwrap().is_empty());
            assert_eq!(fs::read(root.path.join(".coding-journal")).unwrap(), journal);
        }
        tokio::time::sleep(state.cooldown_remaining + std::time::Duration::from_millis(10)).await;
        Runtime::new("127.0.0.1:9".parse().unwrap(), 1, scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(), scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1)).unwrap(), scramjet_rs::ConcurrencyMode::Fixed,
).unwrap().shutdown().await;
    });
}

#[test]
fn exact_context_ceiling_is_carried_untruncated_on_all_eight_actual_turns() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let root = OwnedRoot::new();
        fs::write(root.path.join("sample.txt"), "\n".repeat(1024)).unwrap();
        let name = "s".repeat(64);
        let prompt = format!("{}x", "\"".repeat(967));
        let session = CodingSession::create(&root.path, &name, CodingCapability::ReadOnly).unwrap();
        let agent =
            CodingAgent::start(session, None, EditCancellation::default(), &prompt).unwrap();
        let replies = (1..=8)
            .map(|op| {
                Reply::success(
                    &serde_json::json!({"kind":"Read", "op":op, "target":"sample.txt"}).to_string(),
                )
            })
            .collect();
        let fixture = ModelFixture::start(replies).await;
        let runtime = Runtime::new(
            fixture.endpoint,
            1,
            scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65536,
                65536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        assert!(agent.run(&runtime).await.is_err());
        let pre = runtime.shutdown().await;
        fixture.settle(8).await;
        let requests = fixture.requests();
        fixture.stop().await;
        assert_eq!(requests.len(), 8);
        for (index, text) in requests.iter().enumerate() {
            assert_eq!(text.len(), if index == 0 { 2048 } else { 4096 });
            let context: serde_json::Value = serde_json::from_str(text).unwrap();
            assert_eq!(context["prompt"], prompt);
            assert_eq!(context["session"], name);
            assert_eq!(context["turn"], index + 1);
            assert_eq!(
                context["previous"],
                if index == 0 {
                    String::new()
                } else {
                    "\n".repeat(1024)
                }
            );
        }
        assert_eq!(pre.synthetic_measured, Some(8));
        assert!(reconstruct_coding(&root.path, &name).unwrap().is_empty());
        let journal = fs::read(root.path.join(".coding-journal")).unwrap();
        assert!(journal.len() <= 1_048_576);
        assert!(
            journal
                .split(|b| *b == b'\n')
                .all(|line| line.len() < 16384)
        );
        tokio::time::sleep(pre.rate_full_refill).await;
    });
}

#[test]
fn golden_journey_correlates_durable_effect_tools_final_and_synthetic_usage() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let (root, result, requests) = journey(
            golden(),
            CodingCapability::ControlledEdit,
            true,
            EditCancellation::default(),
        )
        .await;
        let result = result.unwrap();
        assert_eq!(result.turns, 4);
        assert_eq!(result.edit_operation, 1);
        assert_eq!(result.usage_tokens, Some(4));
        assert_eq!(result.request_ids.len(), 4);
        assert!(result.request_ids.windows(2).all(|w| w[0] != w[1]));
        assert_eq!(requests.len(), 4);
        assert!(requests[1].contains(r#""previous":"before\n""#));
        assert!(requests[3].contains("exact content verified"));
        assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), b"after\n");
        let journal = fs::read_to_string(root.path.join(".coding-journal")).unwrap();
        for status in ["Read", "Replaced", "Verified", "Final"] {
            assert!(journal.contains(&format!(r#""status":"{status}""#)));
        }
        for id in &result.request_ids {
            assert!(journal.contains(&format!(r#""request":{}"#, id.0)));
        }
        let before = journal.clone();
        for _ in 0..3 {
            let facts = reconstruct_coding(&root.path, "loop-test").unwrap();
            assert_eq!(facts[0].state, OperationState::Completed);
            assert_eq!(
                fs::read_to_string(root.path.join(".coding-journal")).unwrap(),
                before
            );
        }
    });
}

#[test]
fn failed_incomplete_malformed_and_oversized_completed_responses_have_no_effect() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let mut incomplete = Reply::success(r#"{"kind":"Read","op":1,"target":"sample.txt"}"#);
        incomplete.done = false;
        let mut rejected = Reply::success("denied");
        rejected.status = 503;
        let bad = [
            "{",
            "[]",
            r#"{"kind":"Read","op":1,"target":"../sample.txt"}"#,
            r#"{"kind":"Read","op":1,"target":"sample.txt","unknown":[0,0]}"#,
            r#"{"kind":"Read","op":1,"op":1,"target":"sample.txt"}"#,
            r#"{"kind":"Read","op":0,"target":"sample.txt"}"#,
            r#"{"kind":"Read","op":1,"target":"sample.txt"} {}"#,
            r#"{"kind":"Read","op":1,"target":[]}"#,
            r#"{"kind":"Final","summary":"I edited it"}"#,
            r#"{"kind":"Read","op":1,"target":"sample.txt","expected":"before\n"}"#,
        ];
        let mut cases: Vec<_> = bad.into_iter().map(Reply::success).collect();
        cases.extend([incomplete, rejected, Reply::success(&"x".repeat(17_000))]);
        for reply in cases {
            let (root, result, requests) = journey(
                vec![reply],
                CodingCapability::ControlledEdit,
                true,
                EditCancellation::default(),
            )
            .await;
            assert!(result.is_err());
            assert_eq!(requests.len(), 1);
            assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), b"before\n");
            assert!(
                reconstruct_coding(&root.path, "loop-test")
                    .unwrap()
                    .is_empty()
            );
            let journal = fs::read_to_string(root.path.join(".coding-journal")).unwrap();
            assert!(!journal.contains(r#""kind":"Authorized""#));
            assert!(!journal.contains(r#""status":"Read""#));
        }
    });
}

#[test]
fn exact_response_ceiling_and_escaped_payload_are_durable_before_tool_execution() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for whitespace in [' ', '\t'] {
            let mut replies = golden();
            let base = replies[0].text.clone();
            replies[0].text = format!(
                "{}{}",
                base,
                whitespace.to_string().repeat(16_384 - base.len())
            );
            let (root, result, _) = journey(
                replies,
                CodingCapability::ControlledEdit,
                true,
                EditCancellation::default(),
            )
            .await;
            assert!(result.is_ok());
            let journal = fs::read_to_string(root.path.join(".coding-journal")).unwrap();
            assert_eq!(
                journal
                    .lines()
                    .filter(|l| l.contains("ResponsePart") && l.contains(r#""turn":1,"#))
                    .count(),
                8
            );
            assert!(journal.lines().all(|l| l.len() < 16_384));
            assert_eq!(
                reconstruct_coding(&root.path, "loop-test").unwrap()[0].state,
                OperationState::Completed
            );
        }
    });
}

#[test]
fn denied_mismatched_stale_and_tool_errors_never_dispatch() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for capability in [CodingCapability::ReadOnly, CodingCapability::ControlledEdit] {
            let (root, result, _) = journey(golden(), capability, false, EditCancellation::default()).await;
            assert!(result.is_err());
            assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), b"before\n");
            assert!(reconstruct_coding(&root.path, "loop-test").unwrap().is_empty());
        }
        for text in [
            r#"{"kind":"Replace","op":2,"target":"sample.txt","expected":"before\n","replacement":"not approved\n"}"#,
            r#"{"kind":"Replace","op":1,"target":"sample.txt","expected":"before\n","replacement":"after\n"}"#,
            r#"{"kind":"Replace","op":2,"target":"sample.txt","expected":"wrong","replacement":"after\n"}"#,
            r#"{"kind":"Read","op":2,"target":"missing.txt"}"#,
        ] {
            let mut replies = golden(); replies.truncate(2); replies[1] = Reply::success(text);
            let (root, result, _) = journey(replies, CodingCapability::ControlledEdit, true, EditCancellation::default()).await;
            assert!(result.is_err());
            assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), b"before\n");
            assert!(reconstruct_coding(&root.path, "loop-test").unwrap().is_empty());
        }
    });
}

#[test]
fn cancellation_before_admission_and_during_response_joins_worker_without_tools() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let cancel = EditCancellation::default();
        cancel.cancel();
        let (root, result, requests) =
            journey(golden(), CodingCapability::ControlledEdit, true, cancel).await;
        assert!(result.is_err());
        assert!(requests.is_empty());
        assert!(
            reconstruct_coding(&root.path, "loop-test")
                .unwrap()
                .is_empty()
        );
        let cancel = EditCancellation::default();
        let root = OwnedRoot::new();
        let session =
            CodingSession::create(&root.path, "loop-test", CodingCapability::ControlledEdit)
                .unwrap();
        let agent = CodingAgent::start(session, None, cancel.clone(), "").unwrap();
        let mut replies = golden();
        replies.truncate(1);
        replies[0].delay_ms = 1000;
        let fixture = ModelFixture::start(replies).await;
        let runtime = std::sync::Arc::new(
            Runtime::new(
                fixture.endpoint,
                1,
                scramjet_rs::RequestRate::new(1, std::time::Duration::from_millis(20)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap(),
        );
        let owner = runtime.clone();
        let task = tokio::spawn(async move { agent.run(&owner).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while fixture.requests().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        assert!(task.await.unwrap().is_err());
        let runtime = std::sync::Arc::try_unwrap(runtime).ok().unwrap();
        let stopped = runtime.shutdown().await;
        fixture.settle(stopped.attempts).await;
        assert_eq!(fixture.requests().len(), 1);
        fixture.stop().await;
        tokio::time::sleep(stopped.rate_full_refill).await;
        assert!(
            reconstruct_coding(&root.path, "loop-test")
                .unwrap()
                .is_empty()
        );
        assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), b"before\n");
    });
}

#[test]
fn turn_output_and_final_truthfulness_limits_fail_closed() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let replies = (1..=8)
            .map(|op| {
                Reply::success(&format!(
                    r#"{{"kind":"Read","op":{op},"target":"sample.txt"}}"#
                ))
            })
            .collect();
        let (root, result, requests) = journey(
            replies,
            CodingCapability::ControlledEdit,
            true,
            EditCancellation::default(),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(requests.len(), 8);
        assert!(
            reconstruct_coding(&root.path, "loop-test")
                .unwrap()
                .is_empty()
        );
        let mut replies = golden();
        replies[2] =
            Reply::success(r#"{"kind":"Verify","op":3,"target":"sample.txt","expected":"wrong"}"#);
        let (root, result, _) = journey(
            replies,
            CodingCapability::ControlledEdit,
            true,
            EditCancellation::default(),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            reconstruct_coding(&root.path, "loop-test").unwrap()[0].state,
            OperationState::Completed
        );
        let mut replies = golden();
        replies[3] = Reply::success(&format!(
            r#"{{"kind":"Final","summary":"{}"}}"#,
            "x".repeat(1025)
        ));
        let (root, result, _) = journey(
            replies,
            CodingCapability::ControlledEdit,
            true,
            EditCancellation::default(),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            reconstruct_coding(&root.path, "loop-test").unwrap()[0].state,
            OperationState::Completed
        );
    });
}

#[test]
fn recovery_rejects_material_correlation_and_transition_corruption_without_repair() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let (root, result, _) = journey(
            golden(),
            CodingCapability::ControlledEdit,
            true,
            EditCancellation::default(),
        )
        .await;
        let result = result.unwrap();
        let path = root.path.join(".coding-journal");
        let original = fs::read_to_string(&path).unwrap();
        for changed in [
            original.replace(
                &format!(r#""turn":2,"request":{}"#, result.request_ids[1].0),
                &format!(r#""turn":2,"request":{}"#, result.request_ids[0].0),
            ),
            original.replace(
                r#"\"kind\":\"Verify\",\"op\":3,\"target\":\"sample.txt\""#,
                r#"\"kind\":\"Verify\",\"op\":3,\"target\":\"other.txt\""#,
            ),
            original.replacen(r#""turn":1,"request":0"#, r#""turn":2,"request":0"#, 1),
            original.replacen(
                r#""event":"Tool","status":"Read""#,
                r#""event":"Final","status":"Read""#,
                1,
            ),
            original.replace(r#"\"expected\":\"after\\n\""#, r#"\"expected\":\"wrong\""#),
            original.replacen(
                r#""operation":1,"event":"Tool""#,
                r#""operation":2,"event":"Tool""#,
                1,
            ),
        ] {
            assert_ne!(changed, original);
            fs::write(&path, &changed).unwrap();
            assert!(reconstruct_coding(&root.path, "loop-test").is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), changed);
            assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), b"after\n");
        }
        fs::write(&path, original).unwrap();
        reconstruct_coding(&root.path, "loop-test").unwrap();
    });
}

#[test]
fn adaptive_coding_preserves_success_failure_cancel_and_no_replay() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for case in 0..3 {
            let mut replies = golden();
            let cancel = EditCancellation::default();
            if case == 1 {
                replies[0].status = 502;
                replies.truncate(1);
            }
            if case == 2 {
                cancel.cancel();
            }
            let (root, result, requests) = journey_in_mode(
                replies,
                CodingCapability::ControlledEdit,
                true,
                cancel,
                scramjet_rs::ConcurrencyMode::Adaptive,
            )
            .await;
            let journal = fs::read(root.path.join(".coding-journal")).unwrap();
            if case == 0 {
                assert_eq!(result.unwrap().usage_tokens, Some(4));
                assert_eq!(requests.len(), 4);
                assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), b"after\n");
            } else {
                assert!(result.is_err());
                assert_eq!(requests.len(), usize::from(case == 1));
                assert!(!String::from_utf8_lossy(&journal).contains("Authorized"));
                assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), b"before\n");
            }
            for _ in 0..3 {
                let facts = reconstruct_coding(&root.path, "loop-test").unwrap();
                assert_eq!(facts.len(), usize::from(case == 0));
                assert_eq!(
                    fs::read(root.path.join(".coding-journal")).unwrap(),
                    journal
                );
            }
            assert_eq!(Runtime::process_snapshot().concurrency.target, 1);
        }
    });
}
