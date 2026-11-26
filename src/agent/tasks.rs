//! Commands the agent runs in the background, as Claude Code's bash `run_in_background`: each
//! writes its output to a file and runs on across turns until it ends, `task_stop` or the user's
//! `/tasks stop` stops it, or Scramjet exits (`tools::end_commands`). The model is told how
//! one ended before its next request: with the next prompt, or within the turn running.
use super::tools::{Group, Output};
use super::{AgentError, AgentUi};
use serde_json::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    process::{Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// The tool that stops a task, offered with bash.
pub const STOP: &str = "task_stop";

/// What bash's `run_in_background` parameter says.
pub(super) const PARAMETER: &str = "Run the command in the background and return at once with \
its task id and the file its output goes to (read it with read). Use it for servers, watchers and \
long builds or tests you need not wait for; no `&` needed. It runs on across turns; when it ends \
you are told how, with the end of its output, before your next request. task_stop stops it.";

/// Lines of a task's output its notification ends with, and the bytes they come from at most.
const TAIL_LINES: usize = 20;
const TAIL_BYTES: u64 = 4096;
/// Lines of a task's output `/tasks` shows, and the bytes they come from at most.
const SHOWN_LINES: usize = 2000;
const SHOWN_BYTES: u64 = 256 * 1024;

/// The agent's background tasks, numbered from 1.
#[derive(Default)]
pub struct Tasks {
    list: Mutex<Vec<Entry>>,
}

struct Entry {
    id: u32,
    command: String,
    output: PathBuf,
    started: Instant,
    /// Its process group, which stopping kills while it runs.
    group: u32,
    ended: Option<(Ending, Duration)>,
    /// Whether the model knows how it ended: told at a prompt, or it stopped it itself.
    told: bool,
    stopping: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ending {
    Exited(i32),
    /// Ended by a signal, or with no code for another reason (its status as shown).
    Terminated(String),
    Stopped,
    /// Waiting for it failed.
    Lost(String),
}

impl Ending {
    fn of(status: std::io::Result<ExitStatus>, stopping: bool) -> Self {
        match status {
            _ if stopping => Self::Stopped,
            Ok(status) => match status.code() {
                Some(code) => Self::Exited(code),
                None => Self::Terminated(status.to_string()),
            },
            Err(error) => Self::Lost(error.to_string()),
        }
    }

    /// How it ended, as `/tasks` and the notification say: `exited with code 0`.
    pub fn said(&self) -> String {
        match self {
            Self::Exited(code) => format!("exited with code {code}"),
            Self::Terminated(status) => format!("was terminated ({status})"),
            Self::Stopped => "was stopped".into(),
            Self::Lost(error) => format!("could not be waited for: {error}"),
        }
    }
}

/// A task as `/tasks` lists it.
#[derive(Clone, Debug)]
pub struct Task {
    pub id: u32,
    pub command: String,
    pub output: PathBuf,
    /// How long it ran, or has run so far.
    pub ran: Duration,
    pub ended: Option<Ending>,
}

impl Task {
    /// The last lines of its output so far, as `/tasks` shows them; `None` when unreadable.
    pub fn output_tail(&self) -> Option<String> {
        tail(&self.output, SHOWN_LINES, SHOWN_BYTES)
    }
}

impl Tasks {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.list.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts `process`, the shell (or the sandbox around it) that runs `command`, as `program`
    /// names it in errors, with its output to a new file in the temporary directory.
    pub(super) fn start(
        self: &Arc<Self>,
        process: Command,
        program: &str,
        command: &str,
    ) -> Result<Output, String> {
        let mut list = self.lock();
        let id = list.last().map_or(1, |last| last.id + 1);
        // Numbered across the process, whose agents each number their tasks from 1.
        static FILES: AtomicU64 = AtomicU64::new(1);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let (file, path) = loop {
            let number = FILES.fetch_add(1, Ordering::Relaxed);
            let name = format!("scramjet-task-{}-{number}.log", std::process::id());
            let path = std::env::temp_dir().join(name);
            match options.open(&path) {
                Ok(file) => break (file, path),
                // Left by an earlier process with this id.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("cannot make {}: {e}", path.display())),
            }
        };
        let cannot = |e: std::io::Error| format!("cannot start {program}: {e}");
        let mut process = crate::os::own_session(process).map_err(cannot)?;
        process
            .stdin(Stdio::null())
            .stdout(file.try_clone().map_err(cannot)?)
            .stderr(file);
        let spawned = Group::spawn(&mut process);
        // It holds the file, which the task alone should keep open.
        drop(process);
        let (mut child, group) = spawned.map_err(|e| {
            let _ = std::fs::remove_file(&path);
            cannot(e)
        })?;
        list.push(Entry {
            id,
            command: command.to_owned(),
            output: path.clone(),
            started: Instant::now(),
            group: child.id(),
            ended: None,
            told: false,
            stopping: false,
        });
        drop(list);
        let tasks = Arc::downgrade(self);
        std::thread::spawn(move || {
            let status = child.wait();
            if let Some(tasks) = tasks.upgrade() {
                let mut list = tasks.lock();
                if let Some(entry) = list.iter_mut().find(|entry| entry.id == id) {
                    let ending = Ending::of(status, entry.stopping);
                    entry.ended = Some((ending, entry.started.elapsed()));
                }
                // Released only now, so that a stop never reaches a group no longer its own.
                drop(group);
            }
        });
        Ok(Output {
            text: format!(
                "Command running in background with task id {id}. Its output goes to {}: read \
                 it with read. You are told how it ended once it ends; {STOP} stops it.",
                path.display()
            ),
            detail: None,
            failed: false,
            images: Vec::new(),
            before: None,
        })
    }

    /// The tasks, in the order they started.
    pub fn list(&self) -> Vec<Task> {
        let list = self.lock();
        (list.iter())
            .map(|entry| Task {
                id: entry.id,
                command: entry.command.clone(),
                output: entry.output.clone(),
                ran: entry
                    .ended
                    .as_ref()
                    .map_or_else(|| entry.started.elapsed(), |(_, ran)| *ran),
                ended: entry.ended.as_ref().map(|(ending, _)| ending.clone()),
            })
            .collect()
    }

    /// How many still run.
    pub fn running(&self) -> usize {
        self.lock()
            .iter()
            .filter(|entry| entry.ended.is_none())
            .count()
    }

    /// The prompt that wakes an idle agent for the tasks that ended by themselves since it was
    /// last told, as Claude Code's: `Background task 1 exited with code 0.`; `None` when none
    /// did. One the user stopped wakes nothing, and is told at the next prompt.
    pub fn woken(&self) -> Option<String> {
        let list = self.lock();
        let said: Vec<String> = (list.iter().filter(|entry| !entry.told))
            .filter_map(|entry| match &entry.ended {
                Some((Ending::Stopped, _)) | None => None,
                Some((ending, _)) => {
                    Some(format!("Background task {} {}.", entry.id, ending.said()))
                }
            })
            .collect();
        (!said.is_empty()).then(|| said.join("\n"))
    }

    /// Waits at most `within` for task `id`'s end to be recorded, as after stopping it.
    pub fn wait_ended(&self, id: u32, within: Duration) {
        let until = Instant::now() + within;
        let ended = || (self.lock().iter()).any(|entry| entry.id == id && entry.ended.is_some());
        while !ended() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Stops task `id` with all it started; `by_model`: the model asked, so it is not told again.
    pub fn stop(&self, id: u32, by_model: bool) -> Result<String, String> {
        let mut list = self.lock();
        let Some(entry) = list.iter_mut().find(|entry| entry.id == id) else {
            return Err(format!("there is no task {id}"));
        };
        if let Some((ending, _)) = &entry.ended {
            return Err(format!("task {id} has ended: it {}", ending.said()));
        }
        entry.stopping = true;
        entry.told |= by_model;
        crate::os::kill_group(entry.group);
        Ok(format!(
            "Task {id} stopped; its output is in {}",
            entry.output.display()
        ))
    }

    /// `task_stop`'s result for its `arguments`.
    pub(super) fn stop_call(&self, arguments: &Value) -> Output {
        let id = arguments["task_id"]
            .as_u64()
            .or_else(|| arguments["task_id"].as_str()?.trim().parse().ok());
        let stopped = match id.and_then(|id| u32::try_from(id).ok()) {
            Some(id) => self.stop(id, true),
            None => Err("task_id must be the number bash gave the task".into()),
        };
        match stopped {
            Ok(text) => Output {
                text,
                detail: None,
                failed: false,
                images: Vec::new(),
                before: None,
            },
            Err(error) => Output::failure(format!("Error: {error}")),
        }
    }

    /// What the model is told of the tasks that ended since it was last told, which it is now,
    /// with a line for the user for each (`Background task 1 exited with code 0.`); `None` when
    /// none did.
    pub(super) fn notifications(&self) -> Option<(String, Vec<String>)> {
        let mut list = self.lock();
        let (mut text, mut said) = (String::new(), Vec::new());
        for entry in list.iter_mut().filter(|entry| !entry.told) {
            let Some((ending, ran)) = &entry.ended else {
                continue;
            };
            entry.told = true;
            said.push(format!("Background task {} {}.", entry.id, ending.said()));
            text += &format!(
                "<task-notification>\n<task-id>{}</task-id>\n<command>{}</command>\n\
                 <status>{} after {}s</status>\n<output-file>{}</output-file>\n",
                entry.id,
                entry.command,
                ending.said(),
                ran.as_secs(),
                entry.output.display()
            );
            match tail(&entry.output, TAIL_LINES, TAIL_BYTES) {
                Some(tail) if !tail.is_empty() => {
                    text += &format!("<output-tail>\n{tail}\n</output-tail>\n");
                }
                Some(_) => text += "<output-tail>(no output)</output-tail>\n",
                None => {}
            }
            text += "</task-notification>\n";
        }
        (!text.is_empty()).then(|| {
            let text = format!(
                "<system-reminder>Background commands ended:\n{text}</system-reminder>\n\n"
            );
            (text, said)
        })
    }
}

/// The last `lines` lines of the file at `path`, from its last `bytes` at most.
fn tail(path: &std::path::Path, lines: usize, bytes: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let from = length.saturating_sub(bytes);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut read = Vec::new();
    file.read_to_end(&mut read).ok()?;
    let text = String::from_utf8_lossy(&read);
    let text = text.trim_end();
    let mut kept: Vec<&str> = text.lines().collect();
    // A cut first line is left out, as is what is before the last lines.
    if from > 0 && !kept.is_empty() {
        kept.remove(0);
    }
    let start = kept.len().saturating_sub(lines);
    Some(kept[start..].join("\n"))
}

/// `task_stop`'s definition.
pub(super) fn definition() -> Value {
    json!({"type":"function","name":STOP,"strict":false,
     "description":"Stop a command bash runs in the background (run_in_background), with all it started, by its task id.",
     "parameters":{"type":"object","properties":{
        "task_id":{"type":"integer","description":"The task id bash gave the command"}},
      "required":["task_id"]}})
}

impl super::Agent {
    /// The commands bash runs in the background.
    pub fn tasks(&self) -> &Arc<Tasks> {
        &self.tasks
    }

    /// Puts before `message`'s text how the tasks that ended since the last prompt ended.
    pub(super) fn tell_tasks(&mut self, message: &mut Value) {
        if let Some((note, _)) = self.tasks.notifications() {
            super::plan::put_before(message, &note);
        }
    }

    /// Before a request within a turn, tells the model how the tasks that ended since it was
    /// last told ended, in a message the user never typed, and the user in a notice.
    pub(super) fn tell_tasks_within(&mut self, ui: &mut dyn AgentUi) -> Result<(), AgentError> {
        if let Some((note, said)) = self.tasks.notifications() {
            ui.notice(&said.join("\n"));
            let note = note.trim_end();
            self.add(json!({"role":"user","content":note,super::PROBE:true}))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Ending;
    use crate::agent::TYPED;
    use crate::agent::tests::{Recorder, agent, serve, stream, tool_call};
    use crate::provider_fixture as fixture;
    use serde_json::{Value, json};
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    fn sent(requests: &[Value], at: usize) -> String {
        let input = requests[at]["input"].as_array().unwrap();
        let user = input.iter().rev().find(|item| item["role"] == "user");
        user.unwrap()["content"].as_str().unwrap().to_owned()
    }

    fn result(requests: &[Value], at: usize) -> String {
        let input = requests[at]["input"].as_array().unwrap();
        let output = input
            .iter()
            .rev()
            .find(|item| item["type"] == "function_call_output");
        output.unwrap()["output"].as_str().unwrap().to_owned()
    }

    fn until(what: impl Fn() -> bool) {
        let start = Instant::now();
        while !what() {
            assert!(start.elapsed() < Duration::from_secs(20), "timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs bash")]
    fn a_background_command_returns_at_once_and_the_next_prompt_says_how_it_ended() {
        let text = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let command = "until [ -e go ]; do sleep 0.02; done; echo first; echo last; exit 3";
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                "bash",
                json!({"command": command, "run_in_background": true}),
            ),
            text("Started."),
            text("Seen."),
        ]);
        let (mut agent, scratch) = agent(endpoint, "task-ends");
        let (mut ui, cancel) = (Recorder::default(), AtomicBool::new(false));
        // The turn ends while the command waits.
        agent.turn("build it", &mut ui, &cancel).unwrap();
        assert_eq!(agent.tasks().running(), 1);
        assert!(agent.tasks().notifications().is_none());
        std::fs::write(scratch.0.join("go"), "").unwrap();
        until(|| agent.tasks().running() == 0);
        let listed = agent.tasks().list();
        assert_eq!(listed[0].ended, Some(Ending::Exited(3)));
        assert_eq!(listed[0].command, command);
        let woken = agent.tasks().woken();
        assert_eq!(
            woken.as_deref(),
            Some("Background task 1 exited with code 3.")
        );
        agent.turn("and?", &mut ui, &cancel).unwrap();
        assert!(agent.tasks().woken().is_none());
        let requests = server.join().unwrap();
        let started = result(&requests, 1);
        assert!(
            started.starts_with("Command running in background with task id 1."),
            "{started}"
        );
        assert!(started.contains(&listed[0].output.display().to_string()));
        let told = sent(&requests, 2);
        assert!(
            told.starts_with("<system-reminder>Background commands ended"),
            "{told}"
        );
        assert!(told.contains("<task-id>1</task-id>"), "{told}");
        assert!(
            told.contains("<status>exited with code 3 after 0s</status>"),
            "{told}"
        );
        assert!(
            told.contains("<output-tail>\nfirst\nlast\n</output-tail>"),
            "{told}"
        );
        assert!(told.ends_with("</system-reminder>\n\nand?"), "{told}");
        // Told once, and the prompt shows as typed.
        assert!(agent.tasks().notifications().is_none());
        let last = agent
            .conversation()
            .iter()
            .rev()
            .find(|item| item["role"] == "user");
        assert_eq!(last.unwrap()[TYPED], "and?");
        let mut shown = Recorder::default();
        agent.show_conversation(&mut shown);
        assert!(
            shown.shown.contains(&"prompt and?".to_owned()),
            "{:?}",
            shown.shown
        );
        let _ = std::fs::remove_file(&listed[0].output);
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs bash")]
    fn a_task_that_ends_within_a_turn_is_told_before_the_next_request() {
        let text = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        // It ends once the second call runs, so after the second request, however slow the
        // machine; that call waits until the test has seen its end recorded.
        let hold = "for _ in $(seq 1200); do [ -e started ] && break; sleep 0.05; done";
        let background =
            json!({"command": format!("{hold}; echo built; exit 4"), "run_in_background": true});
        let wait =
            "touch started; for _ in $(seq 1200); do [ -e recorded ] && break; sleep 0.05; done";
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "bash", background),
            tool_call("call_2", "bash", json!({"command": wait})),
            text("Seen."),
            text("Nothing more."),
        ]);
        let (mut agent, scratch) = agent(endpoint, "task-within");
        let (tasks, recorded) = (agent.tasks().clone(), scratch.0.join("recorded"));
        let watcher = std::thread::spawn(move || {
            until(|| {
                tasks
                    .list()
                    .first()
                    .is_some_and(|task| task.ended.is_some())
            });
            std::fs::write(recorded, "").unwrap();
        });
        let (mut ui, cancel) = (Recorder::default(), AtomicBool::new(false));
        agent.turn("build it", &mut ui, &cancel).unwrap();
        watcher.join().unwrap();
        // Told within the turn, so it neither wakes the agent nor goes with the next prompt.
        assert!(agent.tasks().woken().is_none());
        agent.turn("and?", &mut ui, &cancel).unwrap();
        let requests = server.join().unwrap();
        let input = requests[2]["input"].as_array().unwrap();
        let told = input.last().unwrap();
        assert_eq!(told["role"], "user", "{told}");
        let told = told["content"].as_str().unwrap();
        assert!(told.starts_with("<system-reminder>Background commands ended:"));
        assert!(told.contains("<status>exited with code 4 after "), "{told}");
        assert!(
            told.contains("<output-tail>\nbuilt\n</output-tail>"),
            "{told}"
        );
        assert!(told.ends_with("</system-reminder>"), "{told}");
        // The bash result comes before it.
        assert_eq!(input[input.len() - 2]["type"], "function_call_output");
        assert_eq!(sent(&requests, 3), "and?");
        assert!(
            (ui.shown.iter()).any(|line| line == "notice Background task 1 exited with code 4."),
            "{:?}",
            ui.shown
        );
        // Neither shown again nor recalled as a prompt.
        let mut shown = Recorder::default();
        agent.show_conversation(&mut shown);
        assert!(
            !(shown.shown.iter()).any(|line| line.contains("system-reminder")),
            "{:?}",
            shown.shown
        );
        for task in agent.tasks().list() {
            let _ = std::fs::remove_file(&task.output);
        }
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs bash")]
    fn task_stop_stops_a_task_and_a_stop_by_the_user_is_told() {
        let text = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let background = json!({"command": "sleep 30", "run_in_background": true});
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "bash", background.clone()),
            tool_call("call_2", "task_stop", json!({"task_id": 1})),
            tool_call("call_3", "task_stop", json!({"task_id": 1})),
            tool_call("call_4", "bash", background),
            text("Two."),
            text("Stopped by you."),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "task-stop");
        let (mut ui, cancel) = (Recorder::default(), AtomicBool::new(false));
        agent.turn("serve", &mut ui, &cancel).unwrap();
        until(|| agent.tasks().list()[0].ended.is_some());
        assert_eq!(agent.tasks().list()[0].ended, Some(Ending::Stopped));
        assert_eq!(agent.tasks().running(), 1);
        agent.tasks().stop(2, false).unwrap();
        assert!(agent.tasks().stop(9, false).is_err());
        until(|| agent.tasks().running() == 0);
        // Stopped by the user, so it wakes nothing, but is told.
        assert!(agent.tasks().woken().is_none());
        agent.turn("well?", &mut ui, &cancel).unwrap();
        let requests = server.join().unwrap();
        assert!(result(&requests, 2).starts_with("Task 1 stopped"));
        let again = result(&requests, 3);
        assert!(
            again.contains("task 1 has ended: it was stopped"),
            "{again}"
        );
        let told = sent(&requests, 5);
        assert!(told.contains("<task-id>2</task-id>"), "{told}");
        assert!(!told.contains("<task-id>1</task-id>"), "{told}");
        assert!(told.contains("was stopped"), "{told}");
        for task in agent.tasks().list() {
            let _ = std::fs::remove_file(&task.output);
        }
    }

    #[test]
    #[cfg_attr(windows, ignore = "needs the sandbox, which Windows lacks")]
    fn a_background_command_runs_in_the_sandbox_as_others_do() {
        use crate::agent::{Sandbox, SandboxMode};
        let text = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let command = "echo made > made.txt; touch ../outside.txt; echo ran";
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                "bash",
                json!({"command": command, "run_in_background": true}),
            ),
            text("Started."),
        ]);
        let (mut agent, scratch) = agent(endpoint, "task-sandbox");
        agent.set_sandbox(Some(Sandbox {
            mode: SandboxMode::WorkspaceWrite,
            network: false,
            writable: Vec::new(),
        }));
        let (mut ui, cancel) = (Recorder::default(), AtomicBool::new(false));
        agent.turn("make it", &mut ui, &cancel).unwrap();
        until(|| agent.tasks().running() == 0);
        server.join().unwrap();
        assert!(scratch.0.join("made.txt").exists());
        assert!(!scratch.0.join("../outside.txt").exists());
        let output = &agent.tasks().list()[0].output;
        assert_eq!(std::fs::read_to_string(output).unwrap(), "ran\n");
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn bash_offers_the_background_and_task_stop_comes_with_it() {
        let offered = crate::agent::tools::offered(&["bash".into()], &[], None, false, false);
        assert_eq!(offered[..2], ["bash", super::STOP]);
        let offered = crate::agent::tools::offered(&["read".into()], &[], None, false, false);
        assert!(!offered.contains(&super::STOP));
        let tools = crate::agent::tools::definitions(&["bash", super::STOP]);
        let bash = &tools[0]["parameters"]["properties"];
        assert_eq!(bash["run_in_background"]["type"], "boolean");
        assert_eq!(tools[1]["name"], super::STOP);
    }

    #[test]
    fn the_tail_keeps_the_last_lines_and_drops_a_line_cut_by_the_byte_limit() {
        let file = std::env::temp_dir().join(format!("scramjet-tail-{}.log", std::process::id()));
        std::fs::write(&file, "one\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(super::tail(&file, 2, 1024).unwrap(), "three\nfour");
        assert_eq!(
            super::tail(&file, 10, 1024).unwrap(),
            "one\ntwo\nthree\nfour"
        );
        assert_eq!(super::tail(&file, 10, 12).unwrap(), "three\nfour");
        std::fs::write(&file, "").unwrap();
        assert_eq!(super::tail(&file, 10, 1024).unwrap(), "");
        let _ = std::fs::remove_file(&file);
        assert_eq!(super::tail(&file, 10, 1024), None);
    }
}
