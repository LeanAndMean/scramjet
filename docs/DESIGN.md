# Design direction

Established 2026-10-08 from the founding discussion. This file owns product direction and essential invariants; source and executable evidence own implemented behavior. Proposed mechanisms below remain revisable as demonstrations produce evidence.

## Agreed scope

- Build an independent Rust product under Apache-2.0, not a Rust port of the Pi package structure.
- Make Ubuntu Linux, macOS, and WSL Ubuntu first-class. Native Windows is in daily use (a colleague and soon others; user, 2026-11-09), so its fixes come first; macOS follows, then the scale work. The Raspberry Pi 5 (64-bit Linux on aarch64) is supported too (user decision, 2026-11-09).
- Improve current usage first: independent project tabs, a main agent, and efficient parallel subagents.
- Preserve valuable Scramjet outcomes: coding tools, reusable commands/agents/skills, project instructions, command composition and continuation, graduated autonomy, informed approval, model selection, and useful session continuity.
- Keep the retained alternate-screen experience: browse unfinished output, edit while browsing, expand tool/agent details, and copy selected text without disrupting live work.
- Investigate approximately 8,000 simultaneous LLM requests from one node. Queued tasks, open requests, engine-scheduled requests, and completed-request throughput are separate measurements.
- Actively manage provider concurrency and rate limits. On-prem vLLM behind a reverse proxy is a first-class deployment scenario, not the only provider type.
- Extensions are packages of data and separate executables ([packages](#packages-providers-and-mcp-2026-11-04)). No Python, TypeScript, Pi extension API, or Pi ecosystem compatibility is required.
- PromptSwarm is a reference for scale ambition, not a workflow model to port or a runtime dependency.

## Current direction (2026-11-03)

Make scramjet-rs a rock-solid daily driver with Scramjet's features, its retained full-screen UI included, on a small, well-organized codebase. Build beyond Scramjet (admission and bounds for many children, children that write, capacity) only after that (user decision, 2026-11-03); the scale part is now cheap, isolated ACP sessions ([design](#scale-through-acp-sessions-and-an-external-runner-steer-of-2026-11-23), steer of 2026-11-23; its dispatch backend deferred at the steer of 2026-11-24). Platforms come before that scale work: native Windows, in daily use since 2026-11-09, then macOS in Terminal.app and in xterm (Homebrew's, under XQuartz). That scale work stays lower priority, but every design keeps it in mind, so that nothing built now corners it (user, 2026-11-06). The first order (demos removed, test-only models' code reviewed, then Scramjet's features, usability first) is done; [the task list](TASKS.md) holds today's.

Approvals (user decision, 2026-10-30, replacing the same day's read-only command list): tool calls run without approval questions by default, as in Scramjet. `--ask` opts in to questions (every shell command, writes and edits outside the project, `a` to allow all until exit) for when the user wants to watch. Approval questions and command allowlists are not containment: the shell can do anything, and a question shows a command, not what it does. Recoverability comes from Git and clones or worktrees; real confinement needs an OS sandbox (Landlock or bubblewrap on Linux, Seatbelt on macOS), planned with parallel subagents and unattended runs, and offered for the main agent too ([sandbox](#a-sandbox-for-the-main-agent-2026-11-06)).

Approval checks serve the user's choices (user decision, 2026-11-09). Publication approval and command autonomy let the user choose what runs without asking; they are not a defence against a malicious repository, command set or agent, which could go around them. The harness is not made harder to use for safety it cannot guarantee: a project's command set may auto-approve its own publications, as in Scramjet. Where an OS sandbox gives a real guarantee, that guarantee holds and comes first.

Scramjet (`~/repos/scramjet`) and the company's local proxy are the working reference for protocol and tool behavior. A limit or check needs a concrete basis: a documented contract, a user decision, a real safety risk, or an observed failure. "No document guarantees X" is not a reason to reject X. When a self-imposed rule blocks the real system, compare with Scramjet and loosen or remove it. Where Scramjet's behavior comes from its history rather than its design (command files registered as Pi prompt templates, `/scramjet settings` apart from `/settings`), do better (user decision, 2026-11-03).

Familiar names (user decision, 2026-11-04). The UI should be no less familiar than Claude Code's or Codex's. Where they and Pi/Scramjet name or bind the same action differently, the name their users know is the one listed and documented, and the other names a user would type stay accepted without being listed, so that typing one never sends it to the model as a prompt. `/clear` is listed, with `/new` and `/reset` accepted (both Claude Code and Codex understand `/new`). By the user's choice, `/settings` stays listed over Claude Code's `/config`, which is accepted.

On the local proxy's route, the proxy keeps authentication: scramjet-rs talks to it as to OpenAI and Anthropic endpoints, as Scramjet does through its `models.json`, and never changes the proxy's state. scramjet-rs's own logins ([providers](#packages-providers-and-mcp-2026-11-04)) store only their own credentials, in its own file; it never reads other tools' credential stores. Missing usage is recorded as unknown, not zero, and local cancellation is not proof that remote generation or billing stopped.

### Agreed product shape (2026-11-03)

User decisions from `/rs:steer` on 2026-11-03:

- **Interface.** `scramjet` itself is the agent (`scramjet`, `scramjet -p …`, `--continue`, as Scramjet 1's CLI), with no `agent` subcommand or demo commands. The retained full-screen transcript becomes the main interactive UI, as Scramjet's default `retained` mode: an editor that stays visible during a turn (Enter queues a steering message, Alt+Enter a follow-up; `steeringMode` `all` and `followUpMode` `one-at-a-time` by default, as in Scramjet), tool output and summaries expanded in place with Ctrl+O, mouse-wheel scrolling and the UI's own selection and copy; there is one renderer. Commands that act on the harness rather than the agent (`/settings`, `/autopilot`, `/model` …) run at once while a turn runs, as Scramjet's do, and never queue as messages, so that changing a setting never means interrupting the agent (user, 2026-11-06). A next step's selector shows which steps start a fresh session, and a key flips that for the selected step before it runs (user, 2026-11-06).
- **Command sets are their own concept**, not Pi prompt templates: a set brings commands with goals, a lifecycle, next-step policies and delegation, and its agents. Scramjet behavior that exists only because it registers command files as prompt templates (such as `delegate-only` commands staying typeable and in the `/` list) is not binding.
- **One source of truth for command sets.** A set is installed by reference from a location (`install`, `remove`, `list`; recorded in the settings, not copied), as Pi installs a local path. Where sets live: [command sets' home](#command-sets-home-and-scramjets-sunset-2026-11-09). An agent named `<set>:<name>` belongs to the installed set `<set>`: a loose file claiming that namespace is skipped with a warning. `# Available subagents` lists command-set agents, as Scramjet's does.
- **Settings.** scramjet-rs has its own settings file, kept apart from Scramjet's, with Scramjet's key names where the behavior matches (`hideThinkingBlock`, `doubleEscapeAction`, `steeringMode` …) until Pi's shape is in the way ([Scramjet 2](#scramjet-2-and-a-clean-history-2026-11-10)). One `/settings` covers everything, command autonomy and publication approval included; there is no separate `/scramjet settings`.
- **Command autonomy must be understandable at a glance** by someone who doesn't know the concepts, through its layout and wording rather than explanatory text (Scramjet's menu confuses everyone but its designer).
- **What is installed can be explored** in the TUI: command sets, commands, agents (from sets, the user's directory, the project), skills, context and settings files, each with its source and any problem (broken or stale links, skipped or shadowed files, warnings), so the user can find and fix them. At launch the `header` banner, the default since 2026-11-06, lists what was loaded, as Scramjet's does; this replaces the earlier rule that launch shows nothing about it.
- **Credentials.** A setting decides when the main agent's tools run with known credential stores hidden: `never` by default, `unattended` (chained or forced next steps, `-p`) or `always` for users who want it (user decision, 2026-11-11: hiding them by default was a nuisance, `gh` and `git` over SSH failing in loop turns, not an asset). A command that fails for want of a hidden store is told why, so that the agent raises a real need with the user or takes it as a sign it went out of scope. Subagents always run with them hidden. On native Windows, which has no sandbox in scramjet-rs yet, an unattended turn's shell commands run unhidden with a warning, rather than refused (user decision, 2026-11-09); subagents' at the low integrity level, read-only on local disks but not on WSL's files or network drives, with nothing hidden, said once (user decision, 2026-11-10).
- **Tools** (user decision, 2026-11-09, replacing 2026-11-03's Scramjet defaults). All seven tools are on by default (read, bash, edit, write, grep, find, ls), and `grep` and `find` are built in, with nothing to install, rather than running ripgrep and fd. `tools` in `/settings` narrows the list. A subagent whose file has no `tools:` takes the read-only ones of that list, as Claude Code's inherit; `disallowedTools` removes some, from that list or its own.
- **Shells on Windows** (user decision, 2026-11-09). Native Windows has three shell tools, each named for its shell and saying when to use it: `bash` (Git for Windows, else MSYS2 or Cygwin, never WSL's) and `powershell` (`pwsh` 7, else Windows PowerShell 5.1, its description naming the edition and its limits), on by default when found, and `cmd` (batch files, tools that expect it), listed in `tools` but off by default. Git for Windows is optional: without it `bash` is off and each launch says so, naming it. A Windows-only setting chooses the shell of `!` commands; `models.json` `!command` values run in bash when found, else PowerShell. No `-EncodedCommand` or `-ExecutionPolicy Bypass`. Other systems have only `bash`.
- **Typing an answer of your own** (steer of 2026-11-20, third). A question the model asks with options (`select` and `confirm`) always ends with a "Type something." row, as Claude Code's: choosing it opens a text field there, and what the user types goes back to the model as the answer, in a field of its own, and the turn goes on. Escape still cancels and ends the turn. The tool's description says the row is always there, so the model adds no "Other" option of its own. An ACP client with forms gets an optional text field beside the options. Codex's notes on an option are not built until missed.
- **Project agents.** Their approval is remembered per project and asked again when a project agent file is added or changed.
- **Command lifecycle** (second steer of 2026-11-03, after the user's decisions of 2026-10-30 for Scramjet's issue #638, which Scramjet has not built). A started command keeps going until it reports `completed` (its part is over, not that the task succeeded), the user confirms abandoning it, or the user starts another command or types an unknown slash command. Any other stop (Escape, a cancelled or freetext question, a failed response) leaves it waiting for the user, whose next message resumes it. A reply ending without a report gets a short status check, at most 3 in a row without the user's input, then the command waits with a notice. `abandoned` ends the command with no next step, not even a forced one, and only once the user confirms after reading its summary. There is no `blocked`, `incomplete` or `continuing`; Mach 12's text naming them gets a result steering the model to keep working, ask or abandon.

### Look and themes (2026-11-04)

User decisions of 2026-11-04 and 2026-11-06 on the scrollbar, the editor's box, themes, a distinct look (`panels` and the `header` banner by default; `tinted` and the `bar` bottom since the second steer of 2026-11-20) and the theme editor, and of 2026-11-19 on developing every look to panels' level, with new ones, and designs for the bottom of the screen: [THEMES.md](THEMES.md).

### Copy, paste and keys (2026-11-04)

Built; moved to [DECISIONS.md](DECISIONS.md#copy-paste-and-keys-2026-11-04) on 2026-11-27 for room: clipboard built in with nothing to install, nothing that alarms endpoint security, no OS-level input monitoring, copy-on-select on macOS, key remapping (`/hotkeys`), selection keys as in an editor, effort levels as they are.

### Packages, providers and MCP (2026-11-04)

Built; moved to [DECISIONS.md](DECISIONS.md#packages-providers-and-mcp-2026-11-04) on 2026-11-27 for room: packages as the extension model, their code only in separate executables, their programs from their releases; no code for the company's proxy in the public core; one provider boundary, Copilot and ChatGPT first; model lists from the providers; no Claude subscription login; Copilot's `X-Initiator`; requests name Scramjet; `/model` lists only providers in use; MCP in the core with progressive disclosure.

### Usage (2026-11-06)

Built; moved to [DECISIONS.md](DECISIONS.md#usage-2026-11-06) on 2026-11-27 for room: `/usage` shows what is used and left, at a glance (the approved sketch), the rest behind a details key.

### Session chains (2026-11-05)

Built; moved to [DECISIONS.md](DECISIONS.md#session-chains-2026-11-05) on 2026-11-27 for room: fresh-session next steps save `parentSession`, `/resume` shows chains flat, threaded or folded, earlier sessions shown above for display only.

### A sandbox for the main agent (2026-11-06)

Built; moved to [DECISIONS.md](DECISIONS.md#a-sandbox-for-the-main-agent-2026-11-06) on 2026-11-27 for room: `--sandbox read-only|workspace-write` with network a setting of its own; MCP servers start outside it; protected paths and Unix sockets as Codex's (`.claude` no longer protected since the [scope reset](#scope-reset-steer-of-2026-11-27)).

### Hand-offs and going back (2026-11-07)

Built; moved to [DECISIONS.md](DECISIONS.md#hand-offs-and-going-back-2026-11-07) on 2026-11-27 for room: `Waiting for you` whenever the turn is with the user; going back reopens the choice that was offered there.

### RPC mode and off-device front ends (2026-11-09)

User decisions of 2026-11-09 to 2026-11-16, moved to [RPC.md](RPC.md#the-user-decisions-it-rests-on) on 2026-11-17 for room: RPC mode speaks ACP; approvals, questions and sign-in go over it, credentials never out; one core for the full screen and RPC mode; no listening port, server or daemon in the terminal binary; the dashboard (Flightdeck) in a repository of its own, local until the user asks, terminal-like, its overview showing running and waiting sessions first.

### Command sets' home and Scramjet's sunset (2026-11-09)

Built; moved to [DECISIONS.md](DECISIONS.md#command-sets-home-and-scramjets-sunset-2026-11-09) on 2026-11-27 for room: set files named `commands/<command>.md` and `agents/<agent>.md`; Mach 12 in its own repository; the `scramjet` set, its skill and the lint in this repository.

### Releases, updates and going public (2026-11-09)

Built; moved to [DECISIONS.md](DECISIONS.md#releases-updates-and-going-public-2026-11-09) on 2026-11-27 for room: releases for testing on every platform, only for a version the user asks for, public as alphas; `update` and `update self` as Scramjet's; "what's new" and `/changelog`.

### Scramjet 2 and a clean history (2026-11-10)

Built; moved to [DECISIONS.md](DECISIONS.md#scramjet-2-and-a-clean-history-2026-11-10) on 2026-11-27 for room: scramjet-rs becomes Scramjet 2 in a new public `LeanAndMean/scramjet`; versions 2.x.y; v2 owns its layout and formats (`scramjet`, `~/.config/scramjet`, `.scramjet/`); upgrading from Scramjet 1; the changelog; a history with nothing of the company in it.

### Leaving alpha (steer of 2026-11-25)

User decisions:

- **The next release, or the one after, leaves alpha,** once the user has signed off the new functionality; the user says which. Second steer of 2026-11-25: the next is one more alpha, for the user to try; the stable one follows their sign-off. Second steer of 2026-11-27: the next release, carrying the scope reset and what followed it, is again an alpha. Features nobody here can try for real (the API-key providers: no keys) ship saying so, and alpha users' tries show whether they work. The platform gate is met: macOS is used by several people (their reports led to fixes, such as the private package's releases building for macOS), the Raspberry Pi 5 has run the last several releases for one to two weeks, and three people use Windows daily. npm's packages are dropped as a requirement: the one-line installers and `scramjet update` serve every platform without npm's `sudo` and configuration trouble.
- **No freeze and no stabilisation stage:** features and designs keep evolving, through items the user agrees at a steer ([scope](#scope-reset-steer-of-2026-11-27)). What stable promises instead: a change to a setting, key id, file format or command-set format keeps reading the old form, or converts it, rather than refusing it, so that an update never breaks a user's files. `delegate`'s name and which looks and bottoms stay change later on those terms.
- **One stream:** no alphas after the stable release, and no alpha or canary channel; releases are normal GitHub releases and `update self` takes the newest.
- **The product is Scramjet,** version 2, not branded "Scramjet 2": the README, guides, `--help`, the program's messages and release titles say Scramjet, naming Scramjet 1 only where telling them apart matters (upgrading, running both).
- **What the stable release needs, built ahead of other work:** a README aimed at new users (what it is and why, screenshots from `scripts/screens.py`, a short section per feature linking to its guide, saying up front that tool calls run without asking and without a sandbox by default and how to change that, a truthful status, written for someone who never used Scramjet 1); a getting-started guide (install, connecting a model by each path, a first task, steering, queueing, Escape and undo, resuming and branching, installing Mach 12 and running a workflow); a concepts page (commands, sets, agents, skills, next steps and their policies, autopilot, delegation) with a first command set as a tutorial; built-in API-key providers (Anthropic, OpenAI, OpenRouter, Google Gemini: the key entered in `/login`, the models from the provider's own list, so that a key needs no hand-written `models.json`; the first run's message says so); `SECURITY.md` (how to report, what the sandbox guarantees and what it does not) and a privacy note (what leaves the machine, checked in the code, and where data is kept and how to delete it); uninstalling and troubleshooting; the compatibility promise above; `release.yml` publishing a normal release titled without "(alpha)" from the stable tag, and "alpha" gone from the README and guides.
- **After it, as ordinary work:** the reference guides in short sections, tables and examples, with a settings reference; a short `--help` with the full one behind it.
- **Not now:** issue templates, a contributor guide or outside contributions, Discussions, a homepage or social preview.

### Scope reset (steer of 2026-11-27)

On 2026-11-25 and 26 the loop, its queue waiting on the user, built about 70 features no steer had asked for (2.350.0–2.418.0), most taken from Claude Code and Codex. The user went through each. Their principles, for what comes next:

- **Scope comes from the user.** A new user-facing feature is built only from an item agreed at a steer ([CLAUDE.md](../CLAUDE.md)). What other harnesses have is evidence for a steer, not a backlog; "no freeze" ([above](#leaving-alpha-steer-of-2026-11-25)) means agreed features keep coming.
- **No friction for security that cannot be guaranteed.** The OS sandbox contains; permission rules (allow, ask and deny lists, command patterns) do not: a capable model gets past them, they are opaque and hard to audit, and they keep stopping the agent for trivial things. `--ask` stays opt-in, as it is.
- **What the agent can do through bash needs no feature of its own:** a subagent in the background (`scramjet -p` through bash; native ones let the main agent move on before they finish, as when one posted a PR review before half its reviewers were done), a command in the background, a session in a new worktree, reading a web page (`curl`), a diff (`!git diff`).
- **Nothing frontier models no longer need:** todo tools (Claude Code withdrew them on newer models, which keep track without a checklist while the tools take context), plan mode, commands that only wrap a prompt (`/init`, `/review`, `/security-review`: asking does the same), creation wizards (asking the agent is faster).
- **Few slash commands.** One few would use, or one that a `/settings` row already does, goes.
- **Names show their source.** The `/` list shows a set's or a plugin's command as `/SET:COMMAND` and a skill as `/skill:NAME`; typing the bare name filters to it, and Tab takes it. Nothing runs by a bare name that hides where it comes from, which would erode trust in what is chosen (2.399.0's skills as `/NAME` go). Scramjet's built-ins, and the built-in set's `/bug` (2026-11-17; confirmed at the second steer of 2026-11-27), keep their names.
- **No unbounded copies of the user's files.** A feature that saves the content of files the agent changes (as `/rewind` kept a copy of each file it replaced, never removed, however large) can fill the disk; it needs a bound and a cleanup, or it does not ship. Sessions, logs and the other records Scramjet exists to keep are not meant here.

Out, removed from `main`: Claude Code's permission options (`--permission-mode`, `--dangerously-skip-permissions`, `--allowedTools`, `--permission-prompts`), `--disallowedTools`, `--settings`, `--mcp-config`, `--agents`, `--fork-session`; hooks and `/hooks`; background commands and `/tasks`; `todo_write`; `web_fetch`; plan mode; `/rewind`; `/btw`, `/recap`, `/color`, `/autocompact`, `/diff`, `/doctor`, `/init`, `/review`, `/security-review`; the `/agents`, `/memory` and `/skills` lists (`/settings` → Installed shows them); `/copy N` and its list; `.txt` export; the status line script; the notifications when the agent waits and the title's ○/● (the progress indicator, OSC 9;4, does it better); Ctrl+R's prompt search (the keys decision of 2026-11-16 stands); the `env` setting. Not built: native background subagents, a session in a new worktree, vim mode. Not a goal: drop-in compatibility with `claude -p`'s options.

Kept: `grep`, `find` and `ls` built in (they work without ripgrep, on Windows, and with bash off); `/copy` (every harness has it, and line mode has no selection of Scramjet's); `/effort` (for those who don't know Shift+Tab) and `/theme` (`/settings`' themes); the settings keys by Claude Code's names (`effortLevel`, `model`, `alwaysThinkingEnabled`); headless JSON in Claude Code's form (`--output-format json|stream-json`), with `--max-turns`, `--max-budget-usd`, `-n`/`--name`, `--resume ID`, `--plugin-dir` and `--no-session-persistence`. `/loop` and `/goal` stay; the user has not tried them yet (second steer of 2026-11-27).

Agreed designs:

- **Copying a selection as markdown** (a setting, on by default): a selection in a reply copies that part of its markdown source, the markers within it included (`**`, backticks, link targets), and a block selected whole with its own (fences with the language, bullets, `#`, `>`, table pipes); prompts and tool output copy as shown. Off: the rendered text, as before.
- **Other harnesses' files, recognized but off by default:** a toggle each in `/settings`, its row saying what was found: Claude Code's skills, its commands (whose `!` lines run when typed), its instructions (`~/.claude/CLAUDE.md`, rules, `.claude/CLAUDE.md`, `@` imports), its agents, its enabled plugins, and its memory of the project (`~/.claude/projects/<project>/memory/MEMORY.md`, read only). Each command, skill and agent shows by its source's name. The first start that finds any says once where to turn them on; no import command. A project's `CLAUDE.md`, `CLAUDE.local.md`, `AGENTS.md` and `.agents/skills` load as before. The sandbox no longer keeps `.claude` read-only. Other harnesses' files when someone asks.
- **Memory:** today the agent reads past sessions (the prior-session fallback, and the `reading-past-sessions` skill describing Scramjet's and other harnesses' session formats). No memory the agent writes until a design is clearly worth its complexity. Other harnesses' memories are read through their toggle; the agent never changes them on its own, and when the user asks it to, only the sandbox's settings limit it.
- **Going back without a question:** in `/tree` (Escape twice), Enter goes back with no summary; two other keys, shown in its hints, summarize, or summarize with the user's instructions.
- **Escape before any reply** puts the prompt back in the editor and takes it out of the conversation, as Claude Code does since 2.1.83. Before any reply: no text or tool call saved yet. Any prompt is taken back, not only typed ones: a next step's, a `/loop` run's and a `/goal` continuation's too (confirmed at the second steer of 2026-11-27).
- **Subagent worktrees** (`isolation: worktree`, item 77) stay as built (second steer of 2026-11-27).

### Ahead of RPC mode (2026-11-11)

User decisions of the second steer of 2026-11-11, all built, moved to [DECISIONS.md](DECISIONS.md#ahead-of-rpc-mode-2026-11-11) on 2026-11-17 for room: the progress indicator, other harnesses' sessions readable, write access granted while working, new command-set repositories, the linter's gaps, local models in `/login`, provider-only packages under `[Providers]`.

### Local models' scope (2026-11-12)

Built; moved to [DECISIONS.md](DECISIONS.md#local-models-scope-2026-11-12) on 2026-11-27 for room: no catering to one model's quirks; Qwen 3.8 27B the reference local model; models are untrusted and new ones tried only with the user's approval, sandboxed; what stayed of 2.168–2.191 and what went.

### Providers from a model catalog (2026-11-16)

Built; moved to [DECISIONS.md](DECISIONS.md#providers-from-a-model-catalog-2026-11-16) on 2026-11-27 for room: providers and models from a catalog URL in models.dev's shape, kept in Scramjet's settings; a package stays independent of a proxy it also reaches; no more of Claude Code's and Codex's keys adopted; installing Scramjet from a proxy's setup.

### Agents, the sandbox's reach and model changes (2026-11-17)

Built; moved to [DECISIONS.md](DECISIONS.md#agents-the-sandboxs-reach-and-model-changes-2026-11-17) on 2026-11-27 for room: model changes the agent did not make marked as Codex and Claude Code do; the harness's tools can be turned off; built-in `explore` and `general-purpose` agents; `/bug`; a sandbox policy ([below](#a-sandbox-policy-signed-off-2026-11-23)); subagents that write at moderate priority.

### A sandbox policy (signed off 2026-11-23)

Built; moved to [DECISIONS.md](DECISIONS.md#a-sandbox-policy-signed-off-2026-11-23) on 2026-11-27 for room: `sandbox` as an object in Claude Code's shape (`filesystem.allowWrite`, `denyRead`, `allowRead`) with Codex's modes; subagents set apart (`sandbox.subagents`), read-only by default; the project-only reach (`blockReadsOutsideWorkingDirectories`), honored by the file tools always; network on or off; processes hidden on macOS as Codex's.

### Scale through ACP sessions and an external runner (steer of 2026-11-23)

The user runs large swarms over `scramjet acp` from a proprietary repository of their own: an external runner drives many processes of 16–64 sessions each, one task per session with its own sandbox and access, against self-hosted vLLM models on HPC nodes, with partly trusted open-weights models. That work is the scale work, done from elsewhere. Its measurements (in `LeanAndMean/scramjet#3`, `#5` and the findings relayed at this steer) show in-process sessions are cheap (about 220 KB idle, 5 ms of agent CPU a turn) but one big process loses per-task sandboxing, cannot span machines or job lifetimes, cannot keep the key from a sandboxed main agent, and makes each command's fork dearer. So the scale work here is now (user decision, 2026-11-23):

- **Cheap, isolated, configurable ACP sessions.** What a session costs per command and per request (no fork of the agent per command, directives only where they apply, `!command` keys cached), what it can reach (file tools refusing the agent's own `/proc`, a sandbox root from an allowlist, a launcher setting for a site's bubblewrap wrapper, an isolation flag), what a client sets per session (a profile in `session/new`), and what it leaves to the client (overload: with retries off, no shared Retry-After wait either; the client's own adaptive concurrency is the overload control).
- **A dispatch backend** (deferred at the steer of 2026-11-24, below). Children handed to an external runner through a file contract (a batch written atomically into an inbox, `status.json` and `results.jsonl` in an output directory, a batch's name never run twice), so one agent can drive tens of thousands of tasks without holding them.

Dropped: admission inside one process for thousands of children and S5's target of about 8,000 requests in flight in one process. Overload control lives with the client. What stays: the invariants under [Resource bounds and admission](#resource-bounds-and-admission) for what one process does hold (bounded buffers, every attempt cancellable, uncertain effects never re-run), the 8 children per process, and the shared Retry-After wait while retries are on. The session profile is signed off (steer of 2026-11-24; `docs/RPC.md` section 3). The dispatch tool below is deferred (steer of 2026-11-24): a frontier model driving cheap on-prem workers drives the runner's own command line through `bash` (the batch written with `write`, then the runner's submit and wait, with a long `timeout`), which needs nothing of Scramjet and leaves the runner owning its contract; a few cheap subagents are agent files whose `model:` names their model (built, `src/agent/subagent.rs`), so which endpoints and models are eligible stays in the runner or in those files, and Scramjet opens no routing or budget design of its own. The tool is built only when driving the runner that way shows a concrete failure (a batch submitted twice after a resume, a wait that cannot last, progress missed), and then only after the swarm runner's agent has reviewed the contract (the user relays it). If built, it also gets a cap on a batch's tasks in the settings (a runner may point at a paid API) and says that results come from partly trusted models and are untrusted data.

**The dispatch tool and its file contract** (proposed at the second steer of 2026-11-23; deferred at the steer of 2026-11-24, above; nothing of it built). One agent hands batches of tasks to a runner outside the process, which runs them where and how it likes (ACP sessions on HPC nodes, a queue, anything), and reads their results from files, so it neither holds the children nor spends its context on them. Scramjet documents the contract (a user guide page, once built), so that any runner can serve it:

- **Where:** the user's settings name two directories, `"dispatch": {"inbox": "/gpfs/runner/inbox", "out": "/gpfs/runner/out"}`, which a session profile (`docs/RPC.md`, section 3) may also give; without them the tool is not offered. Plain files, so that they work across machines on a shared filesystem, where change notification does not.
- **Submitting:** a batch named NAME (letters, digits, `-` and `_`) is `<inbox>/NAME.jsonl`, one JSON object a line, each a task: Scramjet requires nothing of a line but that it is an object, and suggests `id` and `prompt`, with `profile` in a session profile's shape for a runner that drives `scramjet acp`. It is written under a name starting with `.` and renamed into place, so the runner never reads half a batch; an optional `<inbox>/NAME.why` (the agent's reason, plain text) is in place before it. The runner ignores names starting with `.`.
- **Status:** `<out>/NAME/status.json`, `{"state", "summary", "updatedAt", "counts"?}`, `state` one of `running`, `done`, `failed`, `refused` (the runner will not run it, with why) or `stopped` (stopped on the runner's side), `counts` `{total, done, failed}` while it runs. The runner writes it, also by a renamed temporary file, before it takes the batch out of the inbox, so a submitted batch is always in one place or the other. A NAME never runs twice at once: a batch whose NAME is queued or running is `refused`.
- **Results:** `<out>/NAME/results.jsonl`, a line a task (`id`, `state`, `summary`, and what the runner adds), and `<out>/NAME/log`, the runner's own words. No cancel in this first contract.

The tool, `dispatch`, offered to the main agent only (subagents stay read-only):

- `{tasks, name?, why?}` submits: `tasks` is a list of objects, or `file`, a JSONL file in the project the agent wrote, for batches too large to pass as arguments (tens of thousands of tasks are a script's output, not a model's). Every line is checked before anything is written; an empty batch, or a NAME already queued or with a status that is not finished, is refused without writing. Without `name`, NAME is the session's id and the call's, so it is known from the call alone.
- It then waits on the status: read every second, its `summary` and `counts` shown as the call's output so far, with no time limit (a batch may take hours). Escape stops the wait, never the batch: the result says it still runs and how to wait again. When the status is finished, the result is its `state` and `summary`, with the paths of `results.jsonl` and the log, which `read` and `grep` may read (the `out` directory is opened to them, read-only, as what Scramjet gives the agent to read is). Their contents stay out of the context until the agent reads them.
- `{wait: NAME}` only waits, and never writes to the inbox. A session that ended mid-call answers that call on its resume, as every interrupted call is answered (`answer_interrupted_calls`), but naming its batch and the `wait` to call, since the batch may be running: the agent waits again, and nothing is submitted twice. A NAME found in neither place was never taken by the runner, which the result says, leaving it to the agent to submit again.

**How it would be checked:** a small scripted runner in the tests (takes a batch, writes `running`, then results and `done`; refuses a NAME running) for submitting, waiting, Escape and a resume mid-wait; the rename order checked by a runner that lists the inbox in a loop and must never see a partial file.

**Open if it is built** (the recommendations of 2026-11-24, not decided): the names `dispatch`, `inbox` and `out` (whatever the runner already calls them); every NAME used once (no results overwritten while read, a resumed wait never mistaking a new run for its own); `file` in the first version, since tens of thousands of tasks cannot be a model's tool-call arguments.

### The sandbox screen (steer of 2026-11-24)

Built; moved to [DECISIONS.md](DECISIONS.md#the-sandbox-screen-steer-of-2026-11-24) on 2026-11-27 for room: `/sandbox` (`/permissions`) is the one place for the main agent's and subagents' sandbox, in plain questions; each choice means what it says, commands included; changes apply at once and are remembered per project, with a key to save them as the default; `/settings`' headings stand out.

### The draft and its undo (second steer of 2026-11-24)

Built; moved to [DECISIONS.md](DECISIONS.md#the-draft-and-its-undo-second-steer-of-2026-11-24) on 2026-11-27 for room: the unsent prompt is saved with its session; undo has a default key in Windows Terminal, Ctrl+Z on native Windows, several steps and redo.

### What the context holds, shown (2026-11-17)

Built; moved to [DECISIONS.md](DECISIONS.md#what-the-context-holds-shown-2026-11-17) on 2026-11-27 for room: `/context` as Claude Code's; `/system-prompt` shows the instructions as last sent; the README as a front page with the guides under `docs/guide/`.

### Selecting by page, /settings' headings and /resume (2026-11-18)

Built; moved to [DECISIONS.md](DECISIONS.md#selecting-by-page-settings-headings-and-resume-2026-11-18) on 2026-11-27 for room: Shift+PgUp/PgDn move a selection's end a page; `/settings` as one filterable list under headings; `/resume` opens fast; WSL interop stays open with the stores hidden.

### /clear (2026-11-19)

Built; moved to [DECISIONS.md](DECISIONS.md#clear-2026-11-19) on 2026-11-27 for room: `/clear` lands on the launch screen.

## Essential responsibilities and invariants

### Execution and authority

The runtime owns sessions, request admission, tool execution, cancellation, command continuation, approval, and provenance. Commands own their intended outcomes and eligible continuation edges. The TUI presents state and returns user intent; provider adapters and extensions cannot silently redefine authority.

The runtime must operate without a TUI or optional plugins. Do not introduce a centralized workflow DAG, control-flow DSL, mandatory daemon, or general persistent job engine to run a normal session. Internal lifecycle representations are allowed; command-owned relationships must not acquire a second workflow authority.

Isolate each agent's context, route, working directory, capabilities, and cancellation. Do not change process-global cwd, environment, or credentials to switch agents. Failed/incomplete model responses must not execute partial tool calls. Correlate results with actual invocations and reject stale results after session replacement.

Default specialist children to genuinely read-only capabilities. An unrestricted shell is not read-only. Parallel writers need explicit workspace ownership or separate worktrees; conversation isolation does not prevent file conflicts. Readers of a changing checkout are not automatically consistent snapshots. Tool allowlists are not an OS sandbox.

### Boundary robustness

Accept valid opaque data and legal protocol representations within explicit aggregate byte, storage and work budgets. Provider identifiers are data, not paths, tool capabilities or assumed short names. Preserve required values exactly; reject ambiguity rather than silently repairing contradictions. Execution approval, provenance, complete-response authority and uncertain-effect no-retry rules remain strict.

Separate documented protocol constraints, user/safety policy, client resource budgets and unverified hypotheses. Samples do not establish maxima. Capacities must preserve the selected useful consumer and account for framing/serialization overhead; saving trivial bytes is not a reason to reject valid data through unsupported field-size assumptions. Refuse actual budget exhaustion as a resource limitation, unsupported execution semantics as unsupported, and malformed or incomplete protocol as such. Fixtures and review must challenge—not canonize—representation assumptions.

### Resource bounds and admission

Bound live agent state, queued work, bytes in event/output buffers, request serialization, tool processes, storage backlog, and presentation caches. A request-count semaphore alone cannot establish bounded memory. Do not recursively embed descendant transcripts in parent results.

Admission covers every model attempt, including retries and compaction. Separate agent existence, provider-request slots, tool-process slots, and downstream inference execution. Parents awaiting children must not monopolize permits needed by those children. Preserve interactive responsiveness and fairness under bulk work.

Separate transport-sharing identity from quota-pool identity: multiple routes/models/tabs can consume one provider quota. Scope credentials safely, capture them per request, and coordinate refresh without exposing secrets in logs or cache keys.

Enforce applicable request/token rate limits, concurrency ceilings, and provider cooldowns. Token accounting may require estimation before dispatch and reconciliation from actual usage; unsupported or unknown provider limits are not unlimited verified capacity. Keep backend context/output constraints separate from rate and admission policy.

Adaptive control chooses concurrency inside hard safety ceilings. Useful observations include throughput, latency/TTFT, error classes, and optional vLLM running/waiting counts, queue time, KV-cache usage, and preemptions. Seek enough outstanding work for useful batching, not the largest waiting queue. Use damped changes and bounded experimentation; do not pick universal queue-depth or KV-cache thresholds now.

A 429 with applicable rate-limit evidence differs from a proxy 502/504 or client timeout. Timeout is pressure evidence, not proof of rate limiting. Respect valid Retry-After signals; avoid synchronized retry storms. Telemetry is deployment/version-qualified and may be stale or unavailable; correct client-only operation must not depend on it. Proxy queues and replica topology can make backend metrics differ from client experience.

State quota scope honestly. Per-process admission does not enforce aggregate limits across tabs. Exact shared enforcement may later require an optional local coordinator; ordinary use must remain standalone.

Cancellation must remove queued work and settle owned local operations. Local cancellation does not prove remote generation or billing stopped. Track uncertain remote demand/usage separately from local transport slots; do not present uncertainty as settled zero. Bound unresolved remote exposure so replacement attempts cannot accumulate indefinitely after local slots are released. At that bound, pause additional dispatch pending reconciliation or an explicit recovery policy; do not invent a universal remote-completion timeout.

### Durable history, context, and presentation

Keep three responsibilities distinct: durable material execution history, active model context, and disposable presentation caches. Share immutable content where safe; spill large outputs to artifacts with bounded excerpts and load child detail on demand. Memory bounds must not silently falsify model limits or discard required context.

Define truthful acknowledgement boundaries for execution and persistence. Critical authorizations/transitions cannot be treated as committed before their required records are durable. Persist referenced artifacts before durable references. Handle storage pressure explicitly rather than growing unlimited queues.

A full disk must not stop the harness (user objective, 2026-11-06; lower priority). An agent can fill the disk by accident. It must then still be able to start, reach providers and run tools, so that the user can ask it to diagnose the problem and free space rather than having to intervene by hand. History it cannot save is held in memory within a bound, the user is told it is not saved, and it is written once space frees. A failed write never leaves a session file unreadable. The records the invariants above require before an effect stay strict.

Recovery reconstructs supported session facts, not automatic re-execution of external effects. Bind approval to exact payload, target, and current invocation. A dispatched write may have happened even if its result is lost: retain uncertainty and reconcile rather than blindly retry. Known usage includes failed work; unknown usage stays unknown.

### Native interaction

Use structured text/layout information with stable identities, grapheme/display-column mapping, and explicit hard-break versus soft-wrap semantics. Selection copies the last displayed selection, not a mutable future range or hidden source. Preserve reading anchors without tying every repaint to total history size.

UI progress may be coalesced, but material execution events and final results must not silently disappear. One terminal owner presents runtime state through bounded projections; children never acquire the terminal. At high agent counts show summaries and load details on demand rather than painting every token from every agent.

Presentation stays easy to change (user decision, 2026-11-02). The look (glyphs, colors, emphasis, spacing), much of it matched to Scramjet so far, is a starting point, not a commitment, and so is any later design language or theme. Display code says what it shows (a prompt, a reply, reasoning, a pending or failed tool, a diff line, a status, a warning); one place decides how each looks, shared by the printed output (`-p`, piped input) and the full-screen UI, so restyling or adding themes changes that place rather than every renderer. Choosing a look does not hold up other work.

Preserve focus/input precedence, multiline paste without submission, Unicode editing, current approval presentation, and safe external-editor/suspend/exit restoration. Libraries do not guarantee those behaviors. Native clipboard/input journeys are required for supported platform claims; headless cells and successful clipboard API calls are insufficient.

The editor checks spelling as Scramjet's does (user request, 2026-11-06): misspelled words are marked in the theme's `spellcheckError` color with a curly underline where the terminal supports it. Code-like tokens are left alone: identifiers in camelCase or snake_case, paths, URLs, slash commands and short abbreviations. Checking never delays typing; the editor works normally until the dictionary has loaded. Beyond Scramjet's, it knows common developer words, takes the user's own from `words.txt` beside the settings and can be turned off (`spellCheck`).

## Proposed starting architecture

One Rust process per project tab, with lightweight async agent contexts sharing immutable resources and compatible HTTP infrastructure. One headless-capable runtime; a separately scheduled TUI consumes bounded state projections. Blocking work runs outside async workers with its own limits.

A supervised worker-process pool can follow a demonstrated crash-containment need (in-process supervision does not contain an OOM or abort); process separation is not a sandbox.

Session storage is simple and local, with rebuildable indexes; no shared database on a cluster filesystem as a coordination strategy.

Use internal Rust contracts for tools, provider transport, typed events, and structured interaction requests. Installable extensions are packages whose code runs as separate executables over versioned JSON ([packages](#packages-providers-and-mcp-2026-11-04)); do not promise a stable Rust dynamic-library ABI or unrestricted mutable hooks. WASI is a later option only if sandboxed distribution earns its cost.

Select and pin only the libraries the current slice needs; verify the licenses and notices of every enabled feature and transitive dependency.

## Open decisions, resolved when needed

- Providers beyond the first (GitHub Copilot and OpenAI Codex, 2026-11-04) and corporate proxy/TLS needs: resolve when needed; do not implement every inherited provider.
- Exact command and session-import contracts: choose representative valuable behavior, not broad compatibility by appearance.
- Storage representation, first-party crate boundaries, adaptive algorithm, and the package executables' protocol: choose from demonstrated consumers (the private provider package, MCP) and constraints.
- Cross-tab quota sharing and remote workers: establish need before adding coordination infrastructure.
- Target host/workload envelope and acceptance thresholds: record before a capacity claim, not from speculative benchmark targets.

## Reference evidence and limits

`~/repos/scramjet` is a read-only behavioral reference, not a dependency. What shaped the scale work: one full CLI subprocess per child, eight tasks and four launches per call rather than a global admission; streaming updates carrying growing message snapshots, serialized again each time. vLLM's optimization, tool-calling and metrics pages (2026-10-08) give no universal optimal client concurrency.

No benchmark or live load test has run; do not import performance claims from other projects. `~/repos/pi_agent_rust` must not be copied or ported ([CLAUDE.md](../CLAUDE.md)).
