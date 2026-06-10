# Changelog

## 0.13.3 — Switch pr-pre-merge next-step policy to open

Changes `mach12:pr-pre-merge` from `ask` to `open` with two candidates: merge (`mach12:pr-merge`) when the checklist passes cleanly, and fix (`mach12:pr-review-fix`) when the checklist surfaces issues that warrant code changes (issue #99).

### Changed

- `mach12:pr-pre-merge` next-step policy switched from `ask` to `open` with candidates `mach12:pr-merge` and `mach12:pr-review-fix`, letting the agent recommend the appropriate follow-up after the checklist.
- Added status-reporting guidance requiring both candidates in `next_steps` with `recommended_next_step` based on checklist results.
- Vision doc wiring and Mach 12 wiring test updated to pin the new policy.

## 0.13.2 — Present all open-policy candidates in next_steps

Rewrites status-reporting instructions across all open-policy commands to always present every declared candidate with `recommended_next_step` instead of conditional single-entry logic (issue #97).

### Changed

- `mach12:issue-plan`, `mach12:issue-review`, `mach12:pr-review-assessment`, and `mach12:pr-review-fix` status-reporting sections rewritten from conditional single-entry to unconditional multi-entry with `recommended_next_step` index, so the agent always presents all declared candidates to the selector.

## 0.13.1 — Switch issue-review next-step policy to open

Changes `mach12:issue-review` from `ask` to `open` with two candidates: re-review (`mach12:issue-review`) when critical/important findings remain, and proceed (`mach12:issue-implement`) when the plan is approved (issue #93).

### Changed

- `mach12:issue-review` next-step policy switched from `ask` to `open` with candidates `mach12:issue-review` and `mach12:issue-implement`, letting the agent recommend a next step based on review findings.
- Added `scramjet_command_status` reporting section to guide candidate selection.
- Vision doc wiring table updated to reflect the new policy.

## 0.13.0 — Bounded per-stage quality review

Re-scopes the Phase 6 "Quality review" step in the implement-flow commands so per-stage review is a bounded, single-pass sanity check rather than an unbounded battery of specialized review subagents that re-fires until clean (issue #95). Comprehensive scrutiny is explicitly deferred to the full-branch `mach12:pr-review`. The change is prose-only coaching plus an explicit cap; no harness code changes (dispatch caps are not harness-enforced in the MVP).

### Changed

- `mach12:issue-implement` and `mach12:pr-review-fix` Phase 6 now cap per-stage review at **3 `mach12:code-reviewer` subagents total** (including any re-review), dispatched in a single parallel batch with focused briefs. Three is framed as a ceiling for unusually risky stages, not a quota — most stages need one or two, and trivial/low-risk stages may skip review entirely. The prose mandates a single pass (re-review only for non-trivial fixes that reworked a flagged area, counted against the same cap), forbids dispatching subagents to re-report or restate findings already in hand, and replaces the previous five-specialized-lens enumeration (`code-reviewer`, `test-analyzer`, `silent-failure-hunter`, `type-design-analyzer`, `code-simplifier`) with `mach12:code-reviewer` instances given focus briefs. The four specialized lenses remain fully covered at PR-review time by `mach12:pr-review`.

## 0.12.0 — Next-step selector routing

Adds selector-aware next-step routing for `closed` and `open` policies (issue #92). Scramjet now presents validated next-step options with labels, rationales, and a recommendation instead of treating the first valid command as the only handoff. With `/scramjet on`, a recommended command auto-selects after the countdown unless the user chooses another option or dismisses the selector; with `/scramjet off`, the selector remains manual-only.

### Added

- Selector payload support in `scramjet_command_status`: `next_steps[]` entries can now be command or free-text options, include selector-visible labels/reasons, and identify a zero-based `recommended_next_step`.
- Interactive selector routing for valid `closed` / `open` options, including manual free-text insertion for open-policy suggestions while preventing free-text from being auto-dispatched.
- Validation coverage for selector candidates, skipped invalid entries, and invalid recommendations without falling back to another option.

### Changed

- README and Mach 12 command guidance now describe selector options, recommendation rationale, and manual free-text choices instead of the previous single-pick countdown behavior.
- Auto-continuation routes completed `closed` / `open` transitions through the selector UI; `forced` handoffs remain direct and do not show the selector.

## 0.11.0 — Resumable `waiting_for_user`

Lets an interactive command that paused at `waiting_for_user` resume its lifecycle when the user answers, instead of treating the pause as terminal (issue #88). A command such as `mach12:pr-create` can now draft a PR, ask for approval, and — after the user approves and the command completes — offer its declared `mach12:pr-review` next step. The pause now also survives `pi --resume` / branch switch.

### Added

- A stable `waiting` lifecycle phase (`types.ts`): the only resting phase besides `idle`. `auto-continue.ts` parks a `waiting_for_user` report at `waiting` (keeping `activeTopLevelCommand`) instead of resetting to `idle`; `completed` / `blocked` / `incomplete` stay terminal.
- Forward resume (`history.ts`): an interactive, non-slash reply while a command rests at `waiting` flips the phase back to `running`, re-arming the existing `running → probing` probe so the resumed turn can later report `completed` and chain. A stray `agent_end` while `waiting` is a defensive no-op; exiting the workflow via an unknown slash drops `waiting → idle`.
- Rewind/resume reconstruction (`history.ts` + `command-status.ts`): each `scramjet_command_status` report is journaled as a `COMMAND_STATUS_TYPE` (`scramjet:command-status`) entry via `recordCommandStatus`; `replayHistory` reconstructs `waiting` on `session_start` / `session_tree` when the active command's last journaled status was `waiting_for_user`. Journaling *all* statuses (not just `waiting_for_user`) makes a command that waited, was answered, then completed without chaining reconstruct to `idle` — never resurrected.

### Changed

- Resume safety (amends the 0.10.0 note below): the transient phases (`running` / `probing` / `reported`) are still never journaled and self-heal to `idle` on `rebuild`, but the stable `waiting` halt is now reconstructed from the journaled command-status entries. Only the phase is reconstructed, never `latestCommandStatus`. Chaining still requires an explicit `completed` report, so an accidental or off-topic resume can only re-probe — never mis-chain — preserving the issue 84 safety properties (no status calls outside a probe, no infinite probe loop, no chaining after unresolved questions or blockers).

### Fixed

- Duplicate-dispatch on completed transitions (`auto-continue.ts`): the completed-transition dispatch was fired synchronously from the probe turn's `agent_end`, while Pi still counts the run as streaming. Pi expanded the slash command and queued its body as a follow-up, but the agent loop had already passed its follow-up polling point for the just-ending run, so the expanded body lingered stale in the queue and was delivered as a duplicate command body (no preceding `scramjet:command-start`) on a later unrelated turn. The single `routeCompleted` call site is now scheduled on a deferred tick (`scheduleCompletedDispatch`, `setTimeout(0)`), mirroring the existing probe deferral, so the next command dispatches exactly once as a clean new turn. The deferral also covers the no-UI `closed` / `open` path that dispatches immediately rather than through the deferred countdown, and the pending dispatch is torn down on `session_shutdown`.

## 0.10.0 — Two-phase command-status protocol

Replaces the single-turn, terminating `task_complete` tool with a two-phase `scramjet_command_status` protocol (issue #84): a command writes its normal user-facing answer first, then Scramjet probes for structured lifecycle status in a separate follow-up turn. This removes the failure mode where the agent poured its answer into the terminating tool's `summary` field instead of writing prose, and lays the groundwork for a future next-step choice-list UI.

### Added

- `scramjet_command_status` tool (`command-status.ts`): the agent's structured end-of-command report, supplied in a separate turn from the command's user-facing answer. Carries a `status` (`completed` / `waiting_for_user` / `blocked` / `incomplete`), a `summary`, an optional `user_prompt`, and a `next_steps[]` array (each entry: `name`, optional `args`, `fresh_session`, optional `label` / `reason`). The array shape carries candidates for the deferred choice-list UI. `execute` is harness-phase-gated — outside the probe window it returns a helpful error without terminating; in-phase it stores the report, advances the phase to `reported`, and terminates the short probe turn.
- `buildProbeMessage` (`next-step.ts`): builds the hardcoded status-check preamble wrapping the per-policy `<scramjet-next-step>` block, asking the agent to call `scramjet_command_status` and nothing else.
- Per-invocation lifecycle state on `ScramjetState`: `commandPhase` (`idle` / `running` / `probing` / `reported`) and `latestCommandStatus`.
- Differentiated handling of non-completed statuses: `blocked` warns, `waiting_for_user` (optionally echoing `user_prompt`) and `incomplete` pause quietly; only `completed` chains.

### Changed

- The command's answer turn no longer injects any completion/next-step instruction — the running turn is just the answer. After it goes idle, `auto-continue.ts` defers (after the run settles — `isStreaming` clears once `agent.prompt()` resolves — so `triggerTurn` reaches a fresh `agent.prompt()`) a TUI-hidden status-check message via `pi.sendMessage({ display: false }, { triggerTurn: true })` to start the probe turn, then routes on the probe turn's `agent_end`. Forced/closed/open validation and dispatch (including `forced` firing under `/scramjet off` and headless auto-follow) are preserved.
- The `next_steps[]` array replaces the singular `next_step`; auto-continue dispatches the first policy-valid entry. The agent-facing next-step strings and the bundled Mach 12 command prose now name `scramjet_command_status` / `next_steps`.
- Resume safety: `commandPhase` self-heals to `idle` on `rebuild` (resume / branch switch), so a stale post-resume `scramjet_command_status` call hits the phase guard instead of mis-dispatching. The phase is intentionally not journaled. (Amended in 0.11.0: the stable `waiting` halt *is* reconstructed on resume from journaled command-status entries; only the transient phases remain un-journaled.)
- `task-complete.ts` renamed to `command-status.ts`, with the `tsconfig.build.json` include entry and the `index.ts` registration (`registerTaskCompleteTool` → `registerCommandStatusTool`) updated to match.

### Removed

- The generic `task_complete` tool and its same-turn, summary-bearing completion shape, plus the now-dead `CompletionSignal` type.

## 0.9.0 — Base-prompt coding-agent directives

Appends a general coding-agent quality block to Pi's assembled system prompt on every turn.

### Added

- `base-directives.ts`: a `before_agent_start` hook that appends `SCRAMJET_BASE_DIRECTIVES` to Pi's assembled system prompt on every turn. The prose is adopted from a captured Claude Code CLI system prompt and product-neutralized per issue #78 — covering external/tool content as data not instruction (prompt-injection flagging), exploratory questions not triggering implementation, scope discipline, risky/hard-to-reverse/externally-visible actions requiring clear authorization, not retrying denied tool calls unchanged, and navigable `file_path:line_number` code references. Authorization for risky actions may come from the user, the active command's instructions, or durable project instructions (CLAUDE.md / AGENTS.md), preserving Scramjet's command-owned workflow model.
- Two Scramjet-specific reference blocks (orientation + feedback routing) as conditional self-knowledge modeled on Pi's own documentation section, with doc pointers (README, vision doc) resolved from the installed package root. The block returns only `systemPrompt` (composing cleanly with the next-step `message` injection), appends on top of any user SYSTEM.md, and is unconditional (flag-independent, like `pr-indicator.ts`).
- `docs/scramjet-vision.md` is now shipped in the npm package so the runtime doc pointer resolves in an installed copy.

## 0.8.0 — Active-PR footer indicator

Adds an ambient footer hint surfacing the current branch's open GitHub PR.

### Added

- `pr-indicator.ts`: an ambient footer hint that shows the current branch's active GitHub PR number (`PR #<n>`) via `ctx.ui.setStatus` when exactly one open PR matches the branch, and shows nothing in every other case (no PR, multiple PRs, unsupported remote, missing/unauthenticated `gh`, not a git repo). Detection uses `gh pr list --head <branch> --state open` with an exactly-one-match rule. Resolves on `session_start` / `session_tree` / `agent_end`, with the `agent_end` `gh` call gated behind a cheap local branch-diff. It is an opportunistic hint, not workflow state: nothing is journaled, nothing is added to `ScramjetState`, and it shows regardless of `/scramjet on|off`. A commented forge-swap seam marks where a future `glab` (`MR !<iid>`) branch would slot in.

## 0.7.0 — Mach 12 command effectiveness

Improves Mach 12 command and agent effectiveness against the approved tranche from #60, and lets `forced` transitions pass runtime context to their target.

### Added

- `forced` next-step argument handoff: a `forced` command can pass `args`/`fresh_session` to its declared target via `task_complete`'s `next_step`, without letting the agent redirect to a different target. A supplied `next_step.name` that does not match the forced target is ignored with a warning.
- Explicit `task_complete.next_step.args` guidance in the `closed` and `open` next-step instruction blocks, so follow-up commands receive the runtime identifiers they need.
- `mach12:pr-review` now parses review aspects, gathers changed-file context, maps to explicit Mach 12 review lenses, and aggregates findings structurally (ported from `pr-review-toolkit:review-pr`).
- `mach12:issue-review` gained F/S finding IDs and an independent assessment/classification pass.
- Issue-creation due diligence, behavior framing, and issue-quality self-checks in `mach12:issue-create`.

### Changed

- `mach12:code-simplifier` is now advisory/read-only; review agents gained project-guidance anchors and high-signal checks.
- `mach12:issue-implement` and `mach12:pr-review-fix` now allow prior planning/assessment to satisfy exploration/design when current and sufficient, and apply explicit quality-review lenses.

## 0.6.0 — Vision-alignment continuation semantics

Scramjet's command chaining now runs through Pi's normal slash/input pipeline, with fresh-session continuation, policy semantics, delegation scope, and history behavior aligned to the vision document.

### Added

- Next-step dispatch now uses Pi input dispatch instead of Scramjet locally expanding command bodies. Current-session continuations submit slash input through Pi, and fresh-session continuations use `ctx.newSession({ withSession })` plus replacement-context dispatch.
- Open-mode next steps can now target non-Scramjet slash commands; Scramjet passes them to Pi instead of requiring a Scramjet registry match.
- Added a focused `next-step-dispatch.ts` helper for current/fresh next-step dispatch and forced-origin cleanup.
- Delegated command invocations are now journaled with `depth > 0`, so persisted history contains both top-level and delegated command entries.
- First-level delegation now inherits the active top-level command's `allowed-tools` before intersecting with the callee's tool scope, preventing delegate escalation.

### Changed

- Scramjet now consumes the published LeanAndMean-patched Pi coding-agent package via npm alias: `@earendil-works/pi-coding-agent -> @leanandmean/pi-coding-agent@0.74.0-scramjet.1`. The patch is based on upstream Pi `0.74.0`; `@earendil-works/pi-tui` remains upstream `0.74.0`.
- The CI Pi-version drift guard now understands the patched dependency model (`piBaseVersion`, `piPatchFlavor`, and `piTestedVersion`).
- No declared `next:` now pauses like `ask` with no hint; the legacy no-policy free-form auto-follow path is removed.
- `forced` next steps now require the agent to call `task_complete` before Scramjet dispatches the forced target. Completed forced transitions still run under `/scramjet off`.
- `open` with `candidates: []` remains truly open/free-form and is no longer used as a terminus convention.
- Mach 12 `pr-merge` is now a terminus by omitting `next:` rather than declaring empty-open.
- Delegation's latched stack semantics are now explicitly documented and tested as the MVP behavior: frames do not pop within a turn, repeated same-subroutine calls are cycles, and sibling delegations inherit prior narrowing.

### Removed

- Removed the exposed internal `/scramjet-exec-fresh` command. Fresh-session continuation now uses Pi replacement-session dispatch directly.
- Removed local next-step command-body expansion from auto-continuation; body substitution remains only where intended for same-context delegation.

## 0.5.0 — Stage 8 cutover

Scramjet is now distributed as an npm package and ships its own Mach 12
command set. The Claude Code plugin compat layer is gone.

### Breaking changes

- Install path changed. Use `npm install -g @leanandmean/scramjet` instead
  of cloning the repo and running `./install.sh`.
- `install.sh`, `uninstall.sh`, and the `bin/scramjet` bash shim are removed.
  Distribution and lifecycle is now npm's job.
- The Claude Code plugin compat layer is removed. `install.sh` no longer
  clones `mach10`, `feature-dev`, or `pr-review-toolkit` into
  `~/.local/share/scramjet/` and no longer writes namespaced agent copies
  or command symlinks into the Pi agent dir. Mach 12 — bundled in this
  package, including its agents under `mach12/agents/` — is the canonical
  command set.
- `src/tool-aliases/` is removed. PascalCase Claude Code tool-name aliases
  (`Read`, `Bash`, `Edit`, `Write`, `Grep`, `Glob`, `LS`) are no longer
  registered. Mach 12 agents declare Pi-native lowercase tool names
  directly; commands authored against the PascalCase aliases will need
  their `tools:` lists rewritten.
- The `~/.pi/agent/models.json` Anthropic proxy setup that `install.sh`
  performed is no longer automated. Configure it manually: edit
  `~/.pi/agent/models.json` and add the `providers.anthropic.baseUrl` and
  `providers.anthropic.compat.supportsEagerToolInputStreaming: false`
  keys yourself if you route Pi through a proxy.
- Pi's "new Pi version available" startup banner is suppressed
  (`PI_SKIP_VERSION_CHECK=1` is set inside the scramjet bin). Scramjet
  pins Pi at `pi.piTestedVersion`; the upstream `pi update` flow would
  not update the embedded copy and following the prompt would only
  cause confusion.
- The TUI banner is rebranded from `pi vX.Y.Z` to `scramjet vX.Y.Z`.
  The scramjet bin builds a per-version shim package directory under
  `${XDG_CACHE_HOME:-$HOME/.cache}/scramjet/` on first launch so Pi
  sees scramjet's identity for the banner, title, agent-dir env var,
  docs path, and changelog while still resolving its own bundled
  themes and assets. Old shim directories from previous versions
  remain on disk as orphans; remove `~/.cache/scramjet/` to clear
  them. See `bin/env-setup.js` for the mechanism.
- The OS-level agent-dir / session-dir env var names change in lockstep
  with the rebrand: `PI_CODING_AGENT_DIR` becomes
  `SCRAMJET_CODING_AGENT_DIR`; the same for `PI_CODING_AGENT_SESSION_DIR`.
  The scramjet bin bridges the legacy names automatically — if you have
  `PI_CODING_AGENT_DIR` set in your shell profile, scramjet copies it
  into `SCRAMJET_CODING_AGENT_DIR` at startup so your custom agent dir
  keeps working without you renaming the variable.

### Migration from a bash-installed scramjet

1. From your old scramjet checkout: `./uninstall.sh --clear-manifest`.
   This removes the extension symlink, the launcher shim, and the
   plugin wiring the previous installer wrote into your Pi agent dir.
2. `npm install -g @leanandmean/scramjet`. The postinstall step seeds
   Mach 12 at `${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/mach12/`.
3. If a previous Stage 6 install already seeded
   `~/.local/share/scramjet/mach12/`, the postinstall preserves it; the
   contents may be from an older snapshot. Delete the directory before
   running `npm install` to get a fresh seed. If you set `SCRAMJET_CACHE`
   when running the old `install.sh`, the stale snapshot lives at
   `$SCRAMJET_CACHE/mach12/` instead; the new postinstall does not read
   `SCRAMJET_CACHE`, so delete both the stale snapshot and any old npm
   seed at `${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/mach12/` if
   you want a clean state.
4. The bundled plugin clones (`~/.local/share/scramjet/mach10/`,
   `~/.local/share/scramjet/claude-plugins-official/`) continue to work
   for whatever Pi-aware workflow you have wired manually, but are no
   longer cloned or refreshed by scramjet. Remove them with `rm -rf`
   once you're satisfied Mach 12 covers your needs.

### Added

- `bin/scramjet.js` (Node) entrypoint that embeds Pi via its library
  `main()` API and registers scramjet as an extension factory. Replaces
  the bash shim at `bin/scramjet`.
- npm `postinstall` script that idempotently seeds Mach 12 into
  `$XDG_DATA_HOME/scramjet/mach12/` (or `~/.local/share/scramjet/mach12/`
  if unset). Skipped on native Windows with a notice.
- `tsconfig.build.json` for the publish build (`npm run build` →
  `dist/`).

### Removed

- `install.sh`, `uninstall.sh`, `bin/scramjet` (bash shim).
- `src/install/transform.mjs` and `src/tool-aliases/` (Claude Code
  plugin compat layer).
- Plugin-wiring CI matrix (15+ steps); replaced with a single `npm pack`
  round-trip smoke job.
- `.scramjet-manifest` writing. With no symlinks to track, the manifest
  has no purpose.

## 0.4.0 and earlier

Pre-Stage-8 development is recorded only in git history. The legacy
`install.sh` workflow and the Claude Code plugin compat layer were
present from 0.1 through 0.4.
