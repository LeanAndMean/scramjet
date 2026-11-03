//! Scramjet's publication tools (`forge-publication.ts`): `create_issue`, `create_pr`,
//! `add_issue_comment` and `add_pr_comment` write to the project's GitHub repository, or one a
//! URL names. The user approves the exact payload first, unless the running command declares the
//! tool and its policy approves it for the project's own repository. A publication is reported
//! done only once read back, and one that may have happened is reported so, never retried.

use super::{
    AgentUi, Choice,
    autonomy::{self, Publication},
    commands::Commands,
    forge::{self, Exec, Posted, Repository, Request},
    tools::Output,
};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub const TOOLS: [&str; 4] = [
    "create_issue",
    "create_pr",
    "add_issue_comment",
    "add_pr_comment",
];

/// How the instructions describe each tool, after its name.
pub(super) fn snippet(name: &str) -> String {
    format!(
        "{name}: explain the decision context and consequences concisely, then supply the \
complete final content only in this tool call. Do not repeat the full payload in prose. \
Publication follows user policy and always requires exact verification; never retry an \
ambiguous result automatically."
    )
}

pub(super) fn definitions() -> Vec<Value> {
    let rules = "A canonical HTTPS repository URL works without a local origin; omission \
requires origin. Different or unknown current identity requires interactive approval; matching \
origin retains current-repository policy.";
    let string = json!({"type":"string"});
    let line = json!({"type":"string","minLength":1});
    let number = json!({"type":"integer","minimum":1});
    let tool = |name: &str, what: &str, properties: Value, required: &[&str]| {
        json!({"type":"function","name":name,"strict":false,
            "description":format!("{what} in a GitHub repository. {rules}"),
            "parameters":{"type":"object","properties":properties,"required":required}})
    };
    vec![
        tool(
            "create_issue",
            "Create an issue",
            json!({"title":line,"body":string,"repository":string}),
            &["title", "body"],
        ),
        tool(
            "create_pr",
            "Create a pull request from branches",
            json!({"title":line,"body":string,"head":line,"base":line,
                "draft":{"type":"boolean"},"repository":string}),
            &["title", "body", "head", "base", "draft"],
        ),
        tool(
            "add_issue_comment",
            "Add a comment to an issue",
            json!({"number":number,"body":string,"repository":string}),
            &["number", "body"],
        ),
        tool(
            "add_pr_comment",
            "Add a comment to a pull request",
            json!({"number":number,"body":string,"repository":string}),
            &["number", "body"],
        ),
    ]
}

/// The call line's text, as Scramjet's: the title, a pull request's branches, a comment's target.
pub(super) fn summary(name: &str, arguments: &Value) -> String {
    let text = |key: &str| arguments[key].as_str().unwrap_or_default();
    match name {
        "create_issue" => text("title").to_owned(),
        "create_pr" => {
            let draft = if arguments["draft"] == true {
                ", draft"
            } else {
                ""
            };
            format!(
                "{} ({} → {}{draft})",
                text("title"),
                text("head"),
                text("base")
            )
        }
        _ => format!("#{}", arguments["number"]),
    }
}

/// What a session's publications go by: its commands with their defaults, the one running and
/// Scramjet's global `autonomy.yaml`.
pub(super) struct Policy<'a> {
    pub commands: &'a Commands,
    pub active: Option<&'a str>,
    pub autonomy: Option<&'a Path>,
}

impl Policy<'_> {
    /// Whether a publication with `tool` to the project's repository runs without asking, as
    /// Scramjet's `resolvePublicationPolicy` decides: only for a top-level command that declares
    /// the tool, by the user's setting for it, else its set's default. A file that cannot be
    /// read asks.
    fn approves(&self, tool: &str) -> bool {
        let Some(command) = self.active.and_then(|name| self.commands.get(name)) else {
            return false;
        };
        let declared = (command.allowed_tools.iter().flatten()).any(|allowed| allowed == tool);
        if command.delegate_only || !declared {
            return false;
        }
        let setting = match autonomy::publication_override(self.autonomy, &command.name, tool) {
            Ok(setting) => setting,
            Err(_) => return false,
        };
        let setting = setting.or_else(|| self.commands.publication_default(&command.name, tool));
        setting == Some(Publication::Auto)
    }
}

/// A call's request and the repository URL it names, if any.
struct Proposal {
    tool: &'static str,
    request: Request,
    repository: Option<String>,
}

impl Proposal {
    /// What the call asks for, or what is wrong with its arguments as Pi's validation says it.
    fn parse(name: &str, arguments: &Value) -> Result<Self, String> {
        let tool = TOOLS
            .into_iter()
            .find(|tool| *tool == name)
            .unwrap_or(TOOLS[0]);
        let mut errors = Vec::new();
        let text = |key: &str, required: bool, line: bool, errors: &mut Vec<String>| {
            let error = match arguments.get(key) {
                None if required => "must have required property",
                None => return String::new(),
                Some(Value::String(text)) if line && text.is_empty() => {
                    "must NOT have fewer than 1 characters"
                }
                Some(Value::String(text)) => return text.clone(),
                Some(_) => "must be string",
            };
            errors.push(format!("  - {key}: {error}"));
            String::new()
        };
        let request = match tool {
            "create_issue" => Request::Issue {
                title: text("title", true, true, &mut errors),
                body: text("body", true, false, &mut errors),
            },
            "create_pr" => Request::Pull {
                title: text("title", true, true, &mut errors),
                body: text("body", true, false, &mut errors),
                head: text("head", true, true, &mut errors),
                base: text("base", true, true, &mut errors),
                draft: match arguments.get("draft") {
                    Some(Value::Bool(draft)) => *draft,
                    found => {
                        let error = match found {
                            None => "must have required property",
                            Some(_) => "must be boolean",
                        };
                        errors.push(format!("  - draft: {error}"));
                        false
                    }
                },
            },
            _ => {
                let number = match arguments.get("number") {
                    Some(number) if number.as_u64().is_some_and(|n| n >= 1) => number.as_u64(),
                    Some(number) if number.as_i64().is_some() => {
                        errors.push("  - number: must be >= 1".into());
                        None
                    }
                    found => {
                        let error = match found {
                            None => "must have required property",
                            Some(_) => "must be integer",
                        };
                        errors.push(format!("  - number: {error}"));
                        None
                    }
                };
                Request::Comment {
                    number: number.unwrap_or_default(),
                    pull: tool == "add_pr_comment",
                    body: text("body", true, false, &mut errors),
                }
            }
        };
        let repository = arguments
            .get("repository")
            .map(|_| text("repository", false, false, &mut errors));
        if !errors.is_empty() {
            let received = serde_json::to_string_pretty(arguments).unwrap_or_default();
            return Err(format!(
                "Validation failed for tool \"{tool}\":\n{}\n\nReceived arguments:\n{received}",
                errors.join("\n")
            ));
        }
        Ok(Self {
            tool,
            request,
            repository,
        })
    }

    fn title(&self) -> Option<&str> {
        match &self.request {
            Request::Issue { title, .. } | Request::Pull { title, .. } => Some(title),
            Request::Comment { .. } => None,
        }
    }

    /// What publishing it does, as Scramjet says it.
    fn consequence(&self) -> String {
        match &self.request {
            Request::Issue { .. } => "publish a new issue".into(),
            Request::Pull { .. } => "publish a new pull request".into(),
            Request::Comment { number, pull, .. } => {
                let parent = if *pull { "pull request" } else { "issue" };
                format!("publish a comment on {parent} #{number}")
            }
        }
    }

    /// What the user approves, as Scramjet's preview card shows it: the facts, then each field
    /// whole.
    fn preview(&self, repository: &Repository) -> String {
        let mut fields: Vec<(&str, String)> = Vec::new();
        if let Some(url) = &self.repository {
            fields.push(("Repository", url.clone()));
        }
        match &self.request {
            Request::Issue { title, body } => {
                fields.extend([("Title", title.clone()), ("Body", body.clone())])
            }
            Request::Pull {
                title,
                body,
                head,
                base,
                draft,
            } => fields.extend([
                ("Title", title.clone()),
                ("Head", head.clone()),
                ("Base", base.clone()),
                ("Draft", draft.to_string()),
                ("Body", body.clone()),
            ]),
            Request::Comment { number, body, .. } => {
                fields.extend([("Target", format!("#{number}")), ("Comment", body.clone())])
            }
        }
        let fields: Vec<String> = (fields.into_iter())
            .map(|(label, value)| format!("## {label}\n\n{value}"))
            .collect();
        format!(
            "**{}**\nProvider: github\nRepository: {}\nConsequence: {}\n\n{}",
            label(self.tool),
            repository.shown(),
            self.consequence(),
            fields.join("\n\n")
        )
    }
}

/// How a publication ended.
enum Outcome {
    Verified(String),
    Cancelled,
    /// Nothing was written, for this reason.
    NotBegun(String),
    /// It may have been written.
    Ambiguous(String),
}

/// Scramjet's name for what `tool` publishes.
fn label(tool: &str) -> &'static str {
    match tool {
        "create_issue" => "Create issue",
        "create_pr" => "Create pull request",
        "add_issue_comment" => "Add issue comment",
        _ => "Add pull request comment",
    }
}

fn output(tool: &str, outcome: Outcome) -> Output {
    let noun = label(tool);
    let (text, failed) = match outcome {
        Outcome::Verified(url) => (format!("{noun} publication verified: {url}"), false),
        Outcome::Cancelled => (
            format!("{noun} publication cancelled. No remote write was performed."),
            false,
        ),
        Outcome::NotBegun(reason) => (format!("{noun} publication did not begin: {reason}."), true),
        Outcome::Ambiguous(reason) => (
            format!(
                "{noun} publication may have occurred; automatic retry is prohibited. Reconcile \
deliberately. ({reason})"
            ),
            true,
        ),
    };
    Output {
        text,
        detail: None,
        failed,
    }
}

/// Runs the publication call `name` with `arguments`. `forge` runs `gh` and `git`; `cancel`
/// stops it before the write.
pub(super) fn run(
    name: &str,
    arguments: &Value,
    policy: &Policy,
    ui: &mut dyn AgentUi,
    forge: &mut dyn Exec,
    cancel: &AtomicBool,
) -> Output {
    let proposal = match Proposal::parse(name, arguments) {
        Ok(proposal) => proposal,
        Err(error) => {
            return Output {
                text: error,
                detail: None,
                failed: true,
            };
        }
    };
    let tool = proposal.tool;
    let outcome = publish(&proposal, policy, ui, forge, cancel);
    output(tool, outcome)
}

fn publish(
    proposal: &Proposal,
    policy: &Policy,
    ui: &mut dyn AgentUi,
    forge: &mut dyn Exec,
    cancel: &AtomicBool,
) -> Outcome {
    let not_begun = |reason: &str| Outcome::NotBegun(reason.to_owned());
    if proposal
        .title()
        .is_some_and(|title| title.contains(['\r', '\n']))
    {
        return not_begun("publication titles must contain exactly one line");
    }
    let approves = policy.approves(proposal.tool);
    ui.tool_running();
    let (origin, repository) = match &proposal.repository {
        Some(url) => match forge::parse_target(url) {
            Ok(repository) => (forge::origin(forge).ok(), repository),
            Err(error) => return Outcome::NotBegun(error),
        },
        None => match forge::origin(forge) {
            Ok(origin) => (Some(origin.clone()), origin),
            Err(error) => return Outcome::NotBegun(error),
        },
    };
    let external = !origin
        .as_ref()
        .is_some_and(|origin| origin.same(&repository));
    let check = |forge: &mut dyn Exec| -> Result<u64, String> {
        let id = forge::preflight(forge, &repository, &proposal.request)?;
        if let Request::Pull { head, base, .. } = &proposal.request {
            forge::branches(forge, head, base, external.then_some(&repository))?;
        }
        Ok(id)
    };
    let id = match check(forge) {
        Ok(id) => id,
        Err(error) => return Outcome::NotBegun(error),
    };
    if external || !approves {
        ui.question(&proposal.preview(&repository));
        let title = format!(
            "{} in {} — {}",
            label(proposal.tool),
            repository.shown(),
            proposal.consequence()
        );
        let choices = ["Approve publication", "Cancel"].map(|label| Choice {
            label: label.into(),
            description: None,
            value: label.into(),
        });
        if ui.choose(&title, &choices, None) != Some(0) {
            return Outcome::Cancelled;
        }
        ui.tool_running();
        // As Scramjet, what was approved is checked again: time has passed.
        if let Some(origin) = &origin {
            match forge::origin(forge) {
                Ok(current) if current.same(origin) => {}
                Ok(_) => return not_begun("origin-changed"),
                Err(_) => return not_begun("origin-could-not-be-revalidated"),
            }
        }
        match check(forge) {
            Ok(current) if current == id => {}
            Ok(_) => return not_begun("Repository identity changed"),
            Err(error) => return Outcome::NotBegun(error),
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return not_begun("the turn was cancelled");
    }
    match forge::publish(forge, &repository, &proposal.request) {
        Posted::Verified(url) => Outcome::Verified(url),
        Posted::NotSent(reason) => Outcome::NotBegun(reason),
        Posted::Ambiguous(reason) => Outcome::Ambiguous(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{forge::tests::Script, tests::Recorder, tests::ScratchDir};

    fn commands() -> Commands {
        Commands::parsed(&[
            (
                "rs:post.md",
                "---\nallowed-tools: [create_issue, add_pr_comment]\n---\nPost",
            ),
            ("rs:plain.md", "Plain"),
        ])
    }

    fn issue() -> Value {
        json!({"title":"Fix it","body":"It **breaks**."})
    }

    /// `Script::github` with an issue it creates as `issue()` asks.
    fn creating() -> Script {
        let url = "https://github.com/o/r/issues/5";
        Script::github().created(
            "repos/o/r/issues",
            json!({"number":5,"html_url":url}),
            (
                "repos/o/r/issues/5",
                json!({"number":5,"html_url":url,"title":"Fix it","body":"It **breaks**."}),
            ),
        )
    }

    fn policy<'a>(
        commands: &'a Commands,
        active: Option<&'a str>,
        autonomy: &'a Path,
    ) -> Policy<'a> {
        Policy {
            commands,
            active,
            autonomy: Some(autonomy),
        }
    }

    #[test]
    fn the_user_approves_the_exact_payload_before_it_is_published() {
        let commands = commands();
        let none = Path::new("/nonexistent/autonomy.yaml");
        let policy = policy(&commands, None, none);
        let mut ui = Recorder {
            choices: vec![Some(0)],
            ..Recorder::default()
        };
        let mut script = creating();
        let cancel = AtomicBool::new(false);
        let output = run(
            "create_issue",
            &issue(),
            &policy,
            &mut ui,
            &mut script,
            &cancel,
        );
        assert_eq!(
            (output.text.as_str(), output.failed),
            (
                "Create issue publication verified: https://github.com/o/r/issues/5",
                false
            )
        );
        assert_eq!(
            ui.shown,
            [
                "question **Create issue**\nProvider: github\nRepository: o/r\nConsequence: \
publish a new issue\n\n## Title\n\nFix it\n\n## Body\n\nIt **breaks**.",
                "choose Create issue in o/r — publish a new issue [\"Approve publication\", \
\"Cancel\"] None",
            ]
        );
        // Checked before the question and again after it, then written once.
        let reads = |script: &Script| {
            (script.ran.iter())
                .filter(|(line, _)| line == "gh api --hostname github.com repos/o/r")
                .count()
        };
        assert_eq!((reads(&script), script.posts().len()), (2, 1));

        // Cancelled, nothing is written, and it is not a failure.
        let mut ui = Recorder {
            choices: vec![Some(1)],
            ..Recorder::default()
        };
        let mut script = creating();
        let output = run(
            "create_issue",
            &issue(),
            &policy,
            &mut ui,
            &mut script,
            &cancel,
        );
        assert_eq!(
            (output.text.as_str(), output.failed),
            (
                "Create issue publication cancelled. No remote write was performed.",
                false
            )
        );
        assert!(script.posts().is_empty());
    }

    #[test]
    fn a_command_that_declares_the_tool_publishes_by_its_policy_without_asking() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-publish-policy-{}", std::process::id())),
        );
        let autonomy = scratch.0.join("autonomy.yaml");
        let commands = commands();
        let cancel = AtomicBool::new(false);
        // What asks: a choice that the recorder cancels.
        let asked = |active: Option<&str>, arguments: Value| {
            let mut ui = Recorder::default();
            let mut script = creating();
            let policy = policy(&commands, active, &autonomy);
            let output = run(
                "create_issue",
                &arguments,
                &policy,
                &mut ui,
                &mut script,
                &cancel,
            );
            let asked = ui.shown.iter().any(|shown| shown.starts_with("choose "));
            assert!(!output.failed, "{}", output.text);
            (asked, script.posts().len())
        };
        // Without a setting, or a command that declares the tool, the user is asked.
        assert_eq!(asked(Some("rs:post"), issue()), (true, 0));
        std::fs::write(
            &autonomy,
            "publications:\n  rs:post:\n    create_issue: auto-approve\n",
        )
        .unwrap();
        assert_eq!(asked(Some("rs:post"), issue()), (false, 1));
        assert_eq!(asked(None, issue()), (true, 0));
        std::fs::write(
            &autonomy,
            "publications:\n  rs:plain:\n    create_issue: auto-approve\n",
        )
        .unwrap();
        assert_eq!(asked(Some("rs:plain"), issue()), (true, 0));
        // Another repository is always asked about, named by its URL.
        std::fs::write(
            &autonomy,
            "publications:\n  rs:post:\n    create_issue: auto-approve\n",
        )
        .unwrap();
        let mut elsewhere = issue();
        elsewhere["repository"] = json!("https://github.com/x/y");
        let mut ui = Recorder::default();
        let policy = policy(&commands, Some("rs:post"), &autonomy);
        let mut script = creating().reply(
            "gh api --hostname github.com repos/x/y",
            json!({"id":8,"full_name":"x/y","html_url":"https://github.com/x/y"}),
        );
        run(
            "create_issue",
            &elsewhere,
            &policy,
            &mut ui,
            &mut script,
            &cancel,
        );
        assert!(ui.shown[0].starts_with(
            "question **Create issue**\nProvider: github\nRepository: \
x/y\nConsequence: publish a new issue\n\n## Repository\n\nhttps://github.com/x/y\n\n## Title"
        ));
        // The same repository by its URL keeps the policy.
        let mut same = issue();
        same["repository"] = json!("https://github.com/o/r");
        assert_eq!(asked(Some("rs:post"), same), (false, 1));
        // A user setting that cannot be read asks.
        std::fs::write(
            &autonomy,
            "publications:\n  rs:post:\n    create_issue: sure\n",
        )
        .unwrap();
        assert_eq!(asked(Some("rs:post"), issue()), (true, 0));
        std::fs::write(
            &autonomy,
            "publications:\n  rs:post:\n    create_issue: always-ask\n",
        )
        .unwrap();
        assert_eq!(asked(Some("rs:post"), issue()), (true, 0));
    }

    #[test]
    fn what_cannot_be_published_says_why_before_anything_is_written() {
        let commands = commands();
        let none = Path::new("/nonexistent/autonomy.yaml");
        let policy = policy(&commands, None, none);
        let cancel = AtomicBool::new(false);
        let run = |name: &str, arguments: Value, script: &mut Script| {
            let mut ui = Recorder {
                choices: vec![Some(0)],
                ..Recorder::default()
            };
            let output = run(name, &arguments, &policy, &mut ui, script, &cancel);
            assert!(output.failed);
            assert!(script.posts().is_empty());
            output.text
        };
        assert_eq!(
            run(
                "create_issue",
                json!({"title":"a\nb","body":""}),
                &mut creating()
            ),
            "Create issue publication did not begin: publication titles must contain exactly one \
line."
        );
        let mut no_origin = Script::default();
        assert_eq!(
            run("create_issue", issue(), &mut no_origin),
            "Create issue publication did not begin: Unable to read the current repository origin."
        );
        assert_eq!(
            run(
                "add_pr_comment",
                json!({"number":41,"body":"x"}),
                &mut Script::github()
            ),
            "Add pull request comment publication did not begin: Publication target does not \
match the requested GitHub artifact type."
        );
        assert_eq!(
            run(
                "create_pr",
                json!({"title":"t","body":"","head":"gone","base":"main","draft":false}),
                &mut Script::github()
            ),
            "Create pull request publication did not begin: Remote branch does not exist: gone."
        );
        let received = serde_json::to_string_pretty(&json!({"number":0,"body":3})).unwrap();
        assert_eq!(
            run(
                "add_issue_comment",
                json!({"number":0,"body":3}),
                &mut Script::github()
            ),
            format!(
                "Validation failed for tool \"add_issue_comment\":\n  - number: must be >= 1\n  \
- body: must be string\n\nReceived arguments:\n{received}"
            )
        );
        let received = serde_json::to_string_pretty(&json!({"title":""})).unwrap();
        assert_eq!(
            run("create_pr", json!({"title":""}), &mut Script::github()),
            format!(
                "Validation failed for tool \"create_pr\":\n  - title: must NOT have fewer than 1 \
characters\n  - body: must have required property\n  - head: must have required property\n  - \
base: must have required property\n  - draft: must have required property\n\nReceived \
arguments:\n{received}"
            )
        );
        // An origin that changed while the user was asked is not written to.
        let mut moving = creating();
        moving.replies.insert(
            0,
            ("git remote get-url origin".into(), forge::Ran::default()),
        );
        let mut ui = Recorder {
            choices: vec![Some(0)],
            ..Recorder::default()
        };
        let mut once = MovesAfterFirst(Some(creating()), moving);
        let output = super::run(
            "create_issue",
            &issue(),
            &policy,
            &mut ui,
            &mut once,
            &cancel,
        );
        assert_eq!(
            output.text,
            "Create issue publication did not begin: origin-could-not-be-revalidated."
        );
        // A turn cancelled while the user was asked writes nothing.
        let cancelled = AtomicBool::new(true);
        let mut ui = Recorder {
            choices: vec![Some(0)],
            ..Recorder::default()
        };
        let mut script = creating();
        let output = super::run(
            "create_issue",
            &issue(),
            &policy,
            &mut ui,
            &mut script,
            &cancelled,
        );
        assert_eq!(
            output.text,
            "Create issue publication did not begin: the turn was cancelled."
        );
        assert!(script.posts().is_empty());
    }

    /// Runs the first script's first command, then the second script's.
    struct MovesAfterFirst(Option<Script>, Script);

    impl Exec for MovesAfterFirst {
        fn run(&mut self, program: &str, args: &[&str], input: Option<&str>) -> forge::Ran {
            match self.0.take() {
                Some(mut first) => first.run(program, args, input),
                None => self.1.run(program, args, input),
            }
        }
    }

    #[test]
    fn ambiguous_writes_say_so_and_are_failures() {
        let commands = commands();
        let none = Path::new("/nonexistent/autonomy.yaml");
        let policy = policy(&commands, None, none);
        let mut ui = Recorder {
            choices: vec![Some(0)],
            ..Recorder::default()
        };
        let mut script =
            Script::github().fail("gh api --hostname github.com --method POST", "gh: HTTP 502");
        let output = run(
            "add_pr_comment",
            &json!({"number":42,"body":"LGTM"}),
            &policy,
            &mut ui,
            &mut script,
            &AtomicBool::new(false),
        );
        assert_eq!(
            (output.text.as_str(), output.failed),
            (
                "Add pull request comment publication may have occurred; automatic retry is \
prohibited. Reconcile deliberately. (mutation-process-failed-after-spawn (gh: HTTP 502))",
                true
            )
        );
        assert_eq!(script.posts().len(), 1);
    }

    #[test]
    fn call_lines_show_the_title_branches_or_target() {
        assert_eq!(summary("create_issue", &issue()), "Fix it");
        let pull = json!({"title":"Add","head":"feature","base":"main","draft":true});
        assert_eq!(summary("create_pr", &pull), "Add (feature → main, draft)");
        assert_eq!(summary("add_pr_comment", &json!({"number":42})), "#42");
    }
}
