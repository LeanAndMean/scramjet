//! The command line: its options and the usage text.
use super::settings::{LEVELS, card_level};
use scramjet_rs::agent::SessionChoice;
use std::{ffi::OsString, net::SocketAddr, path::PathBuf};

pub(super) const USAGE: &str = "Usage: scramjet-rs [--project DIR] [--endpoint ADDR] [--model ID] \
[--effort LEVEL] [--max-output-tokens N] [--retries N] [--continue | -r | --session FILE]
[--ask] [-p PROMPT]
       scramjet-rs install DIR | remove NAME|DIR | list

Interactive coding agent using Proxy (default 127.0.0.1:18080): the Responses API for GPT models,
the Messages API for Claude models (those whose Proxy model card lists only Messages).
  --project DIR          project directory (default: current directory)
  --model ID             model (default: gpt-6-luna, or the one a resumed session last used)
  --effort LEVEL         reasoning effort: off, minimal, low, medium, high, xhigh or max, the
                         nearest the model's Proxy card lists (default: the one a resumed
                         session last used, else the settings' defaultThinkingLevel, else
                         medium)
  --retries N            resend a request that failed transiently up to N times (default 3)
  --continue             resume the session for this project written to last
  -r, --resume           choose one of this project's sessions to resume, as /resume does
  --session FILE         resume a specific session file
  --ask                  ask before each bash command and each write or edit outside the
                         project (answer `a` to stop asking until exit); by default tool
                         calls run without asking, as in Scramjet. Bash is not sandboxed.
  -p PROMPT              run one prompt and exit (`-p -` reads it from stdin)
The session fills the terminal's alternate screen: the conversation above a status line and the
prompt. Page Up/Down and the mouse wheel scroll the conversation; Escape goes back to its end.
In the prompt, Enter or Alt+Enter sends; Shift+Enter, Ctrl-J or a trailing \\ starts a new
line; pasted text is inserted without sending. Up/Down recall earlier prompts. Ctrl-G edits the
prompt in $VISUAL or $EDITOR; its text replaces the prompt when the editor exits with 0.
`!command` runs a bash command in the project directory without a model turn and adds its output
to the conversation for the next prompt; `!!command` runs it without adding it.
The prompt stays while a turn runs: Enter then queues a message for the model's next request,
Alt+Enter one for when the turn would end (steeringMode and followUpMode: all or one-at-a-time
at once); Escape interrupts the turn and puts them back in the editor, as Alt+Up does.
Ctrl-C clears the prompt, and interrupts a turn; on an empty prompt press it twice, or Ctrl-D, or
type /quit, to quit. Dragging selects text, a double click a word, a triple click the line;
Ctrl+C or a right click copies it, and a right click without a selection pastes the clipboard
into the prompt. Ctrl+V or Alt+V saves the clipboard's image to a file in the temporary directory
and pastes its path, for the model to read. Ctrl+O shows tool results whole, or cut again. Ctrl+T hides reasoning summaries
behind a `Thinking...` line, or shows them again. Shift+Tab sets the reasoning effort to the next
level the model's Proxy card lists. Both are saved in the settings. /new starts a new session;
/resume chooses one of this project's sessions to switch to, forks under the session they came
from; in its list Ctrl+R renames the selected session and Ctrl+D deletes it once Enter confirms
(to the trash when a `trash` command is installed). /fork chooses one of this
session's prompts and continues in a new session from before it, with it in the prompt for
editing. /tree shows this session's branches and continues from the entry chosen, in the same
session: from before a prompt, which is put in the prompt for editing, or from after a reply
or tool result. When that leaves part of the conversation, it asks whether the model should
summarize that branch, also with your own instructions, for the conversation to continue
with. Escape twice on an empty prompt opens /tree too. /model chooses a model.
Their lists show in the prompt's place and take Up/Down, Page Up/Down, Home/End, Enter, and
Escape or Ctrl-C to cancel; typed text filters the /resume and /model lists, best match first
(a \"quoted phrase\" matches as written). With piped input they are printed numbered instead,
for /resume N, /fork N, /tree N and /model ID.
/name NAME names the session, which /resume then lists by its name; /name alone shows it.
/session shows the session's file, the messages in its context and the tokens they used.
Typed while a turn runs, /session, /name and /model run before its next request; the other
commands stop it and run once it has.
As in Scramjet, /SET:COMMAND ARGS runs a command of a command set: its file
commands/SET:COMMAND.md is sent with the arguments substituted, inside <scramjet-command>. The
sets are those installed, then those under $XDG_DATA_HOME/scramjet (~/.local/share/scramjet)
and the project's .scramjet. `scramjet-rs install DIR` installs the set in DIR by reference,
recorded in the settings' commandSets; `remove NAME` takes it out and `list` shows the sets
loaded in the current directory, what each brings and what was skipped.
/compact [focus] replaces all but the last ~20k tokens of the conversation with a summary;
this also happens by itself when the context nears the model's window.
Settings are read from $XDG_CONFIG_HOME/scramjet-rs/settings.json (~/.config/scramjet-rs/),
with Scramjet's keys: hideThinkingBlock, doubleEscapeAction (tree, fork or none for the double
Escape), branchSummary.skipPrompt (true: /tree never asks for a summary) and
defaultThinkingLevel; and tools, the agent's coding tools among read, bash, edit, write (the
default) and grep, find, ls.";

/// What the command line asks for.
pub(super) struct Options {
    pub(super) project: Option<PathBuf>,
    pub(super) endpoint: SocketAddr,
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) max_output_tokens: Option<u64>,
    pub(super) choice: SessionChoice,
    /// `--resume`: the session starts by choosing one to resume.
    pub(super) resume: bool,
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
            resume: false,
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
                Some("--effort") => {
                    let level = value()?;
                    let Some(level) = card_level(&level) else {
                        return Err(format!(
                            "invalid --effort {level:?}: valid levels are {}",
                            LEVELS.join(", ")
                        ));
                    };
                    options.effort = Some(level.to_owned())
                }
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
                Some("--continue") => {
                    (options.choice, options.resume) = (SessionChoice::Continue, false)
                }
                Some("-r" | "--resume") => {
                    (options.choice, options.resume) = (SessionChoice::New, true)
                }
                Some("--session") => {
                    let path = PathBuf::from(value()?);
                    (options.choice, options.resume) = (SessionChoice::Open(path), false)
                }
                Some("--ask") => options.ask = true,
                Some("-p" | "--print") => options.prompt = Some(value()?),
                Some("-h" | "--help") => return Ok(None),
                _ => return Err(format!("unknown argument {argument:?}\n\n{USAGE}")),
            }
        }
        if options.resume && options.prompt.is_some() {
            return Err(format!("--resume cannot be used with -p\n\n{USAGE}"));
        }
        Ok(Some(options))
    }
}
