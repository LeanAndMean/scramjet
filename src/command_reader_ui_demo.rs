use crate::{
    command_readers_demo::{CATALOG, CONTENTS},
    command_ui_demo::Offer,
    reader_ui_fixture::{Fixture, Report},
    ui,
};
use scramjet_rs::{
    CommandControl, CommandObserver, CommandPhase, CommandPolicy, CommandReaderAdapter,
    CommandReaderControl, CommandReaderSettlement, CommandSession, CommandSnapshot,
    ConcurrencyMode, ReaderObservation, ReaderObserver, RequestRate, Runtime, Snapshot,
    SyntheticTokenBudget, reconstruct_readers,
};
use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
pub(crate) struct Projection {
    pub offer: Option<Offer>,
    pub command: Option<CommandSnapshot>,
    pub readers: Option<ReaderObservation>,
    pub before_report: bool,
    pub pre: Option<Snapshot>,
    pub post: Option<Snapshot>,
    pub fixture: Option<Report>,
    pub reconstructed: bool,
    pub ended: bool,
    pub error: Option<String>,
    pub retained_root: Option<PathBuf>,
}
struct State {
    projection: Projection,
    command: Option<CommandObserver>,
    control: Option<CommandControl>,
    readers: Option<ReaderObserver>,
    reader_control: Option<CommandReaderControl>,
    painted: Option<Offer>,
    invoked: bool,
    stop: bool,
    terminal_returned: bool,
}
#[derive(Clone)]
pub(crate) struct Bridge {
    state: Arc<(Mutex<State>, Condvar)>,
}
impl Bridge {
    fn new() -> Self {
        Self {
            state: Arc::new((
                Mutex::new(State {
                    projection: Projection::default(),
                    command: None,
                    control: None,
                    readers: None,
                    reader_control: None,
                    painted: None,
                    invoked: false,
                    stop: false,
                    terminal_returned: false,
                }),
                Condvar::new(),
            )),
        }
    }
    pub fn snapshot(&self) -> Projection {
        let (mut p, command, readers) = {
            let s = self.state.0.lock().unwrap();
            let mut p = s.projection.clone();
            if s.stop {
                p.offer = None;
            }
            (p, s.command.clone(), s.readers.clone())
        };
        if let Some(command) = command {
            p.command = Some(command.snapshot());
        }
        if let Some(readers) = readers {
            p.readers = Some(readers.snapshot());
        }
        p
    }
    fn attach(&self, command: &CommandSession) {
        let control = command.control();
        let mut s = self.state.0.lock().unwrap();
        s.projection.offer = Some(Offer::Compare(command.observer().session()));
        s.command = Some(command.observer());
        s.control = Some(control.clone());
        if s.stop {
            control.cancel();
        }
    }
    pub fn painted(&self, offer: Option<Offer>) {
        let mut s = self.state.0.lock().unwrap();
        if offer.is_none() {
            s.painted = None;
        } else if !s.stop && offer == s.projection.offer {
            s.painted = offer;
        }
    }
    pub fn invoke(&self, offer: Option<Offer>) -> bool {
        let mut s = self.state.0.lock().unwrap();
        if s.stop
            || s.invoked
            || offer.is_none()
            || offer != s.painted
            || offer != s.projection.offer
        {
            return false;
        }
        s.invoked = true;
        s.projection.offer = None;
        s.painted = None;
        self.state.1.notify_one();
        true
    }
    pub fn stop(&self) {
        let control = {
            let mut s = self.state.0.lock().unwrap();
            s.stop = true;
            s.painted = None;
            self.state.1.notify_one();
            s.control.clone()
        };
        if let Some(control) = control {
            control.cancel();
        }
    }
    pub fn cancel_child(&self, child: u8) {
        let (control, observer) = {
            let s = self.state.0.lock().unwrap();
            (s.reader_control.clone(), s.readers.clone())
        };
        if let (Some(control), Some(observer)) = (control, observer) {
            let observation = observer.snapshot();
            let _ = control.cancel_child(&observation.session, observation.generation, child);
        }
    }
    fn wait_invoke(&self) -> bool {
        let mut s = self.state.0.lock().unwrap();
        while !s.stop && !s.invoked {
            s = self.state.1.wait(s).unwrap();
        }
        !s.stop && s.invoked
    }
    fn terminal_returned(&self) {
        let mut s = self.state.0.lock().unwrap();
        s.terminal_returned = true;
        self.state.1.notify_one();
    }
    fn wait_terminal(&self) {
        let mut s = self.state.0.lock().unwrap();
        while !s.terminal_returned {
            s = self.state.1.wait(s).unwrap();
        }
    }
    fn error(&self, message: &str) {
        let mut s = self.state.0.lock().unwrap();
        if s.projection.error.is_none() {
            let mut end = message.len().min(512);
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            s.projection.error = Some(message[..end].into());
        }
    }
}
async fn logical(
    bridge: &Bridge,
    command: &mut CommandSession,
    adapter: &mut CommandReaderAdapter,
    fixture: &Fixture,
    paced: bool,
) -> io::Result<()> {
    let observer = adapter.observer();
    {
        let mut s = bridge.state.0.lock().unwrap();
        s.readers = Some(observer.clone());
        s.reader_control = Some(adapter.control());
    }
    let start = tokio::time::Instant::now();
    let mut released = [false; 3];
    let mut timer = tokio::time::interval(Duration::from_millis(20));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut receipt = loop {
        let observation = observer.snapshot();
        for child in [1, 0, 2] {
            let delays = if paced {
                [2500, 500, 3000]
            } else {
                [50, 0, 100]
            };
            let predecessor = match child {
                0 => Some(1),
                2 => Some(0),
                _ => None,
            };
            if !released[child]
                && fixture.captured(child)
                && start.elapsed() >= Duration::from_millis(delays[child])
                && predecessor.is_none_or(|i| observation.children[i].fact.is_some())
            {
                fixture.release(child);
                released[child] = true;
            }
        }
        tokio::select! {
            result = adapter.logical() => break result.map_err(io::Error::other)?,
            _ = timer.tick() => {},
        }
    };
    command
        .accept_readers(&mut receipt)
        .map_err(io::Error::other)?;
    let snapshot = command.snapshot();
    let id = snapshot.invocations[0].as_ref().unwrap().id;
    if snapshot.invocations[0].as_ref().unwrap().phase != CommandPhase::AwaitingCompletion {
        return Err(io::Error::other("logical receipt not pending report"));
    }
    bridge.state.0.lock().unwrap().projection.before_report = true;
    crate::command_readers_demo::report(command, id).map_err(io::Error::other)
}
fn journey(bridge: &Bridge, paced: bool) -> io::Result<()> {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut roots = crate::command_fixture::Roots::new_retained(CATALOG, b"", b"", |root| {
        bridge.state.0.lock().unwrap().projection.retained_root = Some(root.to_owned());
    })?;
    let mut command = CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause)
        .map_err(io::Error::other)?;
    bridge.attach(&command);
    let mut fixture = None;
    let mut adapter = None;
    let mut settlement: Option<CommandReaderSettlement> = None;
    let execution = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> io::Result<()> {
        if !bridge.wait_invoke() {
            return Ok(());
        }
        let id = match command.begin_readers("summarize", CONTENTS) {
            Ok(id) => id,
            Err(_) if command.snapshot().cancellation_requested => return Ok(()),
            Err(e) => return Err(io::Error::other(e)),
        };
        let session = command.snapshot().invocations[0]
            .as_ref()
            .unwrap()
            .readers
            .as_ref()
            .unwrap()
            .session
            .clone();
        fixture = Some(executor.block_on(Fixture::new(&session, 1))?);
        let _entered = executor.enter();
        let runtime = Runtime::new(
            fixture.as_ref().unwrap().address,
            2,
            RequestRate::new(4, Duration::from_millis(1)).unwrap(),
            SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
            ConcurrencyMode::Fixed,
        )
        .map_err(|_| io::Error::other("runtime busy"))?;
        let (reader, cleanup) = {
            let _entered = executor.enter();
            command
                .start_readers(id, runtime, &roots.inputs)
                .map_err(io::Error::other)?
        };
        adapter = Some(reader);
        settlement = Some(cleanup);
        executor.block_on(logical(
            bridge,
            &mut command,
            adapter.as_mut().unwrap(),
            fixture.as_ref().unwrap(),
            paced,
        ))
    }))
    .unwrap_or_else(|_| {
        Err(io::Error::other(
            "command reader executor panic; actual facts retained",
        ))
    });
    if let Err(error) = &execution {
        bridge.error(&error.to_string());
    }
    bridge.wait_terminal();
    let observer = adapter.as_ref().map(CommandReaderAdapter::observer);
    drop(adapter);
    let cleaned = settlement.map(|settlement| executor.block_on(settlement.wait()));
    let final_observer = command.observer();
    bridge.state.0.lock().unwrap().projection.command = Some(command.finalize());
    if let Some(cleaned) = &cleaned {
        match cleaned {
            Ok(snapshot) => bridge.state.0.lock().unwrap().projection.pre = Some(*snapshot),
            Err(error) => bridge.error(&error.to_string()),
        }
    }
    let observation = observer.map(|o| o.snapshot());
    let mut fixture_joined = fixture.is_none();
    if let Some(fixture) = fixture {
        match executor.block_on(fixture.shutdown()) {
            Ok(report) => {
                fixture_joined = report.error.is_none();
                if let Some(observation) = &observation
                    && let Err(error) = crate::reader_ui_demo::reconcile(observation, &report)
                {
                    bridge.error(error);
                }
                bridge.state.0.lock().unwrap().projection.fixture = Some(report);
            }
            Err(error) => bridge.error(error),
        }
    }
    let post = Runtime::process_snapshot();
    bridge.state.0.lock().unwrap().projection.post = Some(post);
    if let Some(observation) = &observation
        && cleaned.as_ref().is_some_and(Result::is_ok)
    {
        match reconstruct_readers(&roots.inputs, &observation.session, 1) {
            Ok(tree)
                if tree.children.iter().enumerate().all(|(i, f)| {
                    observation.children[i]
                        .fact
                        .as_ref()
                        .is_none_or(|live| live == f)
                }) =>
            {
                bridge.state.0.lock().unwrap().projection.reconstructed = true
            }
            _ => bridge.error("reader reconstruction mismatch"),
        }
    }
    if fixture_joined && cleaned.as_ref().is_none_or(Result::is_ok) {
        roots.cleanup()?;
        bridge.state.0.lock().unwrap().projection.retained_root = None;
    }
    bridge.state.0.lock().unwrap().projection.command = Some(final_observer.snapshot());
    execution
}
pub fn run() -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("requires TTY stdin and stdout"));
    }
    let bridge = Bridge::new();
    let owner = bridge.clone();
    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let worker = std::thread::Builder::new()
        .name("command-reader-ui".into())
        .spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| journey(&owner, true)));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => owner.error(&e.to_string()),
                Err(_) => owner.error("command reader host panic; root retained"),
            }
            owner.state.0.lock().unwrap().projection.ended = true;
        });
    let worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            std::panic::set_hook(panic_hook);
            return Err(error);
        }
    };
    let execution = ui::run_command_reader(&bridge);
    bridge.stop();
    bridge.terminal_returned();
    let joined = worker.join().is_ok();
    std::panic::set_hook(panic_hook);
    let projection = bridge.snapshot();
    let receipt =
        ui::runtime_output().and_then(|mut writer| write_report(&projection, joined, &mut writer));
    execution?;
    receipt?;
    if !joined || projection.error.is_some() {
        Err(io::Error::other(
            projection
                .error
                .unwrap_or_else(|| "executor join failed".into()),
        ))
    } else {
        Ok(())
    }
}
#[cfg(test)]
pub(crate) fn coordinate_test(mut paint: impl FnMut(&Projection)) -> Projection {
    let bridge = Bridge::new();
    let owner = bridge.clone();
    let worker = std::thread::spawn(move || journey(&owner, false));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let p = bridge.snapshot();
        paint(&p);
        if let Some(offer) = p.offer {
            bridge.painted(Some(offer));
            bridge.invoke(Some(offer));
        }
        if p.command.as_ref().is_some_and(|c| {
            c.invocations[0]
                .as_ref()
                .is_some_and(|f| f.report.is_some())
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    bridge.stop();
    bridge.terminal_returned();
    worker.join().unwrap().unwrap();
    bridge.snapshot()
}

fn write_report(p: &Projection, joined: bool, writer: &mut impl Write) -> io::Result<()> {
    writeln!(
        writer,
        "command-reader-ui: executor_joined={joined} ended={} root_retained={} reconstructed={} error={}",
        p.ended,
        p.retained_root.is_some(),
        p.reconstructed,
        p.error.is_some()
    )?;
    if let Some(command) = &p.command {
        writeln!(
            writer,
            "invocations={} reads={} settled={} before_report={} cancellation_requested={}",
            command.invocations.iter().flatten().count(),
            command.reads,
            command.settled,
            p.before_report,
            command.cancellation_requested
        )?;
        for fact in command.invocations.iter().flatten() {
            writeln!(
                writer,
                "command={} phase={:?} explicit_report={:?}",
                fact.command,
                fact.phase,
                fact.report.as_ref().map(|r| r.completion)
            )?;
        }
    }
    if let Some(readers) = &p.readers {
        writeln!(
            writer,
            "tree_end={} storage_joined={} runtime_joined={}",
            readers.tree_ended, readers.storage_joined, readers.runtime_joined
        )?;
        for (child, slot) in readers.children.iter().enumerate() {
            writeln!(
                writer,
                "child={child} id={:?} ordinal={:?} state={:?} accounting={:?}",
                slot.request_id,
                slot.acceptance_ordinal,
                slot.fact.as_ref().map(|f| &f.state),
                slot.accounting
            )?;
        }
    }
    for (label, snapshot) in [("pre_attestation", p.pre), ("post_attestation", p.post)] {
        if let Some(s) = snapshot {
            writeln!(
                writer,
                "{label}: active={} pending={} handles={} exposure={} unknown_usage={} obligations={} debt={} measured={:?}",
                s.active,
                s.pending,
                s.handles,
                s.potential + s.unresolved,
                s.unknown_usage,
                s.usage_obligations,
                s.token_debt,
                s.synthetic_measured
            )?;
        }
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "explicit owned-PTY caught-panic child"]
    fn panic_child() {
        assert!(std::env::var_os("S412_PANIC_CHILD").is_some());
        assert!(run().is_err());
    }
    #[test]
    fn caught_projection_panic_restores_before_owned_cleanup() {
        let output = std::process::Command::new("timeout")
            .args(["20", "python3", "tests/command_reader_pty.py"])
            .arg(std::env::current_exe().unwrap())
            .arg("panic")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[test]
    fn painted_offer_stop_and_duplicates_never_admit_without_current_authority() {
        let roots = crate::command_fixture::Roots::new(CATALOG, b"", b"").unwrap();
        let command =
            CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause).unwrap();
        let bridge = Bridge::new();
        assert!(!bridge.invoke(None));
        bridge.attach(&command);
        let offer = bridge.snapshot().offer;
        assert!(!bridge.invoke(offer));
        bridge.painted(Some(Offer::Compare(command.observer().session() + 1)));
        assert!(!bridge.invoke(offer));
        bridge.painted(offer);
        assert!(bridge.invoke(offer));
        assert!(!bridge.invoke(offer));
        assert!(command.snapshot().invocations[0].is_none());
        bridge.stop();
        assert!(!bridge.invoke(offer));
        assert!(command.snapshot().cancellation_requested);
        assert!(bridge.snapshot().offer.is_none());
    }
}
