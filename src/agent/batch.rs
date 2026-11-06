//! Coding tools and subagents running in threads of their own, so that the turn can pause for
//! the user's commands while they run; and the calls of one reply that run at the same time, as
//! Pi runs them: shown each in its place as it runs where the interface can
//! (`AgentUi::running_calls`), else as if they ran one after another, what a call shows waiting
//! until the calls before it have ended.
use super::tools::{self, Call, Output};
use super::{AgentUi, Answer, Children, Choice, CompactRead, ToolDetail, Usage};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

/// What a call shows, in order, and how it ended.
enum Shown {
    /// The end of its output so far, which replaces the one before it while it waits.
    Output(String),
    /// Its children as they are now, which replace those before them while it waits.
    Subagents(Children),
    Other(Show),
    Ended(Output, Usage),
}

/// Any other display call, made on the thread that shows it.
type Show = Box<dyn FnOnce(&mut dyn AgentUi) + Send>;

/// Runs a call, showing it on the interface given, until the flag given stops it.
pub(super) type Run =
    Arc<dyn Fn(&Call, &mut dyn AgentUi, &AtomicBool) -> (Output, Usage) + Send + Sync>;

/// How long the turn waits for a call's display before it looks whether to pause.
const LOOK: Duration = Duration::from_millis(50);

/// Calls of a reply running in threads of their own, which go on while the turn pauses for the
/// user's commands (`AgentUi::pause`), shown on the turn's thread as it waits for them. Dropped,
/// it stops them.
pub(super) struct Batch {
    from: mpsc::Receiver<(usize, Shown)>,
    /// Set from the turn's own flag while it waits, and when dropped.
    cancel: Arc<AtomicBool>,
    /// Each call's display shows in its place as it runs (`AgentUi::running_calls`).
    together: bool,
    ended: Vec<Option<(Output, Usage)>>,
    /// Without `together`: what each call showed that waits for the calls before it.
    queues: Vec<VecDeque<Shown>>,
    /// Without `together`: the calls shown to their end.
    shown: usize,
    /// With `together`: whose display shows now.
    of: Option<usize>,
}

/// How waiting for a `Batch` ended.
pub(super) enum Waited {
    /// The outputs and usage of its calls, in order.
    Ended(Vec<(Output, Usage)>),
    /// The interface asked for a pause; the calls run on.
    Paused,
}

impl Batch {
    /// Starts each of `calls` with `run`, those that change the same file one after another in
    /// their order (`lanes`). With `together`, the interface shows each call's display as it
    /// happens, told whose it is; otherwise once the calls before it have ended, only its latest
    /// output and children kept until then.
    pub(super) fn start(calls: Vec<Value>, root: &Path, run: Run, together: bool) -> Self {
        let parsed: Vec<_> = calls.iter().map(|item| Call::parse(item).ok()).collect();
        let lanes = lanes(&parsed, root);
        let calls = Arc::new(calls);
        let cancel = Arc::new(AtomicBool::new(false));
        let (to, from) = mpsc::channel();
        for lane in lanes {
            let (to, calls, run, cancel) = (to.clone(), calls.clone(), run.clone(), cancel.clone());
            std::thread::spawn(move || {
                for index in lane {
                    let mut relay = Relay {
                        index,
                        to: to.clone(),
                        cancel: cancel.clone(),
                    };
                    let (output, usage) = match Call::parse(&calls[index]) {
                        Ok(call) => run(&call, &mut relay, &cancel),
                        Err(error) => {
                            (Output::failure(format!("Error: {error}")), Usage::default())
                        }
                    };
                    let _ = to.send((index, Shown::Ended(output, usage)));
                }
            });
        }
        Self {
            from,
            cancel,
            together,
            ended: calls.iter().map(|_| None).collect(),
            queues: calls.iter().map(|_| VecDeque::new()).collect(),
            shown: 0,
            of: None,
        }
    }

    /// Shows the calls' display on `ui` until they have all ended, stopping them once `cancel`
    /// is set, or until `ui` asks for a pause.
    pub(super) fn wait(&mut self, ui: &mut dyn AgentUi, cancel: &AtomicBool) -> Waited {
        while self.ended.iter().any(Option::is_none) {
            if cancel.load(Ordering::Relaxed) {
                self.cancel.store(true, Ordering::Relaxed);
            } else if ui.pause() {
                // What the session shows meanwhile is none of the calls'.
                if self.of.take().is_some() {
                    ui.of_call(None);
                }
                return Waited::Paused;
            }
            match self.from.recv_timeout(LOOK) {
                Ok((index, event)) => self.show(index, event, ui),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if self.together {
            ui.of_call(None);
        }
        let ended = std::mem::take(&mut self.ended).into_iter();
        Waited::Ended(ended.map(|ended| ended.expect("every call ends")).collect())
    }

    fn show(&mut self, index: usize, event: Shown, ui: &mut dyn AgentUi) {
        if self.together {
            if self.of != Some(index) {
                ui.of_call(Some(index));
                self.of = Some(index);
            }
            match event {
                Shown::Output(output) => ui.tool_output(&output),
                Shown::Subagents(children) => ui.subagents(&children),
                Shown::Other(show) => show(ui),
                Shown::Ended(output, usage) => self.ended[index] = Some((output, usage)),
            }
            return;
        }
        let queue = &mut self.queues[index];
        match (queue.back_mut(), event) {
            (Some(Shown::Output(last)), Shown::Output(output)) => *last = output,
            (Some(Shown::Subagents(last)), Shown::Subagents(children)) => *last = children,
            (_, event) => queue.push_back(event),
        }
        while let Some(event) = self
            .queues
            .get_mut(self.shown)
            .and_then(VecDeque::pop_front)
        {
            match event {
                Shown::Output(output) => ui.tool_output(&output),
                Shown::Subagents(children) => ui.subagents(&children),
                Shown::Other(show) => show(ui),
                Shown::Ended(output, usage) => {
                    self.ended[self.shown] = Some((output, usage));
                    self.shown += 1;
                }
            }
        }
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The calls each thread runs, in order: edits and writes of one file go to the same thread,
/// as Pi's file mutation queue runs them one after another; every other call has its own.
fn lanes(calls: &[Option<Call>], root: &Path) -> Vec<Vec<usize>> {
    let mut lanes: Vec<Vec<usize>> = Vec::new();
    let mut files: HashMap<std::path::PathBuf, usize> = HashMap::new();
    for (index, call) in calls.iter().enumerate() {
        let call = call
            .as_ref()
            .filter(|call| matches!(call.name, "edit" | "write"));
        let file = call.map(|call| {
            let path = tools::resolve(root, call.arguments["path"].as_str().unwrap_or_default());
            std::fs::canonicalize(&path).unwrap_or(path)
        });
        match file.as_ref().and_then(|file| files.get(file)) {
            Some(&lane) => lanes[lane].push(index),
            None => {
                if let Some(file) = file {
                    files.insert(file, lanes.len());
                }
                lanes.push(vec![index]);
            }
        }
    }
    lanes
}

/// A running call's display, sent to the thread that shows it.
struct Relay {
    index: usize,
    to: mpsc::Sender<(usize, Shown)>,
    cancel: Arc<AtomicBool>,
}

impl Relay {
    fn send(&self, shown: Shown) {
        let _ = self.to.send((self.index, shown));
    }
    fn show(&self, show: impl FnOnce(&mut dyn AgentUi) + Send + 'static) {
        self.send(Shown::Other(Box::new(show)));
    }
    /// The user's answer to `ask`, asked when the interface shows this call's display: at once
    /// with the calls shown together, else once the calls before this one have ended; `None`
    /// when the turn was stopped by then.
    fn ask<T: Send + 'static>(
        &self,
        ask: impl FnOnce(&mut dyn AgentUi) -> T + Send + 'static,
    ) -> Option<T> {
        let (answer, answered) = mpsc::channel();
        let cancel = self.cancel.clone();
        self.show(move |ui| {
            if !cancel.load(Ordering::Relaxed) {
                let _ = answer.send(ask(ui));
            }
        });
        answered.recv().ok()
    }
}

/// What a tool shows; the rest of the interface is a turn's, which no tool shows.
impl AgentUi for Relay {
    fn tool_call(&mut self, name: &str, summary: &str, compact: Option<&CompactRead>) {
        let (name, summary, compact) = (name.to_owned(), summary.to_owned(), compact.cloned());
        self.show(move |ui| ui.tool_call(&name, &summary, compact.as_ref()));
    }
    fn tool_running(&mut self, since: Instant) {
        self.show(move |ui| ui.tool_running(since));
    }
    fn tool_output(&mut self, output: &str) {
        self.send(Shown::Output(output.to_owned()));
    }
    fn subagents(&mut self, children: &Children) {
        self.send(Shown::Subagents(children.clone()));
    }
    fn tool_result(&mut self, name: &str, output: &str, detail: Option<&ToolDetail>, failed: bool) {
        let (name, output, detail) = (name.to_owned(), output.to_owned(), detail.cloned());
        self.show(move |ui| ui.tool_result(&name, &output, detail.as_ref(), failed));
    }
    fn question(&mut self, message: &str) {
        let message = message.to_owned();
        self.show(move |ui| ui.question(&message));
    }
    fn notice(&mut self, text: &str) {
        let text = text.to_owned();
        self.show(move |ui| ui.notice(&text));
    }
    fn approve(&mut self, question: &str, _: &AtomicBool) -> Answer {
        let (question, cancel) = (question.to_owned(), self.cancel.clone());
        self.ask(move |ui| ui.approve(&question, &cancel))
            .unwrap_or(Answer::Deny)
    }
    fn choose(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
    ) -> Option<usize> {
        let (title, options) = (title.to_owned(), options.to_vec());
        self.ask(move |ui| ui.choose(&title, &options, recommended))
            .flatten()
    }
    fn waiting(&mut self, _: super::Purpose) {}
    fn progress(&mut self, _: &super::StreamProgress) {}
    fn retrying(&mut self, _: &str, _: u32, _: u32, _: std::time::Duration) {}
    fn assistant(&mut self, _: &str) {}
    fn command_started(&mut self, _: &str, _: bool) {}
    fn command_result(&mut self, _: &super::UserCommand) {}
    fn stopped(&mut self, _: Option<&str>) {}
    fn compacted(&mut self, _: &super::Compaction, _: Option<super::AutoCompaction>) {}
    fn compaction_failed(&mut self, _: &super::AgentError) {}
    fn user_prompt(&mut self, _: &str) {}
    fn compaction_summary(&mut self, _: &str) {}
    fn branch_summary(&mut self, _: &str) {}
    fn next_step(&mut self, _: &str) {}
}
