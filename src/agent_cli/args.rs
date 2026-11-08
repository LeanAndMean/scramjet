//! The command line: its options and the usage text.
use super::settings::{LEVELS, card_level};
use scramjet_rs::agent::{HARNESS_TOOLS, SandboxMode, SystemPrompt};
use std::{ffi::OsString, path::PathBuf};

pub(super) const USAGE: &str =
    "Usage: scramjet-rs [--project DIR] [--model PATTERN] [--models PATTERNS]
[--thinking LEVEL] [--max-output-tokens N] [--retries N]
[-c | -r | --session FILE|ID | --fork FILE|ID | --no-session] [--session-dir DIR]
[--system-prompt TEXT]
[--append-system-prompt TEXT]... [--no-context-files] [--skill PATH]... [--no-skills]
[--theme PATH]... [--no-themes] [-t LIST | -nt | -nbt] [--ask] [-s MODE] [-p]
[@FILE...] [MESSAGE...]
       scramjet-rs --list-models [SEARCH] | --list-credential-stores | --export FILE [OUTPUT]
       scramjet-rs --version
       scramjet-rs install DIR|REPO | remove NAME|DIR|REPO | update [REPO|NAME] | list

Interactive coding agent. Its models are those of the providers in models.json beside the
settings file (by default ~/.config/scramjet-rs/models.json) and in installed command sets, in
Scramjet's format: each provider has a baseUrl (http:// or https://), an api (openai-responses,
anthropic-messages or openai-completions, with its compat), an apiKey (the value of the
environment variable of that name, else the text; !command: what the command prints, run for
each request, as headers' values may be) and its models, with their contextWindow, maxTokens, reasoning, input and cost; a file's
providersCommand names a program printing more, as {\"version\": 1, \"providers\": ...}, and
its credentialFiles the files holding their keys, hidden as --list-credential-stores says,
and its logins the programs that log in to more providers. /login adds the models of ChatGPT,
GitHub Copilot and those logins, each saved in auth.json beside the settings.
  --project DIR          project directory (default: current directory)
  --model PATTERN        model, as Scramjet finds it: the one whose provider/id or id PATTERN
                         is (an id two providers offer: the first's), else the newest whose
                         id or name contains it (after provider/, among that provider's),
                         with an optional :LEVEL that sets the thinking level (default: the
                         one a resumed session last used; for a new session, the one last
                         chosen in this project, else anywhere, as in Scramjet, else
                         gpt-6-luna)
  --models PATTERNS      the models Ctrl+P cycles through, comma-separated, in place of the
                         settings' enabledModels: each a model's id, part of its id or name,
                         or a glob (claude-*), with an optional :LEVEL; a new session without
                         --model starts with the one last chosen when it is one of them, else
                         with the first, at its level
  --thinking LEVEL       reasoning effort (also --effort): off, minimal, low, medium, high,
                         xhigh or max, the nearest the model takes (default: the
                         one --model's :LEVEL sets, else the one a resumed session last used,
                         else the settings' defaultThinkingLevel, else medium)
  --max-output-tokens N  the most tokens a reply may have (default: the model's limit)
  --retries N            resend a request that failed transiently up to N times (default:
                         the settings' retry.maxRetries, else 3)
  -c, --continue         resume the session for this project written to last
  -r, --resume           choose one of this project's sessions to resume, as /resume does
  --session FILE|ID      resume a session file (a word with a / or ending in .jsonl), or the
                         newest session of this project whose id (as /status shows it) starts
                         with ID, else of another project, which is forked here once you
                         agree, as in Scramjet
  --fork FILE|ID         continue a copy of that session, its branches and name included, in a
                         new session of this project
  --no-session           save no session, as Scramjet's: those of this run (/clear, /fork,
                         /branch and /import too) are kept in a private temporary directory
                         removed at exit, and /resume lists only them
  --session-dir DIR      save and find sessions in DIR, in place of the project's directory
                         under ~/.local/state/scramjet-rs/sessions, as Scramjet's (default:
                         the settings' sessionDir); -c, -r, /resume and --session ID look
                         there, whatever project a session there was of
  --system-prompt TEXT   the instructions' opening, in place of the tools and guidelines: TEXT,
                         or the contents of the file it names (default: the project's
                         .scramjet/SYSTEM.md, else ~/.scramjet/agent/SYSTEM.md, else
                         scramjet-rs's), as in Scramjet
  --append-system-prompt TEXT
                         text, or a file's contents, after the opening; several are joined
                         (default: an APPEND_SYSTEM.md found as SYSTEM.md is)
  -nc, --no-context-files
                         leave out the AGENTS.md and CLAUDE.md files of ~/.scramjet/agent, the
                         project and the directories above it
  --skill PATH           a skill file, or a directory of skills, looked in after the others
                         (repeatable); a relative PATH is from the current directory
  -ns, --no-skills       no skills but those --skill names: none from the skill directories
                         or the settings' skills
  --theme PATH           a theme file, or a directory of themes, for /settings to offer after
                         the others (repeatable); a relative PATH is from the current
                         directory
  --no-themes            no themes but the built-in ones and those --theme names: none from
                         the theme directories or the settings' themes
  -t, --tools LIST       the only tools the agent has in this run, comma-separated, as in
                         Scramjet: coding tools, in place of the settings' tools, and
                         scramjet-rs's own (subagent, get_scramjet_user_input, delegate,
                         report_scramjet_command_status…), left out unless named; without
                         report_scramjet_command_status a command cannot finish, and waits
                         for you after each reply
  -nt, --no-tools        no tools at all, unless --tools names some
  -nbt, --no-builtin-tools
                         no coding tools, unless --tools names some; scramjet-rs's own stay
  --ask                  ask before each bash command and each write or edit outside the
                         project (answer `a` to stop asking until exit); by default tool
                         calls run without asking, as in Scramjet. Bash is not sandboxed
                         unless --sandbox says so.
  -s, --sandbox MODE     confine the agent's tools, as Codex's modes do (default: the
                         settings' sandbox, else danger-full-access): read-only runs bash with
                         every file read-only, a private /tmp, its own processes and the
                         credential stores hidden, and refuses write and edit;
                         workspace-write lets both change the project but its .git,
                         .scramjet and .agents. MCP servers that run as programs start
                         inside too. Commands reach the network unless the
                         setting sandboxNetwork is false. It needs bubblewrap (bwrap) and
                         refuses to start without it; on macOS it runs under Seatbelt
                         (sandbox-exec). `!` commands you type stay outside
  -p, --print            run the messages and exit; a message `-` is read from stdin
  @FILE                  send FILE with the first message (or alone), as Scramjet does: a
                         text file's contents, or a PNG, JPEG, GIF or WebP image, scaled down
                         as read scales it; a relative path is from the current directory
  MESSAGE                prompts sent one after another when the session starts
  --export FILE [OUTPUT] write the session file FILE as a web page to OUTPUT (default:
                         scramjet-rs-session-<its name>.html here), as /export does, and exit
  --list-models [SEARCH] list the providers' models (those SEARCH matches) by provider, with
                         their context window, output limit, and whether they reason and
                         take images, as Scramjet lists them, and exit
  --list-credential-stores
                         list the credential files and directories that subagents' tools, and
                         the agent's in turns nobody started (setting hideCredentials), do not
                         see: the common ones and those models.json files name, and exit
  -v, --version          print the version and exit
The session fills the terminal's alternate screen: the conversation above a status line and the
prompt, and under it the project's directory and Git branch, the session's tokens, how full the
context is and the model, as Scramjet's footer. Page Up/Down and the mouse wheel scroll the
conversation; Escape goes back to its end.
In the prompt, Enter or Alt+Enter sends; Shift+Enter, Ctrl-J or a trailing \\ starts a new
line; pasted text is inserted without sending. Up/Down recall earlier prompts. Ctrl-G edits the
prompt in $VISUAL or $EDITOR; its text replaces the prompt when the editor exits with 0.
`!command` runs a bash command in the project directory without a model turn and adds its output
to the conversation for the next prompt; `!!command` runs it without adding it.
The prompt stays while a turn runs: Enter then queues a message for the model's next request,
Alt+Enter one for when the turn would end (steeringMode and followUpMode: all or one-at-a-time
at once); Escape interrupts the turn and puts them back in the editor, as Alt+Up does.
Ctrl-C clears the prompt, and interrupts a turn; on an empty prompt press it twice, or Ctrl-D, or
type /exit, to quit. Dragging selects text, a double click a word, a triple click the line;
Ctrl+C or a right click copies it, and a right click without a selection pastes the clipboard
into the prompt. Ctrl+V or Alt+V saves the clipboard's image to a file in the temporary directory
and pastes its path, for the model to read. Ctrl+O shows tool results whole, or cut again. Ctrl+T
hides reasoning summaries behind a `Thinking...` line, or shows them again. Shift+Tab sets the
reasoning effort to the next level the model takes. Both are saved in the settings.
/model, or Ctrl+L, chooses a model. Ctrl+P switches to the next model of --models or
enabledModels, else of all the providers', Shift+Ctrl+P (in a terminal that reports it) to the
one before; during a turn, from its next request. /scoped-models lists
the providers' models, those Ctrl+P takes first in its order: Enter switches one on or off,
Ctrl+A and Ctrl+X all those shown, Ctrl+P its provider's, Alt+Up/Down moves it, for the session;
Ctrl+S saves them as enabledModels. /clear [NAME] starts a new session;
/resume chooses one of this project's sessions to switch to, forks under the session they came
from; in its list Ctrl+S sorts them (threaded, recent, relevance), Ctrl+N shows only named ones,
Ctrl+P their paths, Ctrl+R renames the selected session and Ctrl+D deletes it once Enter confirms
(to the trash when a `trash` command is installed), and Tab lists every project's sessions with
their folders, one of another project's continuing in a new session of this one; sessions next
steps started in a chain show their summaries, and Ctrl+X shows chains flat, threaded or folded
(Right or Enter opens one, Left closes it), the form saved as resumeChains. /fork chooses
one of this session's prompts and continues in a new session from before it, with it in the
prompt for editing; /branch [NAME] continues in a new session with the conversation as it is.
/tree shows this session's branches and continues from the entry chosen, in the same session:
from before a prompt, which is put in the prompt for editing, or from after a reply
or tool result. When that leaves part of the conversation, it asks whether the model should
summarize that branch, also with your own instructions, for the conversation to continue
with; Shift+L there labels the selected entry. Escape twice on an empty prompt opens /tree too.
The lists of /resume, /fork, /tree and /model show in the prompt's place and take Up/Down,
Page Up/Down, Home/End, Enter, and Escape or Ctrl-C to cancel; typed text filters the /resume
and /model lists, best match first (a \"quoted phrase\" matches as written, and re:PATTERN is a
case-insensitive regular expression). With piped input they are printed numbered instead, for
/resume N, /fork N, /tree N and /model ID.
In the prompt, Ctrl+Y puts back the text deleted last, Alt+Y then replaces it with what was
deleted before; Ctrl+- undoes; Ctrl+] then a character jumps to it, Ctrl+Alt+] backward.
Ctrl+Home and Ctrl+End scroll to the conversation's start and end.
/rename NAME names the session, which /resume then lists by its name; /rename alone shows it.
/status shows the session's file, the messages in its context, the tokens they used, the cost and
the current provider's quota as its latest response reported it.
/usage (or /cost, /stats) shows what is left of each quota, with when it resets and whether it
lasts at this pace, what the saved sessions of every project spent today, this week and in the
last 30 days, the models most of it went to and the costliest session; /usage details adds up
requests, tokens, cache hits, cost, days, weeks, providers, models and their speed. On the full
screen it opens in the conversation's place on the current provider's; d shows its details,
Tab (or Left and Right) all of it or another provider's, Escape closes it.
/export [PATH] writes the session's current branch as a web page that runs no script and
loads nothing (by default scramjet-rs-session-<id>.html here), or, to a PATH ending in .jsonl,
as a session file of its own; /import FILE.jsonl continues in a copy of such a file, saved
with this project's sessions. /copy copies the last reply's text, as the model wrote it, to the
clipboard. /release-notes (or /changelog) shows what changed in each version, the newest
first. The keys named here are the defaults: keybindings.json beside the settings file
gives actions others; /hotkeys lists those in use and changes them, and /keybindings opens the
file in your editor. /settings lists the settings, each with what it does, and
changes them; it also shows each command's next steps, publication approval, and what the
session loaded from where, with any problems. /reload reads the settings, command sets, agents,
skills and context files again, and starts the MCP servers whose entries changed or that ended.
/skill:NAME ARGS sends a skill's instructions with ARGS.
/autopilot on|off|status: when on, a finished command's recommended next step runs after 3s.
/mcp shows the settings' MCP servers (mcpServers), each with its tools or why it is not
available; /mcp reconnect [SERVER|all] starts them again, and /mcp enable|disable [SERVER|all]
starts or stops them, saved in disabledMcpServers.
Typed while a turn runs, /status, /usage, /release-notes, /hotkeys, /keybindings, /rename,
/model, /scoped-models, /copy, /export, /settings, /autopilot and /mcp run before its next
request, /reload and /mcp's
changes are refused,
and the other commands stop the turn and run once it has.
Scramjet's names of the commands named otherwise here are accepted too (/new, /quit, /name,
/session, /clone), and so are Claude Code's /reset, /config, /continue and /help, and
Codex's /keymap.
As in Scramjet, /SET:COMMAND ARGS runs a command of a command set: its file
commands/SET:COMMAND.md is sent with the arguments substituted, inside <scramjet-command>. The
sets are those installed, then those under $XDG_DATA_HOME/scramjet (~/.local/share/scramjet)
and the project's .scramjet. `scramjet-rs install DIR` installs the set in DIR by reference,
recorded in the settings' commandSets (~ for the home directory), or, as Pi installs a local
path, DIR as a package in the settings' packages: the sets it holds as a package's clone holds
them, and its skills and themes directories; `remove NAME|DIR` takes them out and `list`
shows the sets loaded in the current directory, what each brings and what was skipped.
`install REPO` clones a package from git (git:github.com/USER/REPO[@REF],
git:git@github.com:USER/REPO, or an https://, ssh://, git:// or file:// URL) into
$XDG_DATA_HOME/scramjet-rs/git, recorded in the settings' packages: its sets are its top
directory when that is a set, else the sets at its top and those with a set.yaml deeper, and
its skills and themes directories come too. `update` pulls the packages not pinned to a REF,
and `remove REPO` deletes the clone.
/compact [focus] replaces all but the last ~20k tokens of the conversation with a summary;
this also happens by itself when the context nears the model's window, unless the settings'
compaction.enabled is false (Auto-compact in /settings).
Settings are read from $XDG_CONFIG_HOME/scramjet-rs/settings.json (~/.config/scramjet-rs/),
with Scramjet's keys: hideThinkingBlock, doubleEscapeAction (tree, fork or none for the double
Escape), branchSummary.skipPrompt (true: /tree never asks for a summary), treeFilterMode
(the entries /tree opens with: default, no-tools, user-only, labeled-only or all) and
defaultThinkingLevel; tools, the agent's coding tools among read, bash, edit, write (the
default) and grep, find, ls; enabledModels, the patterns of the models Ctrl+P cycles through;
defaultProvider and defaultModel, the model chosen last, which /model, Ctrl+P, a next step's
list and the agent save; and startupModel: per-directory (new sessions start with the model
chosen last in their project, kept in $XDG_STATE_HOME/scramjet-rs/last-used-models.json) or
global; retry: enabled, maxRetries (3), baseDelayMs (2000, doubled at each retry) and
provider.maxRetryDelayMs (60000: a longer wait, the provider's or the backoff's, fails the
request; 0 for no limit); compaction: enabled (true), reserveTokens (16384: compaction runs
once the context passes the window less this) and keepRecentTokens (20000, kept whole), and
branchSummary.reserveTokens (16384); images: autoResize (true: images over 2000x2000 pixels or
4.5 MB are scaled down before they are sent) and blockImages (false; true: no image is sent);
and the full screen's scrollWheelStep (3 lines a notch,
1-20), editorMaxHeightPercent (30: the prompt's most rows as a share of the screen's, 10-50) and
autocompleteMaxVisible (5 completion entries shown at once, 3-20), and
terminal.showTerminalProgress (false; true: the terminal's tab shows the agent busy, OSC 9;4).";

/// The session the agent starts in.
pub(super) enum Start {
    New,
    /// `-c`: the project's session written to last.
    Continue,
    /// `-r`: a new one, which a choice of the project's sessions then replaces.
    Resume,
    /// `--session`: a session file, or the start of a saved session's id.
    Session(String),
    /// `--fork`: the same, continued in a new session of the project.
    Fork(String),
    /// `--no-session`: a new one that is not saved.
    Unsaved,
}

/// What the command line asks for.
pub(super) struct Options {
    pub(super) project: Option<PathBuf>,
    pub(super) model: Option<String>,
    /// `--models`: the patterns of the models Ctrl+P cycles through, for the session.
    pub(super) models: Option<Vec<String>>,
    pub(super) effort: Option<String>,
    pub(super) max_output_tokens: Option<u64>,
    pub(super) start: Start,
    /// `--session-dir`: as typed, resolved once the session starts.
    pub(super) session_dir: Option<String>,
    pub(super) prompt: SystemPrompt,
    /// `--skill`: as typed, resolved once the session starts.
    pub(super) skills: Vec<String>,
    /// `--no-skills`.
    pub(super) no_skills: bool,
    /// `--theme`: as typed, resolved once the session starts.
    pub(super) themes: Vec<String>,
    /// `--no-themes`.
    pub(super) no_themes: bool,
    /// `--tools`, `--no-tools` or `--no-builtin-tools`: the only tools of the run.
    pub(super) only_tools: Option<Vec<String>>,
    pub(super) ask: bool,
    /// `--sandbox`: `Some(None)` for `danger-full-access`.
    pub(super) sandbox: Option<Option<SandboxMode>>,
    /// `--retries`, over the settings' `retry.maxRetries`.
    pub(super) retries: Option<u32>,
    /// `-p`: run the messages and exit.
    pub(super) print: bool,
    /// `@FILE` arguments, without the `@`: sent with the first message.
    pub(super) files: Vec<String>,
    /// The other words: prompts to send in order; with `-p`, `-` is read from stdin.
    pub(super) messages: Vec<String>,
    /// `--export FILE`: the session file to write as a page, to the first message if any.
    pub(super) export: Option<PathBuf>,
    /// `--list-models [SEARCH]`.
    pub(super) list_models: Option<Option<String>>,
    pub(super) list_credential_stores: bool,
    pub(super) version: bool,
}

impl Options {
    /// The options `arguments` give; `None` when they ask for `--help`.
    pub(super) fn parse(arguments: impl Iterator<Item = OsString>) -> Result<Option<Self>, String> {
        let mut options = Self {
            project: None,
            model: None,
            models: None,
            effort: None,
            max_output_tokens: None,
            start: Start::New,
            session_dir: None,
            prompt: SystemPrompt::default(),
            skills: Vec::new(),
            no_skills: false,
            themes: Vec::new(),
            no_themes: false,
            only_tools: None,
            ask: false,
            sandbox: None,
            retries: None,
            print: false,
            files: Vec::new(),
            messages: Vec::new(),
            export: None,
            list_models: None,
            list_credential_stores: false,
            version: false,
        };
        let (mut fork, mut no_session) = (None, false);
        let (mut tools, mut no_tools, mut no_builtin_tools) = (None, false, false);
        // The other options naming the session to start in, which `--fork` cannot be given with.
        let mut chosen = Vec::new();
        let mut arguments = arguments.peekable();
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
                Some("--model") => options.model = Some(value()?),
                Some("--models") => {
                    let patterns = value()?.split(',').map(|p| p.trim().to_owned()).collect();
                    options.models = Some(patterns)
                }
                Some(name @ ("--thinking" | "--effort")) => {
                    let level = value()?;
                    let Some(level) = card_level(&level) else {
                        return Err(format!(
                            "invalid {name} {level:?}: valid levels are {}",
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
                    options.retries = Some(value()?.parse().map_err(|_| "invalid --retries")?)
                }
                Some("-c" | "--continue") => {
                    chosen.push("--continue");
                    options.start = Start::Continue
                }
                Some("-r" | "--resume") => {
                    chosen.push("--resume");
                    options.start = Start::Resume
                }
                Some("--session") => {
                    chosen.push("--session");
                    options.start = Start::Session(value()?)
                }
                Some("--fork") => fork = Some(value()?),
                Some("--no-session") => no_session = true,
                Some("--session-dir") => options.session_dir = Some(value()?),
                Some("--system-prompt") => options.prompt.system = Some(value()?),
                Some("--append-system-prompt") => options.prompt.append.push(value()?),
                Some("-nc" | "--no-context-files") => options.prompt.no_context_files = true,
                Some("--skill") => options.skills.push(value()?),
                Some("-ns" | "--no-skills") => options.no_skills = true,
                Some("--theme") => options.themes.push(value()?),
                Some("--no-themes") => options.no_themes = true,
                Some("-t" | "--tools") => {
                    let names = value()?;
                    let names = names
                        .split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty());
                    tools = Some(names.map(str::to_owned).collect())
                }
                Some("-nt" | "--no-tools") => no_tools = true,
                Some("-nbt" | "--no-builtin-tools") => no_builtin_tools = true,
                Some("--export") => options.export = Some(PathBuf::from(value()?)),
                // As Scramjet's, a search is the next word unless it is an option or a file.
                Some("--list-models") => {
                    let search = arguments.next_if(|next| {
                        (next.to_str()).is_some_and(|next| !next.starts_with(['-', '@']))
                    });
                    options.list_models = Some(search.and_then(|search| search.into_string().ok()))
                }
                Some("--list-credential-stores") => options.list_credential_stores = true,
                Some("-v" | "--version") => options.version = true,
                Some("--ask") => options.ask = true,
                Some("-s" | "--sandbox") => {
                    let mode = value()?;
                    options.sandbox = Some(SandboxMode::parse(&mode).ok_or(format!(
                        "invalid --sandbox {mode:?}: valid modes are read-only, \
workspace-write and danger-full-access"
                    ))?)
                }
                Some("-p" | "--print") => options.print = true,
                Some("-h" | "--help") => return Ok(None),
                Some(file) if file.starts_with('@') => options.files.push(file[1..].to_owned()),
                Some(message) if message == "-" || !message.starts_with('-') => {
                    options.messages.push(message.to_owned())
                }
                _ => return Err(format!("unknown argument {argument:?}\n\n{USAGE}")),
            }
        }
        // As Scramjet's, `--tools` wins over the other two.
        options.only_tools = tools
            .or_else(|| no_tools.then(Vec::new))
            .or_else(|| no_builtin_tools.then(|| HARNESS_TOOLS.map(String::from).to_vec()));
        // As Scramjet's, `--fork` cannot be given with the others, `--no-session` included; nor
        // can `--no-session`, where Scramjet's quietly wins over them.
        let alone = match (fork, no_session) {
            (Some(_), true) => {
                return Err(format!(
                    "--fork cannot be combined with --no-session\n\n{USAGE}"
                ));
            }
            (Some(fork), false) => Some(("--fork", Start::Fork(fork))),
            (None, true) => Some(("--no-session", Start::Unsaved)),
            (None, false) => None,
        };
        if let Some((option, start)) = alone {
            if !chosen.is_empty() {
                let chosen = (["--session", "--continue", "--resume"].into_iter())
                    .filter(|option| chosen.contains(option))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(format!(
                    "{option} cannot be combined with {chosen}\n\n{USAGE}"
                ));
            }
            options.start = start;
        }
        if matches!(options.start, Start::Resume) && options.print {
            return Err(format!("--resume cannot be used with -p\n\n{USAGE}"));
        }
        if options.print && options.messages.is_empty() && options.files.is_empty() {
            return Err(format!("-p needs a message or an @FILE\n\n{USAGE}"));
        }
        if !options.print && options.messages.iter().any(|message| message == "-") {
            return Err(format!(
                "the message - (read from stdin) needs -p\n\n{USAGE}"
            ));
        }
        Ok(Some(options))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only_tools(arguments: &[&str]) -> Option<Vec<String>> {
        let arguments = arguments.iter().map(OsString::from);
        Options::parse(arguments).unwrap().unwrap().only_tools
    }

    #[test]
    fn sandbox_takes_codexs_modes() {
        let sandbox = |arguments: &[&str]| {
            let arguments = arguments.iter().map(OsString::from);
            Options::parse(arguments).map(|options| options.unwrap().sandbox)
        };
        assert_eq!(sandbox(&[]), Ok(None));
        assert_eq!(
            sandbox(&["-s", "read-only"]),
            Ok(Some(Some(SandboxMode::ReadOnly)))
        );
        let write = Ok(Some(Some(SandboxMode::WorkspaceWrite)));
        assert_eq!(sandbox(&["--sandbox", "workspace-write"]), write);
        assert_eq!(sandbox(&["-s", "danger-full-access"]), Ok(Some(None)));
        let refused = sandbox(&["-s", "full"]).unwrap_err();
        assert!(
            refused.starts_with("invalid --sandbox \"full\""),
            "{refused}"
        );
    }

    #[test]
    fn tools_options_give_the_only_tools_of_the_run_as_scramjets() {
        assert_eq!(only_tools(&[]), None);
        assert_eq!(
            only_tools(&["-t", " read, grep,,subagent "]),
            Some(vec!["read".into(), "grep".into(), "subagent".into()])
        );
        assert_eq!(only_tools(&["--no-tools"]), Some(Vec::new()));
        let harness = Some(HARNESS_TOOLS.map(String::from).to_vec());
        assert_eq!(only_tools(&["-nbt"]), harness);
        assert_eq!(only_tools(&["-nt", "-nbt"]), Some(Vec::new()));
        for wins in [
            ["-nt", "--tools", "ls"],
            ["--tools", "ls", "--no-builtin-tools"],
        ] {
            assert_eq!(only_tools(&wins), Some(vec!["ls".into()]));
        }
        let missing = Options::parse(["--tools"].into_iter().map(OsString::from));
        assert!(missing.is_err_and(|error| error.starts_with("--tools requires a value")));
    }

    #[test]
    fn no_session_starts_one_not_saved_and_goes_with_no_other_session_option() {
        let parse = |arguments: &[&str]| Options::parse(arguments.iter().map(OsString::from));
        let options = parse(&["--no-session", "-p", "hi"]).unwrap().unwrap();
        assert!(matches!(options.start, Start::Unsaved));
        let refused = |arguments: &[&str]| parse(arguments).err().unwrap();
        let error = refused(&["--fork", "abc", "--no-session"]);
        assert!(error.starts_with("--fork cannot be combined with --no-session\n"));
        let error = refused(&["--no-session", "-c", "--session", "abc"]);
        assert!(error.starts_with("--no-session cannot be combined with --session, --continue\n"));
        let error = refused(&["-r", "--no-session"]);
        assert!(error.starts_with("--no-session cannot be combined with --resume\n"));
        let error = refused(&["-c", "--fork", "abc"]);
        assert!(error.starts_with("--fork cannot be combined with --continue\n"));
    }

    #[test]
    fn skill_options_are_scramjets() {
        let parse = |arguments: &[&str]| {
            let arguments = arguments.iter().map(OsString::from);
            let options = Options::parse(arguments).unwrap().unwrap();
            (options.skills, options.no_skills, options.messages)
        };
        assert_eq!(parse(&[]), (vec![], false, vec![]));
        assert_eq!(
            parse(&["--skill", "a", "-ns", "--skill", "~/b.md", "go"]),
            (vec!["a".into(), "~/b.md".into()], true, vec!["go".into()])
        );
        assert!(parse(&["--no-skills"]).1);
        let missing = Options::parse(["--skill"].into_iter().map(OsString::from));
        assert!(missing.is_err_and(|error| error.starts_with("--skill requires a value")));
    }

    #[test]
    fn theme_options_are_scramjets() {
        let parse = |arguments: &[&str]| {
            let arguments = arguments.iter().map(OsString::from);
            let options = Options::parse(arguments).unwrap().unwrap();
            (options.themes, options.no_themes, options.messages)
        };
        assert_eq!(parse(&[]), (vec![], false, vec![]));
        assert_eq!(
            parse(&["--theme", "a.json", "--no-themes", "--theme", "~/t", "go"]),
            (vec!["a.json".into(), "~/t".into()], true, vec!["go".into()])
        );
        let missing = Options::parse(["--theme"].into_iter().map(OsString::from));
        assert!(missing.is_err_and(|error| error.starts_with("--theme requires a value")));
    }
}
