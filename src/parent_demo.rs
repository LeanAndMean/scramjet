#[path = "../tests/support/readers_fixture.rs"]
pub(crate) mod fixture;
#[path = "../tests/support/readers_root.rs"]
pub(crate) mod roots;
use scramjet_rs::{
    ReaderInputs, ReaderParent, ReaderState, RequestId, Runtime, reconstruct_readers,
};
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let started = std::time::Instant::now();
    let root = roots::OwnedRoot::new();
    let fixture = fixture::ReadersFixture::start(fixture::golden("parent-demo", 1)).await;
    let runtime = Runtime::new(
        fixture.endpoint,
        2,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .map_err(|_| "runtime busy")?;
    let (parent, cleanup) = ReaderParent::start(
        runtime,
        &root.path,
        ReaderInputs::new(
            "parent-demo",
            1,
            [
                "alpha: immutable reader zero",
                "beta: immutable reader one",
                "gamma: immutable reader two",
            ],
        )?,
    )?;
    fixture.wait_child(0).await;
    fixture.wait_child(1).await;
    while Runtime::process_snapshot().pending != 1 {
        tokio::task::yield_now().await;
    }
    let overlap = Runtime::process_snapshot();
    fixture.release(1);
    fixture.wait_child(2).await;
    fixture.release(2);
    fixture.release(0);
    fixture.release_all();
    let run = parent.finish().await?;
    let joined = cleanup.wait().await?;
    let fixture_peak = fixture.peak();
    let contexts = fixture.contexts();
    fixture.shutdown().await;
    let facts = reconstruct_readers(&root.path, "parent-demo", 1)?;
    if overlap.active != 2
        || overlap.pending != 1
        || fixture_peak != 2
        || run.tree.children != facts.children
        || !facts.parent_finished
        || contexts.iter().any(Option::is_none)
        || !run
            .tree
            .children
            .iter()
            .all(|f| matches!(f.state, ReaderState::Completed { .. }))
    {
        return Err("parallel reader invariant".into());
    }
    assert_eq!(run.usage_tokens, Some(3));
    println!(
        "parent-demo: children=3 attempts={} active_overlap={}/{} pending={} out_of_order=1-before-0 elapsed_ms={} synthetic_usage=3",
        run.snapshot.attempts,
        overlap.active,
        fixture_peak,
        overlap.pending,
        started.elapsed().as_millis()
    );
    println!(
        "bounds: content=3072 request_peak={} delta_peak={}/{} journal_max=32768 children=3 queued_storage=1",
        run.snapshot.input_peak, run.snapshot.delta_bytes_peak, run.snapshot.delta_items_peak
    );
    println!(
        "joined: persistence=true runtime=true active={} pending={} handles={} unresolved={} reconstructed=3 no_replay=true",
        joined.active, joined.pending, joined.handles, joined.unresolved
    );
    Ok(())
}
