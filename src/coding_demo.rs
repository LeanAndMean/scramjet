#[path = "../tests/support/coding_fixture.rs"]
mod fixture;
#[path = "../tests/support/coding.rs"]
mod roots;
use scramjet_rs::{
    CodingAgent, CodingCapability, CodingSession, EditCancellation, OperationState, RequestId,
    Runtime, reconstruct_coding,
};

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = roots::OwnedRoot::new();
    let session =
        CodingSession::create(&root.path, "coding-demo", CodingCapability::ControlledEdit)?;
    let exact = session.intent("sample.txt", "before\n", "after\n")?;
    let approval = session.approve_exact(&exact)?;
    let agent = CodingAgent::start(session, Some(approval), EditCancellation::default(), "")?;
    let fixture = fixture::ModelFixture::start(fixture::golden()).await;
    let runtime = Runtime::new(
        fixture.endpoint,
        1,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .map_err(|_| "runtime busy")?;
    let result = agent.run(&runtime).await;
    let attempts = runtime.snapshot().attempts;
    let snapshot = runtime.shutdown().await;
    fixture.settle(attempts).await;
    let requests = fixture.requests();
    fixture.stop().await;
    let result = result?;
    let facts = reconstruct_coding(&root.path, "coding-demo")?;
    if facts.len() != 1 || facts[0].state != OperationState::Completed || requests.len() != 4 {
        return Err("coding demonstration correlation failed".into());
    }
    assert_eq!(result.usage_tokens, Some(4));
    println!("coding-demo: {}", result.summary);
    println!(
        "turns={} admitted={} edit_operation={} verified=true synthetic_usage=4 worker=joined local_final={} unresolved={}",
        result.turns,
        attempts,
        result.edit_operation,
        snapshot.active + snapshot.pending + snapshot.handles,
        Runtime::process_snapshot().unresolved
    );
    Ok(())
}
