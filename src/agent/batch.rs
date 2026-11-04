//! The tool calls of one reply that run at the same time, as Pi runs them: shown each in its
//! place as it runs where the interface can (`AgentUi::running_calls`), else as if they ran one
//! after another, what a call shows waiting until the calls before it have ended.
use super::tools::{self, Call, Output};
use super::{AgentUi, Answer, Children, Choice, CompactRead, ToolDetail, Usage};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Instant;

/// What a call shows, in order, and how it ended.
enum Shown<'a> {
    /// The end of its output so far, which replaces the one before it while it waits.
    Output(String),
    /// Its children as they are now, which replace those before them while it waits.
    Subagents(Children),
    Other(Show<'a>),
    Ended(Output, Usage),
}

/// Any other display call, made on the thread that shows it.
type Show<'a> = Box<dyn FnOnce(&mut dyn AgentUi) + Send + 'a>;

/// Runs each of `calls` with `run` at the same time, those that change the same file one after
/// another in their order (`lanes`), and returns their outputs and usage in order. `ui` shows
/// each call's display as it happens, told whose it is, or, when it cannot, once the calls before
/// it have ended, only its latest output and children kept until then.
pub(super) fn run(
    calls: &[Call],
    root: &Path,
    run: &(dyn Fn(&Call, &mut dyn AgentUi) -> (Output, Usage) + Sync),
    ui: &mut dyn AgentUi,
    cancel: &AtomicBool,
) -> Vec<(Output, Usage)> {
    let mut ended: Vec<Option<(Output, Usage)>> = calls.iter().map(|_| None).collect();
    let together = ui.running_calls(calls.len());
    std::thread::scope(|scope| {
        let (to, from) = mpsc::channel();
        for lane in lanes(calls, root) {
            let to = to.clone();
            scope.spawn(move || {
                for index in lane {
                    let mut relay = Relay {
                        index,
                        to: to.clone(),
                        cancel,
                    };
                    let (output, usage) = run(&calls[index], &mut relay);
                    let _ = to.send((index, Shown::Ended(output, usage)));
                }
            });
        }
        drop(to);
        let mut queues: Vec<VecDeque<Shown>> = calls.iter().map(|_| VecDeque::new()).collect();
        let mut shown = 0;
        let mut of = None;
        for (index, event) in from {
            if together {
                if of != Some(index) {
                    ui.of_call(Some(index));
                    of = Some(index);
                }
                match event {
                    Shown::Output(output) => ui.tool_output(&output),
                    Shown::Subagents(children) => ui.subagents(&children),
                    Shown::Other(show) => show(ui),
                    Shown::Ended(output, usage) => ended[index] = Some((output, usage)),
                }
                continue;
            }
            let queue = &mut queues[index];
            match (queue.back_mut(), event) {
                (Some(Shown::Output(last)), Shown::Output(output)) => *last = output,
                (Some(Shown::Subagents(last)), Shown::Subagents(children)) => *last = children,
                (_, event) => queue.push_back(event),
            }
            while let Some(event) = queues.get_mut(shown).and_then(VecDeque::pop_front) {
                match event {
                    Shown::Output(output) => ui.tool_output(&output),
                    Shown::Subagents(children) => ui.subagents(&children),
                    Shown::Other(show) => show(ui),
                    Shown::Ended(output, usage) => {
                        ended[shown] = Some((output, usage));
                        shown += 1;
                    }
                }
            }
        }
        if together {
            ui.of_call(None);
        }
    });
    let ended = ended
        .into_iter()
        .map(|ended| ended.expect("every call ends"));
    ended.collect()
}

/// The calls each thread runs, in order: edits and writes of one file go to the same thread,
/// as Pi's file mutation queue runs them one after another; every other call has its own.
fn lanes(calls: &[Call], root: &Path) -> Vec<Vec<usize>> {
    let mut lanes: Vec<Vec<usize>> = Vec::new();
    let mut files: HashMap<std::path::PathBuf, usize> = HashMap::new();
    for (index, call) in calls.iter().enumerate() {
        let file = matches!(call.name, "edit" | "write").then(|| {
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
struct Relay<'a> {
    index: usize,
    to: mpsc::Sender<(usize, Shown<'a>)>,
    cancel: &'a AtomicBool,
}

impl<'a> Relay<'a> {
    fn send(&self, shown: Shown<'a>) {
        let _ = self.to.send((self.index, shown));
    }
    fn show(&self, show: impl FnOnce(&mut dyn AgentUi) + Send + 'a) {
        self.send(Shown::Other(Box::new(show)));
    }
    /// The user's answer to `ask`, asked when the interface shows this call's display: at once
    /// with the calls shown together, else once the calls before this one have ended; `None`
    /// when the turn was stopped by then.
    fn ask<T: Send + 'a>(&self, ask: impl FnOnce(&mut dyn AgentUi) -> T + Send + 'a) -> Option<T> {
        let (answer, answered) = mpsc::channel();
        let cancel = self.cancel;
        self.show(move |ui| {
            if !cancel.load(Ordering::Relaxed) {
                let _ = answer.send(ask(ui));
            }
        });
        answered.recv().ok()
    }
}

/// What a tool shows; the rest of the interface is a turn's, which no tool shows.
impl AgentUi for Relay<'_> {
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
        let (question, cancel) = (question.to_owned(), self.cancel);
        self.ask(move |ui| ui.approve(&question, cancel))
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
