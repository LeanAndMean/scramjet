# Changelog

## [Unreleased] — Model identity tracking for accurate GitHub attribution

Adds model identity tracking so agents have reliable model attribution without relying on self-knowledge. A new `model-identity.ts` module captures the initial model at session start, injects a stable `# Model Identity` block into the system prompt, and delivers change notifications on model switch via lifecycle-appropriate paths. Command prose in `pr-review` and `pr-review-assessment` updated to use harness-provided attribution instead of "identify yourself" directives (issue #163).

### Added

- `model-identity.ts` — tracks active model via `session_start` and `model_select` (500ms debounce), injects identity block into system prompt (cache-friendly, set once), delivers change messages via `input` transform (idle) or `before_agent_start` message return (active work). Reconstructs state on resume/fork from `ModelChangeEntry` session entries.
- `ModelRecord` type and `currentModel`/`modelHistory` fields on `ScramjetState`.
- `tests/model-identity.test.ts` — 39 tests covering system prompt injection, debounce, delivery paths, resume reconstruction, and probe-phase safety.

### Changed

- `mach12:pr-review.md` — model attribution directive now references the harness-provided Model Identity system prompt section.
- `mach12:pr-review-assessment.md` — same change.

## 0.22.4 — Pi API surface reference

Adds `docs/pi-api-surface.md`, a generated reference of all public exports from the four installed Pi packages (`pi-agent-core`, `pi-ai`, `pi-coding-agent`, `pi-tui`), kept in sync by a CI staleness guard (issue #168).

### Added

- `scripts/generate-pi-api-surface.js` — TypeScript compiler API script that resolves re-exports, follows aliases, and extracts full declaration text with docstrings into a per-package Markdown reference.
- `docs/pi-api-surface.md` — generated reference with per-package sections, exports grouped by source module, type signatures in fenced code blocks, and a version header.
- CI staleness guard — regenerates the file and fails on diff, ensuring the committed reference stays current with installed packages.
- `tests/pi-api-surface-generate.test.ts` — black-box coverage verifying script execution, package section presence, type signatures, model catalog exclusion, deterministic output, and version header.

### Changed

- CLAUDE.md dependency orientation section — directs agents to read `docs/pi-api-surface.md` before proposing new capabilities and to regenerate it on Pi version bumps.

## 0.22.3 — Fix stale freetext tool contract descriptions

### Fixed

- `docs/command-authoring.md` — probe-turn guidance now distinguishes confirm/select (same-turn continuation) from freetext (parks at `waiting`, resumes on next user reply).
- `user-input.ts` — `placeholder` schema description clarifies it is accepted for compatibility but unused by freetext.

## 0.22.2 — Show freetext user-input prompts

Freetext `get_scramjet_user_input` prompts now render the requested `message` in the tool call row before the command parks at `waiting`, so the user can see the question they need to answer in the standard editor (issue #166).

### Fixed

- `user-input.ts` — added a custom `renderCall` that displays the user-input prompt message while preserving the existing parked result and wait/resume lifecycle.
- `docs/command-authoring.md`, `docs/scramjet-vision.md` — documented visible freetext prompts and the current wait/resume semantics.

## 0.22.1 — Add test-designer agent and planning workflow integration

Adds a `mach12:test-designer` subagent that designs test strategies from requirements and architecture at planning time, distinct from the existing `mach12:test-analyzer` which reviews existing tests at review time. Integrates into the Mach 12 planning workflow with conditional dispatch, testability notes for bug reports, and soft test-first guidance during implementation (issue #159).

### Added

- `mach12/agents/mach12:test-designer.md` — new subagent providing per-test cost/benefit assessments, coverage intent categorization (problem verification / invariant protection / implementation completeness / regression prevention), and test-first recommendations for bug fixes.
- `mach12:issue-plan` Step 7 — conditional test-design step between architecture selection and plan drafting; dispatches `mach12:test-designer` for bugs, non-trivial features, and critical-path refactors.
- `mach12:issue-create` — `## Testability` section in bug reports noting reproducibility, assertions, and test type.
- `mach12:issue-implement` — soft test-first guidance in the Implementation phase when the plan's Test Strategy includes test-first directives.

## 0.22.0 — Eliminate redundant waiting_for_user path

Removes `waiting_for_user` from `report_scramjet_command_status` status enum. All "park for user input" flows now go exclusively through `get_scramjet_user_input` (freetext for unstructured, confirm/select for structured). The status tool accepts only `completed`, `continuing`, `blocked`, and `incomplete` (issue #156).

### Removed

- `waiting_for_user` status from `CommandStatusPayload.status` union and `STATUS_SCHEMA`.
- `user_prompt` field from the status tool parameters.
- `waiting_for_user` routing branch in `auto-continue.ts` (`routeNonCompleted`) and associated `reported → waiting-parked` handling.
- Competing-paths framing in the probe message (`buildProbeMessage`).

### Changed

- `user-input.ts` — freetext and cancellation paths now journal `scramjet:user-input-parked` entries directly (via `pi.appendEntry`) instead of calling `recordCommandStatus("waiting_for_user")`.
- `phase-machine.ts` — `reconstructPhase` recognizes `scramjet:user-input-parked` entries as the signal for `waiting` reconstruction.
- `history.ts` — exports `USER_INPUT_PARKED_TYPE` constant.
- `next-step.ts` — probe message lists 4 statuses (completed/continuing/blocked/incomplete); user-input tool described as the unified mechanism for all user input needs.
- 9 mach12 command files — replaced `waiting_for_user` guidance with `get_scramjet_user_input` (freetext) direction.
- `docs/command-authoring.md`, `docs/lifecycle-state-space.md`, `docs/scramjet-vision.md`, `CLAUDE.md` — updated to reflect single user-input path.

### Degradation

- Old `waiting_for_user` command-status journal entries from prior sessions are skipped; affected commands reconstruct to `dormant` (still resumable on user reply).

## 0.21.2 — Lifecycle state hardening via discriminated union

Refactors Scramjet's command lifecycle from independently-typed fields (`commandPhase`, `activeTopLevelCommand`, `latestCommandStatus`) into a discriminated `LifecycleState` union where each phase carries exactly the data it needs, making invalid state combinations unrepresentable at the type level, including excluding `continuing` from stored `reported` statuses (issue #135).

### Changed

- `types.ts` — replaced `commandPhase`, `activeTopLevelCommand`, `latestCommandStatus`, and `resetConsecutiveContinues` with a single `lifecycle: LifecycleState` field and optional `lifecycleTimers?: LifecycleTimerAccessors`.
- `phase-machine.ts` — added `LifecycleState` / `LifecycleEvent` types, pure `transition()` function, `getActiveCommand()` helper, and `assertInvariant()` validator. Removed legacy `transitionPhase()`, `LEGAL_TRANSITIONS`, bridge helpers.
- `auto-continue.ts` — migrated to discriminant narrowing on `state.lifecycle.phase`; timer observability exposed via state-attached accessors (`isProbeScheduled`, `isWatchdogActive`, `isDispatchScheduled`).
- `command-status.ts` — phase gate and continue counter now read from lifecycle variants; closure counter removed.
- `user-input.ts` — phase gate reads from lifecycle variants.
- `history.ts` — command-start, user-reply, workflow-exit, and replay reconstruction use lifecycle transitions.
- `delegate.ts` — reads active command via `getActiveCommand(state.lifecycle)`.

### Added

- `docs/lifecycle-state-space.md` — documents lifecycle dimensions, valid states, transition table, module ownership, and design rationale.
- Explicit `dormant` lifecycle phase for the "idle but command-associated" state (probe self-heal, replayed command starts), replacing the implicit `idle + activeTopLevelCommand !== null` combination.
- Cross-module integration smoke tests covering probe self-heal → dormant → resume, waiting → replay/resume → completion, continuing cycle limits, and structured user input during probing.

## 0.21.1 — Minimality pressure across planning, implementation, and review prompts

Adds the minimum-sufficient solution ladder and tailored minimality guidance to CLAUDE.md and 11 Mach 12 command/agent prompts (issue #150).

### Added

- `CLAUDE.md` — added the canonical minimal implementation discipline ladder, test proportionality guidance, safety exceptions, and dependency-orientation guidance.
- Mach 12 commands — added tailored minimality guidance across issue planning, implementation, issue review, PR review, PR review assessment, and PR review fixing.
- Mach 12 agents — added tailored minimality checks across simplification, architecture, testing, silent-failure, and feature-completeness lenses.

## 0.21.0 — Freetext user input terminates and parks at waiting

Freetext `get_scramjet_user_input` now returns `terminate: true` and parks at the `waiting` phase, so the user replies in the standard message editor instead of a single-line input widget (issue #147).

### Changed

- `user-input.ts` — freetext short-circuits before the UI block, returns `terminate: true`, and parks at `waiting`; `handleFreetext` removed entirely.
- `docs/command-authoring.md` — freetext section updated to document terminate-and-wait behavior.
- `CLAUDE.md` — architecture description updated for freetext flow.

## 0.20.3 — Refresh settings autonomy summaries

Settings submenus now reload autonomy config when opened and refresh parent summary values when exiting, so edge overrides remain visible throughout a settings session (issue #145).

### Fixed

- `settings-ui.ts` — per-command edge submenus read fresh autonomy config instead of captured snapshots.
- `settings-ui.ts` — command and top-level autonomy summaries update on Escape after submenu edits.

## 0.20.2 — Enrich pr-merge release notes context

`mach12:pr-merge` Step 5 now gathers the PR body, linked issues (with implementation plans via `mach12:gh-issue-read --marker mach12-plan`), and commit history before drafting release notes, producing richer drafts without user re-prompting (issue #141).

### Changed

- `mach12:pr-merge.md` — Step 5 reads PR title/body/commits and linked issues before drafting release notes; `delegate` added to `allowed-tools`.

## 0.20.1 — Terminate user input cancellation

Cancellation of `get_scramjet_user_input` now ends the current agent turn, parks the active command in `waiting`, and journals a `waiting_for_user` status for resume reconstruction (issue #142).

### Changed

- `user-input.ts` — cancellation from confirm/select/freetext prompts returns `terminate: true`, transitions active command phases to `waiting`, and records waiting command status when a top-level command is active.
- `phase-machine.ts` — allows `running → waiting` and `probing → waiting` transitions.
- User-input command authoring and vision docs now distinguish successful in-turn input from cancellation.

### Fixed

- `user-input.ts` — typecheck failure (TS2454) from uninitialized `result` variable in `finally` block; added `default` switch case and widened the type to `| undefined`.

## 0.20.0 — TUI settings widget

Interactive TUI widget for browsing and editing Scramjet settings, including per-edge autonomy overrides and the auto-continuation toggle, accessible via `/scramjet settings` (issue #138).

### Added

- `settings-ui.ts` — three-level `SettingsList` navigation: top-level settings (auto-continuation on/off) → command list (commands with `next:` policies) → per-edge autonomy overrides (chain/pause/default cycle).
- `/scramjet settings` subcommand with TUI environment check and tab-completion support.
- `saveAutonomyConfig()` write path in `autonomy-settings.ts` with atomic writes (temp file + rename), parent directory creation, cache invalidation, and cleanup semantics.
- Graceful handling: corrupt config shows warning and starts fresh, empty registry shows informational message, no-TUI environment shows error notification.
- Unit tests for settings UI item builders, edge display computation, config mutation semantics, and settings subcommand routing.

### Changed

- `scramjet-command.ts` — added `settings` subcommand routing alongside existing `on`/`off`/`status`.

## 0.19.0 — Edge-level autonomy settings

Per-transition autonomy settings that let users control which command chains auto-fire and which pause, at the granularity of individual edges. Users configure `~/.config/scramjet/autonomy.yaml` (XDG-respecting) to pin specific transitions to `chain` (always auto-dispatch) or `pause` (always show selector), while unconfigured edges follow the existing `/scramjet on|off` flag (issue #129).

### Added

- `autonomy-settings.ts` — settings loader with mtime-cached YAML parsing, edge lookup with wildcard fallback, and registry-aware validation diagnostics.
- Per-edge `chain` behavior: bypasses the selector entirely and dispatches immediately, regardless of `/scramjet on|off`.
- Per-edge `pause` behavior: forces the selector without auto-select or countdown, regardless of `/scramjet on|off`.
- Validation warnings on first dispatch when config references unknown command names.
- 26 unit tests covering parsing, lookup, caching, validation, and integration.

### Changed

- `auto-continue.ts` — edge setting lookup inserted after `validateNextSteps` and before the selector/dispatch decision in both UI and headless paths.
- `types.ts` — added `EdgeSetting`, `AutonomyConfig` types and `autonomyConfigPath` on `ScramjetState`.
- `package.json` — added `yaml` as direct dependency.
- `README.md` — added autonomy settings documentation section.

## 0.18.0 — Extend probe router and rename status/input tools

Adds a non-terminating `continuing` status for probe turns that need more work, renames the status and user-input tools to verb-first names, and tightens the probe lifecycle so commands can resume cleanly after structured input (issues #128 and #134).

### Added

- `continuing` status for `report_scramjet_command_status`, allowing a probe turn to transition back to `running` without terminating command work.
- Consecutive-continue bounding to prevent probe loops from continuing indefinitely.
- Integration coverage for continue flow, loop bounds, user-input during probe, watchdog behavior, and terminal-status regressions.

### Changed

- Renamed `scramjet_command_status` to `report_scramjet_command_status` and `scramjet_user_input` to `get_scramjet_user_input` across tools, bundled Mach 12 commands, and authoring documentation.
- Reworked the hidden probe message into a concise router that directs agents to either report status or request structured user input.
- Fixed phase re-arming and completed-command clearing so multi-turn command replies resume only active commands.

## 0.17.4 — Add `scramjet_user_input` tool for structured intra-command interactions

New tool that lets agents request confirm/select/freetext input from the user mid-turn without ending the turn. The harness shows TUI widgets, blocks until the user responds, and returns the result as a non-terminating tool result (issue #127).

### Added

- `user-input.ts` — `scramjet_user_input` tool with three interaction types: `confirm` (Yes/No/cancel via MultiLineSelectList), `select` (structured options with descriptions and recommended marker), `freetext` (open-ended input via `ctx.ui.input()`).
- Phase gating: tool accepts calls in `running` and `probing` phases only; out-of-phase calls return a helpful non-terminating error.
- Non-TUI guard: returns error without terminating when no TUI is available.
- Runtime validation of type-specific required fields (options for select, message non-empty, recommended in range).
- Probe watchdog suspension: suspends the 30s probe watchdog while awaiting UI during `probing` phase, then transitions back to `running` after the response so work can continue.
- Journaling: each interaction appended as `scramjet:user-input` custom entry type.
- `promptSnippet` on tool definition for system prompt visibility.
- `tests/user-input.test.ts` — 37 tests covering registration, phase gate, validation, UI interactions, watchdog coordination, and journaling.

### Changed

- `index.ts` — wires `registerUserInputTool(pi, state)` alongside other tools.
- `auto-continue.ts` — exposes `suspendProbeWatchdog`/`rearmProbeWatchdog` callbacks via state.
- `types.ts` — added optional watchdog callback fields to `ScramjetState`.
- `CLAUDE.md` — documents `user-input.ts` in architecture notes.
- `docs/command-authoring.md` — new section documenting `scramjet_user_input` for command authors.

## 0.17.3 — Restructure README for public npm audience

Rewrite README for external npm users: new framing, status notice, background section, motivation discovery arc, and removal of internal implementation details.

### Changed

- `README.md` — restructured for public npm audience; removed implementation internals, contributing/dev-setup section, and command authoring format details; added alpha status notice and background section connecting to Mach 10 origin.

## 0.17.2 — Vision doc: add §3 intra-command interactions

Add intra-command user interaction design to `docs/scramjet-vision.md` — the `scramjet_user_input` tool, the probe-as-router extension, the "continue" nudge, phase machine implications, auto-answer semantics, and the `/scramjet on` scope clarification (issue #126).

### Added

- `docs/scramjet-vision.md` §3 "Intra-command interactions" covering `scramjet_user_input` tool types (confirm/select/freetext), probe-as-router extension (continue/input/status), "continue" nudge for premature stops, relationship to `scramjet_command_status`, phase machine non-interaction, auto-answer semantics, design decisions, and non-goals.
- `docs/scramjet-vision.md` §5 new subsection "Scope: between-command chaining only" clarifying that `/scramjet on|off` does not gate intra-command interactions.

### Changed

- `docs/scramjet-vision.md` — renumbered §3-§7 → §4-§8; updated all internal §-number cross-references.
- `docs/scramjet-vision.md` §2.1 — added cross-reference distinguishing `scramjet_user_input` (proactive mid-turn) from `waiting_for_user` (turn-ending lifecycle status).

## 0.17.1 — Centralize commandPhase state machine transitions

Extracts all `commandPhase` mutation logic into a new `phase-machine.ts` module with a validated transition table, replacing 13 direct assignments scattered across `history.ts`, `command-status.ts`, and `auto-continue.ts` (issue #121).

### Added

- `phase-machine.ts` — `LEGAL_TRANSITIONS` adjacency map, `transitionPhase()` with auto-clear of `latestCommandStatus` on →idle, and `reconstructPhase()` for rebuild/resume derivation.
- `tests/phase-machine.test.ts` — covers legal/illegal transitions, self-transitions, auto-clear, and `reconstructPhase` derivation.

### Changed

- `history.ts`, `command-status.ts`, `auto-continue.ts` — all direct `state.commandPhase =` assignments replaced with `transitionPhase()` calls.
- Removed duplicated `COMMAND_STATUS_TYPE` scanning logic from `history.ts` (now delegated to `reconstructPhase`).

## 0.17.0 — Inject subagent catalog into system prompt

Adds an agent-catalog module that injects available subagent names and descriptions into the system prompt, enabling commands with open-ended agent selection to discover agents before dispatching (issue #119).

### Added

- `agent-catalog.ts` — `buildAgentCatalogBlock()` formats the agent registry alphabetically; registered via `before_agent_start` hook.
- `tests/agent-catalog.test.ts` — unit tests for empty registry, formatting, sorting, hook registration, and prompt composition.
- CLAUDE.md architecture documentation for the new module.

## 0.16.0 — Agent-discoverable command authoring documentation

Adds a comprehensive command authoring guide and a centralized doc path resolution module, so agents can discover authoring conventions from the system prompt (issue #111).

### Added

- `docs/command-authoring.md` — authoring guide covering frontmatter schema, next-step policies, same-name-different-args pattern, delegation, tool scoping, status-reporting conventions, and selector transparency.
- `docs-registry.ts` — centralized doc path resolution module replacing inline path variables in `base-directives.ts`.
- Authoring doc pointer wired into the system prompt via `base-directives.ts` with conditional-read instruction.
- CLAUDE.md documentation sync directive for the authoring doc.
- README reference to the new authoring guide.

### Changed

- `package.json` `files` array broadened from `"docs/scramjet-vision.md"` to `"docs/"` to ship the whole docs directory.

## 0.15.0 — Unified next-step message schema; same-command-different-args

Redesigns the `next_steps` schema from a discriminated union (`CommandStatusCommandNextStep` / `CommandStatusFreeTextNextStep`) to a single flat `{ message, fresh_session?, reason? }` shape, and adds support for multiple entries that invoke the same command with different arguments (issue #108).

### Changed

- `scramjet_command_status` `next_steps[]` entries now use a single `message` field instead of `command`/`name`/`args`/`text`/`label`/`type`. A leading `/` makes the message a slash command; anything else pastes into the editor (open policies only).
- Removed the `label` field: the selector always shows the exact message that will run.
- Added `parseSlashCommand` to `commands/validator.ts` for harness-side `/` prefix parsing.
- Simplified all four policy-mode instruction blocks in `next-step.ts`.
- Rewrote status-reporting prose in 9 Mach 12 command files to the `message` form.
- `mach12:pr-review-assessment` demonstrates same-command-different-args with conditional genuine-only vs genuine+nitpicks fix variants.

### Added

- Same-command-different-args pattern: multiple `next_steps` entries may suggest the same command with different arguments. Documented in README.
- `parseSlashCommand` function and corresponding validator tests.

### Removed

- `CommandStatusCommandNextStep` and `CommandStatusFreeTextNextStep` type aliases.
- `label` field from next-step entries.

## 0.14.0 — Multi-line layout for next-step selector

Adds `MultiLineSelectList` component that renders selector items with the full command on line 1 and reason text indented below, improving readability for long command args and descriptions (issue #107).

### Added

- `MultiLineSelectList` component with word-wrapping, per-field line cap (4 lines with `…` truncation), variable-height scrolling, and keyboard navigation with wrap-around.
- 27 unit tests for the new component.

### Changed

- Next-step selector uses `MultiLineSelectList` instead of `SelectList`, showing reason text on a separate indented line.
- `[recommended]` tag moved from the command label to the description line.

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
