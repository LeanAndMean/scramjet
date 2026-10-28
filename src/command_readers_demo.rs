use crate::parent_demo::{fixture, roots};
use scramjet_rs::{
    CommandCompletion, CommandInvocation, CommandPhase, CommandPolicy, CommandSession,
    ConcurrencyMode, RequestRate, Runtime, SyntheticTokenBudget, reconstruct_readers,
};
use std::time::Duration;
pub(crate) const CATALOG: &str = r#"{"commands":[{"name":"summarize","outcome":"AllReaders"}]}"#;
pub(crate) const CONTENTS: [&str; 3] = [
    "immutable alpha\n\t界",
    "immutable beta\\🙂",
    "immutable gamma",
];
pub(crate) fn report(
    command: &mut CommandSession,
    id: CommandInvocation,
) -> Result<(), scramjet_rs::CommandError> {
    let snapshot = command.snapshot();
    let execution = snapshot.invocations[0]
        .as_ref()
        .unwrap()
        .readers
        .as_ref()
        .unwrap()
        .execution
        .as_ref()
        .unwrap();
    let completion = if snapshot.cancellation_requested {
        CommandCompletion::Cancelled
    } else {
        execution.disposition
    };
    let summary = match completion {
        CommandCompletion::Completed => {
            "Three model summaries durably accepted; not independently verified conclusions."
        }
        CommandCompletion::Failed => "AllReaders not satisfied; complete child facts retained.",
        CommandCompletion::Cancelled => "Command Stop accepted; actual child facts retained.",
        CommandCompletion::Incomplete => {
            "Logical execution incomplete; acknowledged child facts retained."
        }
    };
    match command.report(id, completion, summary, None) {
        Err(_)
            if command.snapshot().cancellation_requested
                && completion != CommandCompletion::Cancelled =>
        {
            command.report(
                id,
                CommandCompletion::Cancelled,
                "Command Stop accepted; actual child facts retained.",
                None,
            )
        }
        result => result,
    }
}
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let definitions = crate::command_fixture::Roots::new(CATALOG, b"", b"")?;
    let mut command = CommandSession::load(
        &definitions.definitions,
        &definitions.inputs,
        CommandPolicy::Pause,
    )?;
    let id = command.begin_readers("summarize", CONTENTS)?;
    let session = command.snapshot().invocations[0]
        .as_ref()
        .unwrap()
        .readers
        .as_ref()
        .unwrap()
        .session
        .clone();
    let root = roots::OwnedRoot::new();
    let fixture = fixture::ReadersFixture::start(fixture::golden(&session, 1)).await;
    let runtime = Runtime::new(
        fixture.endpoint,
        2,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Fixed,
    )
    .map_err(|_| "runtime busy")?;
    let (mut adapter, settlement) = command.start_readers(id, runtime, &root.path)?;
    let observer = adapter.observer();
    let release = async {
        fixture.wait_child(0).await;
        fixture.wait_child(1).await;
        while Runtime::process_snapshot().pending != 1 {
            tokio::task::yield_now().await;
        }
        let overlap = Runtime::process_snapshot();
        fixture.release(1);
        while observer.snapshot().children[1].acceptance_ordinal != Some(1) {
            tokio::task::yield_now().await;
        }
        fixture.wait_child(2).await;
        fixture.release_all();
        overlap
    };
    let (receipt, overlap) = tokio::join!(adapter.logical(), release);
    let result = (|| {
        let mut receipt = receipt?;
        command.accept_readers(&mut receipt)?;
        assert_eq!(
            command.snapshot().invocations[0].as_ref().unwrap().phase,
            CommandPhase::AwaitingCompletion
        );
        assert!(!observer.snapshot().storage_joined);
        report(&mut command, id)
    })();
    let joined = settlement.wait().await;
    let contexts = fixture.contexts();
    let peak = fixture.peak();
    fixture.shutdown().await;
    result?;
    let joined = joined?;
    let final_state = command.finalize();
    let fact = final_state.invocations[0].as_ref().unwrap();
    let execution = fact.readers.as_ref().unwrap().execution.as_ref().unwrap();
    let reconstructed = reconstruct_readers(&root.path, &session, 1)?;
    if fact.phase != CommandPhase::Completed
        || !final_state.settled
        || overlap.active != 2
        || overlap.pending != 1
        || peak != 2
        || !reconstructed.parent_finished
        || execution.usage_tokens != Some(3)
        || reconstructed
            .children
            .iter()
            .enumerate()
            .any(|(i, f)| execution.observation.children[i].fact.as_ref() != Some(f))
    {
        return Err("command reader invariant".into());
    }
    for (child, context) in contexts.iter().enumerate() {
        let value: serde_json::Value =
            serde_json::from_str(context.as_ref().ok_or("missing context")?)?;
        if value["content"] != CONTENTS[child] {
            return Err("incomplete input".into());
        }
    }
    println!(
        "command-readers-demo: children=3 attempts={} active_overlap={}/{} pending=1 out_of_order=1-before-0 synthetic_usage=3",
        joined.attempts, overlap.active, peak
    );
    println!(
        "before_report=AwaitingCompletion explicit_report=Completed settled={} persistence_joined=true runtime_joined=true fixture_joined=true reconstructed=3 no_replay=true",
        final_state.settled
    );
    let post = Runtime::process_snapshot();
    println!(
        "joined: active={} pending={} handles={} unresolved={} obligations={} debt={}",
        post.active,
        post.pending,
        post.handles,
        post.unresolved,
        post.usage_obligations,
        post.token_debt
    );
    Ok(())
}
