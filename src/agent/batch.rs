//! Coding tools and subagents running in threads of their own, so that the turn can pause for
//! the user's commands while they run; and the calls of one reply that run at the same time, as
//! Pi runs them, but for those that must follow others (`waits`): shown each in its place as it runs where the interface can
//! (`AgentUi::running_calls`), else as if they ran one after another, what a call shows waiting
//! until the calls before it have ended.
use super::tools::{self, Call, Output};
use super::{AgentUi, Answer, Children, Choice, CompactRead, ToolDetail, Usage};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{
    Arc, Condvar, Mutex,
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
    /// A question to the user, held back while another thread shows the calls (`pump`).
    Ask(Show),
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
    /// With `together`: the questions `pump` held back, asked when `wait` next shows the calls.
    held: VecDeque<(usize, Shown)>,
}

/// How waiting for a `Batch` ended.
pub(super) enum Waited {
    /// The outputs and usage of its calls, in order.
    Ended(Vec<(Output, Usage)>),
    /// The interface asked for a pause; the calls run on.
    Paused,
}

impl Batch {
    /// Starts each of `calls` with `run`, each in a thread of its own once the earlier calls it
    /// must follow have ended (`waits`), shown first, before it waits, when `announce`. With
    /// `together`, the interface shows each call's
    /// display as it happens, told whose it is; otherwise once the calls before it have ended,
    /// only its latest output and children kept until then.
    pub(super) fn start(
        calls: Vec<Value>,
        root: &Path,
        run: Run,
        together: bool,
        announce: bool,
    ) -> Self {
        let parsed: Vec<_> = calls.iter().map(|item| Call::parse(item).ok()).collect();
        let waits = waits(&parsed, root);
        let calls = Arc::new(calls);
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new((Mutex::new(vec![false; calls.len()]), Condvar::new()));
        let (to, from) = mpsc::channel();
        for (index, waits) in waits.into_iter().enumerate() {
            let (to, calls, run, cancel) = (to.clone(), calls.clone(), run.clone(), cancel.clone());
            let (done, root) = (done.clone(), root.to_owned());
            std::thread::spawn(move || {
                let _ended = Ended(index, done.clone());
                let mut relay = Relay {
                    index,
                    to: to.clone(),
                    cancel: cancel.clone(),
                };
                let parsed = Call::parse(&calls[index]);
                if let (true, Ok(call)) = (announce, &parsed) {
                    let id = calls[index]["call_id"].as_str().unwrap_or_default();
                    relay.tool_input(id, &call.arguments);
                    relay.tool_call(call.name, &call.summary(), call.compact(&root).as_ref());
                }
                let (ended, changed) = &*done;
                let mut ended = ended.lock().unwrap_or_else(|e| e.into_inner());
                while !waits.iter().all(|&before| ended[before]) {
                    ended = changed.wait(ended).unwrap_or_else(|e| e.into_inner());
                }
                drop(ended);
                let (output, usage) = match parsed {
                    // Nothing starts after a stop, not even what waited for the calls it stopped.
                    Ok(call) if cancel.load(Ordering::Relaxed) => {
                        relay.tool_result(call.name, super::calls::NOT_RUN, None, true);
                        (
                            Output::failure(super::calls::NOT_RUN.into()),
                            Usage::default(),
                        )
                    }
                    Ok(call) => run(&call, &mut relay, &cancel),
                    Err(error) => (Output::failure(format!("Error: {error}")), Usage::default()),
                };
                let _ = to.send((index, Shown::Ended(output, usage)));
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
            held: VecDeque::new(),
        }
    }

    /// Shows the calls' display on `ui` until they have all ended, stopping them once `cancel`
    /// is set, or until `ui` asks for a pause.
    pub(super) fn wait(&mut self, ui: &mut dyn AgentUi, cancel: &AtomicBool) -> Waited {
        while let Some((index, event)) = self.held.pop_front() {
            self.show(index, event, ui, false);
        }
        self.drain(ui, false);
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
                Ok((index, event)) => self.show(index, event, ui, false),
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

    /// Shows the calls' display on `ui`, which another thread than the turn's shows while the
    /// turn is paused, until `stop` is set or the calls have ended; their questions to the user
    /// and what follows them wait for `wait`, as do their outputs.
    pub(super) fn pump(&mut self, ui: &mut dyn AgentUi, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) && self.ended.iter().any(Option::is_none) {
            match self.from.recv_timeout(LOOK) {
                Ok((index, event)) => self.show(index, event, ui, true),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if self.of.take().is_some() {
            ui.of_call(None);
        }
    }

    /// Shows `event` of the call `index`; `hold`: its questions to the user wait.
    fn show(&mut self, index: usize, event: Shown, ui: &mut dyn AgentUi, hold: bool) {
        if self.together {
            if hold && matches!(event, Shown::Ask(_)) {
                // The call waits for the answer, so it shows nothing more meanwhile.
                self.held.push_back((index, event));
                return;
            }
            if self.of != Some(index) {
                ui.of_call(Some(index));
                self.of = Some(index);
            }
            match event {
                Shown::Output(output) => ui.tool_output(&output),
                Shown::Subagents(children) => ui.subagents(&children),
                Shown::Other(show) | Shown::Ask(show) => show(ui),
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
        self.drain(ui, hold);
    }

    /// Without `together`: shows what waits of the calls in order, up to the first that has
    /// not ended; `hold`: up to a question to the user too.
    fn drain(&mut self, ui: &mut dyn AgentUi, hold: bool) {
        while let Some(queue) = self.queues.get_mut(self.shown) {
            if hold && matches!(queue.front(), Some(Shown::Ask(_))) {
                break;
            }
            let Some(event) = queue.pop_front() else {
                break;
            };
            match event {
                Shown::Output(output) => ui.tool_output(&output),
                Shown::Subagents(children) => ui.subagents(&children),
                Shown::Other(show) | Shown::Ask(show) => show(ui),
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

/// Marks its call ended when dropped, even by a panic, so that the calls after it go on.
struct Ended(usize, Arc<(Mutex<Vec<bool>>, Condvar)>);

impl Drop for Ended {
    fn drop(&mut self) {
        let (ended, changed) = &*self.1;
        ended.lock().unwrap_or_else(|e| e.into_inner())[self.0] = true;
        changed.notify_all();
    }
}

/// The earlier calls each call waits for: those that change files (`bash`, `write`, `edit`) end
/// before the calls after them start and wait for those before them, as Claude Code and Codex
/// run calls that change things on their own, so that a command's output, or an edit, is there
/// for the next call that reads, builds or edits it. Calls that only look run beside each other,
/// as in Pi, and so do `bash` calls, and edits and writes of different files.
fn waits(calls: &[Option<Call>], root: &Path) -> Vec<Vec<usize>> {
    #[derive(PartialEq)]
    enum Kind {
        Looks,
        Bash,
        File(std::path::PathBuf),
    }
    let kinds: Vec<Kind> = (calls.iter())
        .map(|call| match call.as_ref().map(|call| (call.name, call)) {
            Some((name, _)) if tools::is_shell(name) => Kind::Bash,
            Some(("edit" | "write", call)) => {
                let path =
                    tools::resolve(root, call.arguments["path"].as_str().unwrap_or_default());
                Kind::File(std::fs::canonicalize(&path).unwrap_or(path))
            }
            Some(_) => Kind::Looks,
            // A call that cannot be read fails at once.
            None => Kind::Looks,
        })
        .collect();
    let follows = |later: &Kind, earlier: &Kind| match (later, earlier) {
        (Kind::Looks, Kind::Looks) | (Kind::Bash, Kind::Bash) => false,
        (Kind::File(later), Kind::File(earlier)) => later == earlier,
        _ => true,
    };
    (kinds.iter().enumerate())
        .map(|(index, later)| {
            (0..index)
                .filter(|&before| follows(later, &kinds[before]))
                .collect()
        })
        .collect()
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
        self.send(Shown::Ask(Box::new(move |ui| {
            if !cancel.load(Ordering::Relaxed) {
                let _ = answer.send(ask(ui));
            }
        })));
        answered.recv().ok()
    }
}

/// What a tool shows; the rest of the interface is a turn's, which no tool shows.
impl AgentUi for Relay {
    fn tool_call(&mut self, name: &str, summary: &str, compact: Option<&CompactRead>) {
        let (name, summary, compact) = (name.to_owned(), summary.to_owned(), compact.cloned());
        self.show(move |ui| ui.tool_call(&name, &summary, compact.as_ref()));
    }
    fn tool_input(&mut self, call_id: &str, arguments: &Value) {
        let (call_id, arguments) = (call_id.to_owned(), arguments.clone());
        self.show(move |ui| ui.tool_input(&call_id, &arguments));
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
        let at = Instant::now();
        self.show(move |ui| ui.tool_result_at(&name, &output, detail.as_ref(), failed, at));
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
    fn confirm(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
    ) -> Option<usize> {
        let (title, options) = (title.to_owned(), options.to_vec());
        self.ask(move |ui| ui.confirm(&title, &options, recommended))
            .flatten()
    }
    fn permit(&mut self, title: &str, options: &[Choice]) -> Option<usize> {
        let (title, options) = (title.to_owned(), options.to_vec());
        self.ask(move |ui| ui.permit(&title, &options)).flatten()
    }
    fn offers_choices(&mut self) -> bool {
        self.ask(|ui| ui.offers_choices()).unwrap_or(true)
    }
    fn approve_publication(&mut self, title: &str) -> bool {
        let title = title.to_owned();
        (self.ask(move |ui| ui.approve_publication(&title))).unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::{Recorder, Watching};
    use serde_json::json;

    #[test]
    fn calls_that_change_files_follow_the_calls_before_them_and_lead_those_after() {
        let call = |name: &str, arguments: Value| {
            let arguments = arguments.to_string();
            json!({"type":"function_call","call_id":"c","name":name,"arguments":arguments})
        };
        let items = [
            call("read", json!({"path":"a"})),
            call("grep", json!({"pattern":"x"})),
            call("bash", json!({"command":"make"})),
            call("bash", json!({"command":"make test"})),
            call("read", json!({"path":"a"})),
            call("edit", json!({"path":"a","edits":[]})),
            call("write", json!({"path":"b","content":""})),
            call("write", json!({"path":"./a","content":""})),
            json!({"type":"function_call","call_id":"c","name":"read","arguments":"{"}),
            call("subagent", json!({"agent":"x","task":"y"})),
        ];
        let calls: Vec<_> = items.iter().map(|item| Call::parse(item).ok()).collect();
        let waits = waits(&calls, Path::new("/project"));
        let expected: [&[usize]; 10] = [
            &[],
            &[],
            // Commands wait for what came before, but not for each other.
            &[0, 1],
            &[0, 1],
            &[2, 3],
            &[0, 1, 2, 3, 4],
            // Writes and edits of other files run beside each other; of one file, in order.
            &[0, 1, 2, 3, 4],
            &[0, 1, 2, 3, 4, 5],
            &[2, 3, 5, 6, 7],
            &[2, 3, 5, 6, 7],
        ];
        assert_eq!(waits, expected.map(<[usize]>::to_vec));
    }

    #[test]
    fn questions_wait_while_another_thread_shows_the_calls() {
        let call =
            |id: &str| json!({"type":"function_call","call_id":id,"name":"bash","arguments":"{}"});
        let run: Run = Arc::new(|_: &Call, ui: &mut dyn AgentUi, cancel: &AtomicBool| {
            ui.tool_output("before");
            let answer = ui.approve("go?", cancel);
            let text = format!("{answer:?}");
            ui.tool_output(&text);
            (
                Output {
                    failed: false,
                    ..Output::failure(text)
                },
                Usage::default(),
            )
        });
        for together in [true, false] {
            let mut batch = Batch::start(
                vec![call("a"), call("b")],
                Path::new("/"),
                run.clone(),
                together,
                false,
            );
            let (to, seen) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = stop.clone();
            let stopper = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                stopping.store(true, Ordering::Relaxed);
            });
            batch.pump(&mut Watching(to.clone()), &stop);
            stopper.join().unwrap();
            let pumped: Vec<_> = seen.try_iter().collect();
            // Without `together`, the second call's display waits for the first's end.
            let shown = if together { 2 } else { 1 };
            assert_eq!(pumped, vec!["output before"; shown], "together: {together}");
            let Waited::Ended(ended) = batch.wait(&mut Watching(to), &AtomicBool::new(false))
            else {
                panic!("paused");
            };
            let texts: Vec<_> = ended
                .iter()
                .map(|(output, _)| output.text.as_str())
                .collect();
            assert_eq!(texts, ["Allow", "Allow"]);
            let waited: Vec<_> = seen.try_iter().collect();
            assert!(waited.contains(&"asked go?".to_owned()), "{waited:?}");
        }
    }

    #[test]
    fn a_result_shown_after_the_calls_before_it_keeps_the_time_it_came() {
        let call = |id: &str, wait: u64| {
            let arguments = json!({"label": id, "wait": wait}).to_string();
            json!({"type":"function_call","call_id":id,"name":"bash","arguments":arguments})
        };
        let run: Run = Arc::new(|call: &Call, ui: &mut dyn AgentUi, _: &AtomicBool| {
            let wait = call.arguments["wait"].as_u64().unwrap();
            std::thread::sleep(Duration::from_millis(wait));
            ui.tool_result(
                call.arguments["label"].as_str().unwrap(),
                "done",
                None,
                false,
            );
            let output = Output {
                failed: false,
                ..Output::failure("done".into())
            };
            (output, Usage::default())
        });
        let calls = vec![call("slow", 600), call("quick", 50)];
        let mut batch = Batch::start(calls, Path::new("/"), run, false, false);
        let mut ui = Recorder::default();
        let Waited::Ended(_) = batch.wait(&mut ui, &AtomicBool::new(false)) else {
            panic!("paused");
        };
        let ended = &ui.ended;
        assert!(
            ended[0].0 == "slow" && ended[0].1 < Duration::from_millis(300),
            "{ended:?}"
        );
        assert!(
            ended[1].0 == "quick" && ended[1].1 >= Duration::from_millis(300),
            "{ended:?}"
        );
    }
}
