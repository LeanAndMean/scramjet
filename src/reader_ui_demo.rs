use crate::{
    reader_ui_fixture::{Fixture, Report},
    ui,
};
use scramjet_rs::{
    ConcurrencyMode, ReaderInputs, ReaderObservation, ReaderParent, ReaderState, RequestRate,
    Runtime, Snapshot, SyntheticTokenBudget, reconstruct_readers,
};
use std::os::unix::fs::DirBuilderExt;
use std::{
    io::{self, IsTerminal, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

pub(crate) const SESSION: &str = "reader-ui";
pub(crate) const GENERATION: u64 = 1;
#[derive(Clone, Copy, Debug, Default)]
struct Intent {
    ready: bool,
    children: u8,
    stop: bool,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Projection {
    pub observation: Option<ReaderObservation>,
    pub pre: Option<Snapshot>,
    pub post: Option<Snapshot>,
    pub fixture: Option<Report>,
    pub ended: bool,
    pub reconstructed: bool,
    pub error: Option<String>,
}
impl Projection {
    fn error(&mut self, error: &str) {
        let retained = self
            .error
            .get_or_insert_with(|| String::with_capacity(4096));
        if retained.len() > 3500 {
            return;
        }
        if !retained.is_empty() {
            retained.push_str("; ");
        }
        let mut end = error.len().min(500);
        while !error.is_char_boundary(end) {
            end -= 1;
        }
        retained.push_str(&error[..end]);
    }
}
#[derive(Clone)]
pub(crate) struct Bridge {
    store: Arc<Mutex<Projection>>,
    intent: watch::Sender<Intent>,
}
impl Bridge {
    fn new() -> (Self, watch::Receiver<Intent>) {
        let (intent, receiver) = watch::channel(Intent::default());
        (
            Self {
                store: Arc::new(Mutex::new(Projection::default())),
                intent,
            },
            receiver,
        )
    }
    pub fn ready(&self) {
        self.intent.send_modify(|state| {
            if !state.stop {
                state.ready = true;
            }
        });
    }
    pub fn cancel_child(&self, child: u8) {
        if child < 3 {
            self.intent
                .send_modify(|state| state.children |= 1 << child);
        }
    }
    pub fn stop(&self) {
        self.intent.send_modify(|state| state.stop = true);
    }
    pub fn snapshot(&self) -> Projection {
        self.store.lock().unwrap().clone()
    }
}
pub(crate) fn reconcile(
    observation: &ReaderObservation,
    report: &Report,
) -> Result<(), &'static str> {
    if report.session != observation.session || report.error.is_some() {
        return Err("fixture invocation/join failed");
    }
    for (child, slot) in observation.children.iter().enumerate() {
        let Some(accounting) = slot.accounting else {
            if slot.request_id.is_some() {
                return Err("missing runtime accounting receipt");
            }
            continue;
        };
        if slot.request_id != Some(accounting.request_id) {
            return Err("foreign runtime accounting receipt");
        }
        if accounting.unknown_usage || accounting.unresolved_exposure {
            let receipt = report.receipts[child]
                .filter(|r| {
                    r.child as usize == child
                        && r.generation == observation.generation
                        && r.request_id == accounting.request_id
                        && r.joined
                        && r.cost == 1
                })
                .ok_or("unattested request: accounting remains unknown")?;
            if accounting.unknown_usage {
                Runtime::reconcile_usage_fixture(receipt.request_id, receipt.cost)?;
            }
            if accounting.unresolved_exposure {
                Runtime::reconcile_fixture(receipt.request_id)?;
            }
        }
    }
    Ok(())
}
async fn journey(store: Arc<Mutex<Projection>>, mut intent: watch::Receiver<Intent>, paced: bool) {
    loop {
        let state = *intent.borrow_and_update();
        if state.stop {
            store.lock().unwrap().ended = true;
            return;
        }
        if state.ready {
            break;
        }
        if intent.changed().await.is_err() {
            store.lock().unwrap().ended = true;
            return;
        }
    }
    let fixture = match Fixture::new(SESSION, GENERATION).await {
        Ok(fixture) => fixture,
        Err(_) => {
            let mut store = store.lock().unwrap();
            store.error("fixture init failed");
            store.ended = true;
            return;
        }
    };
    let root = std::env::temp_dir().join(format!(
        "scramjet-reader-ui-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut root_created = false;
    let setup = (|| {
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        root_created = true;
        let runtime = Runtime::new(
            fixture.address,
            2,
            RequestRate::new(4, Duration::from_millis(1)).unwrap(),
            SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
            ConcurrencyMode::Fixed,
        )
        .map_err(|_| io::Error::other("runtime busy"))?;
        ReaderParent::start(
            runtime,
            &root,
            ReaderInputs::new(
                SESSION,
                GENERATION,
                ["immutable alpha", "immutable beta", "immutable gamma"],
            )
            .unwrap(),
        )
        .map_err(io::Error::other)
    })();
    match setup {
        Ok((parent, cleanup)) => {
            let observer = parent.observer();
            let control = parent.control();
            let mut finish = Box::pin(parent.finish());
            let start = tokio::time::Instant::now();
            let mut sample = tokio::time::interval(Duration::from_millis(20));
            sample.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut open = true;
            let mut released = [false; 3];
            let mut sent = 0u8;
            let mut stopped = false;
            let result = loop {
                let state = *intent.borrow_and_update();
                if (state.stop || !open) && !stopped {
                    stopped = true;
                    if control.cancel_tree(SESSION, GENERATION).is_err() {
                        store.lock().unwrap().error("foreign tree control");
                    }
                }
                for child in 0..3 {
                    if state.children & (1 << child) != 0 && sent & (1 << child) == 0 {
                        sent |= 1 << child;
                        if control.cancel_child(SESSION, GENERATION, child).is_err() {
                            store.lock().unwrap().error("foreign child control");
                        }
                    }
                }
                let observation = observer.snapshot();
                store.lock().unwrap().observation = Some(observation.clone());
                let delays = if paced {
                    [2500, 500, 3000]
                } else {
                    [50, 0, 100]
                };
                for child in [1, 0, 2] {
                    let predecessor = match child {
                        0 => Some(1),
                        2 => Some(0),
                        _ => None,
                    };
                    if !released[child]
                        && fixture.captured(child)
                        && start.elapsed() >= Duration::from_millis(delays[child])
                        && predecessor.is_none_or(|index| {
                            observation.children[index].acceptance_ordinal.is_some()
                                || stopped
                                || sent & (1 << index) != 0
                        })
                    {
                        fixture.release(child);
                        released[child] = true;
                    }
                }
                tokio::select! {
                    biased;
                    changed = intent.changed(), if open => { if changed.is_err() { open = false; } },
                    result = &mut finish => break result,
                    _ = sample.tick() => {},
                }
            };
            let cleaned = cleanup.wait().await;
            let observation = observer.snapshot();
            {
                let mut store = store.lock().unwrap();
                store.observation = Some(observation.clone());
                match &cleaned {
                    Ok(snapshot) => store.pre = Some(*snapshot),
                    Err(_) => store.error("reader cleanup unavailable/unjoined"),
                }
                if result.is_err() {
                    store.error("reader tree failed; acknowledged siblings retained");
                }
                if observation.error.is_some() {
                    store.error("reader observation reports failure");
                }
            }
            match fixture.shutdown().await {
                Ok(report) => {
                    if let Err(error) = reconcile(&observation, &report) {
                        store.lock().unwrap().error(error);
                    }
                    store.lock().unwrap().fixture = Some(report);
                }
                Err(error) => store.lock().unwrap().error(error),
            }
            if cleaned.is_ok() {
                let post = Runtime::process_snapshot();
                let mut store = store.lock().unwrap();
                store.post = Some(post);
                if post.active != 0
                    || post.pending != 0
                    || post.handles != 0
                    || post.input_bytes != 0
                    || post.delta_bytes != 0
                    || post.delta_items != 0
                    || post.potential != 0
                    || post.unresolved != 0
                    || post.usage_obligations != 0
                    || post.token_debt != 0
                {
                    store.error("joined cleanup retains work/exposure/usage/debt");
                }
            }
            if observation.storage_joined {
                match reconstruct_readers(&root, SESSION, GENERATION) {
                    Ok(tree)
                        if tree.children.iter().enumerate().all(|(child, fact)| {
                            observation.children[child]
                                .fact
                                .as_ref()
                                .is_none_or(|live| live == fact)
                        }) =>
                    {
                        store.lock().unwrap().reconstructed = true
                    }
                    _ => store
                        .lock()
                        .unwrap()
                        .error("live/durable reconstruction mismatch"),
                }
                if std::fs::remove_dir_all(&root).is_err() {
                    store
                        .lock()
                        .unwrap()
                        .error("disposable root cleanup failed");
                }
            } else {
                store
                    .lock()
                    .unwrap()
                    .error("storage join unavailable: disposable root retained");
            }
        }
        Err(error) => {
            store
                .lock()
                .unwrap()
                .error(&format!("reader setup: {error}"));
            match fixture.shutdown().await {
                Ok(report) => store.lock().unwrap().fixture = Some(report),
                Err(error) => store.lock().unwrap().error(error),
            }
            // No journal worker exists when start itself returns an error.
            if root_created && std::fs::remove_dir_all(&root).is_err() {
                store.lock().unwrap().error("setup root cleanup failed");
            }
        }
    }
    store.lock().unwrap().ended = true;
}
pub fn run() -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("requires TTY stdin and stdout"));
    }
    let (bridge, intent) = Bridge::new();
    let store = bridge.store.clone();
    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let worker = std::thread::Builder::new()
        .name("reader-ui-worker".into())
        .spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> io::Result<()> {
                    let executor = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    executor.block_on(journey(store.clone(), intent, true));
                    Ok(())
                }));
            if !matches!(result, Ok(Ok(()))) {
                store
                    .lock()
                    .unwrap()
                    .error("executor failed; cleanup unavailable/unjoined");
            }
        });
    let worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            std::panic::set_hook(panic_hook);
            return Err(error);
        }
    };
    let execution = ui::run_reader(&bridge);
    bridge.stop();
    let joined = worker.join();
    std::panic::set_hook(panic_hook);
    if joined.is_err() {
        bridge
            .store
            .lock()
            .unwrap()
            .error("executor thread join failed");
    }
    let projection = bridge.snapshot();
    let report = ui::runtime_output().and_then(|mut writer| write_report(&projection, &mut writer));
    let mut errors = Vec::new();
    if let Err(error) = execution {
        errors.push(format!("terminal: {error}"));
    }
    if let Some(error) = projection.error {
        errors.push(error);
    }
    if let Err(error) = report {
        errors.push(format!("report: {error}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(errors.join("; ")))
    }
}
pub fn report_error(error: &io::Error) {
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        if let Ok(mut writer) = ui::runtime_output() {
            let _ = writeln!(writer, "reader-ui-demo: {error}");
        }
    } else {
        let _ = writeln!(io::stderr(), "reader-ui-demo: {error}");
    }
}
fn write_report(projection: &Projection, writer: &mut impl Write) -> io::Result<()> {
    writeln!(
        writer,
        "reader-ui-demo: workload=3 ceiling=2 retries=0 ended={} fixture_joined={} reconstructed={} error={}",
        projection.ended,
        projection.fixture.is_some(),
        projection.reconstructed,
        projection.error.is_some()
    )?;
    if let Some(observation) = &projection.observation {
        writeln!(
            writer,
            "invocation={}/{} tree_end={} storage_joined={} runtime_joined={}",
            observation.session,
            observation.generation,
            observation.tree_ended,
            observation.storage_joined,
            observation.runtime_joined
        )?;
        for (child, slot) in observation.children.iter().enumerate() {
            let disposition = slot.fact.as_ref().map(|fact| match fact.state {
                ReaderState::Completed { .. } => "Completed",
                ReaderState::Cancelled => "Cancelled",
                ReaderState::Failed(_) => "Failed",
                ReaderState::NotDispatched => "NotDispatched",
                ReaderState::Interrupted => "Interrupted",
            });
            writeln!(
                writer,
                "child={child} id={:?} ordinal={:?} disposition={disposition:?} received={} accounting={:?}",
                slot.request_id, slot.acceptance_ordinal, slot.received, slot.accounting
            )?;
        }
    }
    for (label, snapshot) in [
        ("pre_attestation", projection.pre),
        ("post_attestation", projection.post),
    ] {
        if let Some(snapshot) = snapshot {
            writeln!(
                writer,
                "{label}: active={} pending={} handles={} exposure={} unknown_usage={} obligations={} debt={} measured={:?}",
                snapshot.active,
                snapshot.pending,
                snapshot.handles,
                snapshot.potential + snapshot.unresolved,
                snapshot.unknown_usage,
                snapshot.usage_obligations,
                snapshot.token_debt,
                snapshot.synthetic_measured
            )?;
        } else {
            writeln!(writer, "{label}: unavailable")?;
        }
    }
    writer.flush()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn three_live_facts_without_paint_and_checked_joined_reconciliation() {
        let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        let (bridge, intent) = Bridge::new();
        bridge.ready();
        tokio::time::timeout(
            Duration::from_secs(4),
            journey(bridge.store.clone(), intent, false),
        )
        .await
        .unwrap();
        let projection = bridge.snapshot();
        assert!(projection.error.is_none(), "{projection:?}");
        let observation = projection.observation.as_ref().unwrap();
        assert_eq!(
            observation
                .children
                .each_ref()
                .map(|slot| slot.acceptance_ordinal),
            [Some(2), Some(1), Some(3)]
        );
        assert!(
            observation.tree_ended
                && observation.storage_joined
                && observation.runtime_joined
                && projection.reconstructed
        );
        for (child, slot) in observation.children.iter().enumerate() {
            assert_eq!(
                slot.fact.as_ref().unwrap().state,
                ReaderState::Completed {
                    summary: format!("reader-{child}: immutable synthetic content summarized"),
                }
            );
            let receipt = slot.accounting.unwrap();
            assert_eq!(slot.request_id, Some(receipt.request_id));
            assert!(!receipt.unknown_usage && !receipt.unresolved_exposure);
        }
        assert_eq!(projection.post.unwrap().synthetic_measured, Some(3));
        assert!(projection.fixture.as_ref().unwrap().peak <= 2);
        let mut report = Vec::new();
        write_report(&projection, &mut report).unwrap();
        assert!(report.len() < 4096);
    }
    #[tokio::test]
    async fn child_tree_and_closed_control_keep_accepted_sibling_and_checked_receipts() {
        let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
        for mode in 0..3 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let (bridge, intent) = Bridge::new();
            bridge.ready();
            let task = tokio::spawn(journey(bridge.store.clone(), intent, true));
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if bridge
                        .snapshot()
                        .observation
                        .as_ref()
                        .is_some_and(|o| o.children[1].fact.is_some())
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            assert!(!task.is_finished());
            bridge.cancel_child(1);
            let store = bridge.store.clone();
            if mode == 0 {
                bridge.cancel_child(0);
            } else if mode == 1 {
                bridge.stop();
            }
            let retained = if mode == 2 {
                drop(bridge);
                None
            } else {
                Some(bridge)
            };
            tokio::time::timeout(Duration::from_secs(4), task)
                .await
                .unwrap()
                .unwrap();
            drop(retained);
            let projection = store.lock().unwrap().clone();
            assert!(projection.error.is_none(), "{projection:?}");
            let observation = projection.observation.unwrap();
            assert!(matches!(
                observation.children[1].fact.as_ref().unwrap().state,
                ReaderState::Completed { .. }
            ));
            assert_eq!(observation.children[1].acceptance_ordinal, Some(1));
            assert!(observation.children.iter().all(|s| s.fact.is_some()));
            assert_eq!(
                observation
                    .children
                    .iter()
                    .filter(|s| s
                        .fact
                        .as_ref()
                        .is_some_and(|f| f.state == ReaderState::Cancelled))
                    .count(),
                if mode == 0 { 1 } else { 2 }
            );
            assert_eq!(projection.post.unwrap().unknown_usage, 0);
            assert_eq!(projection.post.unwrap().unresolved, 0);
        }
    }
    #[test]
    fn reconciliation_never_calls_false_bits_and_rejects_missing_foreign_attestations() {
        let mut observation = ReaderObservation {
            session: SESSION.into(),
            generation: GENERATION,
            children: std::array::from_fn(|_| scramjet_rs::ReaderSlot::default()),
            tree_ended: false,
            storage_joined: false,
            runtime_joined: false,
            error: None,
        };
        let report = Report {
            session: SESSION.into(),
            receipts: [None; 3],
            accepts: 0,
            peak: 0,
            error: None,
        };
        observation.children[0].request_id = Some(scramjet_rs::RequestId(u64::MAX));
        assert!(reconcile(&observation, &report).is_err());
        observation.children[0].accounting = Some(scramjet_rs::ReaderAccounting {
            request_id: scramjet_rs::RequestId(u64::MAX),
            unknown_usage: false,
            unresolved_exposure: false,
        });
        assert!(reconcile(&observation, &report).is_ok());
        observation.children[0]
            .accounting
            .as_mut()
            .unwrap()
            .unknown_usage = true;
        assert!(reconcile(&observation, &report).is_err());
        observation.children[0]
            .accounting
            .as_mut()
            .unwrap()
            .unknown_usage = false;
        observation.children[0]
            .accounting
            .as_mut()
            .unwrap()
            .unresolved_exposure = true;
        assert!(reconcile(&observation, &report).is_err());
        let mut foreign = report.clone();
        foreign.receipts[0] = Some(crate::reader_ui_fixture::Receipt {
            child: 0,
            request_id: scramjet_rs::RequestId(u64::MAX),
            generation: GENERATION + 1,
            joined: true,
            cost: 1,
        });
        assert!(reconcile(&observation, &foreign).is_err());
    }
    #[tokio::test]
    async fn stop_before_ready_starts_nothing() {
        let (bridge, intent) = Bridge::new();
        bridge.stop();
        journey(bridge.store.clone(), intent, false).await;
        assert!(bridge.snapshot().ended && bridge.snapshot().observation.is_none());
    }
}
