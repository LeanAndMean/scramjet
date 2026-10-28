#[allow(dead_code)]
#[path = "support/project_fixture.rs"]
mod model;
#[path = "support/coding.rs"]
mod roots;
use scramjet_rs::*;
use std::{fs, os::unix::fs::PermissionsExt, sync::Mutex, time::Duration};
static TEST_GATE: Mutex<()> = Mutex::new(());
#[path = "project_session/authority.rs"]
mod authority;
#[path = "project_session/failures.rs"]
mod failures;
#[path = "project_session/foreign.rs"]
mod foreign;
#[path = "project_session/history.rs"]
mod history;
const BEFORE: &str = "Header\nOLD body stays here\nFooter\n";
fn request(needle: &str, replacement: &str) -> String {
    serde_json::json!({"operation":"replace_once","target":"sample.txt","supporting":["rules.txt","notes.txt"],"needle":needle,"replacement":replacement}).to_string()
}
fn project() -> roots::OwnedRoot {
    let root = roots::OwnedRoot::new();
    fs::set_permissions(&root.path, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(root.path.join("sample.txt"), BEFORE).unwrap();
    for name in ["rules.txt", "notes.txt"] {
        fs::write(
            root.path.join(name),
            format!("complete support {name}: keep Header/Footer\n"),
        )
        .unwrap();
    }
    for name in ["sample.txt", "rules.txt", "notes.txt"] {
        fs::set_permissions(root.path.join(name), fs::Permissions::from_mode(0o644)).unwrap();
    }
    root
}
fn runtime(endpoint: std::net::SocketAddr) -> Runtime {
    Runtime::new(
        endpoint,
        2,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Fixed,
    )
    .unwrap()
}
async fn submit(
    session: &mut ProjectSession,
    json: &str,
    old: Option<&CodingProposal>,
    decision: CodingDecision,
) -> (Result<ProjectSourceReceipt, CodingError>, CodingProposal) {
    let observer = session.observer();
    let control = session.control();
    let decide = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = observer.snapshot();
                if let Some(id) = snapshot.current
                    && let Some(coding) = &snapshot.journeys[id.journey() as usize - 1].coding
                    && let Some(proposal) = &coding.proposal
                {
                    if let Some(old) = old {
                        assert!(control.decide(old, CodingDecision::Approve).is_err());
                    }
                    control.decide(proposal, decision).unwrap();
                    break proposal.as_ref().clone();
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap()
    };
    let (receipt, proposal) = tokio::join!(session.submit(json), decide);
    (receipt, proposal)
}
fn summary(session: &ProjectSession) -> String {
    let snapshot = session.observer().snapshot();
    let id = snapshot.current.unwrap();
    snapshot.journeys[id.journey() as usize - 1]
        .coding
        .as_ref()
        .unwrap()
        .final_summary
        .as_deref()
        .unwrap()
        .to_owned()
}
async fn verify(session: &mut ProjectSession, receipt: &mut ProjectSourceReceipt) {
    let summary = summary(session);
    assert_eq!(
        session.observer().snapshot().phase,
        ProjectPhase::AwaitingSourceReport
    );
    let continuation = session
        .report_source(receipt, CommandCompletion::Completed, &summary)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.observer().snapshot().phase, ProjectPhase::Paused);
    let before = session
        .observer()
        .snapshot()
        .records
        .iter()
        .filter(|r| matches!(r.as_ref(), ProjectRecord::VerificationRead(_)))
        .count();
    let ready = session.resume(continuation).await.unwrap();
    assert_eq!(
        session.observer().snapshot().phase,
        ProjectPhase::VerificationReady
    );
    assert_eq!(
        session
            .observer()
            .snapshot()
            .records
            .iter()
            .filter(|r| matches!(r.as_ref(), ProjectRecord::VerificationRead(_)))
            .count(),
        before
    );
    let mut receipt = session.verify(ready).await.unwrap();
    session
        .report_verification(
            &mut receipt,
            CommandCompletion::Completed,
            "full edited target matched",
        )
        .await
        .unwrap();
    assert_eq!(session.observer().snapshot().phase, ProjectPhase::Verified);
}
#[test]
fn retained_two_journeys_use_actual_readers_one_runtime_fresh_approval_and_explicit_verification() {
    let _gate = TEST_GATE.lock().unwrap();
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let root = project(); let storage = roots::OwnedRoot::new();
        let fixture = model::Fixture::start(model::Mode::Golden).await;
        let (mut session,settlement) = ProjectSession::start(runtime(fixture.endpoint),&root.path,&storage.path,"retained-golden");
        let (receipt,proposal) = submit(&mut session,&request("OLD","NEW"),None,CodingDecision::Approve).await;
        let mut receipt = receipt.unwrap();
        assert_eq!(fixture.requests().len(),7);
        assert_eq!(fs::read_to_string(root.path.join("sample.txt")).unwrap(),BEFORE.replace("OLD","NEW"));
        assert!(session.observer().snapshot().records.iter().all(|r| !matches!(r.as_ref(),ProjectRecord::VerificationReady(_)|ProjectRecord::VerificationRead(_))));
        verify(&mut session,&mut receipt).await;
        let (receipt,second) = submit(&mut session,&request("NEW","NEXT"),Some(&proposal),CodingDecision::Approve).await;
        let mut receipt = receipt.unwrap();
        assert_ne!(proposal.owner,second.owner);
        assert_eq!(fixture.requests().len(),14);
        verify(&mut session,&mut receipt).await;
        assert!(session.submit(&request("NEXT","LAST")).await.is_err());
        assert_eq!(fixture.requests().len(),14);
        fixture.settle(14).await;
        let finish = settlement.finish().await.unwrap();
        assert!(finish.readers_joined && finish.coding_joined && finish.storage_joined && finish.runtime_joined && finish.history_acknowledged);
        assert!(!finish.retains_runtime_owner());
        assert_eq!(finish.runtime.attempts,14);
        assert_eq!(finish.runtime.handles,0);
        assert_eq!(finish.runtime.active,0);
        assert_eq!(finish.runtime.pending,0);
        assert_eq!(finish.runtime.unknown_usage,0);
        assert_eq!(finish.runtime.unresolved,0);
        assert!(session.observer().snapshot().settled);
        let fixture = fixture.shutdown().await;
        assert!(fixture.joined);
        assert_eq!(fixture.peak,2);
        assert_eq!(fixture.requests.len(),14);
        assert_eq!(fixture.settled.len(),14);
        assert_eq!(fs::metadata(root.path.join("sample.txt")).unwrap().permissions().mode()&0o7777,0o644);
        assert_eq!(fs::read_to_string(root.path.join("sample.txt")).unwrap(),BEFORE.replace("OLD","NEXT"));
        assert!(root.path.join(PROJECT_LOCK_NAME).exists());
        assert!(!root.path.join(".coding-journal").exists());
        let history = reconstruct_project_history(&storage.path,"retained-golden").unwrap();
        assert!(history.closure_recorded);
        assert!(history.readers.iter().flatten().all(|r| r.parent_finished));
        assert!(history.coding.iter().flatten().all(|c| c.ending()==CodingHistoryEnding::RecordedFinal));
        assert_eq!(history.records.iter().filter(|r| matches!(r.as_ref(),ProjectRecord::SourceReport(report) if report.completion=="Completed")).count(),2);
        let moved = root.path.with_extension("moved"); fs::rename(&root.path,&moved).unwrap();
        assert_eq!(reconstruct_project_history(&storage.path,"retained-golden").unwrap().records.len(),history.records.len());
        fs::rename(moved,&root.path).unwrap();
    });
}
