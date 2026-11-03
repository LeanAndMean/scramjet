# Tasks

Pending work only (`CLAUDE.md`). What was done and how it was verified is in the commit messages; product choices awaiting the user's review are `Choice:` trailers in them.

Last steer: 5c9b5b8

## Current focus

None. `/rs:loop <focus>` records a user-given focus here until it is finished.

## Next candidates

Order agreed with the user on 2026-10-30 (`/rs:steer`); see the open question on commands.

1. **Use it and fix what hurts.** Run the agent on real work (practice runs, `CLAUDE.md`) and fix the first problems found. Next run: a real task within the cap on claude-haiku-5-5 (the last was gpt-6-luna); try Ctrl+O on its tool output.
2. **Commands.** Next, roughly by use in Mach 12: `delegate` (Scramjet's `delegate.ts`) and skills (`SKILL.md` discovery and `<available_skills>`, coding-agent `src/core/skills.ts`), both mattering mainly for the bundled sets (open question below), and `/autopilot` (Scramjet's 3-second countdown that selects the recommended step; it changes nothing for the `rs` commands, whose candidates all have `autonomy.yaml` edges).
3. **Terminal UI.** Continue the retained alternate-screen experience from the design beyond Ctrl+O's conversation view (`src/agent_cli/view.rs`): expanding or folding one tool block instead of all, search, and a selection that copies (native selection works now, as mouse reporting is off). New display code names its escape codes in `src/agent_cli/style.rs`, which the full-screen UI uses too ([design](DESIGN.md#native-interaction)).
4. **Parallel subagents** ([roadmap S3](ROADMAP.md)). Next: macOS (Seatbelt; untested platform); the design's admission and bounds for many children (shared request slots, no parent holding slots its children need); children that write (in a worktree, with the sandbox allowing that directory); keeping children's conversations for a resume or `/tree` (Scramjet keeps their messages in the result's details).
5. **Clean up.** Remove specialty code that no longer serves a consumer (haiku journey, fixed validation pair, `coding` invocation store) once nothing depends on it. Ask before deleting large user-visible commands.

## Known issues

Not scheduled; fix them when they matter.

- `bg` after Ctrl-Z during a turn stops the agent again at once: the SIGTSTP handler restores the turn's terminal mode, which a background job may not do (Scramjet's TUI cannot run in the background either). A reply rewritten in place after a lossy stream (`text_rows`) still counts rows from before a stop.
- Every start writes a session file before any prompt (Scramjet writes none before the first reply). `/resume` hides empty ones, but `--continue` picks one written last, which keeps the `/model`-switch-before-a-prompt behaviour agreed on 2026-10-30.
- Scramjet's edit tool falls back to fuzzy matching (NFKC, trailing whitespace, smart quotes, Unicode dashes and spaces, CRLF) and then writes the whole file normalized. Consider it if edits fail on such differences.
- The GPT overflow wording through Proxy is unverified (needs a request over 1.05M tokens; Scramjet's OpenAI pattern is `exceeds the context window`). deepseek-v4.1-flash's gateway answers an overflow with HTTP 400 `This model's maximum context length is 327680 tokens…` (its card says 320000).
- `claude-haiku-4-5-20251001` runs without thinking: its Proxy card has no `reasoning` field ("no reasoning control" in Proxy's card schema), though Scramjet's registry marks it as reasoning.
- A failing test can leave its directory in `/tmp`; only the provider history tests and the agent tests (`agent::tests::ScratchDir`) use a drop guard.

## Open questions

For the user, reviewed with `/rs:steer`. None blocks current work.

- Credentials (2026-11-02): in a claude-haiku-5-5 practice run, an agent's `cat ~/.scramjet/agent/*.json` printed an OAuth token, which went to the provider. As in Scramjet, nothing stops this in the main agent: its bash and `read` see every file. Subagents no longer see the common stores: they are empty in their sandbox, where `grep` and `find` run too, and `read`/`ls` refuse them (`src/agent/sandbox.rs`). Options: (a) hide known credential stores from bash, as `scripts/without-credentials.sh` does (bubblewrap over `/` with empty files and directories in their place), and refuse them in `read`/`grep`/`find`/`write`/`edit`; costs: needs `bwrap` for the main agent, `sudo` and other setuid programs stop working in its bash (bubblewrap sets `no_new_privs`), and a list never covers every secret; (b) the same only for subagents and unattended runs (a chained or forced next step, `-p`); (c) leave it as Scramjet does. Recommendation: (b), whose subagent half is done, then (a) behind a flag once used for a while. Rotating the exposed GitHub Copilot token is for the user.
- Bundled command sets (2026-11-02): Scramjet loads `mach12` and `scramjet` from its own package and ignores directories of those names elsewhere; scramjet-rs has no package, so `/mach12:…` is not available here. Options: load them from a Scramjet checkout or install given by a variable or found at a known path (e.g. `~/repos/scramjet/packages/scramjet`), copy them into scramjet-rs, or leave them out. Not blocking: the project's `rs` commands work. Also: commands (item 2) were taken ahead of the terminal UI (item 3), which changes the order agreed on 2026-10-30; reorder if wanted.
- Product choices recorded before `Choice:` trailers (31 entries, through `5c9b5b8`): `git show 5c9b5b8:docs/TASKS.md`, under `## Open questions`. Review them in the next `/rs:steer`, then remove this line.
