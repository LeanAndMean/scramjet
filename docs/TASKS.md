# Tasks

Pending work only (`CLAUDE.md`). What was done and how it was verified is in the commit messages; product choices awaiting the user's review are `Choice:` trailers in them.

Last steer: 61000c1

## Current focus

None. `/rs:loop <focus>` records a user-given focus here until it is finished.

## Next candidates

Order agreed with the user on 2026-11-03 (`/rs:steer`; [design](DESIGN.md#current-direction-2026-11-03)). Item 1 (how the code is organized) is done, and item 2 but for what item 4 brings; item 3 is under way. The small items in 6 fit between larger ones when cheap.

2. **Settings.** The file and its keys so far are in (`src/agent_cli/settings.rs`: `hideThinkingBlock`, `doubleEscapeAction`, `branchSummary.skipPrompt`, `defaultThinkingLevel`, `tools`, `hideCredentials`, `steeringMode`, `followUpMode`), passed to the agent as `AgentConfig` fields or setters in `agent_cli.rs`. Still to add: installed command sets (item 4).
3. **Full-screen UI as the main interface**, as Scramjet's retained mode, then remove the inline renderer. It is the default on a terminal; `--inline` keeps the inline renderer until it goes. In `src/agent_cli/screen.rs` a thread owns the terminal and renders the conversation from `record::Event`s through `Terminal::capture_shown`, one cached part per turn; the session (`interactive.rs`, `slash.rs`) sends it events, messages (`say`) and dialogs (`select::Picker`, text, approval) and waits for prompts; the editor is `editor::Draft` driven by `Editor::key`. Done: the editor visible and editable during a turn, the mouse wheel, drag, double- and triple-click selection of the conversation and the dock under it, copy as written (`text::FROM` marks: gutters left out, wrapped lines joined) and right-click paste, Enter and Alt+Enter queuing steering and follow-up messages (`AgentUi::steering`/`follow_up`, asked by `Agent::run`), Escape interrupting, Page Up/Down, Ctrl+O expanding in place, Ctrl+T re-rendering reasoning already shown, lists in the dock, suspend and Ctrl-G, Shift+Tab during a turn (`AgentUi::effort`, taken before each request), `!` commands during a turn (run on a thread of the screen's, `agent::Shell`, added by the session when the turn ends), built-in commands during a turn (`slash::Builtin`; queued by the screen, run by the session where the turn pauses before a request, `AgentUi::pause` and `Agent::resume_turn`, or after it stopped), summaries and a command's sent text cut until Ctrl+O (`AgentUi::command_prompt`, `Terminal::summary_block`), and a command set's command queued as typed, started where the model gets it (`Agent::add_queued`). Next: move the PTY journeys (`--inline` in `tests/agent_editor_pty.py`) that test session behavior rather than the inline display (fork, tree, resume, settings, questions, next steps, credentials, command sets, the external editor) to the full screen; then delete the inline renderer (`terminal.rs`'s inline paths, `type_ahead.rs`, `view.rs`, `record.rs`'s `Recording`, the inline editor's drawing and `select.rs`'s inline lists, `--inline` and its paragraphs in the usage text and README). `-p` and piped input keep printing plainly. Display code keeps naming its styles in `src/agent_cli/style.rs`.
4. **Command sets.** Install by reference (`install`, `remove`, `list`, recorded in the settings), then install `~/repos/scramjet/packages/scramjet/mach12` and `.../scramjet` here; agents named `<set>:<name>` come only from the installed set `<set>` (loose files claiming it are skipped with a warning; the user's agents directory has stale `mach12:` links into Scramjet's fixtures and `/tmp`); `# Available subagents` lists command-set agents only. Command sets are their own concept, not prompt templates (design); their discovery replaces `d841761`'s, which a test-only model drafted and which was left unreviewed for this; decide `delegate-only` at the prompt accordingly. For Mach 12: `delegate` (Scramjet's `delegate.ts`), the four publication tools with Scramjet's approval behavior and `autonomy.yaml`'s `publications` block, `set.yaml` and `autonomy-defaults.yaml`, warnings for unknown commands in `autonomy.yaml`. Also `/autopilot` (with `/autopilot on to chain` in notices), `/reload`, the chosen next step added to the conversation as Scramjet does, and skills.
5. **One `/settings`** for everything, including command autonomy designed to be understood at a glance by users new to it (layout and wording, not explanatory text; e.g. rows per command's next steps, plain values such as continue automatically / ask me / autopilot decides, the effective value first with where it comes from, the scope asked when changing), publication approval, and an Installed view of everything loaded with its source and problems; launch shows at most one line when something needs attention.
6. **Small changes agreed on 2026-11-03** (Scramjet's behavior unless stated):
   - Compaction status `Compacting context...` / `Auto-compacting...`; the branch-summary request without reasoning and with at most 2048 output tokens; an empty Enter in its instructions editor gives the default summary.
   - Claude: prompt cache `ttl: 1h`; a reply stopped at `max_tokens` keeps its text (its tool calls never run); the `fine-grained-tool-streaming` beta.
   - An unknown `--effort` level is refused at startup, listing the valid ones; the effort is shown on the usage line.
   - Forks record `parentSession` and `/resume` marks them; rename and delete in `/resume`; a `-r`/`--resume` flag; `/tree` rows styled as Scramjet's; errors and cancellations saved and shown on resume; a restored command whose file is gone is dropped with a warning when its turn ends.
   - `!!` commands saved, excluded from the context; `!` commands shown as Scramjet's `$ command` block.
   - Equal fuzzy scores put the shorter name or id first; completion quotes names with spaces and completes inside an open `"`.
   - `(Esc to interrupt)` on the status line; Escape declines an `--ask` question.
   - Project-agent approval remembered per project, asked again when a project agent file is added or changed.
7. **Later, once the daily driver is solid:** admission and bounds for many children (shared request slots, no parent holding slots its children need), children that write (in a worktree, the sandbox allowing that directory), macOS (Seatbelt; untested), capacity ([roadmap](ROADMAP.md) S3, S5).

Practice runs (`CLAUDE.md`) find what hurts; their findings join these items, and nothing they write is adopted.

## Known issues

Not scheduled; fix them when they matter.

- `bg` after Ctrl-Z during a turn stops the agent again at once: the SIGTSTP handler restores the turn's terminal mode, which a background job may not do (Scramjet's TUI cannot run in the background either). A reply rewritten in place after a lossy stream (`text_rows`) still counts rows from before a stop.
- Every start writes a session file before any prompt (Scramjet writes none before the first reply). `/resume` hides empty ones, but `--continue` picks one written last, which keeps the `/model`-switch-before-a-prompt behaviour agreed on 2026-10-30.
- Compaction and turn-prefix requests carry no output cap. Scramjet caps them at 0.8 and 0.5 × `reserveTokens` (13,107 and 8,192, reasoning included on Responses) and keeps a summary cut off there; here a cut-off reply fails the compaction, so adding the cap needs that too.
- Scramjet's edit tool falls back to fuzzy matching (NFKC, trailing whitespace, smart quotes, Unicode dashes and spaces, CRLF) and then writes the whole file normalized. Consider it if edits fail on such differences.
- The GPT overflow wording through Proxy is unverified (needs a request over 1.05M tokens; Scramjet's OpenAI pattern is `exceeds the context window`). deepseek-v4.1-flash's gateway answers an overflow with HTTP 400 `This model's maximum context length is 327680 tokens…` (its card says 320000).
- `claude-haiku-4-5-20251001` runs without thinking: its Proxy card has no `reasoning` field ("no reasoning control" in Proxy's card schema), though Scramjet's registry marks it as reasoning.
- With credentials hidden, the main agent's bash still shares the host's processes, `/tmp`, `/run` and session (so that `setsid nohup … &` outlives a command). Other processes' `/proc/<pid>/environ` and `/proc/<pid>/root` are closed to it (its own user namespace; Permission denied, checked), but servers running outside act for it with everything visible: an ssh-agent (`SSH_AUTH_SOCK`), a tmux server on a socket outside `${TMUX_TMPDIR:-/tmp}/tmux-UID` (`tmux -S`, or under a `TMUX_TMPDIR` other than scramjet-rs's own; its path shows in `/proc/net/unix`, and bubblewrap cannot mount over a socket), and, not tried, Docker's socket and WSL interop (`cmd.exe`, `/run/WSL`); and through the controlling terminal TIOCSTI (allowed before Linux 6.2; WSL runs 5.15) could type a `!` command. Only subagents' sandbox has its own processes, `/tmp`, `/run` and session.
- Subagents' bash cannot start tmux in its private `/tmp` (`error creating /tmp/tmux-UID (Permission denied)`): the sandbox mounts over the user's socket directory there too, as it must when `TMUX_TMPDIR` is elsewhere. It worked before dbe6f2d; if it matters, give the sandbox a `TMUX_TMPDIR` of its own in `/tmp`.
- The full screen renders a running turn's part again from all its events at each frame (at most every 16 ms). Measured on a release build for a turn of 150 calls with 400-line outputs: 4 ms a frame cut, 100 ms with Ctrl+O (61,000 lines). Ctrl+O and Ctrl+T keep the view at the same part and line within it, which can move within a long turn.
- A `!` command run during a turn in the full screen shows only its last 5 lines until the turn ends, not expandable with Ctrl+O (Scramjet's shows 20 and expands); a `!` typed while the session itself runs one (no turn) runs alongside it, where Scramjet refuses a second.
- A built-in command typed during a turn in the full screen waits for the turn's next request, which can be minutes away while a tool or subagent runs (Scramjet runs `/session`, `/name` and `/model` at once); the dock shows it as `Command: …` meanwhile.
- The inline editor's PTY journey `thinking_counts_seconds` once timed out waiting for `(cancelled)` after Ctrl-C (2026-11-03; passed before and on the rerun). The inline renderer is due to go (item 3); look into it if it recurs before then.
- A failing test can leave its directory in `/tmp`: the `autonomy`, `commands`, `agent_cli::editor` and `agent_cli::file_search` tests remove theirs only at their end, where the other agent tests use the `agent::tests::ScratchDir` drop guard.

## Open questions

For the user, reviewed with `/rs:steer`. None blocks current work.

- Rotating the GitHub Copilot token exposed in the 2026-11-02 practice run is for the user.

Resolved 2026-11-03: direction and order (above); credentials become a setting (`unattended` by default); bundled command sets are installed by reference from `~/repos/scramjet`; the 31 earlier choice entries (`git show 5c9b5b8:docs/TASKS.md`) were reviewed one by one: those not listed in items 2–6 stay as they are.
