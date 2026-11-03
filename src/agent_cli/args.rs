//! The command line: its options and the usage text.
use scramjet_rs::agent::SessionChoice;
use std::{ffi::OsString, net::SocketAddr, path::PathBuf};

pub(super) const USAGE: &str = "Usage: scramjet-rs [--project DIR] [--endpoint ADDR] [--model ID] \
[--effort LEVEL] [--max-output-tokens N] [--retries N] [--continue | --session FILE] [--ask]
[-p PROMPT]

Interactive coding agent using Proxy (default 127.0.0.1:18080): the Responses API for GPT models,
the Messages API for Claude models (those whose Proxy model card lists only Messages).
  --project DIR          project directory (default: current directory)
  --model ID             model (default: gpt-6-luna, or the one a resumed session last used)
  --effort LEVEL         reasoning effort, e.g. low, medium, high (default: the model's, or the
                         one a resumed session last used)
  --retries N            resend a request that failed transiently up to N times (default 3)
  --continue             resume the session for this project written to last
  --session FILE         resume a specific session file
  --ask                  ask before each bash command and each write or edit outside the
                         project (answer `a` to stop asking until exit); by default tool
                         calls run without asking, as in Scramjet. Bash is not sandboxed.
  -p PROMPT              run one prompt and exit (`-p -` reads it from stdin)
In the interactive prompt, Enter sends; Shift+Enter, Alt+Enter, Ctrl-J or a trailing \\ starts
a new line; pasted text is inserted without sending. Up/Down recall earlier prompts. Ctrl-G
edits the prompt in $VISUAL or $EDITOR; its text replaces the prompt when the editor exits with 0.
`!command` runs a bash command in the project directory without a model turn and adds its output
to the conversation for the next prompt; `!!command` runs it without adding it.
Ctrl-C clears the prompt; on an empty prompt press it twice, or Ctrl-D, or type /quit, to quit.
Ctrl+O shows the conversation in the alternate screen with every tool result whole, from its
end: Up/Down, Page Up/Down and Home/End scroll it; Escape, q or Ctrl+O closes it.
Ctrl+T hides reasoning summaries behind a `Thinking...` line, or shows them again, from the
next one on; it works while a turn runs too. Shift+Tab sets the reasoning effort to the next
level the model's Proxy card lists. /new starts a new session; /resume chooses one of
this project's sessions to switch to. /fork chooses one of this session's prompts and continues
in a new session from before it, with it in the prompt for editing. /tree shows this session's
branches and continues from the entry chosen, in the same session: from before a prompt, which
is put in the prompt for editing, or from after a reply or tool result. When that leaves part of
the conversation, it asks whether the model should summarize that branch, also with your own
instructions, for the conversation to continue with. Escape twice on an empty prompt opens
/tree too. /model chooses a model.
Their lists take Up/Down, Page Up/Down, Home/End, Enter, and Escape or Ctrl-C to cancel; typed
text filters the /resume and /model lists, best match first (a \"quoted phrase\" matches as
written). With piped input they are printed numbered instead, for /resume N, /fork N, /tree N
and /model ID.
/name NAME names the session, which /resume then lists by its name; /name alone shows it.
/session shows the session's file, the messages in its context and the tokens they used.
As in Scramjet, /SET:COMMAND ARGS runs a command of a command set: the file
.scramjet/SET/commands/SET:COMMAND.md of the project, or under $XDG_DATA_HOME/scramjet
(~/.local/share/scramjet), is sent with the arguments substituted, inside <scramjet-command>.
/compact [focus] replaces all but the last ~20k tokens of the conversation with a summary;
this also happens by itself when the context nears the model's window.
While a turn, /compact or a `!` command runs, typed text is kept for the next prompt without
being shown. Escape cancels it and keeps that text; Ctrl-C cancels it and discards that text;
press Ctrl-C twice to quit.";

/// What the command line asks for.
pub(super) struct Options {
    pub(super) project: Option<PathBuf>,
    pub(super) endpoint: SocketAddr,
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) max_output_tokens: Option<u64>,
    pub(super) choice: SessionChoice,
    pub(super) ask: bool,
    pub(super) retries: u32,
    /// `-p`: the one prompt to run, `-` to read it from stdin.
    pub(super) prompt: Option<String>,
}

impl Options {
    /// The options `arguments` give; `None` when they ask for `--help`.
    pub(super) fn parse(
        mut arguments: impl Iterator<Item = OsString>,
    ) -> Result<Option<Self>, String> {
        let mut options = Self {
            project: None,
            endpoint: ([127, 0, 0, 1], 18080).into(),
            model: None,
            effort: None,
            max_output_tokens: None,
            choice: SessionChoice::New,
            ask: false,
            retries: 3,
            prompt: None,
        };
        while let Some(argument) = arguments.next() {
            let mut value = || {
                arguments
                    .next()
                    .and_then(|v| v.into_string().ok())
                    .ok_or(format!(
                        "{} requires a value\n\n{USAGE}",
                        argument.to_string_lossy()
                    ))
            };
            match argument.to_str() {
                Some("--project") => options.project = Some(PathBuf::from(value()?)),
                Some("--endpoint") => {
                    options.endpoint = value()?.parse().map_err(|_| "invalid --endpoint address")?
                }
                Some("--model") => options.model = Some(value()?),
                Some("--effort") => options.effort = Some(value()?),
                Some("--max-output-tokens") => {
                    options.max_output_tokens = Some(
                        value()?
                            .parse()
                            .map_err(|_| "invalid --max-output-tokens")?,
                    )
                }
                Some("--retries") => {
                    options.retries = value()?.parse().map_err(|_| "invalid --retries")?
                }
                Some("--continue") => options.choice = SessionChoice::Continue,
                Some("--session") => options.choice = SessionChoice::Open(PathBuf::from(value()?)),
                Some("--ask") => options.ask = true,
                Some("-p" | "--print") => options.prompt = Some(value()?),
                Some("-h" | "--help") => return Ok(None),
                _ => return Err(format!("unknown argument {argument:?}\n\n{USAGE}")),
            }
        }
        Ok(Some(options))
    }
}
