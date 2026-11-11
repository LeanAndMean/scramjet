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

Make scramjet-rs a rock-solid daily driver with Scramjet's features, its retained full-screen UI included, on a small, well-organized codebase. Build beyond Scramjet (admission and bounds for many children, children that write, capacity) only after that (user decision, 2026-11-03). Platforms come before that scale work: native Windows, in daily use since 2026-11-09, then macOS in Terminal.app and in xterm (Homebrew's, under XQuartz). That scale work stays lower priority, but every design keeps it in mind, so that nothing built now corners it (user, 2026-11-06). The first order (demos removed, test-only models' code reviewed, then Scramjet's features, usability first) is done; [the task list](TASKS.md) holds today's.

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
- **Credentials.** A setting decides when the main agent's tools run with known credential stores hidden: `unattended` by default (chained or forced next steps, `-p`), `always` or `never`. Subagents always run with them hidden. On native Windows, which has no sandbox in scramjet-rs yet, an unattended turn's shell commands run unhidden with a warning, rather than refused (user decision, 2026-11-09); subagents' at the low integrity level, read-only on local disks but not on WSL's files or network drives, with nothing hidden, said once (user decision, 2026-11-10).
- **Tools** (user decision, 2026-11-09, replacing 2026-11-03's Scramjet defaults). All seven tools are on by default (read, bash, edit, write, grep, find, ls), and `grep` and `find` are built in, with nothing to install, rather than running ripgrep and fd. `tools` in `/settings` narrows the list. A subagent whose file has no `tools:` takes the read-only ones of that list, as Claude Code's inherit; `disallowedTools` removes some, from that list or its own.
- **Shells on Windows** (user decision, 2026-11-09). Native Windows has three shell tools, each named for its shell and saying when to use it: `bash` (Git for Windows, else MSYS2 or Cygwin, never WSL's) and `powershell` (`pwsh` 7, else Windows PowerShell 5.1, its description naming the edition and its limits), on by default when found, and `cmd` (batch files, tools that expect it), listed in `tools` but off by default. Git for Windows is optional: without it `bash` is off and each launch says so, naming it. A Windows-only setting chooses the shell of `!` commands; `models.json` `!command` values run in bash when found, else PowerShell. No `-EncodedCommand` or `-ExecutionPolicy Bypass`. Other systems have only `bash`.
- **Project agents.** Their approval is remembered per project and asked again when a project agent file is added or changed.
- **Command lifecycle** (second steer of 2026-11-03, after the user's decisions of 2026-10-30 for Scramjet's issue #638, which Scramjet has not built). A started command keeps going until it reports `completed` (its part is over, not that the task succeeded), the user confirms abandoning it, or the user starts another command or types an unknown slash command. Any other stop (Escape, a cancelled or freetext question, a failed response) leaves it waiting for the user, whose next message resumes it. A reply ending without a report gets a short status check, at most 3 in a row without the user's input, then the command waits with a notice. `abandoned` ends the command with no next step, not even a forced one, and only once the user confirms after reading its summary. There is no `blocked`, `incomplete` or `continuing`; Mach 12's text naming them gets a result steering the model to keep working, ask or abandon.

### Look and themes (2026-11-04)

User decisions from `/rs:steer` on 2026-11-04 ([the task list](TASKS.md) holds the order; the theme editor is an exception to building beyond Scramjet only once the daily driver is solid):

- **Scrollbar.** Scramjet's: the rightmost column, shown while the conversation is taller than the view, clickable and draggable. It is a visual aid to where the reader is.
- **The editor's box holds only the prompt and its completion list.** Notes and messages go to the status row above it.
- **Themes.** Exact colors (24-bit, with a 256-color fallback), and a built-in "Terminal colors" theme that keeps today's 16 terminal colors. A theme may set the main background; otherwise the terminal's own background is used, asked of the terminal so that contrast can be scored. Underneath, a theme is a palette of main colors plus every role, each following one of them or holding its own exact value. The default theme follows the terminal's background as Scramjet's does: `scramjet-dark` on a dark or unknown one, `pi-light` on a light one; a chosen theme wins (2026-11-06). At start the full screen asks the terminal only for its background, as Scramjet does; its text color and 16 colors when the theme list opens (2026-11-06).
- **A distinct look**: a visual language for reasoning, tool calls, prompts and replies. Picked at the steer of 2026-11-06 from captured screens of one scripted session: `panels` is the default look and `header` the default banner; the other looks and banners stay offered in `/settings`. Panels follows Scramjet's spacing of boxes, without copying Scramjet's interface: a tinted padding row above a call's header, under it and at the bottom, as user messages have; one left edge for all text; a failed call's output in its normal color on the error background. Its own marks stay: the outcome on the header, the time there, the tool's name before its arguments, `›` on prompts. Polishing it means comparing it with Scramjet on the same scripted session, judging each difference.
- **A theme editor in `/settings`** that lets users with no artistic background discover a theme they like, quickly. It shows a live preview of a sample session with every UI element:
  1. Choose how many main colors (1–5), then a harmony that fits that number (monochrome, complementary, analogous, triadic, split-complementary, square, rectangle…), each described in one plain line.
  2. Randomize: a whole theme in that harmony. Text, dim text, borders and block backgrounds are derived at readable contrast and tinted toward the main colors, and every role is filled. Roll again until it is close.
  3. Lock colors you like; a roll changes only the others, and locked colors anchor the harmony.
  4. Set any single element to an exact color, or reset it to follow its main color. All roles are one view away; nothing is hidden.
  5. Bump toward the harmony in small steps, or bump contrast up or down.

  Undo and redo cover rolls and bumps. Choosing another harmony keeps the colors; only the score and what a bump aims at change (2026-11-06). Alt+Up and Alt+Down reorder the main colors, as lists are reordered elsewhere (2026-11-06). A theme started from another stays recognizably itself through contrast and background changes: its colors shift, they do not jump to newly derived ones (2026-11-06). The main background's lightness can be stepped lighter or darker than the derived one, as contrast is stepped, keeping its tint from the first main color (2026-11-06).

  Scores read at a glance (2026-11-06): a number alone, such as a contrast ratio, means little to most users, and words after each number make a wall of text. Show each score's quality by color (red for bad to green for good, or a background), with labels that say what they measure and are capitalized alike. The loop tries several forms on screen and picks one, with `Choice:` trailers for the user's review.

  Harmony is scored on hue only, in a perceptual space where lightness and chroma stay fixed as the hue turns. Each template hue is a sector rather than an exact point. The template's rotation is fitted to the palette, or anchored by locked colors, and each color counts by its chroma, so grays barely count. Repeated bumps converge to a perfect score.

  A contrast guard keeps every text/background pair within a good band, neither too faint nor too harsh: after each step it nudges lightness as little as needed, or the color skips that step. Contrast bumps change only lightness.

  Colors with a meaning (errors, success, warnings, diff added/removed) keep their hue ranges and are pulled toward the harmony only within them; each can be set by hand or have its range switched off. Which color wheel defines "opposite" (the artist's or a perceptual one) and which contrast formula to use (WCAG 2's ratio or APCA) are the loop's choices, judged by how the randomizer's results look, with `Choice:` trailers.

### Copy, paste and keys (2026-11-04)

User decisions from the second `/rs:steer` of 2026-11-04. Copy and paste are used constantly, so they come first.

- **One executable, nothing else to install.** Clipboard access is built into scramjet-rs (for example talking to X11 or Wayland directly, as the `arboard` crate does; `pbcopy` and `pbpaste` on macOS; Windows' own PowerShell under WSL). Tools such as `xclip` or `wl-paste` are fallbacks, not requirements. Requiring other software is a last resort, if there is provably no better option.
- **Nothing that alarms endpoint security, and no stray files.** No compiled code and no encoded commands. Under WSL: a warm PowerShell process that only calls the built-in `System.Windows.Forms.Clipboard`. Scramjet's warm reader compiles C# with `Add-Type` at each start, leaving randomly named DLLs in Windows' `%TEMP%` that endpoint security flagged (Scramjet issue #642).
- **No OS-level input monitoring** (focus tracking, global key listeners), as an agent once proposed for Command+C in Scramjet: it is fragile across sleep and many processes. macOS terminals handle Command+C and Command+V themselves and send the program nothing (Command+V works because the terminal pastes the text), so no program can see Command+C.
- **Copy-on-select**, a setting, on by default on macOS only: a completed mouse selection is copied, so Command+V works anywhere afterwards (Scramjet issue #643). Elsewhere, right-click copies a selection and pastes without one, and Ctrl+C copies a selection, as today.
- When no clipboard route works, say once which key pastes through the terminal itself. Each terminal's quirks (Windows Terminal, Apple Terminal, iTerm2's `ReportRightClick`, xterm, VTE terminals such as Raspberry Pi OS's) are documented in this repository.
- **Key remapping**, as Codex's `/keymap`: `/hotkeys` lists every key and also changes one (press the new key; conflicts shown), saved to a keybindings file; `/keybindings` (Claude Code's name) and `/keymap` open it. It is also the fallback where a terminal keeps right-click or a key to itself.
- **Effort levels stay as they are**: the UI offers only the levels the model lists, the footer shows the level sent, and the level chosen is kept and fitted to each model, so switching back restores it. With more providers, the levels come from each provider's model list.

### Packages, providers and MCP (2026-11-04)

User decisions from the second `/rs:steer` of 2026-11-04:

- **Packages are the extension model.** A package is installed by reference, as command sets are today, and brings data (commands, agents, skills, themes, keybindings, provider and MCP server entries) and, for what needs code, separate executables speaking versioned JSON over stdio (login, token and model-list helpers, MCP servers, later hooks), as Claude Code's and Codex's plugins do. No package code runs inside scramjet-rs: no shared libraries (Rust has no stable interface for them), no embedded scripting, and WebAssembly only if sandboxed third-party distribution earns its cost.
- **A package's programs come from its releases** (user decision, 2026-11-09): one archive per platform and a `SHA256SUMS`, as scramjet-rs's own, which `install` and `update` of a git package download for the machine (through `gh` while private) and check, so that its users need no compiler.
- **The public core has no code for the company's proxy,** which is proprietary and must not be revealed in detail. The local proxy's route is configuration (the OpenAI and Anthropic protocols at a local address); the proxy's own parts (model and price cards, the direct login and routing) live in a private package in a repository of its own.
- **Providers.** One boundary for every provider: HTTPS, the OpenAI Responses and Anthropic Messages protocols, and Chat Completions for open servers (vLLM, Ollama, llama.cpp). GitHub Copilot and OpenAI Codex (a ChatGPT subscription) come first, with `/login` and `/logout`, since the user has both accounts and can test their login. Adding another provider takes only a provider entry. scramjet-rs keeps its own logins' credentials in its own file, hidden from the agent's tools like other credential stores.
- **Model lists come from the providers** (user decision, 2026-11-05). Where a provider lists its models through its API, scramjet-rs reads that list and keeps it current by itself, so a model the provider adds is offered at once; if it does not work, users report it and it gets fixed. scramjet-rs does not keep lists of subscription providers' models, which go stale unnoticed.
- **No Claude subscription login** (steer of 2026-11-08). Scramjet signs in to Claude Pro/Max and sends requests disguised as Claude Code's (its user agent, betas, identity line and tool names). Anthropic's terms forbid it ("Anthropic does not permit third-party developers to offer Claude.ai login into their own applications…", code.claude.com's legal and compliance page, fetched 2026-11-08). The user chose no Anthropic API-key provider for now; Claude comes through the company's routes and Copilot.
- **GitHub Copilot's `X-Initiator`.** A turn's request after the user's message is the user's; the agent's follow-ups, compaction, branch summaries and subagents are the agent's. That matches GitHub's billing docs: "only the prompts you send count as premium requests; actions Copilot takes autonomously to complete your task, such as tool calls, do not" (fetched 2026-11-06). Status checks during commands stay the user's (user decision, 2026-11-06).
- **Requests name scramjet-rs** (user decision, 2026-11-05): a provider is sent the harness's own name wherever it accepts it, and another only where it refuses ours. A name and the version sent with it form one claim: a version is sent in another client's numbering only together with that client's name, as ChatGPT's model list asks for Codex's current release (its model requests and its login take scramjet-rs's name).
- **Less clutter in `/model`.** A provider appears once the user logs in to it or enables it, in `/model` and `/scoped-models` alike, grouped by provider with the current one first and filterable by provider; `/login` lists the others. (Pi lists every provider with any credential, an environment variable such as `HF_TOKEN` included, so the user scrolls past catalogs they never use.)
- **MCP**, beyond Scramjet like the theme editor: a client in the core (the official Rust SDK, `rmcp`), servers declared in the settings or packages, over stdio and streamable HTTP. Progressive disclosure is the harness's job (MCP has none; Claude Code and the Claude and OpenAI APIs disclose tools on the client side; researched 2026-11-04): each server is listed in one line, as skills are, and a fixed pair of tools searches the tools and calls one, loading its schema on demand, which keeps the prompt cache stable on every provider. A server whose tools are small (MCP's page suggests 1–5% of the context window) may have them loaded directly. Provider-native tool search may come later; "code mode" needs an embedded runtime and is not planned.

### Usage (2026-11-06)

User request at the second steer of 2026-11-06, polish rather than a priority. `/usage` (Claude Code's name) opens a view of what the user has used and has left: subscription quota for subscription providers (Copilot's premium requests, ChatGPT's windows), spend and rate limits for pay-per-token ones, for the current provider by default and the others one key away. Each provider's quota is read its own way; packages report their providers'. Its history across sessions comes from the timing recorded in each session. Unknown usage stays unknown, as elsewhere.

Less is more (user decision, 2026-11-07, after seeing 0.77.0's view as a wall of tables that gave little insight). The view answers three questions at a glance: how much is left (each quota window as one bar with its reset and whether it lasts at this pace), what was spent (today, this week, the last 30 days, with the change from the 30 before), and what drove it (the top models by share, the costliest session). Everything else (cache shares, speed percentiles, daily and weekly charts, sessions by size, the per-period table) is dropped or kept behind a details key. The sketch the user approved:

```
Usage · ChatGPT                                    ◂ All · ChatGPT · Copilot · Company ▸

  5-hour   ████████░░░░░░░░░░  41%   resets in 2h 10m
  Weekly   ██░░░░░░░░░░░░░░░░  12%   resets Tue 07:26 · on pace to last the week

  Today $1.20   This week $8.40   Last 30 days $31.10   (up 40% on the 30 before)

  Most of it went to   claude-opus-5-5  $26.80 (86%)   gpt-6-luna  $3.10 (10%)
  Costliest session    "/rs:loop sandbox" yesterday, $4.10

  d details · Tab provider · Esc close
```

### Session chains (2026-11-05)

User decisions of 2026-11-05. Next steps that start a fresh session link sessions into a chain (a `/rs:loop` run is one), but nothing records that link, so `/resume` cannot show a chain again. Scramjet has the same gap: its fresh-session dispatch passes no `parentSession`.

- **Save the link.** A session started by a fresh-session next step names the session it came from in its header, as `parentSession`, as forks do. Pi's handoff example does the same for a new session that copies nothing. The header also marks that a next step started it, so a continuation can be told from a fork. The earlier session's next-step record names the new session's file.
- **A chain can be seen in `/resume`** (user decision, 2026-11-07), in three forms one key cycles through, the last one used remembered: flat, the default so that users who do not know chains are not confused, each step showing its `completed` summary in place of its first prompt and a `chain 11/12` mark; threaded, each step under the one before, as forks are; folded, one row per chain that Right or Enter opens into its steps, newest first, and Left closes.
- **Resuming shows the earlier sessions.** Resuming a session of a chain shows the sessions before it above its own conversation. While a chain runs, a fresh-session step likewise keeps what came before on the screen instead of clearing it. The form is not settled yet. Proposed: one collapsed block per earlier session (its command, its `completed` summary, when it ran), which Ctrl+O opens into its full conversation.
- **For display only.** Earlier sessions never enter the model's context: a fresh session stays fresh.

### A sandbox for the main agent (2026-11-06)

User decisions of 2026-11-06, for machines and filesystems where deleting data is not acceptable. This is not a common need, so it is not front and center in the UI.

- **Two modes, Codex's names:** `--sandbox read-only` and `--sandbox workspace-write` (`-s`), and a `sandbox` setting, off by default. `read-only` runs the main agent's shell in the subagents' sandbox (every file read-only, a private `/tmp`, its own processes, credential stores hidden) and refuses its writes and edits. `workspace-write` does the same with the project directory writable.
- **Network is a setting of its own**, on by default.
- **It must hold.** What the agent starts runs inside: MCP servers over stdio included. Under WSL, Windows programs (`cmd.exe`, `powershell.exe`), which could change Windows' files, are out of its reach. Without Bubblewrap it refuses to start rather than run unconfined. A small mark shows while it is on. The `!` commands the user types are the user's own and stay outside.
- **Protected paths as Codex's** (user decision, 2026-11-07): `workspace-write` keeps only the project's own `.git` (with the gitdir a `.git` file names), `.scramjet` and `.agents` read-only, as Codex's `default_read_only_subpaths_for_writable_root` does (`codex-rs/protocol/src/permissions.rs`, read 2026-11-07; it also keeps `.codex` and `.aws`). Repositories nested in the project are not protected: pinning them made `rm -rf build` fail over a CMake or pip checkout, and an exception list by directory name, or by `.gitignore`, would not be trustworthy.
- **Unix sockets as Codex's** (user decision, 2026-11-07): left open on Linux, as Codex leaves them; on macOS, follow Codex's Seatbelt behavior there when the macOS sandbox comes.
- Item 7's children that write reuse it, with their worktree writable.

### Hand-offs and going back (2026-11-07)

User decisions of 2026-11-07:

- **Waiting for you, always.** Whenever the turn is with the user (the end of every turn, the agent's freetext, select and confirm questions, next-step lists, `--ask` approvals), the status row says `Waiting for you` with the time since the agent stopped, until the user sends something, typing included. Not at start, before any turn.
- **Going back reopens what was there.** Going back (`/tree`, double Escape) to a point where a choice was offered (a next-step list after a `completed` report or from `suggest_scramjet_next_steps`, or the agent's select, confirm or freetext question) reopens it as it was, with its options, rather than leaving the user after it as if dismissed; what the user picks then branches from there, as going back to a prompt puts its text back in the editor.

### RPC mode and off-device front ends (2026-11-09)

User decisions from `/rs:steer` on 2026-11-09. Users of other harnesses move to browser and off-device front ends to check on their agents from a phone. The user does not want one for themselves, nor the attack surface a browser or network adds, but scramjet-rs should let others build one, in their own repositories.

- **RPC mode.** `scramjet --mode rpc` runs headless over JSONL on stdin and stdout, for a program that starts it. Scramjet's RPC mode (`~/repos/scramjet/packages/coding-agent/docs/rpc.md`) is the reference: its command, response and event names and shapes where scramjet-rs has the feature, so that a client written for Scramjet mostly works, and its framing (LF-only records), with a protocol version in the state.
- **For a front end on another device:** an id on every command; the agent's approvals and questions (`--ask`, publications, select, confirm, freetext, next steps) as requests with ids that the client answers, not refused as Scramjet refuses publication approval in RPC mode (how approval works without a terminal is a product choice, recorded as a `Choice:` trailer); the session's state and history for a client that attaches later, with a way to tell which events it missed; no credentials, tokens or auth headers in any event, state or error.
- **One core.** The full screen and RPC mode drive the same agent. What both need (next steps, chains, autopilot) belongs to the core, not copied into each.
- **A deliberate effort, design first.** scramjet-rs is stable now and the full screen must keep working as it does. The protocol's design (what moves into the core, the commands and events, approvals) is written as a proposal and signed off by the user at a steer before anything is built.
- **Out of scope here:** any listening port, HTTP or WebSocket server, authentication or pairing, TLS, web or JavaScript dependencies: the terminal binary has no network-facing surface. Also out of scope: a daemon with the full screen as one of its clients (Codex 0.166's app-server daemon, `codex remote-control`, `codex agents`). The full screen stays in-process; a daemon would be a separate decision, which the user signs off.
- **Long term, elsewhere:** a browser front end reachable from other devices (phones over Tailscale), in a repository of its own: a server that starts `scramjet --mode rpc` and depends only on the protocol, never on scramjet-rs's Rust code. Its threat model (who can reach it, cross-site requests and DNS rebinding, injection and exfiltration through the transcript, approvals on a small screen, tokens at rest) is that repository's; the protocol must not foreclose its mitigations: nothing secret in its output, approvals carrying what a small screen needs to decide, the whole state on request.
- **Open:** checking from another device on a session started in the terminal ([the task list](TASKS.md), Open questions).
- **Proposal:** [RPC.md](RPC.md) (2026-11-09), awaiting sign-off.

### Command sets' home and Scramjet's sunset (2026-11-09)

User decisions from `/rs:steer` on 2026-11-09. scramjet-rs has proved effective enough that Scramjet will likely be sunset: everything scramjet-rs takes from Scramjet's repository moves to repositories of its own, with nothing kept there only for Scramjet (the split followed the Pi fork's limits, not a design).

- **File names that work everywhere.** A set's files are `commands/<command>.md` and `agents/<agent>.md`, named `<set>:<name>` from the set's name, as Claude Code's plugins name theirs from `plugin.json`. Git for Windows cannot check out a name with `:`, and NTFS takes it as a stream. `<set>:<name>.md` stays accepted.
- **Mach 12 in its own repository:** `~/repos/mach12`, split from Scramjet's `packages/scramjet/mach12` with its history, in that form, with a `.claude-plugin/plugin.json` so that Claude Code loads it too (without next steps or command reports there), on the private `LeanAndMean/mach12`. Scramjet's repository keeps its copy.
- **The `scramjet` set** (`troubleshoot` and the command-authoring agents), the `writing-scramjet-commands` skill and command linting move into this repository, which owns the command format; a lint built into scramjet-rs replaces the Node `scramjet-command-lint`.

### Releases, updates and going public (2026-11-09)

User decisions from `/rs:steer` on 2026-11-09, revised on 2026-11-10:

- **Releases for testing,** with executables for every target platform: Linux x86_64 (WSL included), the Raspberry Pi 5 (aarch64), macOS and Windows, each checked by CI within reason before it is published. A release is made only for a version the user asks for; continuous releases are explored once the development rhythm is good.
- **Public before every platform is tried** (second steer of 2026-11-10, replacing "public once every platform works"): releases exist to get it tried on other platforms, macOS first, and a private release takes repository access and `gh` to download. So releases are public alphas of Scramjet 2 (below), their notes and the README saying which platforms nobody has used yet; every platform used gates 2.0.0 itself. Standard runners are free in public repositories (docs.github.com, 2026-11-10); the $5 a month bound covers private ones.
- **Updates as Scramjet's:** `update` updates the packages and the program, `update self` the program only, downloaded from the public releases over HTTPS and checked against the release's checksums before it replaces the binary. A newer release is mentioned at start, at most once a day.
- **"What's new" and `/changelog`:** after an update, the first interactive start shows the changelog's entries since the version last run, in a view of its own with its own scroll, as `/changelog` shows the whole history; neither goes into the conversation.

### Scramjet 2 and a clean history (2026-11-10)

User decisions from the steers of 2026-11-10:

- **scramjet-rs becomes Scramjet 2:** its name only means "Scramjet in Rust", which says little once Scramjet is sunset. It is not a continuation of Scramjet 1's repository (second steer of 2026-11-10, replacing a merge into its `main`): v1's `LeanAndMean/scramjet` is renamed (`scramjet-v1`, say) and a new public `LeanAndMean/scramjet` takes the name with scramjet-rs's history, as a from-scratch v2 usually gets a repository of its own (Yarn's `berry`, `typescript-go`). Its issues and releases start clean; v1's links to its docs and issue tracker then reach v2's. Alphas are tagged `v2.0.0-alpha.N` there and published as GitHub prereleases; npm's `@leanandmean/scramjet` keeps v1 as `latest` until 2.0.0. The private `LeanAndMean/scramjet-rs` never becomes public.
- **Versions:** from the first public alpha, `2.0.0-alpha.N`; each commit that changes the program bumps N (a prerelease has no minor or patch; the changelog's headings say what kind of change). `update self` compares prereleases.
- **v2 owns its layout and formats** (second steer of 2026-11-10), free of design choices traced back to Pi. Before the first alpha the command becomes `scramjet`, with `~/.config/scramjet`, `~/.local/state/scramjet`, `~/.local/share/scramjet` (and the runtime ones) and the project's `.scramjet/` as its own; its global agents, skills, themes and `SYSTEM.md` live under `~/.config/scramjet/`, and scramjet-rs's existing directories are moved by hand, on the user's machines only, the only ones that have them (user, 2026-11-11, after an automatic move at start broke a running older version's session). It stops reading Pi's agent directory (`~/.scramjet/agent`, `SCRAMJET_CODING_AGENT_DIR`). It reads the `autonomy.yaml` that v1 keeps in the same place while their formats agree, free to change it. Pi's shapes in its formats (setting names, keybinding ids, `models.json`, themes, `SYSTEM.md`/`APPEND_SYSTEM.md`) change where they are in the way, judged one at a time, preferably before 2.0.0, with no redesign before the first alpha. Claude Code's and Codex's conventions (`CLAUDE.md`, `AGENTS.md`, `.agents/skills`, plugins) are the compatibility that counts; Scramjet stays the reference for what works (protocols, providers), not for layout.
- **What 2.0.0 needs:** used by someone on every target platform; Scramjet 1's settings, models, keybindings, agents and themes offered for import, converted, at the first start (sessions are not imported); npm's per-platform packages (as esbuild's), so that Scramjet 1's `scramjet update` leads to it, added at a beta or at 2.0. An alpha needs only CI passing on every target and a truthful status.
- **The changelog** keeps the versions before 2.0, under a heading of their own as scramjet-rs.
- **A clean history** (done 2026-11-10): nothing of the company is tracked or written in a commit message (the proxy's, the company's and its services' names, user IDs, colleagues' names and handles, private repositories' names). The history was rewritten once, with the user's review; a pre-commit check keeps it clean, its terms and the loop's private facts outside the repository.

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
