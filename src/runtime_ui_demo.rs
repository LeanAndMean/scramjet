use crate::{
    runtime_ui_fixture::{Fixture, Report},
    ui,
};
use scramjet_rs::{
    ConcurrencyMode, RequestHandle, RequestId, RequestRate, Runtime, Snapshot, SyntheticCost,
    SyntheticTokenBudget, SyntheticUsage, Terminal,
};
use std::{
    io::{self, IsTerminal, Write},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tokio::{sync::watch, task::JoinSet};

pub(crate) const PREFIX: usize = 4096;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Control {
    pub ready: bool,
    pub cancel: bool,
    pub stop: bool,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Slot {
    pub id: Option<RequestId>,
    pub commitment: Option<usize>,
    pub received: usize,
    pub prefix: String,
    pub clipped: bool,
    pub terminal: Option<Terminal>,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Projection {
    pub slots: [Slot; 4],
    pub snapshot: Option<Snapshot>,
    pub pre: Option<Snapshot>,
    pub post: Option<Snapshot>,
    pub fixture: Option<Report>,
    pub runtime_joined: bool,
    pub consumers_joined: bool,
    pub ended: bool,
    pub error: Option<String>,
}
impl Projection {
    fn error(&mut self, error: impl AsRef<str>) {
        let error = error.as_ref();
        let retained = self
            .error
            .get_or_insert_with(|| String::with_capacity(4096));
        if retained.len() >= 4000 {
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
    fn submitted(&mut self, ordinal: usize, id: RequestId) -> Result<(), &'static str> {
        if ordinal >= 4
            || self.slots[ordinal].id.is_some()
            || self.slots.iter().any(|slot| slot.id == Some(id))
        {
            return Err("duplicate/foreign submission");
        }
        self.slots[ordinal] = Slot {
            id: Some(id),
            prefix: String::with_capacity(PREFIX),
            ..Slot::default()
        };
        Ok(())
    }
    fn delta(&mut self, ordinal: usize, id: RequestId, text: &str) -> Result<(), &'static str> {
        let slot = self.slots.get_mut(ordinal).ok_or("foreign delta ordinal")?;
        if slot.id != Some(id) || slot.terminal.is_some() {
            return Err("foreign/late delta");
        }
        slot.received = slot
            .received
            .checked_add(text.len())
            .ok_or("received byte overflow")?;
        if !slot.clipped {
            let mut end = text.len().min(PREFIX - slot.prefix.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            slot.prefix.push_str(&text[..end]);
            slot.clipped = end < text.len();
        }
        Ok(())
    }
    fn terminal(&mut self, ordinal: usize, terminal: Terminal) -> Result<(), &'static str> {
        let slot = self
            .slots
            .get_mut(ordinal)
            .ok_or("foreign terminal ordinal")?;
        if slot.id != Some(terminal.id) || slot.terminal.is_some() {
            return Err("duplicate/foreign terminal");
        }
        if terminal.text_bytes != slot.received {
            return Err("terminal/delta byte mismatch");
        }
        slot.terminal = Some(terminal);
        Ok(())
    }
}
#[derive(Clone)]
pub(crate) struct Bridge {
    pub store: Arc<Mutex<Projection>>,
    control: watch::Sender<Control>,
}
impl Bridge {
    fn new() -> (Self, watch::Receiver<Control>) {
        let (control, receiver) = watch::channel(Control::default());
        (
            Self {
                store: Arc::new(Mutex::new(Projection::default())),
                control,
            },
            receiver,
        )
    }
    pub fn ready(&self) {
        self.control.send_modify(|state| {
            if !state.stop {
                state.ready = true;
            }
        });
    }
    pub fn cancel(&self) {
        self.control.send_modify(|state| state.cancel = true);
    }
    pub fn stop(&self) {
        self.control.send_modify(|state| state.stop = true);
    }
    pub fn snapshot(&self) -> Projection {
        self.store.lock().unwrap().clone()
    }
}
async fn consume(
    mut handle: RequestHandle,
    ordinal: usize,
    store: Arc<Mutex<Projection>>,
    mut control: watch::Receiver<Control>,
) -> Result<(), &'static str> {
    let id = handle.id();
    let mut cancelled = false;
    let mut open = true;
    let mut count = 0;
    loop {
        let state = *control.borrow_and_update();
        if !cancelled && (state.cancel || state.stop || !open) {
            handle.cancel();
            cancelled = true;
        }
        let commitment = handle.commitment().map(|(order, _)| order);
        store.lock().unwrap().slots[ordinal].commitment = commitment;
        tokio::select! {
            biased;
            changed = control.changed(), if open => { if changed.is_err() { open = false; } },
            delta = handle.next_delta() => {
                let Some(text) = delta else { break; };
                store.lock().unwrap().delta(ordinal, id, &text)?;
                count += 1;
                if count == 8 { tokio::task::yield_now().await; count = 0; }
            }
        }
    }
    let terminal = handle
        .terminal()
        .await
        .ok_or("missing correlated terminal")?;
    store.lock().unwrap().terminal(ordinal, terminal)?;
    drop(handle);
    Ok(())
}
fn snapshot(runtime: &Runtime, store: &Arc<Mutex<Projection>>) -> Option<Snapshot> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.snapshot())) {
        Ok(snapshot) => {
            store.lock().unwrap().snapshot = Some(snapshot);
            Some(snapshot)
        }
        Err(_) => {
            store
                .lock()
                .unwrap()
                .error("runtime accounting unavailable/poisoned");
            None
        }
    }
}
async fn drain_consumers(
    runtime: &Runtime,
    store: &Arc<Mutex<Projection>>,
    control: &mut watch::Receiver<Control>,
    consumers: &mut JoinSet<Result<(), &'static str>>,
    start: tokio::time::Instant,
) {
    let mut stop_at = None;
    let mut open = true;
    let mut sample = tokio::time::interval(Duration::from_millis(50));
    sample.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    while !consumers.is_empty() {
        let state = *control.borrow_and_update();
        if (state.stop || !open) && stop_at.is_none() {
            stop_at = Some(tokio::time::Instant::now());
        }
        let limit = stop_at.map_or(start + Duration::from_secs(15), |stop| {
            (stop + Duration::from_secs(6)).min(start + Duration::from_secs(15))
        });
        tokio::select! {
            biased;
            changed = control.changed(), if open => { if changed.is_err() { open = false; } },
            _ = tokio::time::sleep_until(limit) => {
                store.lock().unwrap().error("consumer deadline: missing terminal/accounting; requests abandoned locally");
                consumers.abort_all();
                while consumers.join_next().await.is_some() {}
                break;
            }
            joined = consumers.join_next() => {
                match joined {
                    Some(Ok(Ok(()))) => {},
                    Some(Ok(Err(error))) => store.lock().unwrap().error(error),
                    Some(Err(error)) => store.lock().unwrap().error(format!("consumer failed: {error}")),
                    None => {},
                }
            }
            _ = sample.tick() => { snapshot(runtime, store); }
        }
    }
    store.lock().unwrap().consumers_joined = true;
}

async fn journey(
    store: Arc<Mutex<Projection>>,
    mut control: watch::Receiver<Control>,
    paced: bool,
) {
    loop {
        let state = *control.borrow_and_update();
        if state.stop {
            store.lock().unwrap().ended = true;
            return;
        }
        if state.ready {
            break;
        }
        if control.changed().await.is_err() {
            store.lock().unwrap().ended = true;
            return;
        }
    }
    let start = tokio::time::Instant::now();
    let fixture = match Fixture::new(paced, true).await {
        Ok(fixture) => fixture,
        Err(error) => {
            let mut store = store.lock().unwrap();
            store.error(format!("fixture init: {error}"));
            store.ended = true;
            return;
        }
    };
    let runtime = match Runtime::new(
        fixture.address,
        2,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Fixed,
    ) {
        Ok(runtime) => runtime,
        Err(_) => {
            let report = fixture.shutdown().await;
            let mut store = store.lock().unwrap();
            store.error("runtime busy");
            match report {
                Ok(report) => store.fixture = Some(report),
                Err(error) => store.error(error),
            }
            store.ended = true;
            return;
        }
    };
    let mut consumers = JoinSet::new();
    for (ordinal, input) in ["unicode", "incomplete", "dense", "stalled"]
        .into_iter()
        .enumerate()
    {
        match runtime.submit(input, SyntheticCost::new(1, 1).unwrap()) {
            Ok(handle) => {
                store
                    .lock()
                    .unwrap()
                    .submitted(ordinal, handle.id())
                    .unwrap();
                consumers.spawn(consume(handle, ordinal, store.clone(), control.clone()));
            }
            Err(error) => store
                .lock()
                .unwrap()
                .error(format!("submission {ordinal}: {error:?}")),
        }
    }
    drain_consumers(&runtime, &store, &mut control, &mut consumers, start).await;
    let report = fixture.shutdown().await;
    let pre = snapshot(&runtime, &store);
    store.lock().unwrap().pre = pre;
    match report {
        Ok(report) => {
            if let Some(error) = &report.error {
                store.lock().unwrap().error(error);
            }
            let slots = store.lock().unwrap().slots.clone();
            for (ordinal, slot) in slots.iter().enumerate() {
                if let Some(terminal) = &slot.terminal
                    && terminal.synthetic_usage == SyntheticUsage::Unknown
                {
                    let attested = report.receipts[ordinal].is_some_and(|receipt| {
                        receipt.id == terminal.id && receipt.ordinal == ordinal && receipt.joined
                    });
                    if attested {
                        if let Err(error) = Runtime::reconcile_usage_fixture(terminal.id, 1) {
                            store.lock().unwrap().error(error);
                        }
                        if let Err(error) = Runtime::reconcile_fixture(terminal.id) {
                            store.lock().unwrap().error(error);
                        }
                    } else {
                        store
                            .lock()
                            .unwrap()
                            .error("unattested dispatch: usage/exposure remain unknown");
                    }
                }
            }
            store.lock().unwrap().fixture = Some(report);
        }
        Err(error) => store.lock().unwrap().error(error),
    }
    let mut shutdown = tokio::spawn(runtime.shutdown());
    match tokio::time::timeout(Duration::from_secs(2), &mut shutdown).await {
        Ok(Ok(post)) => {
            let mut store = store.lock().unwrap();
            store.post = Some(post);
            store.snapshot = Some(post);
            store.runtime_joined = true;
            if post.active != 0
                || post.pending != 0
                || post.handles != 0
                || post.delta_items != 0
                || post.input_bytes != 0
                || post.delta_bytes != 0
                || post.potential != 0
                || post.unresolved != 0
                || post.usage_obligations != 0
                || post.token_debt != 0
            {
                store.error("joined cleanup retains local work/exposure/usage/debt");
            }
        }
        Ok(Err(error)) => store
            .lock()
            .unwrap()
            .error(format!("runtime supervisor failed/unjoined: {error}")),
        Err(_) => {
            shutdown.abort();
            let _ = shutdown.await;
            store.lock().unwrap().error(
                "runtime shutdown timeout: unjoined local supervision; executor must be dropped",
            );
        }
    }
    store.lock().unwrap().ended = true;
}

pub fn run() -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("requires TTY stdin and stdout"));
    }
    let (bridge, control) = Bridge::new();
    let store = bridge.store.clone();
    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let worker = thread::Builder::new()
        .name("runtime-ui-worker".into())
        .spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> io::Result<()> {
                    let executor = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    executor.block_on(journey(store.clone(), control, true));
                    drop(executor);
                    Ok(())
                }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => store.lock().unwrap().error(format!("executor: {error}")),
                Err(_) => store
                    .lock()
                    .unwrap()
                    .error("executor panic; local join/settlement unavailable"),
            }
        });
    let worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            std::panic::set_hook(panic_hook);
            return Err(error);
        }
    };
    let execution = ui::run_runtime(&bridge);
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
    if let Some(error) = &projection.error {
        errors.push(error.clone());
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
            let _ = writeln!(writer, "runtime-ui-demo: {error}");
        }
    } else {
        let _ = writeln!(io::stderr(), "runtime-ui-demo: {error}");
    }
}

fn write_report(projection: &Projection, writer: &mut impl Write) -> io::Result<()> {
    writeln!(
        writer,
        "runtime-ui-demo: workload=4 ceiling=2 prefix_limit=4096 render_limit=8192 retries=0 ended={} consumers_joined={} fixture_joined={} runtime_joined={} error={}",
        projection.ended,
        projection.consumers_joined,
        projection.fixture.is_some(),
        projection.runtime_joined,
        projection.error.is_some()
    )?;
    for (ordinal, slot) in projection.slots.iter().enumerate() {
        writeln!(
            writer,
            "request={} id={:?} commitment={:?} received={} excerpt_bytes={} excerpt_clipped={} outcome={:?} terminal_usage={:?}",
            ordinal + 1,
            slot.id,
            slot.commitment,
            slot.received,
            slot.prefix.len(),
            slot.clipped,
            slot.terminal.as_ref().map(|terminal| &terminal.outcome),
            slot.terminal
                .as_ref()
                .map(|terminal| terminal.synthetic_usage)
        )?;
    }
    for (label, snapshot) in [
        ("pre_attestation", projection.pre),
        ("post_join", projection.post),
    ] {
        if let Some(snapshot) = snapshot {
            writeln!(
                writer,
                "{label}: active={} pending={} handles={} deltas={} exposure={} unknown_usage={} usage_obligations={} debt={} measured={:?} commitments={} attempts={} active_peak={} pending_peak={} delta_items_peak={}",
                snapshot.active,
                snapshot.pending,
                snapshot.handles,
                snapshot.delta_items,
                snapshot.potential + snapshot.unresolved,
                snapshot.unknown_usage,
                snapshot.usage_obligations,
                snapshot.token_debt,
                snapshot.synthetic_measured,
                snapshot.commitments,
                snapshot.attempts,
                snapshot.active_peak,
                snapshot.pending_peak,
                snapshot.delta_items_peak
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
    use scramjet_rs::Outcome;
    fn terminal(id: RequestId, text_bytes: usize) -> Terminal {
        Terminal {
            id,
            outcome: Outcome::Success,
            text_bytes,
            body_bytes: 0,
            wire_bytes: 0,
            retry_after: None,
            synthetic_usage: SyntheticUsage::Known(1),
            dispatch_to_head: None,
            commitment_to_terminal: None,
            output_backpressure: false,
        }
    }
    #[test]
    fn fixed_prefix_and_immutable_correlated_material_state() {
        let mut projection = Projection::default();
        for ordinal in 0..4 {
            projection
                .submitted(ordinal, RequestId(ordinal as u64 + 1))
                .unwrap();
        }
        assert!(projection.submitted(4, RequestId(5)).is_err());
        for ordinal in 0..4 {
            for _ in 0..40 {
                projection
                    .delta(ordinal, RequestId(ordinal as u64 + 1), &"x".repeat(1024))
                    .unwrap();
            }
            assert_eq!(projection.slots[ordinal].prefix.len(), PREFIX);
            assert_eq!(projection.slots[ordinal].received, 40960);
            assert!(projection.slots[ordinal].clipped);
        }
        for ordinal in [3, 1, 2, 0] {
            projection
                .terminal(ordinal, terminal(RequestId(ordinal as u64 + 1), 40960))
                .unwrap();
        }
        assert!(
            projection
                .terminal(0, terminal(RequestId(1), 40960))
                .is_err()
        );
        assert!(projection.delta(0, RequestId(1), "late").is_err());
        assert!(
            projection
                .terminal(0, terminal(RequestId(99), 40960))
                .is_err()
        );
        assert_eq!(
            projection
                .slots
                .iter()
                .filter(|slot| slot.terminal.is_some())
                .count(),
            4
        );
    }
    #[test]
    fn raw_prefix_never_resumes_after_utf8_clipping_and_control_is_monotonic() {
        let mut projection = Projection::default();
        projection.submitted(0, RequestId(1)).unwrap();
        projection
            .delta(0, RequestId(1), &"x".repeat(4095))
            .unwrap();
        projection.delta(0, RequestId(1), "界").unwrap();
        projection.delta(0, RequestId(1), "y").unwrap();
        assert_eq!(projection.slots[0].prefix, "x".repeat(4095));
        let (bridge, receiver) = Bridge::new();
        bridge.cancel();
        bridge.ready();
        bridge.stop();
        bridge.ready();
        bridge.cancel();
        assert_eq!(
            *receiver.borrow(),
            Control {
                ready: true,
                cancel: true,
                stop: true
            }
        );
        let (bridge, receiver) = Bridge::new();
        bridge.stop();
        bridge.ready();
        assert_eq!(
            *receiver.borrow(),
            Control {
                ready: false,
                cancel: false,
                stop: true
            }
        );
    }
    #[tokio::test]
    async fn stopped_before_ready_has_no_runtime_or_fixture() {
        let (bridge, receiver) = Bridge::new();
        bridge.stop();
        journey(bridge.store.clone(), receiver, false).await;
        assert!(bridge.snapshot().ended);
        assert!(bridge.snapshot().slots.iter().all(|slot| slot.id.is_none()));
        assert!(bridge.snapshot().fixture.is_none());
    }
    #[tokio::test]
    async fn cancel_before_ready_and_closed_owner_do_not_spin_or_fabricate_results() {
        let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
        for close in [false, true] {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let (bridge, receiver) = Bridge::new();
            let store = bridge.store.clone();
            if !close {
                bridge.cancel();
            }
            bridge.ready();
            if close {
                drop(bridge);
            }
            tokio::time::timeout(
                Duration::from_secs(3),
                journey(store.clone(), receiver, false),
            )
            .await
            .unwrap();
            let projection = store.lock().unwrap().clone();
            assert!(projection.ended && projection.consumers_joined && projection.runtime_joined);
            assert!(projection.slots.iter().all(|slot| {
                slot.terminal
                    .as_ref()
                    .is_some_and(|terminal| terminal.outcome == Outcome::Cancelled)
            }));
            assert_eq!(projection.post.unwrap().active, 0);
            assert_eq!(projection.post.unwrap().handles, 0);
            if projection.error.is_none() {
                assert_eq!(projection.post.unwrap().unresolved, 0);
                assert_eq!(projection.post.unwrap().unknown_usage, 0);
            }
        }
    }

    #[tokio::test]
    async fn real_http_drains_without_paint_and_cancel_joins_with_attested_usage() {
        let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        let (bridge, receiver) = Bridge::new();
        bridge.ready();
        let task = tokio::spawn(journey(bridge.store.clone(), receiver, false));
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let projection = bridge.snapshot();
                assert!(
                    !projection.ended,
                    "journey ended before observation: {:?}",
                    projection.error
                );
                if projection.slots[2].terminal.is_some() && projection.slots[3].received > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let before = bridge.snapshot();
        assert_eq!(before.slots[2].received, 40960);
        assert_eq!(before.slots[2].prefix.len(), 4096);
        assert_eq!(
            before.slots[0].terminal.as_ref().unwrap().outcome,
            Outcome::Success
        );
        assert_eq!(
            before.slots[1].terminal.as_ref().unwrap().outcome,
            Outcome::Incomplete
        );
        bridge.cancel();
        bridge.cancel();
        bridge.stop();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        let projection = bridge.snapshot();
        assert!(projection.error.is_none(), "{:?}", projection.error);
        assert_eq!(
            projection.slots[3].terminal.as_ref().unwrap().outcome,
            Outcome::Cancelled
        );
        assert!(projection.runtime_joined && projection.consumers_joined);
        assert!(projection.pre.unwrap().unknown_usage >= 2);
        assert_eq!(projection.post.unwrap().synthetic_measured, Some(4));
        assert_eq!(projection.post.unwrap().potential, 0);
        assert_eq!(projection.post.unwrap().usage_obligations, 0);
        let report = projection.fixture.as_ref().unwrap();
        assert_eq!(
            report
                .receipts
                .iter()
                .flatten()
                .filter(|receipt| receipt.joined)
                .count(),
            4
        );
        assert!(report.peak <= 2 && report.accepts <= 4);
        let mut output = Vec::new();
        write_report(&projection, &mut output).unwrap();
        assert!(output.len() < 4096);
        assert!(!output.windows(8).any(|window| window == b"external"));
    }
}
