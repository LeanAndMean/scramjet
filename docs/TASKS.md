# Tasks

Pending work only (`CLAUDE.md`). What was done and how it was verified is in the commit messages; product choices awaiting the user's review are `Choice:` trailers in them.

Last steer: 61000c1

## Current focus

None. `/rs:loop <focus>` records a user-given focus here until it is finished.

## Next candidates

Order agreed with the user on 2026-11-03 (`/rs:steer`; [design](DESIGN.md#current-direction-2026-11-03)). Item 1 comes first; the small items in 6 fit between larger ones when cheap.

1. **Review the code drafted by test-only models** (user, 2026-11-03: by the loop's own model, as a priority). These commits adopted code first written in practice runs on claude-haiku-5-5, gpt-6-luna, deepseek-v4.1-flash or gpt-6.1-sol; read each for correctness and fit, fix what is wrong, and remove a commit from this list once reviewed. Riskiest first: `2278380`, `65bd818`, `d465523` (session loading, the resumed display, command state); `f9f91f2`, `a08f455` (compaction); `6bc3c55` (Claude budget thinking); `525c307` (the agent tests' scratch directories). Then `d841761`, `781e3e4`, `8cd02a8`, `3763c82`, `30ef6b1`, `e2c2c15`, `1f71532`, `33d3e9c`, `6f2b6fc`, `54d0977`, `8ed152b`, `7db6fd0`. Also how the agent's code is organized (`src/agent.rs` 4.8k lines, `src/agent_cli.rs` 2.5k).
2. **Settings.** A scramjet-rs settings file of its own, in Scramjet's format and key names where the behavior matches: `hideThinkingBlock` (remembers Ctrl+T), `doubleEscapeAction`, `branchSummary.skipPrompt`, `defaultThinkingLevel` (also Claude's effort without `--effort`), `steeringMode` (`all`) and `followUpMode` (`one-at-a-time`); scramjet-rs keys for credential hiding (`unattended` by default, `always`, `never`: the main agent's tools run with known credential stores hidden in chained or forced next steps and with `-p`, reusing `src/agent/sandbox.rs`), `tools` (Scramjet's read, bash, edit, write by default; can add `grep`, `find`, `ls`) and installed command sets (item 4). Its path is the loop's choice.
3. **Full-screen UI as the main interface**, as Scramjet's retained mode, then remove the inline renderer: the editor visible during a turn, Enter queuing a steering message and Alt+Enter a follow-up (per item 2's modes), `!` commands during a turn; Ctrl+O expanding output in place; compaction and branch summaries and command prompts collapsed (`[command] /rs:loop x`), expandable; Ctrl+T re-rendering reasoning already shown; mouse wheel and the UI's own selection and copy; a row per subagent child with its model, and children's conversations kept for a resume and shown on expansion (Scramjet keeps them in the result's details). Display code keeps naming its styles in `src/agent_cli/style.rs`.
4. **Command sets.** Install by reference (`install`, `remove`, `list`, recorded in the settings), then install `~/repos/scramjet/packages/scramjet/mach12` and `.../scramjet` here; agents named `<set>:<name>` come only from the installed set `<set>` (loose files claiming it are skipped with a warning; the user's agents directory has stale `mach12:` links into Scramjet's fixtures and `/tmp`); `# Available subagents` lists command-set agents only. Command sets are their own concept, not prompt templates (design); decide `delegate-only` at the prompt accordingly. For Mach 12: `delegate` (Scramjet's `delegate.ts`), the four publication tools with Scramjet's approval behavior and `autonomy.yaml`'s `publications` block, `set.yaml` and `autonomy-defaults.yaml`, warnings for unknown commands in `autonomy.yaml`. Also `/autopilot` (with `/autopilot on to chain` in notices), `/reload`, the chosen next step added to the conversation as Scramjet does, and skills.
5. **One `/settings`** for everything, including command autonomy designed to be understood at a glance by users new to it (layout and wording, not explanatory text; e.g. rows per command's next steps, plain values such as continue automatically / ask me / autopilot decides, the effective value first with where it comes from, the scope asked when changing), publication approval, and an Installed view of everything loaded with its source and problems; launch shows at most one line when something needs attention.
6. **Small changes agreed on 2026-11-03** (Scramjet's behavior unless stated):
   - Compaction status `Compacting context...` / `Auto-compacting...`; the branch-summary request without reasoning and with at most 2048 output tokens; an empty Enter in its instructions editor gives the default summary.
   - Claude: prompt cache `ttl: 1h`; a reply stopped at `max_tokens` keeps its text (its tool calls never run); the `fine-grained-tool-streaming` beta.
   - An unknown `--effort` level is refused at startup, listing the valid ones; the effort is shown on the usage line.
   - Forks record `parentSession` and `/resume` marks them; rename and delete in `/resume`; a `-r`/`--resume` flag; `/tree` rows styled as Scramjet's; errors and cancellations saved and shown on resume; a restored command whose file is gone is dropped with a warning when its turn ends.
   - `!!` commands saved, excluded from the context; `!` commands shown as Scramjet's `$ command` block.
   - Equal fuzzy scores put the shorter name or id first; `/quit` listed (`/exit` still works); completion quotes names with spaces and completes inside an open `"`.
   - `(Esc to interrupt)` on the status line; Escape declines an `--ask` question.
   - Project-agent approval remembered per project, asked again when a project agent file is added or changed.
7. **Later, once the daily driver is solid:** admission and bounds for many children (shared request slots, no parent holding slots its children need), children that write (in a worktree, the sandbox allowing that directory), macOS (Seatbelt; untested), capacity ([roadmap](ROADMAP.md) S3, S5).

Practice runs (`CLAUDE.md`) find what hurts; their findings join these items, and nothing they write is adopted.

## Known issues

Not scheduled; fix them when they matter.

- `bg` after Ctrl-Z during a turn stops the agent again at once: the SIGTSTP handler restores the turn's terminal mode, which a background job may not do (Scramjet's TUI cannot run in the background either). A reply rewritten in place after a lossy stream (`text_rows`) still counts rows from before a stop.
- Every start writes a session file before any prompt (Scramjet writes none before the first reply). `/resume` hides empty ones, but `--continue` picks one written last, which keeps the `/model`-switch-before-a-prompt behaviour agreed on 2026-10-30.
- Scramjet's edit tool falls back to fuzzy matching (NFKC, trailing whitespace, smart quotes, Unicode dashes and spaces, CRLF) and then writes the whole file normalized. Consider it if edits fail on such differences.
- The GPT overflow wording through Proxy is unverified (needs a request over 1.05M tokens; Scramjet's OpenAI pattern is `exceeds the context window`). deepseek-v4.1-flash's gateway answers an overflow with HTTP 400 `This model's maximum context length is 327680 tokens…` (its card says 320000).
- `claude-haiku-4-5-20251001` runs without thinking: its Proxy card has no `reasoning` field ("no reasoning control" in Proxy's card schema), though Scramjet's registry marks it as reasoning.
- A failing test can leave its directory in `/tmp`: the `autonomy`, `commands`, `agent_cli::editor` and `agent_cli::file_search` tests remove theirs only at their end, where the other agent tests use the `agent::tests::ScratchDir` drop guard.

## Open questions

For the user, reviewed with `/rs:steer`. None blocks current work.

- Rotating the GitHub Copilot token exposed in the 2026-11-02 practice run is for the user.

Resolved 2026-11-03: direction and order (above); credentials become a setting (`unattended` by default); bundled command sets are installed by reference from `~/repos/scramjet`; the 31 earlier choice entries (`git show 5c9b5b8:docs/TASKS.md`) were reviewed one by one: those not listed in items 2–6 stay as they are.
