use crate::command_fixture as fixture;
use crate::ui;
use scramjet_rs::{
    CommandCompletion, CommandContinuation, CommandControl, CommandInvocation, CommandObserver,
    CommandPhase, CommandPolicy, CommandReadResult, CommandResume, CommandSession, CommandSnapshot,
};
use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Offer {
    Compare(u64),
    Resume(CommandResume),
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Marker {
    pub id: CommandInvocation,
    pub phase: CommandPhase,
    pub reads: usize,
    pub steps: u8,
}
#[derive(Clone)]
pub(crate) struct Projection {
    pub offer: Option<Offer>,
    pub policy: CommandPolicy,
    pub observation: Option<CommandSnapshot>,
    pub markers: [Option<Marker>; 2],
    pub ended: bool,
    pub error: Option<String>,
    pub retained_root: Option<PathBuf>,
}
struct State {
    projection: Projection,
    observer: Option<CommandObserver>,
    control: Option<CommandControl>,
    painted: Option<Offer>,
    intent: Option<Offer>,
    stop: bool,
    terminal_returned: bool,
}
#[derive(Clone)]
pub(crate) struct Bridge {
    state: Arc<(Mutex<State>, Condvar)>,
}
impl Bridge {
    fn new(policy: CommandPolicy) -> Self {
        Self {
            state: Arc::new((
                Mutex::new(State {
                    projection: Projection {
                        offer: None,
                        policy,
                        observation: None,
                        markers: [None, None],
                        ended: false,
                        error: None,
                        retained_root: None,
                    },
                    observer: None,
                    control: None,
                    painted: None,
                    intent: None,
                    stop: false,
                    terminal_returned: false,
                }),
                Condvar::new(),
            )),
        }
    }
    pub fn snapshot(&self) -> Projection {
        let (mut p, observer) = {
            let state = self.state.0.lock().unwrap();
            let mut projection = state.projection.clone();
            if state.stop {
                projection.offer = None;
            }
            (projection, state.observer.clone())
        };
        if let Some(observer) = observer {
            p.observation = Some(observer.snapshot());
        }
        p
    }
    pub fn painted(&self, offer: Option<Offer>) {
        let mut state = self.state.0.lock().unwrap();
        if offer.is_none() {
            state.painted = None;
        } else if !state.stop && offer == state.projection.offer {
            state.painted = offer;
        }
    }
    pub fn invoke(&self, offer: Option<Offer>) -> bool {
        let mut state = self.state.0.lock().unwrap();
        if state.stop
            || state.intent.is_some()
            || offer.is_none()
            || offer != state.painted
            || offer != state.projection.offer
        {
            return false;
        }
        state.intent = offer;
        state.projection.offer = None;
        state.painted = None;
        self.state.1.notify_one();
        true
    }
    pub fn stop(&self) {
        let control = {
            let mut state = self.state.0.lock().unwrap();
            state.stop = true;
            state.painted = None;
            self.state.1.notify_one();
            state.control.clone()
        };
        if let Some(control) = control {
            control.cancel();
        }
    }
    fn attach(&self, session: &CommandSession) {
        let observer = session.observer();
        let control = session.control();
        let stop = {
            let mut state = self.state.0.lock().unwrap();
            state.projection.offer = Some(Offer::Compare(observer.session()));
            state.observer = Some(observer);
            state.control = Some(control.clone());
            state.stop
        };
        if stop {
            control.cancel();
        }
    }
    fn wait(&self) -> bool {
        let mut state = self.state.0.lock().unwrap();
        while !state.stop && state.intent.is_none() {
            state = self.state.1.wait(state).unwrap();
        }
        !state.stop && state.intent.take().is_some()
    }
    fn resume(
        &self,
        session: &mut CommandSession,
        handle: CommandResume,
    ) -> io::Result<Option<CommandInvocation>> {
        let mut state = self.state.0.lock().unwrap();
        if state.stop {
            return Ok(None);
        }
        state.projection.offer = Some(Offer::Resume(handle));
        state.painted = None;
        while !state.stop && state.intent.is_none() {
            state = self.state.1.wait(state).unwrap();
        }
        if state.stop {
            state.intent = None;
            return Ok(None);
        }
        if state.intent.take() != Some(Offer::Resume(handle)) {
            return Err(io::Error::other("foreign companion intent"));
        }
        // Stop and admission share this non-I/O gate; reads never hold the bridge lock.
        session.resume(handle).map_err(io::Error::other)
    }
    fn terminal_returned(&self) {
        let mut state = self.state.0.lock().unwrap();
        state.terminal_returned = true;
        self.state.1.notify_one();
    }
    fn wait_terminal(&self) {
        let mut state = self.state.0.lock().unwrap();
        while !state.terminal_returned {
            state = self.state.1.wait(state).unwrap();
        }
    }
    fn error(&self, message: &str) {
        let mut state = self.state.0.lock().unwrap();
        if state.projection.error.is_none() {
            let mut end = message.len().min(512);
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            state.projection.error = Some(message[..end].into());
        }
    }
    fn mark(&self, session: &CommandSession, id: CommandInvocation) -> io::Result<()> {
        let snapshot = session.snapshot();
        let fact = snapshot.invocations[id.ordinal() as usize - 1]
            .as_ref()
            .unwrap();
        if fact.phase != CommandPhase::AwaitingCompletion {
            return Err(io::Error::other(
                "returned invocation is not awaiting completion",
            ));
        }
        let marker = Marker {
            id,
            phase: fact.phase,
            reads: snapshot.reads,
            steps: fact
                .steps
                .iter()
                .enumerate()
                .fold(0, |mask, (i, s)| mask | ((s.is_some() as u8) << i)),
        };
        self.state.0.lock().unwrap().projection.markers[id.ordinal() as usize - 1] = Some(marker);
        Ok(())
    }
}
fn report(session: &mut CommandSession, id: CommandInvocation) -> io::Result<()> {
    let snapshot = session.snapshot();
    let fact = snapshot.invocations[id.ordinal() as usize - 1]
        .as_ref()
        .unwrap();
    let texts: [Option<&str>; 2] =
        std::array::from_fn(|i| match fact.steps[i].as_ref().map(|s| &s.result) {
            Some(CommandReadResult::Read(text)) => Some(text.as_str()),
            _ => None,
        });
    let satisfied = if id.ordinal() == 1 {
        texts[0].is_some() && texts[0] == texts[1]
    } else {
        texts[0].is_some()
    };
    let (completion, summary, next) = if snapshot.cancellation_requested {
        (
            CommandCompletion::Cancelled,
            "Cancellation accepted; returned read facts retained.",
            None,
        )
    } else if satisfied {
        (
            CommandCompletion::Completed,
            "Required captured read outcome established.",
            (id.ordinal() == 1).then_some("inspect"),
        )
    } else {
        (
            CommandCompletion::Failed,
            "Required captured read outcome not established.",
            None,
        )
    };
    match session.report(id, completion, summary, next) {
        Ok(()) => Ok(()),
        Err(_)
            if completion != CommandCompletion::Cancelled
                && session.snapshot().cancellation_requested =>
        {
            session
                .report(
                    id,
                    CommandCompletion::Cancelled,
                    "Cancellation accepted; returned read facts retained.",
                    None,
                )
                .map_err(io::Error::other)
        }
        Err(e) => Err(io::Error::other(e)),
    }
}
fn execute_primary(bridge: &Bridge, session: &mut CommandSession) -> io::Result<()> {
    if !bridge.wait() {
        return Ok(());
    }
    let primary = match session.begin("compare") {
        Ok(id) => id,
        Err(_) if session.snapshot().cancellation_requested => return Ok(()),
        Err(e) => return Err(io::Error::other(e)),
    };
    session.execute(primary).map_err(io::Error::other)?;
    bridge.mark(session, primary)?;
    report(session, primary)?;
    if matches!(
        session.snapshot().continuation,
        CommandContinuation::Pending { .. }
    ) && let Some(id) = session.follow().map_err(io::Error::other)?
    {
        bridge.mark(session, id)?;
        report(session, id)?;
    }
    Ok(())
}
fn execute(bridge: &Bridge, session: &mut CommandSession) -> io::Result<()> {
    execute_primary(bridge, session)?;
    if let Some(handle) = session.paused_continuation()
        && let Some(id) = bridge.resume(session, handle)?
    {
        session.execute(id).map_err(io::Error::other)?;
        bridge.mark(session, id)?;
        report(session, id)?;
    }
    Ok(())
}
fn journey(bridge: &Bridge, catalog: &str, left: &[u8], right: &[u8]) -> io::Result<()> {
    let mut roots = fixture::Roots::new_retained(catalog, left, right, |root| {
        bridge.state.0.lock().unwrap().projection.retained_root = Some(root.to_owned());
    })?;
    let policy = bridge.snapshot().policy;
    let mut session =
        CommandSession::load(&roots.definitions, &roots.inputs, policy).map_err(io::Error::other);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let session = session
            .as_mut()
            .map_err(|e| io::Error::other(e.to_string()))?;
        session.discover("compare").map_err(io::Error::other)?;
        bridge.attach(session);
        execute(bridge, session)
    }))
    .unwrap_or_else(|_| Err(io::Error::other("executor panic; actual facts retained")));
    if let Err(error) = &result {
        bridge.error(&error.to_string());
    }
    bridge.wait_terminal();
    let session = session?;
    let final_state = session.finalize();
    bridge.state.0.lock().unwrap().projection.observation = Some(final_state);
    roots.cleanup()?;
    bridge.state.0.lock().unwrap().projection.retained_root = None;
    result
}
pub fn run(automatic: bool) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("requires TTY stdin and stdout"));
    }
    let bridge = Bridge::new(if automatic {
        CommandPolicy::Automatic
    } else {
        CommandPolicy::Pause
    });
    let owner = bridge.clone();
    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let worker = std::thread::Builder::new()
        .name("command-ui-executor".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                journey(
                    &owner,
                    fixture::AUTOMATIC_CATALOG,
                    b"same\n\t\x1b[31m\xe2\x80\xae\n",
                    b"same\n\t\x1b[31m\xe2\x80\xae\n",
                )
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => owner.error(&e.to_string()),
                Err(_) => owner.error("executor panic; root retained if cleanup unavailable"),
            }
            owner.state.0.lock().unwrap().projection.ended = true;
        });
    let worker = match worker {
        Ok(w) => w,
        Err(e) => {
            std::panic::set_hook(panic_hook);
            return Err(e);
        }
    };
    let execution = ui::run_command(&bridge);
    bridge.stop();
    bridge.terminal_returned();
    let joined = worker.join().is_ok();
    std::panic::set_hook(panic_hook);
    if !joined {
        bridge.error("executor join failed");
    }
    let p = bridge.snapshot();
    let receipt = ui::runtime_output().and_then(|mut writer| write_report(&p, joined, &mut writer));
    let mut errors = Vec::with_capacity(3);
    if let Err(error) = execution {
        errors.push(format!("terminal: {error}"));
    }
    if let Some(error) = p.error {
        errors.push(error);
    }
    if let Err(error) = receipt {
        errors.push(format!("report: {error}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(errors.join("; ")))
    }
}
fn write_report(p: &Projection, joined: bool, writer: &mut impl Write) -> io::Result<()> {
    writeln!(
        writer,
        "command-ui-demo: host={:?} executor_joined={joined} ended={} error={} root_retained={}",
        p.policy,
        p.ended,
        p.error.is_some(),
        p.retained_root.is_some()
    )?;
    if let Some(s) = &p.observation {
        writeln!(
            writer,
            "invocations={} reads={} settled={} continuation={:?}",
            s.invocations.iter().flatten().count(),
            s.reads,
            s.settled,
            s.continuation
        )?;
        for f in s.invocations.iter().flatten() {
            writeln!(
                writer,
                "session={} invocation={} phase={:?} explicit_report={:?} before_report={:?}",
                f.id.session(),
                f.id.ordinal(),
                f.phase,
                f.report.as_ref().map(|r| r.completion),
                p.markers[f.id.ordinal() as usize - 1].map(|m| m.phase)
            )?;
        }
    }
    if let Some(error) = &p.error {
        writeln!(writer, "error_ascii={}", error.escape_default())?;
    }
    if p.retained_root.is_some() {
        writeln!(writer, "disposable root retained; cleanup unavailable")?;
    }
    writer.flush()
}
pub fn report_error(error: &io::Error) {
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        if let Ok(mut writer) = ui::runtime_output() {
            let _ = writeln!(
                writer,
                "command-ui-demo: {}",
                error.to_string().escape_default()
            );
        }
    } else {
        let _ = writeln!(
            io::stderr(),
            "command-ui-demo: {}",
            error.to_string().escape_default()
        );
    }
}
#[cfg(test)]
#[path = "command_ui_demo_tests.rs"]
pub(crate) mod tests;
