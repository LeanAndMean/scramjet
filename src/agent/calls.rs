//! A reply's tool calls: which run together, the approval question, the harness's own tools
//! (questions, reports, suggestions, model switches, delegations, publications), and the
//! coding tools and subagents.
use super::batch::{self, Batch, Waited};
use super::commands::Sets;
use super::providers::{ModelInfo, Providers};
use super::sandbox::Sandbox;
use super::skills::Skills;
use super::tools::{self, Approval, Call, Output};
use super::{
    Agent, AgentError, AgentUi, Answer, DIFF, FAILED, SUBAGENTS, ToolDetail, Usage, delegate,
    forge, mcp, permissions, publish, question, sandbox, status, subagent, suggest, switch,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What a call does next.
pub(super) enum Called {
    /// Its output, and whether the turn ends after it.
    Done(Output, bool),
    /// It runs in the background.
    Running(Batch),
}

/// What `not_run` answers a call with.
pub(super) const NOT_RUN: &str =
    "Error: the turn was stopped before this tool call ran; it did not run.";

/// A reply's calls as the turn runs them, kept while it pauses (`AgentUi::pause`).
pub(super) struct Calls {
    items: Vec<Value>,
    run: Run,
    again: Again,
}

/// What going back to a question asked among a reply's calls keeps of them (`Agent::navigate`).
#[derive(Default)]
pub(super) struct Again {
    /// The results the calls after the question had, added again rather than run again, by
    /// call id.
    pub kept: std::collections::HashMap<String, Value>,
    /// The summary of the branch left there, added after the calls' results.
    pub summary: Option<(String, super::compaction::FileLists)>,
}

impl Calls {
    /// The calls from `items` on to run again one after another, `ends` saying whether every
    /// call before them ended the turn.
    pub(super) fn again(items: Vec<Value>, ends: bool, again: Again) -> Self {
        let run = Run::OneByOne {
            next: 0,
            ends,
            running: None,
        };
        Self { items, run, again }
    }

    /// Whether the call `id` is among them.
    pub(super) fn has(&self, id: &str) -> bool {
        (self.items.iter()).any(|item| item["call_id"].as_str() == Some(id))
    }

    /// Those of the calls running now.
    pub(super) fn running(&mut self) -> Option<&mut Batch> {
        match &mut self.run {
            Run::Together(batch) => Some(batch),
            Run::OneByOne { running, .. } => running.as_mut(),
        }
    }
}

enum Run {
    Together(Batch),
    /// One after another: the next to start, whether every call so far ends the turn, and the
    /// one running in the background.
    OneByOne {
        next: usize,
        ends: bool,
        running: Option<Batch>,
    },
}

impl Agent {
    /// The calls of a reply that run at the same time, as Pi runs them: when there are several
    /// and each is a coding tool or `subagent` that asks no approval question. Otherwise they
    /// run one after another, as the user's questions, the command's reports and delegations and
    /// the publications (sequential in Scramjet too) need.
    pub(super) fn together<'a>(&self, calls: &'a [Value]) -> Option<Vec<Call<'a>>> {
        let parsed: Vec<Call> = calls
            .iter()
            .map(Call::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        let offered = self.offered();
        let runs = |call: &Call| {
            let tool = (call.name == subagent::NAME || tools::CODING_TOOLS.contains(&call.name))
                && offered.contains(&call.name);
            let asks = matches!(call.approval(&self.config.root), Approval::Ask(_));
            tool && !(asks && self.config.ask)
        };
        (parsed.len() > 1 && parsed.iter().all(runs)).then_some(parsed)
    }

    /// What the agent's coding tools and subagents take from it now, theirs while they run.
    fn kit(&self) -> Kit {
        Kit {
            providers: self.config.providers.clone(),
            retries: self.config.retries,
            model: self.config.model.clone(),
            info: self.info.clone(),
            effort: self.effort().map(str::to_owned),
            max_output_tokens: self.config.max_output_tokens,
            root: self.config.root.clone(),
            cache_key: self.session.id(),
            user_agents: self.agents_directory.clone(),
            sets: self.sets.clone(),
            skills: self.skills.clone(),
            agent_directory: self.agent_directory.clone(),
            approvals: self.approvals.clone(),
            rates: self.rates.clone(),
            coding_tools: match self.command_line_tools() {
                Some(tools) => tools.into_iter().map(str::to_owned).collect(),
                None => self.config.tools.clone(),
            },
            hide_credentials: self.hide_credentials,
            sandbox: self.config.sandbox.clone(),
            mcp: self.mcp.clone(),
        }
    }

    /// Runs a call as `Tools::run` does.
    fn runner(&self) -> batch::Run {
        let kit = self.kit();
        Arc::new(
            move |call: &Call, ui: &mut dyn AgentUi, cancel: &AtomicBool| {
                kit.tools().run(call, ui, cancel)
            },
        )
    }

    /// Starts running a reply's `calls`: together when they can (`together`), else one after
    /// another.
    pub(super) fn start_calls(&self, calls: Vec<Value>, ui: &mut dyn AgentUi) -> Calls {
        let run = match self.together(&calls) {
            Some(_) => {
                let together = ui.running_calls(calls.len());
                let batch = Batch::start(
                    calls.clone(),
                    &self.config.root,
                    self.runner(),
                    together,
                    true,
                );
                Run::Together(batch)
            }
            None => Run::OneByOne {
                next: 0,
                ends: true,
                running: None,
            },
        };
        Calls {
            items: calls,
            run,
            again: Again::default(),
        }
    }

    /// Runs `calls` on to their end, adding their results; returns whether the turn ends after
    /// them, as it does when every call ends it (as in Pi). When `ui` asks for a pause, the
    /// calls are kept in `self.calls` and the turn returns `AgentError::Paused`, those running
    /// going on meanwhile.
    pub(super) fn run_calls(
        &mut self,
        calls: Calls,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<bool, AgentError> {
        let Calls {
            items,
            run,
            mut again,
        } = calls;
        match run {
            Run::Together(mut batch) => {
                let ran = match batch.wait(ui, cancel) {
                    Waited::Ended(ran) => ran,
                    Waited::Paused => {
                        let run = Run::Together(batch);
                        self.calls = Some(Calls { items, run, again });
                        return Err(AgentError::Paused);
                    }
                };
                for (item, (output, usage)) in items.iter().zip(ran) {
                    self.usage.add(&usage);
                    self.add_result(item, &output, ui)?;
                }
                if cancel.load(Ordering::Relaxed) {
                    return Err(AgentError::Cancelled);
                }
                Ok(false)
            }
            Run::OneByOne {
                mut next,
                mut ends,
                mut running,
            } => {
                while let Some(item) = items.get(next) {
                    let id = item["call_id"].as_str().unwrap_or_default();
                    if let Some(output) = again.kept.remove(id) {
                        super::show_items(&[item.clone(), output.clone()], Some(self.root()), ui);
                        ends &= ended(item, &output);
                        self.replay(item, &output);
                        self.add(output)?;
                        next += 1;
                        continue;
                    }
                    let called = match running.take() {
                        Some(batch) => Called::Running(batch),
                        None => self.call(item, ui, cancel),
                    };
                    let (output, ends_turn) = match called {
                        Called::Done(output, ends_turn) => (output, ends_turn),
                        Called::Running(mut batch) => match batch.wait(ui, cancel) {
                            Waited::Ended(mut ran) => {
                                let (output, usage) = ran.pop().expect("one call");
                                self.usage.add(&usage);
                                (output, false)
                            }
                            Waited::Paused => {
                                let running = Some(batch);
                                let run = Run::OneByOne {
                                    next,
                                    ends,
                                    running,
                                };
                                self.calls = Some(Calls { items, run, again });
                                return Err(AgentError::Paused);
                            }
                        },
                    };
                    ends &= ends_turn;
                    self.add_result(item, &output, ui)?;
                    next += 1;
                    if cancel.load(Ordering::Relaxed) {
                        // Every call gets a result, as in Pi, or the next request is refused.
                        for item in &items[next..] {
                            self.not_run(item, ui)?;
                        }
                        if let Some((summary, files)) = again.summary {
                            self.add_branch_summary(&summary, &files)?;
                        }
                        return Err(AgentError::Cancelled);
                    }
                }
                if let Some((summary, files)) = again.summary {
                    self.add_branch_summary(&summary, &files)?;
                }
                Ok(ends)
            }
        }
    }

    /// Does again what the call `item`, kept with its earlier `output` rather than run again,
    /// did to the turn's state as it ran: a question's answer to the active command, and a
    /// delegation's place in the cycle check.
    fn replay(&mut self, item: &Value, output: &Value) {
        if output[super::FAILED] == true {
            return;
        }
        let Ok(call) = Call::parse(item) else { return };
        match call.name {
            question::NAME => {
                let answer: Value = (output["output"].as_str())
                    .and_then(|text| serde_json::from_str(text).ok())
                    .unwrap_or_default();
                if answer.get("parked").is_some() {
                    self.lifecycle.wait();
                    self.suggestion = suggest::Pending::Reply;
                } else if answer["cancelled"] == true {
                    self.lifecycle.wait();
                } else if answer.get("confirmed").is_some() || answer.get("selected").is_some() {
                    self.lifecycle.answered();
                }
            }
            delegate::NAME => {
                if let Ok((name, args)) = delegate::given(&call.arguments) {
                    self.delegated.push((name.to_owned(), args.to_owned()));
                }
            }
            _ => {}
        }
    }

    /// Adds `output` as the result of the call `item`, with what is shown of it on a resume.
    pub(super) fn add_result(
        &mut self,
        item: &Value,
        output: &Output,
        ui: &mut dyn AgentUi,
    ) -> Result<(), AgentError> {
        let mut result =
            json!({"type":"function_call_output","call_id":item["call_id"],"output":output.sent()});
        if output.failed {
            result[FAILED] = json!(true);
        }
        match &output.detail {
            Some(ToolDetail::Diff(diff)) => result[DIFF] = json!(diff),
            Some(ToolDetail::Subagents(children)) => {
                result[SUBAGENTS] = serde_json::to_value(children).unwrap_or_default()
            }
            _ => {}
        }
        let spent = result.get(SUBAGENTS).is_some();
        self.add(result)?;
        if spent {
            ui.usage(
                self.session_tokens(),
                self.context_tokens(),
                self.output_rate(),
            );
        }
        Ok(())
    }

    /// Answers a call of a reply whose turn stopped before it ran.
    pub(super) fn not_run(&mut self, item: &Value, ui: &mut dyn AgentUi) -> Result<(), AgentError> {
        let name = item["name"].as_str().unwrap_or_default();
        let call = Call::parse(item).ok();
        let summary = call.as_ref().map(Call::summary).unwrap_or_default();
        let compact = call.and_then(|call| call.compact(&self.config.root));
        ui.tool_call(name, &summary, compact.as_ref());
        ui.tool_result(name, NOT_RUN, None, true);
        self.add(
            json!({"type":"function_call_output","call_id":item["call_id"],
            "output":NOT_RUN,FAILED:true}),
        )
    }

    /// Runs a tool call, after any approval; also says whether the turn ends after it, as a
    /// question the user cancelled or is to answer at the prompt ends it.
    pub(super) fn call(
        &mut self,
        item: &Value,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Called {
        let failure = |text: String| Output {
            text,
            detail: None,
            failed: true,
            image: None,
        };
        let call = match Call::parse(item) {
            Ok(call) => call,
            Err(error) => {
                let output = format!("Error: {error}");
                ui.tool_result(item["name"].as_str().unwrap_or(""), &output, None, true);
                return Called::Done(failure(output), false);
            }
        };
        let compact = call.compact(&self.config.root);
        let runs = call.name == subagent::NAME
            || tools::CODING_TOOLS.contains(&call.name)
            || [mcp::SEARCH, mcp::CALL].contains(&call.name);
        // A call that runs in the background shows in its place, under what the session shows
        // while the turn pauses for the user's commands.
        let together = runs && self.offers(call.name) && ui.running_calls(1);
        if together {
            ui.of_call(Some(0));
        }
        ui.tool_call(call.name, &call.summary(), compact.as_ref());
        if !self.offers(call.name) {
            let output = tools::not_found(call.name, &self.offered());
            ui.tool_result(call.name, &output, None, true);
            return Called::Done(failure(output), false);
        }
        if call.name == question::NAME {
            let (output, ends) = ask(&call.arguments, &mut self.lifecycle, ui);
            let freetext = question::Question::parse(&call.arguments)
                .is_ok_and(|question| question.kind == question::Kind::Freetext);
            if freetext {
                self.suggestion = suggest::Pending::Reply;
            }
            return Called::Done(output, ends);
        }
        if call.name == suggest::NAME {
            let (active, shows) = (self.lifecycle.active(), ui.offers_suggestions());
            let (commands, pending) = (&self.commands, &self.suggestion);
            let (text, failed) =
                match suggest::run(&call.arguments, commands, active, shows, pending) {
                    Ok((suggested, text)) => {
                        self.suggestion = suggest::Pending::Steps(suggested);
                        (text, false)
                    }
                    Err(refused) => refused,
                };
            ui.tool_result(call.name, &text, None, failed);
            let output = Output {
                text,
                detail: None,
                failed,
                image: None,
            };
            return Called::Done(output, false);
        }
        if call.name == switch::NAME {
            let (text, failed) = match self.switch_model(&call.arguments) {
                switch::Switched::To(text) => {
                    ui.model_switched(self);
                    (text, false)
                }
                switch::Switched::Not(text, failed) => (text, failed),
            };
            ui.tool_result(call.name, &text, None, failed);
            let output = Output {
                text,
                detail: None,
                failed,
                image: None,
            };
            return Called::Done(output, false);
        }
        if call.name == permissions::NAME {
            let output = self.request_permissions(&call.arguments, ui);
            ui.tool_result(call.name, &output.text, None, output.failed);
            return Called::Done(output, false);
        }
        if call.name == delegate::NAME {
            let active = self.lifecycle.active();
            let output =
                delegate::run(&call.arguments, &self.commands, active, &mut self.delegated);
            ui.tool_result(call.name, &output.text, None, output.failed);
            return Called::Done(output, false);
        }
        if publish::TOOLS.contains(&call.name) {
            let policy = publish::Policy {
                commands: &self.commands,
                active: self.lifecycle.active(),
                autonomy: self.autonomy.as_deref(),
            };
            let mut forge = forge::System {
                root: &self.config.root,
                cancel,
            };
            let output = publish::run(call.name, &call.arguments, &policy, ui, &mut forge, cancel);
            let detail = output.detail.as_ref();
            ui.tool_result(call.name, &output.text, detail, output.failed);
            return Called::Done(output, false);
        }
        if call.name == status::NAME {
            let (text, failed, ends) = match self.lifecycle.report(&call.arguments) {
                status::Reported::Answer(text, failed, ends) => (text, failed, ends),
                status::Reported::Abandon(abandon) => {
                    ui.question(&abandon.message());
                    let choices = status::Abandon::choices();
                    let chosen = ui.choose(&abandon.title(), &choices, Some(1));
                    self.lifecycle.abandon(&abandon, chosen)
                }
            };
            ui.tool_result(call.name, &text, None, failed);
            let output = Output {
                text,
                detail: None,
                failed,
                image: None,
            };
            return Called::Done(output, ends);
        }
        let answer = match call.approval(&self.config.root) {
            Approval::Ask(question) if self.config.ask => {
                if let Some(arguments) = (call.name == mcp::CALL)
                    .then(|| mcp::unseen_arguments(&call.arguments))
                    .flatten()
                {
                    ui.question(&arguments);
                }
                ui.approve(&question, cancel)
            }
            _ => Answer::Allow,
        };
        if answer == Answer::AllowAll {
            self.config.ask = false;
        }
        if answer == Answer::Deny {
            let output = if cancel.load(Ordering::Relaxed) {
                "The user cancelled the turn before this tool call ran."
            } else {
                "The user denied this tool call."
            };
            ui.tool_result(call.name, output, None, true);
            if together {
                ui.of_call(None);
            }
            return Called::Done(failure(output.to_owned()), false);
        }
        if together {
            ui.of_call(None);
        }
        let (root, run) = (&self.config.root, self.runner());
        Called::Running(Batch::start(vec![item.clone()], root, run, together, false))
    }
}

/// What the agent's coding tools and subagents take from it, theirs while they run.
struct Kit {
    providers: Providers,
    retries: super::Retries,
    model: String,
    info: ModelInfo,
    effort: Option<String>,
    max_output_tokens: Option<u64>,
    root: PathBuf,
    cache_key: String,
    user_agents: Option<PathBuf>,
    sets: Sets,
    skills: Skills,
    agent_directory: Option<PathBuf>,
    approvals: Option<PathBuf>,
    rates: Option<PathBuf>,
    /// The coding tools of this run, which children take by default.
    coding_tools: Vec<String>,
    hide_credentials: bool,
    sandbox: Option<Sandbox>,
    mcp: Option<Arc<mcp::Servers>>,
}

impl Kit {
    fn tools(&self) -> Tools<'_> {
        Tools {
            parent: subagent::Parent {
                providers: &self.providers,
                retries: self.retries,
                model: &self.model,
                info: &self.info,
                effort: self.effort.as_deref(),
                max_output_tokens: self.max_output_tokens,
                root: &self.root,
                cache_key: self.cache_key.clone(),
                sources: subagent::Sources {
                    user: self.user_agents.as_deref(),
                    sets: &self.sets,
                },
                skills: &self.skills,
                agent_directory: self.agent_directory.as_deref(),
                approvals: self.approvals.as_deref(),
                rates: self.rates.as_deref(),
                tools: &self.coding_tools,
            },
            hide_credentials: self.hide_credentials,
            sandbox: self.sandbox.as_ref(),
            mcp: self.mcp.as_deref(),
        }
    }
}

/// What running coding tools and `subagent` calls takes from the agent.
pub(super) struct Tools<'a> {
    parent: subagent::Parent<'a>,
    hide_credentials: bool,
    sandbox: Option<&'a Sandbox>,
    mcp: Option<&'a mcp::Servers>,
}

impl Tools<'_> {
    /// Runs `call` and shows its result; with the usage of its children, if any.
    pub(super) fn run(
        &self,
        call: &Call,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> (Output, Usage) {
        let failure = |error: String| Output {
            text: format!("Error: {error}"),
            detail: None,
            failed: true,
            image: None,
        };
        let (output, usage) = match call.name {
            subagent::NAME => subagent::run(&call.arguments, &self.parent, ui, cancel),
            mcp::SEARCH | mcp::CALL => {
                ui.tool_running(std::time::Instant::now());
                let servers = self
                    .mcp
                    .ok_or_else(|| "no MCP server is configured".to_owned());
                let output = match call.name {
                    mcp::SEARCH => (servers.and_then(|mcp| mcp.search(&call.arguments, cancel)))
                        .map(|text| Output {
                            text,
                            detail: None,
                            failed: false,
                            image: None,
                        }),
                    _ => servers.and_then(|mcp| mcp.call(&call.arguments, cancel)),
                };
                (output.unwrap_or_else(failure), Usage::default())
            }
            _ => {
                ui.tool_running(std::time::Instant::now());
                let named = self.parent.providers.credential_files();
                let mut hide = self.hide_credentials || self.sandbox.is_some();
                let command = tools::is_shell(call.name) || ["grep", "find"].contains(&call.name);
                if hide && self.sandbox.is_none() && !tools::HIDES_COMMANDS && command {
                    not_hidden(ui);
                    hide = false;
                }
                let hidden = hide.then(|| sandbox::Hidden::current(named));
                let root = self.parent.root;
                let progress = &mut |output: &str| ui.tool_output(output);
                let output = match (self.sandbox, &hidden) {
                    (Some(sandbox), Some(hidden)) => {
                        call.run_in_sandbox(root, sandbox, hidden, cancel, progress)
                    }
                    _ => call.run(root, hidden.as_ref(), cancel, progress),
                };
                let mut output = output.unwrap_or_else(failure);
                if hidden.is_some() && output.failed && tools::is_shell(call.name) {
                    let command = call.arguments["command"].as_str().unwrap_or_default();
                    if needs_a_store(command) {
                        output.text += &stores_hidden(self.sandbox.is_some());
                    }
                }
                (output, Usage::default())
            }
        };
        ui.tool_result(
            call.name,
            &output.text,
            output.detail.as_ref(),
            output.failed,
        );
        (output, usage)
    }
}

/// Programs whose logins or keys live in the stores that hidden credentials leave out.
const STORE_USERS: [&str; 14] = [
    "git", "gh", "glab", "ssh", "scp", "sftp", "rsync", "gpg", "aws", "gcloud", "az", "kubectl",
    "docker", "vault",
];

/// Whether `command` runs one of `STORE_USERS`, so that its failure may come from their hiding.
fn needs_a_store(command: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || "_-./".contains(c);
    (command.split(|c: char| !word(c)))
        .map(|word| word.rsplit('/').next().unwrap_or(word))
        .any(|word| STORE_USERS.contains(&word))
}

/// What a failed command that may have needed a store is told after its output: why it failed,
/// so that the model raises a need for that access with the user, or, where the task did not
/// need it, takes the failure as a sign that it went out of scope and goes on without it.
fn stores_hidden(sandbox: bool) -> String {
    // Hiding alone shares /tmp and /run, so the ssh-agent's socket stays within reach.
    let (agent, why) = match sandbox {
        true => (", the ssh-agent", " in the sandbox (--sandbox)"),
        false => ("", ", as the hideCredentials setting asks for this turn"),
    };
    let way_out = match sandbox {
        true => "run it themselves, or start without --sandbox",
        false => {
            "run it themselves, or set hideCredentials to \"never\" (/settings, Credential stores)"
        }
    };
    format!(
        "\n\nScramjet: this turn's commands run with the credential stores hidden (~/.ssh, \
         ~/.config/gh, ~/.git-credentials, ~/.aws{agent} and others){why}. If the \
         command failed for want of a login or key (such as \"Host key verification failed\", \
         \"Permission denied (publickey)\" or \"not logged in\"), that is why: it is not a fault \
         to work around, and the login itself is likely fine. If the task needs this access, \
         stop and tell the user, who can {way_out}. If it does not, the command was out of the \
         task's scope: go on without it."
    )
}

/// Says, once a process, that commands run with nothing hidden where scramjet-rs cannot hide it.
fn not_hidden(ui: &mut dyn AgentUi) {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        ui.notice(
            "Commands run with the credential stores not hidden: Scramjet has no sandbox \
             on Windows yet (setting hideCredentials)",
        );
    }
}

/// Whether the call `item`, answered with the session's `output`, ended the turn as it ran: a
/// question cancelled or left to the prompt, or a report.
pub(super) fn ended(item: &Value, output: &Value) -> bool {
    match item["name"].as_str() {
        Some(question::NAME) => {
            let answer: Value = (output["output"].as_str())
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or_default();
            answer["cancelled"] == true || answer.get("parked").is_some()
        }
        Some(status::NAME) => true,
        _ => false,
    }
}

/// Asks the user the question of a `get_scramjet_user_input` call, as Scramjet's tool does. An
/// answer is the user's input to the active command; a freetext question or a dismissed one
/// leaves it waiting for their reply.
fn ask(
    arguments: &Value,
    lifecycle: &mut status::Lifecycle,
    ui: &mut dyn AgentUi,
) -> (Output, bool) {
    let question = match question::Question::parse(arguments) {
        Ok(question) => question,
        Err(error) => {
            ui.tool_result(question::NAME, &error, None, true);
            let output = Output {
                text: error,
                detail: None,
                failed: true,
                image: None,
            };
            return (output, false);
        }
    };
    ui.question(&question.message);
    let chosen = match question.kind {
        question::Kind::Freetext => None,
        question::Kind::Confirm => ui.choose("Confirm", &question.options, None),
        question::Kind::Select => {
            ui.choose("Choose an option", &question.options, question.recommended)
        }
    };
    let parked = match (&question.kind, chosen) {
        (question::Kind::Freetext, _) => lifecycle.wait(),
        (_, None) => {
            lifecycle.wait();
            false
        }
        (_, Some(_)) => {
            lifecycle.answered();
            false
        }
    };
    let (text, ends) = question.answer(chosen, parked);
    let detail = question::shown(arguments, &text).map(|(_, shown)| ToolDetail::Answer(shown));
    ui.tool_result(question::NAME, &text, detail.as_ref(), false);
    let output = Output {
        text,
        detail,
        failed: false,
        image: None,
    };
    (output, ends)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::*;
    use crate::agent::{
        ChildItem, ChildState, Delegation, PRODUCER, Providers, SandboxMode, Session, approvals,
        instructions, show_items,
    };
    use crate::provider_fixture as fixture;
    use std::{sync::Arc, time::Duration};

    #[test]
    fn a_command_delegates_to_another_once_per_turn_for_the_same_arguments() {
        let delegate = |id: &str, args: &str| {
            let arguments = json!({"command":"rs:read","args":args});
            tool_call(id, delegate::NAME, arguments)
        };
        let done = || stream(&fixture::text_events("done", fixture::known_usage()));
        let (endpoint, server) = serve(vec![
            delegate("call_1", "42"),
            delegate("call_2", "43"),
            delegate("call_3", "42"),
            tool_call(
                "call_4",
                status::NAME,
                json!({"summary":"Read.","status":"completed"}),
            ),
            delegate("call_5", "42"),
            done(),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "delegate");
        let commands = agent.root().join(".scramjet/rs/commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(commands.join("top.md"), "Read the issues.\n").unwrap();
        let helper = "---\ndelegate-only: true\n---\nRead issue $1.\n";
        std::fs::write(commands.join("read.md"), helper).unwrap();
        project_sets(&mut agent);
        agent.autonomy = None;
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("/rs:top", &mut ui, &cancel).unwrap();
        // The next prompt starts a turn in which the command runs again.
        agent.turn("again", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let read = |issue| {
            format!("<scramjet-command name=\"rs:read\">\nRead issue {issue}.\n</scramjet-command>")
        };
        let outputs: Vec<_> = [1, 2, 3, 5].map(|at| last_output(&bodies[at])).into();
        assert_eq!(bodies.len(), 6);
        let refused = "ERROR: cycle detected in delegation chain rs:read -> rs:read -> rs:read: \
'rs:read' was already delegated with these arguments in this turn. Refusing to recurse; its \
instructions are in that earlier result.";
        assert_eq!(outputs, [read(42), read(43), refused.into(), read(42)]);
        let shown: Vec<_> = (ui.shown.iter())
            .filter(|shown| shown.contains("delegate"))
            .map(|shown| shown.lines().next().unwrap())
            .collect();
        assert_eq!(
            shown,
            [
                "call delegate /rs:read 42",
                "result delegate <scramjet-command name=\"rs:read\">",
                "call delegate /rs:read 43",
                "result delegate <scramjet-command name=\"rs:read\">",
                "call delegate /rs:read 42",
                &format!("result delegate {refused} failed"),
                "call delegate /rs:read 42",
                "result delegate <scramjet-command name=\"rs:read\">",
            ]
        );
    }

    #[test]
    #[cfg_attr(windows, ignore = "needs the sandbox, which Windows lacks")]
    fn a_subagent_runs_its_task_read_only_and_returns_its_last_reply() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                subagent::NAME,
                json!({"agent":"reviewer","task":"Check kept.txt\nCarefully."}),
            ),
            tool_call(
                "call_2",
                "bash",
                json!({"command":"cat kept.txt; echo changed > kept.txt"}),
            ),
            tool_call(
                "call_3",
                "write",
                json!({"path":"kept.txt","content":"changed"}),
            ),
            reply("kept.txt says original"),
            reply("done"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "subagent");
        let agents = agent.root().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews\ntools: read, ls, bash, write\n---\n\n\
You review code.\n",
        )
        .unwrap();
        let kept = agent.root().join("kept.txt");
        std::fs::write(&kept, "original\n").unwrap();
        agent.agents_directory = Some(agents);
        // The child has the parent's model, and so its prices.
        agent.info.prices = prices(1_000.0, 100.0, 10_000.0);
        let rates = agent.root().join("output-rates.json");
        agent.rates = Some(rates.clone());
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("review", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 5);
        // The child's replies keep their rates with the parent's.
        let saved: Value = serde_json::from_slice(&std::fs::read(&rates).unwrap()).unwrap();
        assert_eq!(saved["samples"].as_array().unwrap().len(), 5);
        let parent = bodies[0]["instructions"].as_str().unwrap();
        assert!(parent.contains(&format!("- subagent: {}\n", subagent::SNIPPET)));
        let child = &bodies[1];
        let tools: Vec<_> = child["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| &t["name"])
            .collect();
        assert_eq!(tools, ["read", "ls", "bash"]);
        let instructions = child["instructions"].as_str().unwrap();
        assert!(instructions.starts_with("You are a subagent of Scramjet"));
        assert!(instructions.contains("\n\nYou review code.\nCurrent date: "));
        assert_eq!(
            child["input"],
            json!([{"role":"user","content":"Task: Check kept.txt\nCarefully."}])
        );
        assert_eq!(child["model"], "m");
        let key = format!("{}:reviewer", agent.session_id());
        assert_eq!(child["prompt_cache_key"], key.as_str());
        let bash = last_output(&bodies[2]);
        assert!(bash.starts_with("original\n"), "{bash}");
        assert!(bash.contains(crate::agent::tests::REFUSED_WRITE), "{bash}");
        assert_eq!(
            last_output(&bodies[3]),
            "Tool write not found. Available tools: read, ls, bash"
        );
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "original\n");
        let result = "Agent invocation warnings:\n- write: left out, as subagents run read-only \
here\n\nkept.txt says original";
        assert_eq!(last_output(&bodies[4]), result);
        assert_eq!(agent.usage.requests, 5);
        assert_eq!(ui.replies, ["done"]);
        assert!(
            ui.shown
                .contains(&"call subagent reviewer [user]\nCheck kept.txt …".to_owned())
        );
        assert!(
            ui.shown
                .contains(&format!("result subagent {result} {:?}", ui.details[0]))
        );
        // The child as it started, after each response, call and text, and as it ended.
        let started = &ui.children[0].runs[0];
        assert_eq!(
            (started.state.clone(), started.items.len()),
            (ChildState::Running, 0)
        );
        let call = |name: &str, summary: &str| ChildItem::Call {
            name: name.into(),
            summary: summary.into(),
        };
        let items = vec![
            call("bash", "cat kept.txt; echo changed > kept.txt"),
            call("write", "kept.txt"),
            ChildItem::Text("kept.txt says original".into()),
        ];
        let ToolDetail::Subagents(ended) = &ui.details[0] else {
            panic!("{:?}", ui.details);
        };
        let run = &ended.runs[0];
        assert_eq!(ended.mode, Delegation::Single);
        assert_eq!(
            (
                &run.agent[..],
                &run.source[..],
                &run.model[..],
                &run.task[..]
            ),
            ("reviewer", "user", "m", "Check kept.txt\nCarefully.")
        );
        assert_eq!((&run.state, &run.items), (&ChildState::Done, &items));
        assert_eq!(run.usage.turns, 3);
        // Each response: 7 input tokens not from the cache, 3 from it and 5 output tokens.
        let each = 7e-3 + 3e-4 + 5e-2;
        close(run.usage.cost.unwrap(), 3.0 * each);
        // And each response's usage and timing are kept.
        assert_eq!(run.responses.len(), 3);
        for response in &run.responses {
            assert_eq!(response["usage"]["output"], 5);
            assert!(response["timing"]["duration_ms"].is_u64(), "{response}");
            assert!(response["timing"]["started"].as_u64() > Some(1_700_000_000_000));
        }
        // The session's cost has the child's, its tokens only the parent's; the footer is told
        // after each response and after the child's result.
        let costs: Vec<_> = ui.usage.iter().map(|(tokens, _)| tokens.cost).collect();
        assert_eq!(costs.len(), 3);
        for (cost, expected) in costs.into_iter().zip([1.0, 4.0, 5.0]) {
            close(cost, expected * each);
        }
        assert_eq!(agent.session_tokens().input, 14);
        close(agent.usage.cost, 5.0 * each);
        assert_eq!(
            run.warnings,
            ["write: left out, as subagents run read-only here"]
        );
        assert_eq!(ui.children.last(), Some(ended));
        assert_eq!(ui.children.len(), 1 + 3 + 3 + 1);
        // Stored with the result, so that a resumed conversation shows them.
        let mut resumed = Recorder::default();
        show_items(agent.conversation(), None, &mut resumed);
        assert_eq!(resumed.details, ui.details);
    }

    #[test]
    #[cfg_attr(windows, ignore = "needs the sandbox, which Windows lacks")]
    fn a_failed_command_needing_a_store_says_the_stores_are_hidden() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let bash = |command: &str| json!({ "command": command });
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                "bash",
                bash("echo pushing; /usr/bin/git -C . nowhere 2>/dev/null; exit 128"),
            ),
            tool_call("call_2", "bash", bash("echo digit; exit 1")),
            tool_call("call_3", "bash", bash("echo gh")),
            reply("done"),
            tool_call("call_4", "bash", bash("echo gh; exit 1")),
            reply("shown"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "stores-hidden-note");
        agent.set_hide_credentials(true);
        let cancel = AtomicBool::new(false);
        let mut ui = Recorder::default();
        agent.turn("push", &mut ui, &cancel).unwrap();
        agent.set_hide_credentials(false);
        agent.turn("again", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let note = "Scramjet: this turn's commands run with the credential stores hidden";
        let noted: Vec<bool> = ([1, 2, 3, 5].map(|at| &bodies[at]).iter())
            .map(|body| last_output(body).contains(note))
            .collect();
        assert_eq!(noted, [true, false, false, false]);
        let pushed = last_output(&bodies[1]);
        assert!(
            pushed.starts_with("pushing\n\nCommand exited with code 128\n\nScramjet:")
                && pushed.contains("set hideCredentials to \"never\""),
            "{pushed}"
        );
        assert!(
            ui.shown.iter().any(|line| line.contains(note)),
            "{:?}",
            ui.shown
        );
    }

    #[test]
    #[cfg_attr(windows, ignore = "needs the sandbox, which Windows lacks")]
    fn the_stores_providers_name_are_hidden_from_the_agent_and_its_subagents() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let peek = json!({"command":"cat store/token; echo ran"});
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "bash", peek.clone()),
            tool_call(
                "call_2",
                subagent::NAME,
                json!({"agent":"reviewer","task":"Peek."}),
            ),
            tool_call("call_3", "bash", peek),
            tool_call("call_4", "read", json!({"path":"store/token"})),
            reply("peeked"),
            reply("done"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "named-stores");
        let agents = agent.root().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews\ntools: read, bash\n---\n\nYou review.\n",
        )
        .unwrap();
        agent.agents_directory = Some(agents);
        let store = agent.root().join("store");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("token"), "SECRET-named\n").unwrap();
        let mut document = served(endpoint);
        document["credentialFiles"] = json!([store]);
        agent.set_providers(Providers::given(document));
        agent.set_hide_credentials(true);
        let cancel = AtomicBool::new(false);
        agent
            .turn("peek", &mut Recorder::default(), &cancel)
            .unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 6);
        let (parent, child) = (last_output(&bodies[1]), last_output(&bodies[3]));
        for output in [&parent, &child] {
            assert!(
                output.ends_with("ran\n") && !output.contains("SECRET"),
                "{output}"
            );
        }
        let refused = format!(
            "{}: hidden, as it holds credentials",
            store.join("token").display()
        );
        assert!(
            last_output(&bodies[4]).contains(&refused),
            "{}",
            last_output(&bodies[4])
        );
    }

    #[test]
    #[cfg_attr(windows, ignore = "needs the sandbox, which Windows lacks")]
    fn the_sandbox_confines_the_agents_bash_writes_and_edits() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                "bash",
                json!({"command":"echo bash > made.txt; touch ~/.scramjet-sandbox-probe"}),
            ),
            tool_call(
                "call_2",
                "write",
                json!({"path":"written.txt","content":"w\n"}),
            ),
            tool_call(
                "call_3",
                "write",
                json!({"path":"../outside","content":"w\n"}),
            ),
            tool_call(
                "call_4",
                "edit",
                json!({"path":".git/config","edits":[{"oldText":"a","newText":"b"}]}),
            ),
            reply("done"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "sandboxed");
        let root = agent.root().to_owned();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "a\n").unwrap();
        agent.set_sandbox(Some(Sandbox {
            mode: SandboxMode::WorkspaceWrite,
            network: false,
            writable: Vec::new(),
        }));
        let cancel = AtomicBool::new(false);
        agent
            .turn("write", &mut Recorder::default(), &cancel)
            .unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 5);
        let bash = last_output(&bodies[1]);
        assert!(bash.contains(crate::agent::tests::REFUSED_WRITE), "{bash}");
        assert_eq!(
            std::fs::read_to_string(root.join("made.txt")).unwrap(),
            "bash\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("written.txt")).unwrap(),
            "w\n"
        );
        let home = std::env::home_dir().unwrap();
        assert!(!home.join(".scramjet-sandbox-probe").exists());
        let outside = last_output(&bodies[3]);
        assert!(outside.contains("outside the project"), "{outside}");
        let kept = last_output(&bodies[4]);
        assert!(kept.contains("kept read-only in the sandbox"), "{kept}");
        assert_eq!(
            std::fs::read_to_string(root.join(".git/config")).unwrap(),
            "a\n"
        );
    }

    #[test]
    #[cfg_attr(windows, ignore = "needs the sandbox, which Windows lacks")]
    fn the_agent_asks_the_user_to_write_outside_the_sandbox() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let other = std::env::temp_dir().join(format!(
            "scramjet-agent-permissions-other-{}",
            std::process::id()
        ));
        let _other = ScratchDir::new(other.clone());
        std::fs::create_dir_all(&other).unwrap();
        let other = std::fs::canonicalize(other).unwrap();
        let ask = |id: &str| {
            let arguments = json!({"write":[other.display().to_string()],"reason":"Both change."});
            tool_call(id, permissions::NAME, arguments)
        };
        let written = other.join("made.txt").display().to_string();
        let (endpoint, server) = serve(vec![
            ask("call_1"),
            ask("call_2"),
            tool_call("call_3", "write", json!({"path":written,"content":"w\n"})),
            ask("call_4"),
            reply("done"),
            ask("call_5"),
            reply("done"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "permissions");
        agent.set_sandbox(Some(Sandbox {
            mode: SandboxMode::WorkspaceWrite,
            network: false,
            writable: Vec::new(),
        }));
        let cancel = AtomicBool::new(false);
        let mut ui = Recorder {
            choices: vec![Some(2), Some(1)],
            ..Recorder::default()
        };
        agent.turn("write", &mut ui, &cancel).unwrap();
        agent.set_unattended(true);
        let granted = agent.sandbox().unwrap().writable.clone();
        agent.set_sandbox(Some(Sandbox {
            writable: Vec::new(),
            ..agent.sandbox().unwrap().clone()
        }));
        agent.turn("again", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 7);
        let offered = bodies[0]["tools"].to_string();
        assert!(offered.contains(permissions::NAME), "{offered}");
        let instructions = bodies[0]["instructions"].as_str().unwrap();
        assert!(instructions.contains("\n\n# Sandbox\n"), "{instructions}");
        assert!(last_output(&bodies[1]).contains("The user declined"));
        let allowed = last_output(&bodies[2]);
        assert!(allowed.contains("in this project from now on"), "{allowed}");
        assert_eq!(granted, std::slice::from_ref(&other));
        let remembered = super::super::Remembered::user().unwrap();
        let root = agent.root().to_owned();
        assert_eq!(remembered.of(&root).unwrap(), std::slice::from_ref(&other));
        let mut fresh = Sandbox {
            mode: SandboxMode::ReadOnly,
            network: false,
            writable: Vec::new(),
        };
        assert!(fresh.grant_remembered(&root).is_empty());
        assert_eq!(fresh.writable, std::slice::from_ref(&other));
        assert!(remembered.remove(&root, &other).unwrap());
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "w\n");
        let again = last_output(&bodies[4]);
        assert!(again.contains("already writable"), "{again}");
        let unattended = last_output(&bodies[6]);
        assert!(
            unattended.contains("Not asked: this turn runs unattended"),
            "{unattended}"
        );
        assert!(agent.sandbox().unwrap().writable.is_empty());
        let asked = (ui.shown.iter()).filter(|shown| shown.starts_with("choose Allow writing"));
        assert_eq!(asked.count(), 2);
    }

    #[test]
    #[cfg_attr(windows, ignore = "needs the sandbox, which Windows lacks")]
    fn a_subagent_reply_at_the_output_limit_returns_its_text_as_a_failure_without_its_call() {
        let reply = |blocks: Vec<Value>, reason: &str| {
            let mut events = vec![
                json!({"type":"message_start","message":{"id":"msg","usage":{"input_tokens":3,"output_tokens":1}}}),
            ];
            for (index, block) in blocks.into_iter().enumerate() {
                events.push(
                    json!({"type":"content_block_start","index":index,"content_block":block}),
                );
                events.push(json!({"type":"content_block_stop","index":index}));
            }
            events.push(json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":5}}));
            events.push(json!({"type":"message_stop"}));
            messages_stream(&events)
        };
        let call = |id: &str, name: &str, input: Value| json!({"type":"tool_use","id":id,"name":name,"input":input});
        let (endpoint, server) = serve(vec![
            reply(
                vec![call(
                    "toolu_1",
                    subagent::NAME,
                    json!({"agent":"reviewer","task":"Look"}),
                )],
                "tool_use",
            ),
            reply(
                vec![
                    json!({"type":"text","text":"Found two"}),
                    call("toolu_2", "bash", json!({"command":"touch ran"})),
                ],
                "max_tokens",
            ),
            reply(vec![json!({"type":"text","text":"done"})], "end_turn"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "subagent-limit");
        agent.set_model("claude-opus-5-5").unwrap();
        let agents = agent.root().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews\ntools: bash\n---\n\nYou review.\n",
        )
        .unwrap();
        agent.agents_directory = Some(agents);
        let mut ui = Recorder::default();
        agent
            .turn("review", &mut ui, &AtomicBool::new(false))
            .unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 3);
        assert_eq!(
            bodies[2]["messages"][2]["content"][0]["content"],
            "Agent error: response incomplete: it reached the output token limit (max_tokens); \
the tool call it was making did not run"
        );
        assert!(!agent.root().join("ran").exists());
        let ToolDetail::Subagents(ended) = &ui.details[0] else {
            panic!("{:?}", ui.details);
        };
        let run = &ended.runs[0];
        assert_eq!(run.items, [ChildItem::Text("Found two".into())]);
        assert!(
            matches!(&run.state, ChildState::Failed { reason, .. } if reason.as_deref() == Some("error")),
            "{:?}",
            run.state
        );
    }

    #[test]
    fn children_of_calls_running_together_stay_within_the_process_bound() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::sync::atomic::AtomicUsize;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap();
        let tasks: Vec<Value> = (0..4)
            .map(|n| json!({"agent":"a","task":format!("t{n}")}))
            .collect();
        let fan_out = (subagent::NAME, json!({ "tasks": tasks }));
        let first = tool_calls(&[fan_out.clone(), fan_out.clone(), fan_out]);
        let (running, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let parents = Arc::new(AtomicUsize::new(0));
        let server = {
            let (running, most) = (running.clone(), most.clone());
            std::thread::spawn(move || {
                for connection in listener.incoming().take(14) {
                    let mut reader = BufReader::new(connection.unwrap());
                    let (running, most, parents) = (running.clone(), most.clone(), parents.clone());
                    let first = first.clone();
                    std::thread::spawn(move || {
                        let mut length = 0;
                        loop {
                            let mut line = String::new();
                            reader.read_line(&mut line).unwrap();
                            let lower = line.to_lowercase();
                            if let Some(value) = lower.strip_prefix("content-length:") {
                                length = value.trim().parse().unwrap();
                            }
                            if line == "\r\n" {
                                break;
                            }
                        }
                        let mut body = vec![0; length];
                        reader.read_exact(&mut body).unwrap();
                        let body: Value = serde_json::from_slice(&body).unwrap();
                        let instructions = body["instructions"].as_str().unwrap();
                        let reply =
                            |text| stream(&fixture::text_events(text, fixture::known_usage()));
                        let response = if instructions.starts_with("You are a subagent") {
                            most.fetch_max(
                                running.fetch_add(1, Ordering::SeqCst) + 1,
                                Ordering::SeqCst,
                            );
                            std::thread::sleep(Duration::from_millis(300));
                            running.fetch_sub(1, Ordering::SeqCst);
                            reply("done")
                        } else if parents.fetch_add(1, Ordering::SeqCst) == 0 {
                            first
                        } else {
                            reply("ok")
                        };
                        reader.get_mut().write_all(&response).unwrap();
                    });
                }
            })
        };
        let (mut agent, _scratch) = agent(endpoint, "subagents-bound");
        let agents = agent.root().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let file = "---\nname: a\ndescription: Agent a\ntools: read\n---\n";
        std::fs::write(agents.join("a.md"), file).unwrap();
        agent.agents_directory = Some(agents);
        let mut ui = Recorder {
            together: true,
            ..Recorder::default()
        };
        agent
            .turn("fan out", &mut ui, &AtomicBool::new(false))
            .unwrap();
        server.join().unwrap();
        assert_eq!(agent.usage.requests, 14);
        let results = ui
            .shown
            .iter()
            .filter(|shown| shown.starts_with("result subagent Parallel: 4/4 succeeded"));
        assert_eq!(results.count(), 3, "{:?}", ui.shown);
        // Each call alone allows 4 at once; three together, 12 were in flight.
        let most = most.load(Ordering::SeqCst);
        assert!((1..=8).contains(&most), "{most} children at once");
    }

    #[test]
    fn parallel_and_chained_subagents_share_out_tasks_at_most_at_the_parents_effort() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let mut parent_requests = 0;
        let (endpoint, server) = serve_answering(8, move |body| {
            let instructions = body["instructions"].as_str().unwrap();
            if instructions.starts_with("You are a subagent") {
                let task = body["input"][0]["content"].as_str().unwrap();
                let effort = body["reasoning"]["effort"].as_str().unwrap_or("default");
                return reply(&format!("{task} at {effort}"));
            }
            parent_requests += 1;
            match parent_requests {
                1 => tool_call(
                    "call_1",
                    subagent::NAME,
                    json!({"tasks":[{"agent":"a","task":"one","effort":"high"},
                        {"agent":"nope","task":"two"},{"agent":"b","task":"three"}]}),
                ),
                3 => tool_call(
                    "call_2",
                    subagent::NAME,
                    json!({"chain":[{"agent":"a","task":"first"},
                        {"agent":"b","task":"after {previous}","effort":"minimal"}]}),
                ),
                _ => reply("ok"),
            }
        });
        let (mut agent, _scratch) = agent(endpoint, "subagents");
        let agents = agent.root().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        for name in ["a", "b"] {
            let file = format!("---\nname: {name}\ndescription: Agent {name}\ntools: read\n---\n");
            std::fs::write(agents.join(format!("{name}.md")), file).unwrap();
        }
        agent.agents_directory = Some(agents);
        agent.config.effort = Some("low".into());
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("split", &mut ui, &cancel).unwrap();
        agent.turn("chain", &mut ui, &cancel).unwrap();
        let bodies: Vec<Value> = server
            .join()
            .unwrap()
            .into_iter()
            .map(|(_, body)| body)
            .collect();
        let parents: Vec<&Value> = bodies
            .iter()
            .filter(|body| {
                body["instructions"]
                    .as_str()
                    .unwrap()
                    .starts_with("You are Scramjet")
            })
            .collect();
        assert_eq!(parents.len(), 4);
        assert_eq!(
            last_output(parents[1]),
            "Parallel: 2/3 succeeded\n\n[a] completed: Task: one at low\n\n[nope] failed: \
Unknown agent: \"nope\". Available agents: \"a\", \"b\".\n\n[b] completed: Task: three at low"
        );
        assert_eq!(
            last_output(parents[3]),
            "Task: after Task: first at low at minimal"
        );
        assert_eq!(agent.usage.requests, 8);
        let shown: Vec<_> = (ui.details.iter())
            .map(|detail| {
                let ToolDetail::Subagents(children) = detail else {
                    panic!("{detail:?}");
                };
                let runs = children.runs.iter().map(|run| {
                    let failed = matches!(run.state, ChildState::Failed { reason: None, .. });
                    (
                        &run.agent[..],
                        &run.source[..],
                        run.effort.as_deref(),
                        failed,
                    )
                });
                (children.mode, runs.collect::<Vec<_>>())
            })
            .collect();
        assert_eq!(
            shown,
            [
                (
                    Delegation::Parallel,
                    vec![
                        ("a", "user", Some("low"), false),
                        ("nope", "unknown", None, true),
                        ("b", "user", Some("low"), false)
                    ]
                ),
                (
                    Delegation::Chain,
                    vec![
                        ("a", "user", Some("low"), false),
                        ("b", "user", Some("minimal"), false)
                    ]
                )
            ]
        );
        // A chain's steps are shown as they start.
        let steps: Vec<usize> = (ui.children.iter())
            .filter(|children| children.mode == Delegation::Chain)
            .map(|children| children.runs.len())
            .collect();
        assert_eq!((steps.first(), steps.last()), (Some(&1), Some(&2)));
    }

    #[test]
    fn subagent_calls_are_checked_and_project_agents_need_the_users_approval() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                subagent::NAME,
                json!({"tasks":[{"agent":"a"}],"effort":"huge"}),
            ),
            tool_call("call_2", subagent::NAME, json!({"agent":"a","task":""})),
            tool_call(
                "call_3",
                subagent::NAME,
                json!({"agent":"p","task":"go","agentScope":"both","confirmProjectAgents":false}),
            ),
            reply("done"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "subagent-checks");
        let agents = agent.root().join("agents");
        let project = crate::os::join(agent.root(), ".scramjet/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(agents.join("a.md"), "---\nname: a\ndescription: A\n---\n").unwrap();
        std::fs::write(project.join("p.md"), "---\nname: p\ndescription: P\n---\n").unwrap();
        agent.agents_directory = Some(agents);
        let mut ui = Recorder {
            choices: vec![Some(1)],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        agent.turn("go", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let received =
            serde_json::to_string_pretty(&json!({"tasks":[{"agent":"a"}],"effort":"huge"}))
                .unwrap();
        assert_eq!(
            last_output(&bodies[1]),
            format!(
                "Validation failed for tool \"subagent\":\n  - tasks/0/task: must have required \
property\n  - effort: must be one of off, minimal, low, medium, high, xhigh, max\n\nReceived \
arguments:\n{received}"
            )
        );
        assert_eq!(
            last_output(&bodies[2]),
            "Invalid parameters. Provide exactly one mode.\nAvailable agents: a (user)"
        );
        assert_eq!(
            last_output(&bodies[3]),
            "Canceled: project-local agents not approved."
        );
        let asked = format!(
            "question Agents: p\nSource: {}\n\nProject agents are repo-controlled. Only continue \
for trusted repositories. Yes is remembered for this project until one of its agent files is added \
or changed.",
            project.display()
        );
        assert!(ui.shown.contains(&asked), "{:?}", ui.shown);
        assert!(
            ui.shown
                .contains(&"choose Run project-local agents? [\"Yes\", \"No\"] None".to_owned())
        );
        assert!(!agent.root().join("approved-agents.json").exists());
    }

    #[test]
    fn approving_project_agents_is_remembered_until_one_is_added_or_changed() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let call = |id: &str| {
            let arguments = json!({"agent":"p","task":"go","agentScope":"project"});
            tool_call(id, subagent::NAME, arguments)
        };
        let (endpoint, server) = serve(vec![
            call("call_1"),
            reply("p ran"),
            call("call_2"),
            reply("p ran again"),
            reply("one"),
            call("call_3"),
            reply("two"),
            call("call_4"),
            reply("p ran with q there"),
            reply("three"),
            call("call_5"),
            reply("four"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "subagent-approvals");
        let project = crate::os::join(agent.root(), ".scramjet/agents");
        std::fs::create_dir_all(&project).unwrap();
        let p = "---\nname: p\ndescription: P\ntools: read\n---\n";
        let q = "---\nname: q\ndescription: Q\n---\n";
        std::fs::write(project.join("p.md"), p).unwrap();
        let mut ui = Recorder {
            choices: vec![Some(0), Some(1), Some(0), None],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        agent.turn("one", &mut ui, &cancel).unwrap();
        std::fs::write(project.join("q.md"), q).unwrap();
        agent.turn("two", &mut ui, &cancel).unwrap();
        agent.turn("three", &mut ui, &cancel).unwrap();
        std::fs::write(project.join("p.md"), format!("{p}Changed.\n")).unwrap();
        agent.turn("four", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let canceled = "Canceled: project-local agents not approved.";
        let outputs: Vec<String> = [2, 4, 6, 9, 11].map(|i| last_output(&bodies[i])).into();
        assert_eq!(
            outputs,
            [
                "p ran",
                "p ran again",
                canceled,
                "p ran with q there",
                canceled
            ]
        );
        let asked = |changed: &str| {
            format!(
                "question Agents: p\nSource: {}{changed}\n\nProject agents are repo-controlled. \
Only continue for trusted repositories. Yes is remembered for this project until one of its agent \
files is added or changed.",
                project.display()
            )
        };
        let questions: Vec<&String> = (ui.shown.iter())
            .filter(|shown| shown.starts_with("question "))
            .collect();
        let added = asked("\nAdded or changed since you approved them: q");
        let changed = asked("\nAdded or changed since you approved them: p");
        assert_eq!(questions, [&asked(""), &added, &added, &changed]);
        let remembered =
            std::fs::read_to_string(agent.root().join("approved-agents.json")).unwrap();
        let remembered: Value = serde_json::from_str(&remembered).unwrap();
        let file = |name: &str| project.join(name).display().to_string();
        assert_eq!(
            remembered,
            json!({file("p.md"): approvals::digest(p), file("q.md"): approvals::digest(q)})
        );
    }

    #[test]
    fn allowing_all_stops_later_approval_questions() {
        if crate::agent::tests::missing("bash") {
            return;
        }
        let bash = |id: &str| {
            let events = fixture::call_events(r#"{"command":"true"}"#, fixture::known_usage());
            let text = serde_json::to_string(&events).unwrap();
            let text = text.replace("\"inert\"", "\"bash\"").replace("call_1", id);
            serde_json::from_str::<Vec<Value>>(&text).unwrap()
        };
        let (endpoint, server) = serve(vec![
            stream(&bash("call_1")),
            stream(&bash("call_2")),
            stream(&fixture::text_events("done", fixture::known_usage())),
            stream(&bash("call_3")),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "allow-all");
        agent.config.ask = true;
        let mut ui = Recorder {
            answers: vec![Answer::AllowAll],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        agent.turn("one", &mut ui, &cancel).unwrap();
        agent.turn("two", &mut ui, &cancel).unwrap();
        server.join().unwrap();
        assert_eq!((ui.questions, ui.calls.len()), (1, 3));
        let outputs: Vec<_> = agent
            .conversation()
            .iter()
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(outputs, ["(no output)"; 3]);
    }

    #[test]
    fn the_configured_coding_tools_are_offered_and_only_they_run() {
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "grep", json!({"pattern":"needle","path":"src"})),
            tool_call("call_2", "ls", json!({"path":"src"})),
            tool_call("call_3", "write", json!({"path":"x.txt","content":"x"})),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "configured-tools");
        agent.config.tools = ["ls", "grep", "read"].map(String::from).to_vec();
        agent.instructions = instructions::agent(
            agent.root(),
            &agent.offered(),
            &agent.commands,
            &[],
            &agent.skills,
            &Default::default(),
        );
        std::fs::create_dir(agent.root().join("src")).unwrap();
        std::fs::write(agent.root().join("src/a.txt"), "a needle\n").unwrap();
        let mut ui = Recorder::default();
        agent
            .turn("look", &mut ui, &AtomicBool::new(false))
            .unwrap();
        let bodies = server.join().unwrap();
        let names: Vec<_> = (bodies[0]["tools"].as_array().unwrap().iter())
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "read",
                "grep",
                "ls",
                status::NAME,
                "create_issue",
                "create_pr",
                "add_issue_comment",
                "add_pr_comment",
                question::NAME,
                delegate::NAME,
                subagent::NAME,
                switch::NAME,
                suggest::NAME
            ]
        );
        let instructions = bodies[0]["instructions"].as_str().unwrap();
        for line in [
            "- read: Read file contents\n- grep: Search file contents for patterns (respects \
.gitignore)\n- ls: List directory contents\n- report_scramjet_command_status:",
            "Guidelines:\n- Use read to examine files instead of cat or sed.\n- When idle in an \
interactive session, prefer `suggest_scramjet_next_steps`",
        ] {
            assert!(instructions.contains(line), "{instructions}");
        }
        let outputs: Vec<_> = agent
            .conversation()
            .iter()
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(
            outputs,
            [
                "src/a.txt:1: a needle",
                "a.txt",
                "Tool write not found. Available tools: read, grep, ls, \
report_scramjet_command_status, create_issue, create_pr, add_issue_comment, add_pr_comment, \
get_scramjet_user_input, delegate, subagent, switch_scramjet_model, suggest_scramjet_next_steps"
            ]
        );
        assert!(!agent.root().join("x.txt").exists());
    }

    #[test]
    fn the_tools_the_command_line_gives_are_the_only_ones_offered_and_run() {
        let (endpoint, server) = serve(vec![
            tool_calls(&[
                ("read", json!({"path":"a.txt"})),
                (subagent::NAME, json!({"agent":"x","task":"y"})),
                // gpt-oss's name for grep, with grep's arguments, its browser's, and neither.
                (
                    "search",
                    json!({"pattern":"a","path":"a.txt","max_results":5}),
                ),
                (
                    "search",
                    json!({"query":"x.","path":"b.txt","ignore_case":true,"max_results":5}),
                ),
                ("search", json!({"q":"a"})),
                ("open", json!({"path":"b.txt","line_start":2,"line_end":2})),
                ("read", json!({"path":"b.txt","line_start":1,"line_end":1})),
                // Its find-in-page: text to look for in files.
                ("find", json!({"query":"Xa","path":"b.txt","max_results":5})),
                ("find", json!({"pattern":"*.txt"})),
            ]),
            tool_call("call_2", "bash", json!({"command":"touch x.txt"})),
            tool_call(
                "call_3",
                question::NAME,
                json!({"type":"confirm","message":"Go?"}),
            ),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "only-tools");
        agent.config.only_tools = Some(["grep", "read", "nope"].map(String::from).to_vec());
        agent.instructions = instructions::agent(
            agent.root(),
            &agent.offered(),
            &agent.commands,
            &[],
            &agent.skills,
            &Default::default(),
        );
        std::fs::write(agent.root().join("a.txt"), "a\n").unwrap();
        std::fs::write(agent.root().join("b.txt"), "X.y\nXay\n").unwrap();
        let mut ui = Recorder::default();
        agent
            .turn("look", &mut ui, &AtomicBool::new(false))
            .unwrap();
        let bodies = server.join().unwrap();
        let names: Vec<_> = (bodies[0]["tools"].as_array().unwrap().iter())
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["read", "grep"]);
        let instructions = bodies[0]["instructions"].as_str().unwrap();
        let listed = "Available tools:\n- read: Read file contents\n- grep: Search file contents for \
patterns (respects .gitignore)\n\nGuidelines:\n- Use read to examine files instead of cat or \
sed.\n- Be concise";
        assert!(instructions.contains(listed), "{instructions}");
        let outputs: Vec<_> = (agent.conversation().iter())
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(
            outputs,
            [
                "a\n",
                "Tool subagent not found. Available tools: read, grep",
                "a.txt:1: a",
                "b.txt:1: X.y",
                "Tool search not found. Available tools: read, grep",
                "Xay\n",
                "X.y\n\n[Showing lines 1-1 of 2. Use offset=2 to continue.]",
                "b.txt:2: Xay",
                "Tool find not found. Available tools: read, grep",
                "Tool bash not found. Available tools: read, grep",
                "Tool get_scramjet_user_input not found. Available tools: read, grep"
            ]
        );
        let called: Vec<_> = (agent.conversation().iter())
            .filter(|item| item["type"] == "function_call")
            .map(|item| item["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            called,
            [
                "read",
                subagent::NAME,
                "grep",
                "grep",
                "search",
                "read",
                "read",
                "grep",
                "find",
                "bash",
                question::NAME
            ]
        );
        let mapped = (agent.conversation().iter())
            .find(|item| item["arguments"].as_str().is_some_and(|a| a.contains("x.")))
            .unwrap();
        let mapped: Value = serde_json::from_str(mapped["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(
            mapped,
            json!({"pattern":"x.","literal":true,"path":"b.txt","ignoreCase":true,"limit":5})
        );
        assert!(!agent.root().join("x.txt").exists());
        assert!(!ui.shown.iter().any(|shown| shown.contains("Go?")));
    }

    #[test]
    fn a_tool_call_with_large_arguments_runs() {
        let content = "let x = 1;\n".repeat(4_000);
        let arguments = json!({ "path": "big.rs", "content": content }).to_string();
        let events = fixture::call_events(&arguments, fixture::known_usage());
        let text = serde_json::to_string(&events)
            .unwrap()
            .replace("\"inert\"", "\"write\"");
        let (endpoint, server) = serve(vec![
            stream(&serde_json::from_str::<Vec<Value>>(&text).unwrap()),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "large-arguments");
        let mut ui = Recorder::default();
        agent
            .turn("write it", &mut ui, &AtomicBool::new(false))
            .unwrap();
        server.join().unwrap();
        assert_eq!(
            std::fs::read_to_string(agent.root().join("big.rs")).unwrap(),
            content
        );
    }

    #[test]
    fn tool_calls_run_without_questions_unless_asked() {
        if crate::agent::tests::missing("bash") {
            return;
        }
        let bash = |id: &str, command: &str| {
            let arguments = json!({ "command": command }).to_string();
            let events = fixture::call_events(&arguments, fixture::known_usage());
            let text = serde_json::to_string(&events).unwrap();
            let text = text.replace("\"inert\"", "\"bash\"").replace("call_1", id);
            serde_json::from_str::<Vec<Value>>(&text).unwrap()
        };
        let (endpoint, server) = serve(vec![
            stream(&bash("call_1", "ls -a > listing")),
            stream(&fixture::text_events("done", fixture::known_usage())),
            stream(&bash("call_2", "ls -a")),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "unasked");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("list", &mut ui, &cancel).unwrap();
        assert_eq!((ui.questions, ui.calls.len()), (0, 1));
        assert!(agent.root().join("listing").exists());
        agent.config.ask = true;
        agent.turn("list again", &mut ui, &cancel).unwrap();
        server.join().unwrap();
        assert_eq!((ui.questions, ui.calls.len()), (1, 2));
        let outputs: Vec<_> = agent
            .conversation()
            .iter()
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(outputs, ["(no output)", "The user denied this tool call."]);
    }

    #[test]
    fn calls_after_the_one_a_stop_interrupted_are_answered_as_not_run() {
        if crate::agent::tests::missing("bash") {
            return;
        }
        // With a question among them, a reply's calls run one after another.
        let question = json!({"type":"confirm","message":"Go on?"});
        let (endpoint, server) = serve(vec![
            tool_calls(&[
                ("bash", json!({"command":"sleep 30"})),
                (question::NAME, question),
            ]),
            stream(&fixture::text_events("ok", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "stopped-calls");
        let mut ui = Recorder::default();
        let cancel = stop_after(Duration::from_millis(300));
        let stopped = agent.turn("go", &mut ui, &cancel);
        assert!(matches!(stopped, Err(AgentError::Cancelled)), "{stopped:?}");
        let not_run = "Error: the turn was stopped before this tool call ran; it did not run.";
        assert_eq!(ui.calls, ["bash", question::NAME]);
        assert!(!ui.shown.iter().any(|shown| shown.starts_with("question")));
        assert_eq!(
            ui.shown.last().unwrap(),
            &format!("result {} {not_run} failed", question::NAME)
        );
        cancel.store(false, Ordering::Relaxed);
        agent.turn("again", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies[0]["parallel_tool_calls"], true);
        let outputs = outputs(&bodies[1]);
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].0, "call_0");
        assert_eq!(outputs[1], ("call_1".into(), not_run.into()));
    }

    /// A flag set after `delay`, as when the user stops the turn then.
    fn stop_after(delay: Duration) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        let setter = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            setter.store(true, Ordering::Relaxed);
        });
        cancel
    }

    /// A stop set once `path` exists, or after 10 s if it never does.
    fn stop_once_written(path: std::path::PathBuf) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        let setter = cancel.clone();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            while !path.exists() && start.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(10));
            }
            setter.store(true, Ordering::Relaxed);
        });
        cancel
    }

    #[test]
    fn a_replys_calls_run_together_and_show_one_after_another_in_their_order() {
        if crate::agent::tests::missing("bash") {
            return;
        }
        // Each command waits up to 5 s for the other's file, which one after the other the first
        // would not see.
        let meet = |mine: &str, theirs: &str, then: &str| {
            json!({"command":format!("touch {mine}; for _ in $(seq 100); do [ -e {theirs} ] && \
break; sleep 0.05; done; [ -e {theirs} ] && echo met || echo alone; {then}")})
        };
        let edit = |path: &str, old: &str, new: &str| json!({"path":path,"edits":[{"oldText":old,"newText":new}]});
        let (endpoint, server) = serve(vec![
            tool_calls(&[
                ("bash", meet("a", "b", "sleep 0.3; echo made > made.txt")),
                ("bash", meet("b", "a", "")),
                ("read", json!({"path":"made.txt"})),
                ("write", json!({"path":"f.txt","content":"one\n"})),
                ("edit", edit("f.txt", "one", "two")),
                ("edit", edit("./f.txt", "two", "three")),
                ("bash", json!({"command":"cat f.txt"})),
            ]),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "calls-together");
        let mut ui = Recorder::default();
        let started = std::time::Instant::now();
        agent.turn("go", &mut ui, &AtomicBool::new(false)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(4));
        let shown: Vec<String> = (ui.shown.iter())
            .map(|shown| shown.splitn(3, ' ').take(2).collect::<Vec<_>>().join(" "))
            .collect();
        let each = ["bash", "bash", "read", "write", "edit", "edit", "bash"]
            .map(|name| [format!("call {name}"), format!("result {name}")]);
        let mut expected: Vec<String> = each.concat();
        expected.push("reply done".into());
        assert_eq!(shown, expected);
        assert!(!ui.shown.iter().any(|shown| shown.ends_with(" failed")));
        let bodies = server.join().unwrap();
        let outputs = outputs(&bodies[1]);
        let ids: Vec<&str> = outputs.iter().map(|(id, _)| id.as_str()).collect();
        let expected: Vec<String> = (0..7).map(|index| format!("call_{index}")).collect();
        assert_eq!(ids, expected);
        // The commands ran side by side; the read waited for the command that made its file,
        // and the last command for the edits, which ran in their order.
        assert_eq!(
            (outputs[0].1.as_str(), outputs[1].1.as_str()),
            ("met\n", "met\n")
        );
        assert!(outputs[2].1.contains("made"), "{:?}", outputs[2].1);
        assert_eq!(outputs[6].1, "three\n");
    }

    #[test]
    fn a_question_of_calls_running_together_waits_for_the_calls_before_it() {
        if crate::agent::tests::missing("bash") {
            return;
        }
        let (endpoint, server) = serve(vec![
            tool_calls(&[
                ("bash", json!({"command":"sleep 0.3; echo slept"})),
                (
                    subagent::NAME,
                    json!({"agent":"p","task":"go","agentScope":"both"}),
                ),
            ]),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "calls-together-question");
        let project = crate::os::join(agent.root(), ".scramjet/agents");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("p.md"), "---\nname: p\ndescription: P\n---\n").unwrap();
        let mut ui = Recorder {
            choices: vec![Some(1)],
            ..Recorder::default()
        };
        agent.turn("go", &mut ui, &AtomicBool::new(false)).unwrap();
        let shown: Vec<&str> = (ui.shown.iter())
            .map(|shown| {
                shown
                    .split_once(['\n', ':'])
                    .map_or(shown.as_str(), |(head, _)| head)
            })
            .collect();
        assert_eq!(
            shown,
            [
                "call bash sleep 0.3; echo slept",
                "result bash slept",
                "call subagent p [both]",
                "question Agents",
                "choose Run project-local agents? [\"Yes\", \"No\"] None",
                "result subagent Canceled",
                "reply done",
            ]
        );
        let bodies = server.join().unwrap();
        assert_eq!(
            last_output(&bodies[1]),
            "Canceled: project-local agents not approved."
        );
    }

    #[test]
    #[cfg(unix)] // a named pipe
    fn calls_running_together_show_each_as_it_runs_where_the_interface_can() {
        if crate::agent::tests::missing("bash") {
            return;
        }
        let meet = |mine: &str, theirs: &str| {
            json!({"command":format!("touch {mine}; for _ in $(seq 100); do [ -e {theirs} ] && \
break; sleep 0.05; done; [ -e {theirs} ] && echo met || echo alone")})
        };
        let (endpoint, server) = serve(vec![
            tool_calls(&[("bash", meet("a", "b")), ("bash", meet("b", "a"))]),
            stream(&fixture::text_events("met", fixture::known_usage())),
            tool_calls(&[
                ("read", json!({"path":"slow.txt","limit":1})),
                (
                    subagent::NAME,
                    json!({"agent":"p","task":"go","agentScope":"both"}),
                ),
            ]),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "calls-shown-together");
        let mut ui = Recorder {
            together: true,
            choices: vec![Some(1)],
            ..Recorder::default()
        };
        agent
            .turn("meet", &mut ui, &AtomicBool::new(false))
            .unwrap();
        // Each call's display, after the `of` that says it is that call's.
        let each = |shown: &[String]| {
            let mut calls: Vec<Vec<String>> = vec![Vec::new(), Vec::new()];
            let mut of = None;
            for shown in shown {
                match shown.strip_prefix("of ") {
                    Some(index) => of = index.parse::<usize>().ok(),
                    None => calls[of.unwrap()]
                        .push(shown.split(['\n', ':', ';']).next().unwrap().into()),
                }
            }
            calls
        };
        let (first, last) = (ui.shown.first().unwrap(), ui.shown.last().unwrap());
        assert_eq!((first.as_str(), last.as_str()), ("2 calls", "reply met"));
        let calls = &ui.shown[1..ui.shown.len() - 2];
        assert_eq!(ui.shown[ui.shown.len() - 2], "of none");
        let results = calls.iter().filter(|shown| shown.starts_with("result"));
        assert_eq!(results.count(), 2);
        // They met: both had started before either ended.
        let started = calls.iter().rposition(|shown| shown.starts_with("call"));
        let ended = calls.iter().position(|shown| shown.starts_with("result"));
        assert!(started < ended, "{calls:?}");
        let meeting = ["call bash touch a", "result bash met"];
        assert_eq!(each(calls)[0], meeting);
        assert_eq!(each(calls)[1], ["call bash touch b", "result bash met"]);
        // A question among them is asked at once, while the call before it runs.
        let project = crate::os::join(agent.root(), ".scramjet/agents");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("p.md"), "---\nname: p\ndescription: P\n---\n").unwrap();
        // A read of 4 million lines takes a while.
        std::fs::write(agent.root().join("slow.txt"), "slept\n".repeat(4 << 20)).unwrap();
        ui.shown.clear();
        agent.turn("ask", &mut ui, &AtomicBool::new(false)).unwrap();
        let calls = each(&ui.shown[1..ui.shown.len() - 2]);
        assert_eq!(calls[0], ["call read slow.txt", "result read slept"]);
        assert_eq!(
            calls[1],
            [
                "call subagent p [both]",
                "question Agents",
                "choose Run project-local agents? [\"Yes\", \"No\"] None",
                "result subagent Canceled",
            ]
        );
        let asked = ui
            .shown
            .iter()
            .position(|shown| shown.starts_with("question"));
        let slept = (ui.shown.iter()).position(|shown| shown.starts_with("result read slept"));
        assert!(asked < slept, "{:?}", ui.shown);
        server.join().unwrap();
    }

    #[test]
    fn a_stop_during_calls_running_together_stops_each_with_its_own_result() {
        if crate::agent::tests::missing("bash") {
            return;
        }
        let (endpoint, server) = serve(vec![
            tool_calls(&[
                ("bash", json!({"command":"sleep 30"})),
                ("bash", json!({"command":"touch x.txt; sleep 30"})),
                ("write", json!({"path":"y.txt","content":"y"})),
            ]),
            stream(&fixture::text_events("ok", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "calls-together-stopped");
        let mut ui = Recorder::default();
        // Stopped once the second command runs beside the first.
        let cancel = stop_once_written(agent.root().join("x.txt"));
        let started = std::time::Instant::now();
        let stopped = agent.turn("go", &mut ui, &cancel);
        assert!(matches!(stopped, Err(AgentError::Cancelled)), "{stopped:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        // The write waiting for the commands did not run after the stop.
        assert!(!agent.root().join("y.txt").exists());
        cancel.store(false, Ordering::Relaxed);
        agent.turn("again", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let outputs = outputs(&bodies[1]);
        assert_eq!(outputs.len(), 3);
        assert!(outputs[0].1.starts_with("Command cancelled"), "{outputs:?}");
        assert!(outputs[1].1.starts_with("Command cancelled"), "{outputs:?}");
        assert_eq!(outputs[2].1, NOT_RUN);
    }

    #[test]
    fn questions_wait_for_the_users_choice_and_a_cancel_or_freetext_ends_the_turn() {
        let ask = |id: &str, arguments: Value| {
            let events = fixture::call_events(&arguments.to_string(), fixture::known_usage());
            let text = serde_json::to_string(&events).unwrap();
            let text = text
                .replace("\"inert\"", &format!("\"{}\"", question::NAME))
                .replace("call_1", id);
            serde_json::from_str::<Vec<Value>>(&text).unwrap()
        };
        let select = json!({"type":"select","message":"Which?","recommended":1,
            "options":[{"value":"a","label":"Alpha","description":"first"},{"value":"b","label":"Beta"}]});
        let confirm = json!({"type":"confirm","message":"Go on?"});
        let (endpoint, server) = serve(vec![
            stream(&ask("call_1", select.clone())),
            stream(&ask("call_2", json!({"type":"select","message":"Which?"}))),
            stream(&ask("call_3", confirm.clone())),
            stream(&fixture::text_events("chose", fixture::known_usage())),
            // Cancelled: no request follows within the turn.
            stream(&ask("call_4", confirm)),
            stream(&ask("call_5", json!({"type":"freetext","message":"Why?"}))),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "question");
        let mut ui = Recorder {
            choices: vec![Some(0), Some(1)],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        agent.turn("pick", &mut ui, &cancel).unwrap();
        agent.turn("again", &mut ui, &cancel).unwrap();
        agent.turn("explain", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 6);
        let tools = bodies[0]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|tool| tool["name"] == question::NAME));
        let instructions = bodies[0]["instructions"].as_str().unwrap();
        assert!(instructions.contains(&format!("- {}: {}\n", question::NAME, question::SNIPPET)));
        let outputs: Vec<_> = agent
            .conversation()
            .iter()
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(
            outputs,
            [
                r#"{"selected":"a"}"#,
                "Validation error: 'options' is required and must be a non-empty array for \
select type.",
                r#"{"confirmed":false}"#,
                r#"{"cancelled":true}"#,
                r#"{"parked":false}"#,
            ]
        );
        let first = [
            "call get_scramjet_user_input ",
            "question Which?",
            "choose Choose an option [\"Alpha\", \"Beta\"] Some(1)",
            "result get_scramjet_user_input {\"selected\":\"a\"} Answer(\"→ Alpha — first\\n  Beta\")",
        ];
        assert_eq!(ui.shown[..4], first);
        assert_eq!(
            ui.shown[5],
            "result get_scramjet_user_input Validation error: 'options' is required and must be \
a non-empty array for select type. failed"
        );
        assert_eq!(ui.shown[8], "choose Confirm [\"Yes\", \"No\"] None");
        assert_eq!(ui.replies, ["chose"]);
        // A resumed session shows each question with its answer.
        let mut replay = Recorder::default();
        agent.show_conversation(&mut replay);
        let asked: Vec<_> = replay
            .shown
            .iter()
            .filter(|shown| shown.starts_with("question") || shown.starts_with("result"))
            .map(|shown| shown.split(" {").next().unwrap())
            .collect();
        assert_eq!(
            asked,
            [
                "question Which?",
                "result get_scramjet_user_input",
                "result get_scramjet_user_input Validation error: 'options' is required and must \
be a non-empty array for select type. failed",
                "question Go on?",
                "result get_scramjet_user_input",
                "question Go on?",
                "result get_scramjet_user_input",
                "question Why?",
                "result get_scramjet_user_input",
            ]
        );
        assert!(
            replay
                .shown
                .last()
                .unwrap()
                .ends_with("Answer(\"Reply at the prompt\")")
        );
    }

    #[test]
    fn the_agent_switches_the_model_for_its_next_requests_as_scramjets_tool_does() {
        let switch = |provider: &str, model: &str| {
            (switch::NAME, json!({"provider":provider,"model":model}))
        };
        let (endpoint, server) = serve_requests(vec![
            tool_calls(&[
                (switch::NAME, json!({"model":3})),
                switch("anthropic", "gpt-x"),
                switch("openai", "m"),
                switch("openai", "gpt-x"),
            ]),
            stream(&fixture::text_events("Done.", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "switch-model");
        let base = format!("http://{endpoint}/v1");
        agent.set_providers(crate::agent::Providers::given(json!({"providers": {
            "openai": {"baseUrl": base, "api": "openai-responses", "models": [
                {"id": "m", "name": "M"},
                {"id": "gpt-x", "name": "GPT X", "contextWindow": 400_000},
            ]},
            "anthropic": {"baseUrl": base, "api": "anthropic-messages",
                "models": [{"id": "claude-y", "name": "Claude Y"}]},
        }})));
        agent.set_model("m").unwrap();
        agent.set_model_scope(vec!["openai/m".into(), "anthropic/claude-y".into()]);
        let mut ui = Recorder::default();
        (agent.turn("switch", &mut ui, &AtomicBool::new(false))).unwrap();
        let requests = server.join().unwrap();
        let (first, last) = (&requests[0].1, &requests[1].1);
        assert_eq!(
            (first["model"].as_str(), last["model"].as_str()),
            (Some("m"), Some("gpt-x"))
        );
        let identity =
            "Your model for the imminent response is: GPT X (ID: gpt-x, provider: openai).";
        assert!(last["instructions"].as_str().unwrap().contains(identity));
        let results: Vec<String> = outputs(last).into_iter().map(|(_, text)| text).collect();
        assert!(
            results[0].starts_with(
                "Validation failed for tool \"switch_scramjet_model\":\n  - provider: must have \
required property\n  - model: must be string\n\nReceived arguments:"
            ),
            "{}",
            results[0]
        );
        // An unknown model lists the user's scope; a model of another provider is unknown.
        assert_eq!(
            results[1..],
            [
                "Unknown model \"anthropic/gpt-x\". The model was not switched.\nAvailable models \
(provider/id):\n- anthropic/claude-y (Claude Y)\n- openai/m (M)",
                "Already on M (openai/m). No switch needed.",
                "Switched to GPT X (ID: gpt-x, provider: openai).",
            ]
        );
        let failed: Vec<bool> = (agent.conversation().iter())
            .filter(|item| item["type"] == "function_call_output")
            .map(|item| item[FAILED] == true)
            .collect();
        assert_eq!(failed, [true, false, false, false]);

        // The interface heard of the one switch; the reply after it is the new model's, which
        // the session keeps for a resume, with its window.
        let switched: Vec<_> = (ui.shown.iter())
            .filter(|shown| shown.starts_with("switched"))
            .collect();
        assert_eq!(switched, ["switched openai/gpt-x"]);
        let reply = agent.conversation().last().unwrap();
        assert_eq!(reply[PRODUCER], "openai/gpt-x");
        assert_eq!(
            (agent.model(), agent.context_window()),
            ("openai/gpt-x", Some(400_000))
        );
        let (session, ..) = Session::open(agent.session_path()).unwrap();
        assert_eq!(session.model(), Some("openai/gpt-x"));
    }

    #[test]
    fn only_the_sandbox_note_says_the_ssh_agent_is_hidden() {
        assert!(stores_hidden(true).contains("~/.aws, the ssh-agent and others) in the sandbox"));
        let hiding = stores_hidden(false);
        assert!(!hiding.contains("ssh-agent"), "{hiding}");
        assert!(
            hiding.contains("~/.aws and others), as the hideCredentials"),
            "{hiding}"
        );
    }
}
