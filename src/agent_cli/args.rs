//! The command line: its options and the usage text.
use super::settings::{LEVELS, card_level};
use scramjet::agent::{CacheRetention, HARNESS_TOOLS, SandboxMode, SystemPrompt};
use std::{ffi::OsString, path::PathBuf};

/// What an argument error shows after it.
pub(super) const USAGE: &str = "Usage: scramjet [OPTIONS] [@FILE...] [MESSAGE...]
       scramjet COMMAND [ARGUMENTS]

scramjet --help lists the options and commands, scramjet --help-all says everything.";

pub(super) const HELP: &str = "Usage: scramjet [OPTIONS] [@FILE...] [MESSAGE...]
       scramjet COMMAND [ARGUMENTS]

A coding agent for the terminal. In a project's directory, scramjet opens a session on the
full screen, and scramjet -p MESSAGE prints one answer and exits; with no model yet, type
/login in the session. The agent's tool calls run without asking and outside any sandbox
unless --ask, --sandbox or the settings say otherwise.

Sessions:
  -c, --continue           resume the session of this project written to last
  -r, --resume             choose one of this project's sessions to resume
  --session FILE|ID        resume that session
  --fork FILE|ID           continue a copy of that session in a new one
  --no-session             save no session
  --ephemeral              leave nothing behind: no session, and the settings' changes kept
                           for the run only
  --session-dir DIR        save and find sessions in DIR
  --project DIR            the project's directory (default: the current one)
  -p, --print              send the messages, print the replies and exit (a message -
                           reads stdin)
  @FILE                    send a file's text, or an image, with the first message
  MESSAGE                  prompts sent one after another once the session starts

Models:
  --model PATTERN          the model: provider/id, its id, or part of its id or name,
                           with an optional :LEVEL
  --provider NAME          the provider of --model's model
  --models PATTERNS        the models Ctrl+P cycles through, comma-separated
  --thinking LEVEL         reasoning effort (also --effort): off, minimal, low, medium,
                           high, xhigh or max
  --max-output-tokens N    the most tokens a reply may have
  --retries N              resend a request that failed transiently up to N times
  --cache-retention short|long
                           how long the provider keeps the conversation's start cached
  --api-key KEY            the key for the model's provider, for this run only
  --list-models [SEARCH]   list the providers' models and exit

Instructions and tools:
  --system-prompt TEXT     the instructions' opening: TEXT, or the file it names
  --append-system-prompt TEXT
                           text, or a file's contents, after the opening (repeatable)
  -nc, --no-context-files  leave out the AGENTS.md, CLAUDE.md and CLAUDE.local.md files
  --no-directives          leave out Scramjet's own orientation and directives
  --bare                   read nothing from your own directories but the settings,
                           models.json and auth.json
  --skill PATH             a skill file, or a directory of skills (repeatable)
  -ns, --no-skills         no skills but those --skill names
  --theme PATH             a theme file, or a directory of themes (repeatable)
  --no-themes              no themes but the built-in ones and those --theme names
  -t, --tools LIST         the only tools the agent has, comma-separated
  -nt, --no-tools          no tools at all
  -nbt, --no-builtin-tools no coding tools; Scramjet's own stay

What the agent may do:
  --ask                    ask before each command, and each write or edit outside the
                           project
  -s, --sandbox MODE       confine the agent's commands: read-only, workspace-write, or
                           danger-full-access (the default); on Linux and macOS
  --add-dir DIR            let the sandbox's commands change DIR too (repeatable)
  --list-credential-stores list the credential files and directories hidden from subagents
                           and exit
  --offline                no network at start: nothing cloned, no newer release looked for

Other:
  --export FILE [OUTPUT]   write a session file as a web page, or text, and exit
  -v, --version            print the version and exit
  -h, --help               print this help
  --help-all               print each option in full, the keys, the built-in commands and
                           the main settings

Commands:
  install DIR|REPO         install a command set or a package (git:github.com/USER/REPO)
  remove NAME|DIR|REPO     remove one
  update [REPO|NAME|self]  update the packages and the program (self: the program only)
  list                     list the command sets loaded here
  catalog add|remove|list  add, remove or list the model catalogs' URLs
  lint [--strict] DIR...   check command sets as Scramjet loads them
  import [--dry-run]       copy Scramjet 1's setup
  doctor                   check whether Scramjet can work here
  acp                      run as an Agent Client Protocol agent, for editors
Each takes --help.

In a session, / lists the commands, /hotkeys the keys and /settings the settings.
Guides: https://github.com/LeanAndMean/scramjet/tree/main/docs/guide";

/// Which help the arguments ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Help {
    Short,
    All,
}

impl Help {
    pub(super) fn text(self) -> &'static str {
        match self {
            Self::Short => HELP,
            Self::All => HELP_ALL,
        }
    }
}

pub(super) const HELP_ALL: &str =
    "Usage: scramjet [--project DIR] [--provider NAME] [--model PATTERN] [--models PATTERNS]
[--thinking LEVEL] [--max-output-tokens N] [--retries N] [--cache-retention short|long]
[--api-key KEY] [-c | -r | --session FILE|ID | --fork FILE|ID | --no-session] [--session-dir DIR]
[--bare] [--ephemeral] [--system-prompt TEXT] [--append-system-prompt TEXT]... [--no-context-files] [--no-directives]
[--skill PATH]... [--no-skills] [--theme PATH]... [--no-themes] [-t LIST | -nt | -nbt] [--ask]
[-s MODE] [--offline] [-p] [@FILE...] [MESSAGE...]
       scramjet --list-models [SEARCH] | --list-credential-stores | --export FILE [OUTPUT]
       scramjet --version
       scramjet install DIR|REPO | remove NAME|DIR|REPO | update [REPO|NAME|self] | list
       scramjet catalog add URL | remove URL | list
       scramjet lint [--strict] [--json] DIR...
       scramjet import [--dry-run] [DIR]
       scramjet doctor
       scramjet acp [OPTIONS]

Interactive coding agent. Its models are those of the providers in models.json beside the
settings file (by default ~/.config/scramjet/models.json) and in installed command sets:
each provider has a baseUrl (http:// or https://), an api (openai-responses,
anthropic-messages or openai-completions, with its compat), an apiKey (the value of the
environment variable of that name, else the text; !command: what the command prints, run for
each request, as headers' values may be) and its models, with their contextWindow, maxTokens, reasoning, input and cost; a file's
providersCommand names a program printing more, as {\"version\": 1, \"providers\": ...}, and
its credentialFiles the files holding their keys, hidden as --list-credential-stores says,
its engines the earliest Scramjet it works with ({\"scramjet\": \">=X.Y.Z\"}),
and its logins the programs that log in to more providers. The settings' modelCatalogs
add the providers of model catalogs in models.dev's api.json shape, read from their URLs,
saved, and read again as they age (scramjet catalog add URL, remove URL, list). /login adds the models of ChatGPT,
GitHub Copilot and those logins, and of Anthropic, Google Gemini, OpenAI and OpenRouter with an API key (their models
as their servers list them; --api-key with --model anthropic/... for a run), each saved in
auth.json beside the settings. Requests go
through the http:// proxy HTTPS_PROXY or HTTP_PROXY names (http://user:password@host:port),
but to NO_PROXY's hosts and this machine.
  --project DIR          project directory (default: current directory)
  --model PATTERN        model: the one whose provider/id or id PATTERN
                         is (an id two providers offer: the first's), else the newest whose
                         id or name contains it (after provider/, among that provider's),
                         with an optional :LEVEL that sets the thinking level (default: the
                         one a resumed session last used; for a new session, the one last
                         chosen in this project, else anywhere, else
                         gpt-6-luna)
  --provider NAME        the provider whose model --model names: --provider
                         P --model M is --model P/M; without --model it chooses nothing
  --models PATTERNS      the models Ctrl+P cycles through, comma-separated, in place of the
                         settings' enabledModels: each a model's id, part of its id or name,
                         or a glob (claude-*), with an optional :LEVEL; a new session without
                         --model starts with the one last chosen when it is one of them, else
                         with the first, at its level
  --thinking LEVEL       reasoning effort (also --effort): off, minimal, low, medium, high,
                         xhigh or max, the nearest the model takes (default: the
                         one --model's :LEVEL sets, else the one a resumed session last used,
                         else the settings' effortLevel, else medium)
  --max-output-tokens N  the most tokens a reply may have, at most the model's limit (default)
  --retries N            resend a request that failed transiently up to N times (default:
                         the settings' retry.maxRetries, else 10)
  --cache-retention short|long
                         how long providers keep the conversation's start cached for the next
                         request: long asks for an hour where the provider
                         takes it (24 hours on OpenAI-style servers that do), short for
                         their default, 5 minutes on Anthropic's (default: the variable
                         SCRAMJET_CACHE_RETENTION or PI_CACHE_RETENTION, which may also be
                         none, for no caching asked, else long)
  --api-key KEY          the key the requests to the model's provider carry for this run, as it
                         is, in place of /login's and models.json's (other
                         programs of yours can read the command line: a variable models.json
                         names, or /login, keeps it out of sight)
  -c, --continue         resume the session for this project written to last
  -r, --resume           choose one of this project's sessions to resume, as /resume does
  --session FILE|ID      resume a session file (a word with a / or ending in .jsonl), or the
                         newest session of this project whose id (as /status shows it) starts
                         with ID, else of another project, which is forked here once you
                         agree
  --fork FILE|ID         continue a copy of that session, its branches and name included, in a
                         new session of this project
  --no-session           save no session: those of this run (/clear, /fork,
                         /branch and /import too) are kept in a private temporary directory
                         removed at exit, and /resume lists only them
  --bare                 look for nothing in your own directories but the settings, models.json
                         and auth.json, as Claude Code's --bare: no AGENTS.md, CLAUDE.md,
                         SYSTEM.md or APPEND_SYSTEM.md of ~/.config/scramjet or of the home
                         directory, no skills of ~/.config/scramjet/skills or ~/.agents/skills,
                         no agents of ~/.config/scramjet/agents; what the settings list (command
                         sets, skills, MCP servers) and the project's own still load
  --ephemeral            leave nothing behind, as Codex's --ephemeral: no session saved, as
                         with --no-session; the settings' changes, the models last chosen, the
                         output rates, the quotas and the agents approved last for the run only.
                         Logins and the lists of the providers' models are still saved
  --session-dir DIR      save and find sessions in DIR, in place of the project's directory
                         under ~/.local/state/scramjet/sessions (default:
                         the settings' sessionDir); -c, -r, /resume and --session ID look
                         there, whatever project a session there was of
  --system-prompt TEXT   the instructions' opening, in place of the tools and guidelines: TEXT,
                         or the contents of the file it names (default: the project's
                         .scramjet/SYSTEM.md, else ~/.config/scramjet/SYSTEM.md, else
                         the built-in one)
  --append-system-prompt TEXT
                         text, or a file's contents, after the opening; several are joined
                         (default: an APPEND_SYSTEM.md found as SYSTEM.md is)
  -nc, --no-context-files
                         leave out the AGENTS.md, CLAUDE.md and CLAUDE.local.md files of
                         ~/.config/scramjet, the project and the directories above it
  --no-directives        leave out the # Scramjet orientation and Scramjet's directives that
                         follow the context files and skills (also the directives setting
                         off), for sessions given a task and instructions of their own
  --skill PATH           a skill file, or a directory of skills, looked in after the others
                         (repeatable); a relative PATH is from the current directory
  -ns, --no-skills       no skills but those --skill names: none from the skill directories
                         or the settings' skills
  --theme PATH           a theme file, or a directory of themes, for /settings to offer after
                         the others (repeatable); a relative PATH is from the current
                         directory
  --no-themes            no themes but the built-in ones and those --theme names: none from
                         the theme directories or the settings' themes
  -t, --tools LIST       the only tools the agent has in this run, comma-separated:
                         coding tools, in place of the settings' tools, and
                         Scramjet's own (subagent, get_scramjet_user_input, delegate,
                         report_scramjet_command_status…), left out unless named; without
                         report_scramjet_command_status a command cannot finish, and waits
                         for you after each reply
  -nt, --no-tools        no tools at all, unless --tools names some
  -nbt, --no-builtin-tools
                         no coding tools, unless --tools names some; Scramjet's own stay
  --ask                  ask before each bash command and each write or edit outside the
                         project (answer `a` to stop asking until exit); by default tool
                         calls run without asking. Bash is not sandboxed
                         unless --sandbox says so.
  -s, --sandbox MODE     confine the agent's tools, as Codex's modes do (default: the
                         settings' sandbox, else danger-full-access): read-only runs bash with
                         every file read-only, a private /tmp (on macOS /tmp and TMPDIR
                         shared), its own processes and the credential stores hidden, and
                         refuses write and edit;
                         workspace-write lets both change the project but its .git,
                         .scramjet and .agents. Commands reach the network unless the
                         setting sandbox.network is false. It needs bubblewrap (bwrap) and
                         refuses to start without it; on macOS it runs under Seatbelt
                         (sandbox-exec). `!` commands you type and MCP servers stay outside
  --add-dir DIR          let the sandbox's commands, write and edit change DIR too (its
                         .git, .scramjet and .agents kept read-only); repeatable. /sandbox
                         and /add-dir grant more while working. Without a sandbox, keeps
                         DIR readable and writable where blockReadsOutsideWorkingDirectories
                         would hide it
  --offline              no network at start, as SCRAMJET_OFFLINE=1: packages not cloned
                         yet are not cloned, and no newer release is looked for
  --verbose, --mode text, -ne, --no-extensions, -np, --no-prompt-templates
                         taken as Scramjet 1 takes them, changing nothing: the start always
                         shows what it loaded, and Scramjet has no extensions or prompt
                         templates
  -p, --print            run the messages and exit; a message `-` is read from stdin
  @FILE                  send FILE with the first message (or alone): a
                         text file's contents, or a PNG, JPEG, GIF or WebP image, scaled down
                         as read scales it; a relative path is from the current directory
  MESSAGE                prompts sent one after another when the session starts
  --export FILE [OUTPUT] write the session file FILE as a web page to OUTPUT (default:
                         scramjet-session-<its name>.html here), as text to an OUTPUT
                         ending in .txt, as /export does, and exit
  --list-models [SEARCH] list the providers' models (those SEARCH matches) by provider, with
                         their context window, output limit, and whether they reason and
                         take images, and exit
  --list-credential-stores
                         list the credential files and directories that subagents' tools, and
                         the agent's when the hideCredentials setting asks, do not see: the
                         common ones and those models.json files name, and exit
  -v, --version          print the version and exit
  -h, --help             print the short help: each option in a line, and the commands
  --help-all             print this
The session fills the terminal's alternate screen: the conversation above a status line and the
prompt, and under it the project's directory and Git branch, the session's tokens, how full the
context is and the model. Page Up/Down and the mouse wheel scroll the
conversation; Escape goes back to its end.
In the prompt, Enter or Alt+Enter sends; Shift+Enter, Ctrl-J or a trailing \\ starts a new
line; pasted text is inserted without sending. Up/Down recall earlier prompts. Ctrl-G edits the
prompt in $VISUAL or $EDITOR; its text replaces the prompt when the editor exits with 0.
`!command` runs a bash command in the project directory without a model turn and adds its output
to the conversation for the next prompt; `!!command` runs it without adding it.
The prompt stays while a turn runs: Enter then queues a message for the model's next request,
Alt+Enter one for when the turn would end (steeringMode and followUpMode: all or one-at-a-time
at once); Escape interrupts the turn and puts them back in the editor, as Alt+Up does.
Ctrl-C clears the prompt, or on an empty one interrupts a turn; on an empty prompt press it
twice, or Ctrl-D, or type /exit, to quit. Dragging selects text, a double click a word, a triple
click the line;
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
/rewind (/checkpoint, /undo) goes back to before one of the prompts on the way to here, as Claude
Code's: it restores the files write and edit changed since, the conversation as /tree does, or
both (what commands and subagents changed stays); /rewind N [code|conversation] at once.
The lists of /resume, /fork, /tree, /rewind and /model show in the prompt's place and take Up/Down,
Page Up/Down, Home/End, Enter, and Escape or Ctrl-C to cancel; typed text filters the /resume
and /model lists, best match first (a \"quoted phrase\" matches as written, and re:PATTERN is a
case-insensitive regular expression). With piped input they are printed numbered instead, for
/resume N, /fork N, /tree N, /rewind N and /model ID.
In the prompt, Ctrl+Y puts back the text deleted last, Alt+Y then replaces it with what was
deleted before; Ctrl+_ (Ctrl+Shift+-, or Ctrl+- where the terminal lets it through; Ctrl+Z too
on native Windows) undoes, Ctrl+6 redoes; Ctrl+] then a character jumps to it, Ctrl+Alt+]
backward. Ctrl+Z suspends Scramjet elsewhere, and the shell's fg brings it back.
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
/context shows how much of the model's window each part of the context takes (the system
prompt, the tools, MCP's, the context files, the skills', agents' and commands' listings, the
messages), with the free space and the compaction reserve; each part's size is estimated from its
text and scaled to the context the last response reported. On the full screen d shows the items
under each part, and s the system prompt as /system-prompt does.
/system-prompt shows, on the full screen, the instructions the last request sent, each part under
a heading, then its tool definitions; before any request, those the next sends. Up and Down,
PgUp and PgDn, Home and End scroll it, the mouse or Shift and the arrows select, Escape closes it.
/diff shows the Git repository's uncommitted changes against HEAD, untracked files as new ones
(ignored files left out), without a model turn; on the full screen d lists the files changed.
/tasks lists the commands the agent runs in the background (bash's run_in_background, as Claude
Code's), each running or how it ended, with the file its output goes to; /tasks stop N stops one
(/bashes and Codex's /ps too). On the full screen it is a list: Enter shows a task's output, its
end first, or, while it runs, asks whether to show it or stop the task. They run on across turns until they end, are stopped (the agent's
task_stop) or Scramjet exits; the first prompt after one ends tells the agent how it ended.
/export [PATH] writes the session's current branch as a web page that runs no script and
loads nothing (by default scramjet-session-<id>.html here), or, to a PATH ending in .jsonl,
as a session file of its own, in .txt as text, as -p prints it; without a PATH the full
screen asks whether to save the page, copy the text to the clipboard or save it; /import FILE.jsonl continues in a copy of such a file, saved
with this project's sessions. /copy copies the last reply's text, as the model wrote it, to the
clipboard. /release-notes (or /changelog) shows what changed in each version, the newest
first; the first start after an update shows What's new since the version run before. The keys named here are the defaults: keybindings.json beside the settings file
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
/agents lists the agents subagent calls can name, each with where it comes from, the tools and
model its file asks for and any problem in agent files; on the full screen Enter opens one's file.
/skills lists the skills, each with where it comes from and whether the model is offered it, and
any problem in skill files; on the full screen Enter opens one's SKILL.md.
/memory lists the context files read, those not read, and where your AGENTS.md, the project's and
its CLAUDE.local.md would go; on the full screen Enter opens one in your editor, creating it.
/hooks lists the hooks the settings run, by event with their matchers, after any problem in
them, as Claude Code's; on the full screen Enter opens the settings file.
/doctor checks, as scramjet doctor does, the program and the scramjet on PATH, a newer release,
the disk, the settings and what they load, the models, the providers' servers on this machine,
the sandbox, git and the shell, the terminal and how the MCP servers are.
/btw QUESTION asks the model a side question with the conversation so far, as Claude Code's:
no tool runs, the answer shows apart (on the full screen in a view of its own), and the model
never sees the question or the answer again; what it spent is counted with the session.
/recap asks the model the same way where the session stands, in a sentence or two, shown as a
note, as Claude Code's.
/goal CONDITION sets a goal, as Claude Code's: the agent works toward it, and after each turn
that ends by itself the model is asked, as /btw asks, whether the conversation shows it met; until
it does, the reason goes back to the agent and the work goes on. Escape stops it; /goal says the
goal, /goal clear clears it, and a new or resumed session has none.
/plan starts plan mode, as Claude Code's and Codex's: the agent explores and proposes a plan
without changing files (write and edit are refused, and in a sandbox commands and subagents run
read-only), then the full screen asks whether to implement it, which ends plan mode. /plan PROMPT
starts it with that prompt, /plan off ends it.
/loop [INTERVAL] PROMPT sends the prompt now, then again each INTERVAL (30s, 5m, 2h, 1d; 10m when
none is given, and PROMPT every 20m works too) while the full screen waits for you, as Claude
Code's; one due during a turn runs once the turn ends. It lasts while the session runs, at most
7 days; /loop says it, /loop stop ends it.
/effort LEVEL sets the thinking level, as Shift+Tab does, saved as effortLevel; /effort alone
lists the levels the model takes (on the full screen to pick one).
/theme NAME uses a theme and saves it as theme, as /settings → Theme does; /theme alone lists
the themes (on the full screen to pick one, New theme… among them).
Typed while a turn runs, /status, /diff, /tasks, /agents, /skills, /memory, /hooks, /doctor, /btw, /recap, /goal, /loop, /plan, /usage, /context, /system-prompt, /release-notes, /hotkeys, /keybindings, /rename,
/model, /effort, /theme, /autocompact, /scoped-models, /copy, /export, /settings, /autopilot and /mcp run before its next
request, /reload and /mcp's
changes are refused,
and the other commands stop the turn and run once it has.
Scramjet 1's names of the commands named otherwise here are accepted too (/new, /quit, /name,
/session, /clone), and so are Claude Code's /reset, /config, /continue and /help, and
Codex's /keymap.
/SET:COMMAND ARGS runs a command of a command set: its file
commands/COMMAND.md (or SET:COMMAND.md) is sent with the arguments substituted, inside
<scramjet-command>. Agent files may name tools as Claude Code does (Read, Grep, Glob, Bash). The
sets are those installed, then those in the project's .scramjet, then the built-in scramjet
set (/scramjet:troubleshoot, agents and a skill for writing commands) unless one of them is
named scramjet; sets left in Scramjet 1's ~/.local/share/scramjet are not read, said at start. `scramjet install DIR` installs the set in DIR by reference,
recorded in the settings' commandSets (~ for the home directory), or, as Pi installs a local
path, DIR as a package in the settings' packages: the sets it holds as a package's clone holds
them, and its skills and themes directories; `remove NAME|DIR` takes them out and `list`
shows the sets loaded in the current directory, what each brings and what was skipped.
`install REPO` clones a package from git (git:github.com/USER/REPO[@REF],
git:git@github.com:USER/REPO, or an https://, ssh://, git:// or file:// URL) into
$XDG_DATA_HOME/scramjet/git, recorded in the settings' packages: its sets are its top
directory when that is a set, else the sets at its top and those with a set.yaml deeper, and
its skills and themes directories come too. `update` pulls the packages not pinned to a REF,
and `remove REPO` deletes the clone. `update self` replaces the program with the newest
release of LeanAndMean/scramjet (--force: even when it is not newer); the full screen's start
says when one newer than this is out, looking at most once a day (not with --offline). A plain
`update` does both, `update --extensions` only the packages, `--extension
NAME|REPO` one of them, and `--self` only the program. `lint DIR...` checks the sets in DIR as the harness loads
them (`lint --help`). `import` copies Scramjet
1's setup from ~/.scramjet/agent, converted, never over a file here, and lists what it leaves
and why (`import --help`); the first interactive start that finds one offers it.
`acp` runs as an Agent Client Protocol agent for editors and other clients, speaking JSON-RPC
on stdin and stdout (`acp --help`).
/compact [focus] replaces all but the last ~20k tokens of the conversation with a summary;
this also happens by itself when the context nears the model's window, unless the settings'
compaction.enabled is false (Auto-compact in /settings). /autocompact on|off sets it,
/autocompact TOKENS sets compaction.reserveTokens, what is kept free of the window (auto: its
default), and /autocompact alone says where compaction starts for the model.
Settings are read from $XDG_CONFIG_HOME/scramjet/settings.json (~/.config/scramjet/),
with the keys hideThinkingBlock, doubleEscapeAction (tree, fork or none for the double
Escape), branchSummary.skipPrompt (true: /tree never asks for a summary), treeFilterMode
(the entries /tree opens with: default, no-tools, user-only, labeled-only or all) and
effortLevel (else defaultThinkingLevel; Claude Code's alwaysThinkingEnabled: false starts at
off); tools, the agent's coding tools among read, bash, edit, write (the default) and grep, find, ls; enabledModels, the patterns of the models Ctrl+P cycles through;
defaultProvider and defaultModel, the model chosen last, which /model, Ctrl+P, a next step's
list and the agent save (Claude Code's model, an id or alias such as sonnet, read before them
and saved in their place when set); and startupModel: per-directory (new sessions start with the model
chosen last in their project, kept in $XDG_STATE_HOME/scramjet/last-used-models.json) or
global; retry: enabled (true; false: no retry, and a provider's Retry-After is neither kept
for other requests nor waited out), maxRetries (10), baseDelayMs (2000, doubled at each retry up to
provider.maxRetryDelayMs, give or take a tenth) and provider.maxRetryDelayMs (60000: a longer
wait the provider asks for fails the request; 0 for no limit); compaction: enabled (true),
reserveTokens (16384: compaction runs once the context passes the window less this) and
keepRecentTokens (20000, kept whole), each at most a quarter of the window on a local
runtime's model, and
branchSummary.reserveTokens (16384); images: autoResize (true: images over 2000x2000 pixels or
4.5 MB are scaled down before they are sent) and blockImages (false; true: no image is sent);
and the full screen's scrollWheelStep (3 lines a notch,
1-20), editorMaxHeightPercent (30: the prompt's most rows as a share of the screen's, 10-50) and
autocompleteMaxVisible (5 completion entries shown at once, 3-20),
terminal.showTerminalProgress (on in Windows Terminal, ConEmu, Ghostty 1.2+ and iTerm2 3.6.6+,
off elsewhere and inside tmux: the terminal's tab shows the agent busy while it works, OSC 9;4;
Claude Code's terminalProgressBarEnabled is read too) and Claude Code's preferredNotifChannel
(auto, terminal_bell, iterm2, iterm2_with_bell, kitty, ghostty or notifications_disabled: how
the terminal is told the agent waits for you, a minute after a reply or 6 seconds into a
question; auto: iTerm2's, kitty's or Ghostty's notification, none elsewhere); Claude Code's env,
environment variables set at start, over the shell's, for Scramjet and what it runs.";

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
    /// `--provider`: the provider `--model` names one of the models of.
    pub(super) provider: Option<String>,
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
    /// `--bare`.
    pub(super) bare: bool,
    /// `--ephemeral`.
    pub(super) ephemeral: bool,
    /// `--theme`: as typed, resolved once the session starts.
    pub(super) themes: Vec<String>,
    /// `--no-themes`.
    pub(super) no_themes: bool,
    /// `--tools`, `--no-tools` or `--no-builtin-tools`: the only tools of the run.
    pub(super) only_tools: Option<Vec<String>>,
    pub(super) ask: bool,
    /// `--sandbox`: `Some(None)` for `danger-full-access`.
    pub(super) sandbox: Option<Option<SandboxMode>>,
    /// `--add-dir`: directories the sandbox makes writable too.
    pub(super) add_dirs: Vec<String>,
    /// `--retries`, over the settings' `retry.maxRetries`.
    pub(super) retries: Option<u32>,
    /// `--cache-retention`, over the variables.
    pub(super) cache_retention: Option<CacheRetention>,
    /// `--api-key`.
    pub(super) api_key: Option<String>,
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
    /// Sets the run apart as `--bare` and `--ephemeral` ask, before anything is read or kept.
    pub(super) fn isolate(&self) -> Result<(), String> {
        if self.bare {
            scramjet::agent::set_bare();
        }
        if self.ephemeral {
            let failed = |error| format!("--ephemeral: no private temporary directory: {error}");
            scramjet::agent::keep_records_apart().map_err(failed)?;
        }
        Ok(())
    }

    /// The options `arguments` give, else the help they ask for.
    pub(super) fn parse(
        arguments: impl Iterator<Item = OsString>,
    ) -> Result<Result<Self, Help>, String> {
        let mut options = Self {
            project: None,
            model: None,
            provider: None,
            models: None,
            effort: None,
            max_output_tokens: None,
            start: Start::New,
            session_dir: None,
            prompt: SystemPrompt::default(),
            skills: Vec::new(),
            no_skills: false,
            bare: false,
            ephemeral: false,
            themes: Vec::new(),
            no_themes: false,
            only_tools: None,
            ask: false,
            sandbox: None,
            add_dirs: Vec::new(),
            retries: None,
            cache_retention: None,
            api_key: None,
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
                Some("--provider") => options.provider = Some(value()?),
                // Scramjet 1's, with nothing to change here: the start always shows what it
                // loaded, and there are no extensions or prompt templates to leave out.
                Some("--verbose" | "-ne" | "--no-extensions" | "-np" | "--no-prompt-templates") => {
                }
                Some("--mode") => match value()?.as_str() {
                    "text" => {}
                    "json" => {
                        return Err(
                            "--mode json is not in Scramjet yet; -p prints the replies as text"
                                .to_owned(),
                        );
                    }
                    "rpc" => {
                        return Err(
                            "--mode rpc: Scramjet's RPC mode is `scramjet acp`, the Agent \
Client Protocol on stdin and stdout (`scramjet acp --help`)"
                                .to_owned(),
                        );
                    }
                    mode => {
                        return Err(format!(
                            "invalid --mode {mode:?}: valid modes are text, json and rpc"
                        ));
                    }
                },
                Some(name @ ("-e" | "--extension" | "--prompt-template")) => {
                    return Err(format!(
                        "{name}: Scramjet runs no TypeScript extensions or prompt templates; \
command sets and skills take their place (`scramjet import --dry-run` says what of Scramjet 1's \
setup it leaves)"
                    ));
                }
                Some(name @ ("--model-definition" | "--model-definition-env")) => {
                    return Err(format!(
                        "{name} is not in Scramjet: add the provider and its models to \
models.json (see --help)"
                    ));
                }
                Some("--api-key") => {
                    let key = value()?;
                    // Never repeats the key.
                    if key.is_empty() || !key.chars().all(|ch| ch.is_ascii_graphic()) {
                        return Err("--api-key: a key is letters, digits and punctuation, \
without spaces"
                            .to_owned());
                    }
                    options.api_key = Some(key)
                }
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
                // As Scramjet 1's: `none` only from the variables.
                Some("--cache-retention") => {
                    let given = value()?;
                    let retention = CacheRetention::parse(&given)
                        .filter(|retention| *retention != CacheRetention::None);
                    let Some(retention) = retention else {
                        return Err(format!(
                            "invalid --cache-retention {given:?}: valid values are short and long"
                        ));
                    };
                    options.cache_retention = Some(retention)
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
                Some("--bare") => options.bare = true,
                Some("--ephemeral") => options.ephemeral = true,
                Some("--session-dir") => options.session_dir = Some(value()?),
                Some("--system-prompt") => options.prompt.system = Some(value()?),
                Some("--append-system-prompt") => options.prompt.append.push(value()?),
                Some("-nc" | "--no-context-files") => options.prompt.no_context_files = true,
                Some("--no-directives") => options.prompt.no_directives = true,
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
                Some("--add-dir") => options.add_dirs.push(value()?),
                Some("-p" | "--print") => options.print = true,
                Some("-h" | "--help") => return Ok(Err(Help::Short)),
                Some("--help-all") => return Ok(Err(Help::All)),
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
        let unsaved = match options.ephemeral {
            true => "--ephemeral",
            false => "--no-session",
        };
        let alone = match (fork, no_session || options.ephemeral) {
            (Some(_), true) => {
                return Err(format!(
                    "--fork cannot be combined with {unsaved}\n\n{USAGE}"
                ));
            }
            (Some(fork), false) => Some(("--fork", Start::Fork(fork))),
            (None, true) => Some((unsaved, Start::Unsaved)),
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
        Ok(Ok(options))
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
    fn no_session_and_ephemeral_start_one_not_saved_and_go_with_no_other_session_option() {
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
        let options = parse(&["--ephemeral", "--bare"]).unwrap().unwrap();
        assert!(matches!(options.start, Start::Unsaved) && options.ephemeral && options.bare);
        let error = refused(&["--ephemeral", "-c"]);
        assert!(error.starts_with("--ephemeral cannot be combined with --continue\n"));
        let error = refused(&["--fork", "abc", "--ephemeral"]);
        assert!(error.starts_with("--fork cannot be combined with --ephemeral\n"));
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

    #[test]
    fn the_short_help_names_each_option_the_full_one_describes() {
        // Scramjet 1's options that change nothing are left to the full help.
        let quiet = "  --verbose, --mode text";
        let named = |text: &str| -> Vec<String> {
            (text.lines())
                .filter(|line| line.starts_with("  -") && !line.starts_with(quiet))
                .filter_map(|line| line[2..].split("  ").next())
                .flat_map(|column| {
                    column
                        .split([' ', ','])
                        .filter(|word| word.starts_with('-'))
                })
                .map(String::from)
                .collect()
        };
        let (short, full) = (named(HELP), named(HELP_ALL));
        assert!(full.len() > 40, "{full:?}");
        for option in &full {
            assert!(short.contains(option), "{option} is not in the short help");
        }
        for option in &short {
            assert!(full.contains(option), "{option} is not in the full help");
        }
        for (arguments, help) in [(&["-h"], Help::Short), (&["--help"], Help::Short)] {
            let parsed = Options::parse(arguments.iter().map(OsString::from));
            assert_eq!(parsed.unwrap().err(), Some(help));
        }
        let parsed = Options::parse(["-p", "--help-all", "x"].into_iter().map(OsString::from));
        assert_eq!(parsed.unwrap().err(), Some(Help::All));
        let width = |text: &str| text.lines().map(|line| line.chars().count()).max();
        assert!(width(HELP) <= Some(92), "{:?}", width(HELP));
    }
}
