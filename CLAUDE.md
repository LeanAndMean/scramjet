# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository structure

Scramjet is a product monorepo. Pi runtime packages are vendored in `packages/` and modified directly where appropriate. Scramjet is not an extension of Pi — it IS the product; Pi is the runtime it uses.

```
scramjet/
├── packages/
│   ├── tui/            Pi runtime: terminal UI
│   ├── ai/             Pi runtime: LLM providers
│   ├── agent/          Pi runtime: agent loop, state
│   ├── coding-agent/   Pi runtime: CLI, tools, sessions
│   └── scramjet/       The product: commands, orchestration
│       ├── src/
│       ├── tests/
│       ├── bin/
│       ├── mach12/
│       └── docs/
├── UPSTREAM_DIVERGENCE.md   Tracks Pi modifications for upstream sync
├── .github/workflows/
└── package.json             Root workspace config
```

## Commands

```sh
npm run typecheck    # tsgo --noEmit (all packages)
npm run build        # topological build: tui -> ai -> agent -> coding-agent -> scramjet
npm test             # vitest --run across all workspaces
npm run lint         # biome check .
npx vitest run packages/scramjet/tests/command-status.test.ts   # single test file
```

CI runs typecheck, build, test, lint, an `npm pack` round-trip smoke (verifies tarball contents and probes `scramjet --help`), and a postinstall smoke (against a temporary `XDG_DATA_HOME`) on ubuntu and macos.

## Local development

Scramjet ships as the npm package `@leanandmean/scramjet`. The `scramjet` bin on PATH runs the compiled `dist/index.js`, NOT the TypeScript source. That introduces a staleness trap for Scramjet source edits.

**One-time dev setup (after `npm install`):**

> **Migrating from the single-repo layout?** If you previously symlinked `mach12/` from the repo root (before the monorepo migration), remove the stale symlink first: `rm "${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/mach12"`

```sh
npm run build          # produce dist/ for all packages
npm link -w packages/scramjet   # install `scramjet` globally as a symlink
ln -sfn "$(pwd)/packages/scramjet/mach12" "${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/mach12"
                       # so edits to mach12/*.md files are picked up live
```

**Iteration:**

- **Edited a `.ts` file in `packages/scramjet/src/`** -> `npm run build` (or run the build in watch mode). **This is the most common "am I testing my changes?" confusion** — if you edit, run `scramjet`, and see old behavior, suspect a stale `dist/` before suspecting anything else.
- **Edited a Pi runtime package** (`packages/{tui,ai,agent,coding-agent}`) -> `npm run build` (the full topological build, since Scramjet depends on all four).
- **Edited `mach12/*.md`** -> no rebuild needed if the mach12 symlink is in place.
- **Edited `bin/scramjet.js` or `scripts/postinstall.js`** -> no rebuild needed; they're `.js` and run as-is.
- **To verify which scramjet is on PATH:** `readlink -f "$(which scramjet)"` should resolve into this repo's working tree (via npm's global `lib/node_modules/@leanandmean/scramjet/bin/scramjet.js`).

## Formatting

Biome: tabs, indent width 3, line width 120. Run `npx biome check --write .` to auto-fix.

## Documentation sync

When modifying command frontmatter schema (`commands/loader.ts`, `commands/parse-next-step.ts`), delegation behavior (`delegate.ts`), tool scoping (`delegate.ts`, `tool-scope-advisory.ts`), status reporting (`command-status.ts`), lifecycle facts (`lifecycle.ts`), or next-step dispatch (`auto-continue.ts`, `commands/validator.ts`), update `packages/scramjet/docs/command-authoring.md` to reflect the change. When modifying lifecycle state structure or transitions, also update `packages/scramjet/docs/lifecycle-state-space.md`. When modifying log categories, levels, or lifecycle event instrumentation, update `packages/scramjet/docs/logging.md`. The authoring doc is agent-facing — inaccurate guidance produces malformed commands.

When diagnosing harness misbehavior (probe didn't fire, command didn't chain, unexpected pause), consult `packages/scramjet/docs/logging.md` for the entry schema, `jq` query patterns, and diagnostic workflow.

## Architecture

Scramjet is a product monorepo. The `bin/scramjet.js` entry point calls Pi's library `main(argv, { builtinInit: initScramjet })`; Pi loads Scramjet as a builtin before any disk-discoverable extensions. The product publishes five packages: four Pi runtime packages (`@leanandmean/{tui,ai,agent,coding-agent}`) and the product package (`@leanandmean/scramjet`).

`packages/scramjet/src/index.ts` constructs a single `ScramjetState` (registry, agent registry, delegate stack, sidebar log, `/scramjet on/off` flag, pending forced dispatch, `lifecycle: LifecycleState` — see `types.ts` and `lifecycle.ts`, `lifecycleGeneration: number`, `logger: ScramjetLogger` — see `logger.ts`) and threads it through every register-call. Each capability is its own file; nothing in the harness imports anything outside `types.ts` and Pi's API.

**Vision MVP harness modules** (issue 23 buildout):

- `commands/loader.ts` + `commands/parse-next-step.ts` + `commands/validator.ts` — pure-function parser, next-step policy reader, and pre-dispatch validator. `commands/index.ts` is the discovery wiring: a single `resources_discover` hook that scans the seeded global root and the per-cwd project root, builds the command registry (returned to Pi via `promptPaths`) and the agent registry (consumed locally by the bridge), then calls `ensureAgentBridge` to wire agents into Pi's subagent dispatch path.
- `commands/agent-bridge.ts` — symlinks each registered subagent into `<getAgentDir()>/agents/<name>.md` so Pi's upstream subagent example extension can discover them. Pi has no `agents_discover` hook or `registerAgent` API; the subagent example scans `~/.scramjet/agent/agents/` directly, and the documented install method (per its README) is symlinking. Idempotent, ownership-tracked (a symlink is only treated as scramjet-owned when its resolved target falls under one of the scanned scramjet roots), and self-pruning of dangling scramjet-owned symlinks. Skipped on native Windows for the same reason as `scripts/postinstall.js` (symlinks need admin/developer mode).
- `lifecycle.ts` — defines the `LifecycleState` fact interface (orthogonal booleans: `activeCommand`, `probeArmed`, `probeInFlight`, `parkedForInput`, `continueCount`, `lastReport`) with runtime invariant checks, query helpers (`isDormant`, `isProbeDue`, `isProbeInFlight`, `hasTerminalReport`, etc.), and named mutation helpers (`startCommand`, `clearActiveCommand`, `enterDormant`, `armProbe`, `beginProbe`, `acceptProbeContinuing`, `acceptDormantContinuing`, `acceptTerminalReport`, `parkForFreetext`, `resumeFromParkedInput`, `resumeAfterProbeInput`). Every mutation validates invariants, bumps `lifecycleGeneration`, and logs via `logger.lifecycle()`. See `docs/lifecycle-state-space.md` for the full fact structure, invariants, and design rationale.
- `delegate.ts` — registers the `delegate` tool. The tool looks up a command in the registry, intersects `allowed-tools` with the active top-level command scope for the first delegate (or caller frame for nested delegates), pushes a frame onto the delegate stack (latched scoping; frames never pop within a turn), journals an indented `agent`-origin history entry, and returns the substituted command body as tool result content. Detects cycles. Resets the stack on `before_agent_start`.
- `next-step.ts` — pure block builder. Exports `buildNextStepBlock`, which renders the `<scramjet-next-step>` policy block for a given `next:` policy (one branch per `forced` / `closed` / `open` / `ask` mode, with close-tag escaping for prompt-injection safety), and `buildProbeMessage`, which wraps that block in the hardcoded status-check preamble that asks the agent to call `report_scramjet_command_status`. The probe message is sent (deferred) by `auto-continue.ts`; the `agent_end` dispatcher that reads the reported status/pick and fires the next command also lives in `auto-continue.ts` (see below).
- `history.ts` — appends sidebar entries (`▸` user, `●` agent, `■` forced; delegated entries currently use `agent` origin at `depth > 0`) to the persistent history journal and replays the journal at `session_start` so `/scramjet on/off`, the active top-level command, and the log survive `pi --resume`. Depth > 0 replay entries do not replace the lifecycle's active command. It also journals each `report_scramjet_command_status` report as a `COMMAND_STATUS_TYPE` (`scramjet:command-status`) entry via `recordCommandStatus`; `replayHistory` reads those entries (last-status-wins, scoped to the active command) and uses `lifecycle.ts` helpers to restore the appropriate stable lifecycle facts (`parkedForInput` or dormant) on resume (issue 88 Stage 2 — see the auto-continuation bullet).
- `tool-scope-advisory.ts` — `tool_call` hook that logs a warning via `state.logger.warn("scope", ...)` when the active frame's `effectiveAllowedTools` excludes the called tool. Advisory only; never blocks. Hard enforcement is deferred (see "Design philosophy" below).
- `subagent-output-advisor.ts` — `tool_result` hook that logs a warning via `state.logger.warn("subagent", ...)` and appends a session-only sidebar entry when the upstream subagent example tool returns a literal `(no output)` payload. Surfaces the silent-failure mode the upstream tool produces when a subprocess exits cleanly with no assistant text (crash before stdout flush, unknown model id, config error). Advisory only; never modifies the tool result.

**Auto-continuation core** (originated pre-MVP; reshaped into the two-phase command-status protocol by issue 84):

- `command-status.ts` + `auto-continue.ts` — the harness mechanism that drives next-step dispatch via a two-phase protocol. The command produces its normal user-facing answer first; nothing about completion is injected into that turn. `auto-continue.ts` listens on `agent_end`: when the lifecycle facts show a probe is due (`probeArmed && !parkedForInput`), it transitions to `probeInFlight` via `lifecycle.ts` and **defers** (via `setTimeout(0)`, after the run settles and `isStreaming` clears when `agent.prompt()` resolves) a TUI-hidden status-check message built by `buildProbeMessage`, sent through `pi.sendMessage({ display: false }, { triggerTurn: true })` to start a short probe turn. `command-status.ts` registers the `report_scramjet_command_status` tool; its `execute` is lifecycle-gated — it accepts terminal statuses when `probeInFlight` is true or the command is dormant, and accepts `continuing` in two cases: during a probe (increments `continueCount`, re-arms probe) or while dormant (resets counter, arms probe). Terminal statuses (`completed`/`blocked`/`incomplete`, plus `summary` and a `next_steps[]` array) are stored in `lastReport`, journaled, and returned with `terminate: true`. On the probe turn's `agent_end`, `auto-continue.ts` routes by `lastReport`: `completed` clears the active command and dispatches mode-by-mode (`forced` fires unconditionally even under `/scramjet off`; `closed`/`open` honor the first valid `next_steps` entry when `/scramjet on`; `ask` pauses); `blocked` enters dormant and warns; `incomplete` enters dormant quietly. A command parked via `get_scramjet_user_input` freetext stays associated with its invocation (`parkedForInput = true`), and an **interactive, non-slash** reply (handled in `history.ts`'s `input` handler) resumes from parked input, re-arming the probe so the resumed command can later report `completed` and offer its declared next step. Chaining still requires an explicit `completed` report, so an accidental resume can only re-probe — never mis-chain. Dormant commands (no mode flags active, command still associated) do **not** auto-resume on user reply — dormant commands can either call `continuing` via the status tool (to re-enter the probe cycle for more work) or report a terminal status directly (when work is already done). A probe `agent_end` with no report self-heals to dormant (not idle), preserving the command association. Exiting the workflow via an unknown slash clears the active command and timers. On `rebuild` (resume / branch switch) the transient facts (`probeArmed`, `probeInFlight`, `lastReport`) are never restored, but the stable halts are: `user-input.ts` journals `scramjet:user-input-parked` entries and `replayHistory` uses `lifecycle.ts` helpers to restore parked (`parkedForInput = true`, when a parked entry exists for the active command) or dormant (when a command start is associated but no parked entry is active), so paused or self-healed commands survive `pi --resume`. Journaling all statuses is what makes a command that waited, was answered, then completed **without** chaining reconstruct to idle — never resurrected — since that path writes no subsequent command-start. A stale post-resume tool call still hits the lifecycle guard rather than mis-dispatching; only the stable facts are reconstructed, never the full status payload.

**Independent capabilities:**

- `scramjet-command.ts` — the `/scramjet on|off` slash command.
- `clear-alias.ts` — `/clear` alias.
- `diagram/` — registers `draw_diagram` unconditionally with a custom Mermaid renderer (parser + integer-grid layout + A* edge routing). Supports flowchart/graph and stateDiagram-v2. Theme colors applied via `CharRole` → `ThemeColor` mapping at render time. No external dependencies.
- `subagent/` — registers the `subagent` tool as a builtin (`registerSubagentTool(pi)`), enabling agent dispatch to specialized subagents via isolated subprocess invocations. Copied from `packages/coding-agent/examples/extensions/subagent/` with branding fixes (`getPiInvocation` fallback, temp-dir prefix). No `ScramjetState` dependency; same posture as `diagram/`. Agent discovery reads from `getAgentDir()/agents/` (populated by the agent-bridge) and optional project-local `.scramjet/agents/`. The `subagent-output-advisor.ts` hook watches for silent `(no output)` failures from this tool.
- `pr-indicator.ts` — ambient footer hint showing the current branch's active GitHub PR number (`PR #<n>`) via `ctx.ui.setStatus` when exactly one open PR matches; shows nothing in every other case (no/multiple PRs, unsupported remote, missing/unauthenticated `gh`, not a git repo). Resolves on `session_start` / `session_tree` / `agent_end` (the `gh` call on `agent_end` is gated behind a cheap local branch-diff). An opportunistic hint, not workflow state: nothing is journaled, nothing is added to `ScramjetState`, and it shows regardless of `/scramjet on|off`. This footer `setStatus` text is a different Pi primitive from — and not a violation of — the sidebar UI panel deferred in `docs/scramjet-vision.md` section 5. A commented forge-swap seam marks where a future `glab` (`MR !<iid>`) branch would slot in.
- `agent-catalog.ts` — injects the available subagent catalog into the system prompt via `before_agent_start`, enabling the calling agent to discover agent names and descriptions before dispatching. Reads `state.agentRegistry` (populated by the command-set loader's agent bridge), formats a sorted list of `name: description` entries, and appends only when the registry is non-empty. Returns only `systemPromptSection` (id `scramjet:agent-catalog`, no `message`), unconditional (flag-independent, same posture as `base-directives.ts`). Exports a pure `buildAgentCatalogBlock(registry)` for testability alongside the hook registration function.
- `user-input.ts` — registers the `get_scramjet_user_input` tool, which lets agents request structured user input during command execution. Supports three interaction types: `confirm` (yes/no via `MultiLineSelectList`, distinguishing explicit No from Escape/cancel), `select` (structured options with descriptions and `recommended` index via `MultiLineSelectList`), and `freetext` (parks the command so the user replies in the standard editor). A custom `renderCall` displays the freetext prompt `message` in the tool row so the user can see the question alongside the parked result. A `renderResult` hook persists the prompt message and outcome in the tool-result row after completion or cancellation; for select interactions, the rendered result includes the full options array (labels and descriptions). Available in all lifecycle phases except reported (when a terminal status report is pending dispatch); returns a non-terminating error for report-pending and for confirm/select in non-TUI mode. No lifecycle mutations occur when called outside active command work. Runtime-validates type-specific required fields (e.g., `options` for select, non-empty `message`). Confirm/select called during a probe suspend the probe watchdog via `state.suspendProbeWatchdog()` before awaiting UI, then resume after probe input (clearing `probeInFlight`, re-arming probe, preserving `continueCount`) so command work can continue and the next `agent_end` schedules a fresh probe. Confirm/select cancellation (Escape) returns `{ cancelled: true }` and terminates, entering dormant; freetext always returns `terminate: true`, parks the command (`parkedForInput = true`), journals a `scramjet:user-input-parked` entry when a top-level command is active, and records only the prompt. Each interaction is journaled as a `scramjet:user-input` custom entry type via `pi.appendEntry()`. Advertised via `promptSnippet` on the tool definition.
- `base-directives.ts` — contributes a general coding-agent quality block (`SCRAMJET_BASE_DIRECTIVES`) to the system prompt on every turn via `before_agent_start`. The prose is adopted from a captured Claude Code CLI system prompt and product-neutralized (issue 78). It returns only `systemPromptSection` (id `scramjet:base-directives`, no `message`, so it composes cleanly with any other extension's `before_agent_start.message` return), and is unconditional (flag-independent, same posture as `pr-indicator.ts`, so no `ScramjetState` is threaded). Two Scramjet-specific reference blocks (orientation + feedback routing) are conditional self-knowledge with doc pointers resolved from the installed package root via a module-relative `package.json` walk.
- `logger.ts` — `createLogger(pi)` factory producing a `ScramjetLogger` with `warn()`, `debug()`, and `lifecycle()` methods. All calls journal a `scramjet:log` custom entry via `pi.appendEntry()`; `warn()` additionally writes to stderr when no TUI is detected (`!hasUI`). `hasUI` is set on `session_start`. See `docs/logging.md` for the entry schema, categories, query patterns, and diagnostic workflows.
- `model-identity.ts` — tracks the active model's identity and injects it into agent context so commands can produce accurate GitHub attribution without relying on LLM self-knowledge. Registers `session_start` to capture the initial model, contributes a stable `# Model Identity` section via `systemPromptSection` (id `scramjet:model-identity`, content set once from the initial model — same string = same cache prefix) on every `before_agent_start`, and handles `model_select` events with a 500ms debounce to deliver change notifications via two lifecycle-appropriate paths: `input` transform (prepends `[scramjet] Model changed to: ...` before user text when idle/parked/dormant) or `before_agent_start` message return (with "Please continue" when the agent is actively working). Probe safety: skips message injection when `probeArmed` or `probeInFlight` is true to avoid interfering with the two-phase command-status protocol. On resume/fork/branch-switch, reconstructs `currentModel` and `modelHistory` from `ModelChangeEntry` session entries and detects model divergence (stores as pending input transform). State: `currentModel: ModelRecord | null` and `modelHistory: ModelRecord[]` on `ScramjetState`; debounce timer and pending flags are closure-local. Always-on (flag-independent). Exports `buildModelIdentityBlock` and `reconstructModelState` for testability.
- `subdir-context.ts` — discovers `CLAUDE.md` and `AGENTS.md` from subdirectories and injects them as first-class `read` tool calls into assistant messages via a `message_end` handler. When an assistant message contains `read` tool calls, the handler checks intermediate directories between cwd and the read file's directory (inside-cwd reads, capped at `MAX_DEPTH=10`) or only the immediate target directory (outside-cwd paths: absolute paths outside cwd, `~/`-prefixed outside cwd, or relative escapes). Resolves realpaths for symlink safety (skips directories whose realpath falls outside cwd for inside-cwd paths), deduplicates by directory realpath in `state.subdirLoadedPaths`. Injects normal `read` tool-call blocks immediately before each triggering read in the assistant message; injected IDs are `scrctx-` prefixed and derived from `createStableId(sourceToolCallId + '\0' + displayPath)`. The `beforeToolBatch` runtime hook ensures the async `message_end` mutation completes before the agent loop extracts tool calls. The standard `read` tool then executes the injected reads, producing normal TUI rows and persisted session entries. No custom journal entries are written; `session_compact` clears `subdirLoadedPaths`; `session_start`/`session_tree` reconstruct dedupe state from successful standard read call/result pairs in the session (candidate-file reads matched to non-error tool results). Bounded by `MAX_DIRS=20` (directory cap) and `MAX_DEPTH=10` (path depth cap). Error-discriminating: suppresses ENOENT silently, logs non-ENOENT errors via `state.logger`. Flag-independent (loads regardless of `/scramjet on|off`). Exports functions (`directoriesToCheck`, `discoverContextFilePaths`, `createStableId`, `reconstructSubdirState`) for testability.

**Bundled Mach 12 command set** (`mach12/`):

The tenant of the harness — `mach12/commands/*.md` are command files using the next-step declarations and delegation. Ten top-level commands (`mach12:issue-create`, `mach12:issue-plan`, ..., `mach12:pr-merge`) with top-level `next:` blocks where chaining is intended (`forced`/`closed`/`open`/`ask`); `mach12:pr-merge` intentionally has no `next` and is the default terminus. Seven delegate-only subroutines (`mach12:push`, `mach12:find-contribution-guidelines`, `mach12:gh-issue-read`, `mach12:gh-pr-read`, `mach12:gh-sub-issues`, `mach12:gh-assign`, `mach12:gh-comment`) are invoked via `delegate` from the top-level commands. Subroutines have no `next:` block — the caller's `next:` controls chaining. `mach12/agents/*.md` ships ten bundled subagents (exploration, architecture, code review, comment analysis, test analysis, test design, silent-failure analysis, type-design analysis, feature-completeness checking, code simplification) that the multi-lens commands dispatch to. The npm `postinstall` script seeds the whole `mach12/` tree into `${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/mach12/` on install; the command-set loader picks it up at runtime via `resources_discover`. `gh-*` subroutines are flagged in their prose as forge-swap points for the deferred `glab-*` family.

**Distribution:**

`bin/scramjet.js` is a small Node entry that imports the compiled `dist/index.js` and calls Pi's `main(argv, { builtinInit: initScramjet })`. `scripts/postinstall.js` runs on `npm install` and idempotently seeds the bundled Mach 12 tree (skipped on native Windows with a notice; failure prints a warning but never blocks the install).

**Upstream Pi divergence:**

Pi packages are vendored from the LeanAndMean fork of upstream Pi (base version 0.74.1). Behavioral modifications to Pi source are marked with `// SCRAMJET-DIVERGENCE:` comments. See `UPSTREAM_DIVERGENCE.md` for the complete divergence inventory and cherry-pick workflow.

## Project direction

The architecture section above describes the **current** shape of the code. The **target** shape is laid out in `packages/scramjet/docs/scramjet-vision.md`, which is the source-of-truth design document for the next major rewrite (the "vision MVP"). Consult the vision doc when:

- Planning work that introduces, removes, or reshapes a harness capability (command sets, next-step declarations, delegation, the `/scramjet on/off` flag, history journaling).
- Deciding what is in scope vs. deferred for the MVP. The vision doc carries the MVP-vs-post-MVP boundaries and the per-section deferrals (sidebar UI, hard tool-scoping enforcement, authoring loop).
- Resolving "should we add X?" questions about the harness — the vision doc states the non-goals as well as the goals, and several common asks (workflow DAG, conditional next-step DSL, prose-replacement abstractions) are explicit non-goals.
- Reviewing a design decision and wanting to know what was already considered and rejected, and why.
- Reviewing or planning work that touches design principles, harness behavior, or command-set conventions — the elaborated principles section grounds decisions with context, examples, and counterexamples.

The MVP buildout shipped under GitHub issue 23 (umbrella). Subissues 24-33 carried the individual stages; the staged plan and per-stage progress comments live on issue 23 for the historical record. Post-MVP work (sidebar UI, hard tool-scoping enforcement, authoring loop) is tracked as separate issues — consult the vision doc for the deferred-scope catalog. The CLAUDE.md design-philosophy section below was rewritten to match the vision (commands declare their edges, MVP-specific rationales are explicit); when those bullets reference design decisions you don't recognize, the vision doc is where the long-form reasoning lives.

## Design philosophy

These principles override default instincts. Do not add complexity that violates them.

- **Emergent over prescribed.** Workflows emerge from edges in each command's instructions, not from centralized definitions. Don't add workflow registries, DAG configs, or state machines.
- **Zero lock-in.** The user can press Escape at any transition and be back in normal Pi. No workflow state persists. Don't add resumable state, queues, or progress tracking across sessions. *Exception (issue 88 + issue 135 + issue 156 + issue 215): a command parked via `get_scramjet_user_input` (freetext) sits at a stable parked state (`parkedForInput = true`), and a probe self-heal or blocked/incomplete status parks at dormant (no mode flags active, command still associated); both are reconstructed on `pi --resume` / branch switch via `lifecycle.ts` helpers, so the user can answer and resume the in-flight command. These are the two authorized resumable halts — only the lifecycle facts and command identity are reconstructed (never `probeArmed`, `probeInFlight`, `lastReport`, or `continueCount`), the transient mode flags are deliberately not journaled, and a command that already completed reconstructs to idle and never re-fires. Dormant commands resume through explicit `continuing` via the status tool (to re-enter the probe cycle for more work) or by reporting a terminal status directly (when work is already done); they do not auto-resume on user reply. See the auto-continuation bullet in the architecture section, `docs/lifecycle-state-space.md`, and `docs/scramjet-vision.md` section 2.1. Don't generalize this into broader cross-session workflow persistence.*
- **Invisible when idle.** If Scramjet has nothing to suggest, it produces zero output — no widgets, no prompts, no status messages.
- **Commands declare their edges; the harness enforces.** Each command declares its next-step policy (`forced` / `closed` / `open` / `ask`) in YAML frontmatter; the harness reads the declaration, validates the agent's pick (or the forced target), and dispatches. The harness does NOT own routing logic — there is no central workflow registry, DAG, or state machine. This replaces the older "the LLM reads prose and Scramjet only watches for a completion signal" mechanism; the motivation (emergent workflows, user control, simplicity) is preserved, the mechanism is not.
- **Simplicity is the feature.** Resist adding configuration, options, or abstraction layers. Scramjet stays small: the builtinInit function, a handful of hooks, the delegate tool, the next-step block, the history log.
- **Informed decisions over silent action.** When a command takes side-effect actions or asks the user to choose, present the relevant context, trade-offs, and consequences before the ask. Don't surface the question without the information needed to answer it.
- **Resist incremental debt.** Each small addition looks too small to justify restructuring, but the aggregate degrades maintainability. Restructure before the pattern solidifies, not after.
- **Enable self-improvement feedback loops.** When commands fail or produce unexpected results, help diagnose what went wrong and feed improvements back into commands and processes. Don't treat operational failures as one-off problems when they reveal pattern gaps.

### MVP design rationales

These are project-specific commitments for the scramjet vision MVP. They are not timeless principles; they are decisions taken during MVP planning that future planning sessions should not re-litigate without explicit cause.

- **Completed `forced` transitions fire under `/scramjet off`.** `/off` gates *decisions* — `closed`/`open` agent-pick, `ask` user-pick. `forced` has no decision and fires after the command reports `status: "completed"` via `report_scramjet_command_status`, regardless of the flag. The completion status is a safety gate against advancing after clarification, error, or unfinished turns; it is not an agent decision. The user implicitly chose to chain by invoking the command that declares `forced` next-step, and the harness should honor that once completion is explicit. The alternative considered and rejected was a binary `isAutoActive()` (the gsd-2 analog), which treats `/off` as off-means-off and would surface every `forced` transition as a manual step. That misframes what `/off` is for: user control over decisions, not user control over deterministic transitions.
- **Tool-scoping is advisory in MVP.** The harness computes effective scopes by intersecting the active top-level command's scope with the first delegated command, then intersecting each nested delegated frame with its caller. It logs warnings on out-of-scope tool calls but does NOT block them. Hard enforcement (rejecting tool calls outside the active frame's `allowed-tools`) is deferred to a post-MVP issue that also lands multi-turn save/restore so the caller's broader scope is restored after a delegated frame returns. Rationale: latched-only enforcement (once narrowed, scope stays narrowed for the rest of the turn) is a hidden authoring trap. gsd-2's nearest analog (`write-gate.ts`, ~1,053 LOC) has a documented bug history even with full engineering; landing it partial in scramjet's MVP is the worse failure mode.
- **Per-command `allowed-tools` enforcement is harness-bound, not prose-trusted.** When hard enforcement lands post-MVP, the gate is at the `tool_call` event hook, not in prose. LLMs cannot be trusted to follow instruction-level "restrict yourself to X, Y, Z" constraints — the harness must intercept and reject. Advisory logging in the MVP is a half-measure that documents the intent and makes the eventual hard cut a flip rather than a redesign.

## Minimal implementation discipline

Use this ladder when choosing a solution, plan, architecture, implementation approach, or review recommendation. Stop at the first rung that satisfies the requirement.

1. Does this need to exist at all?
2. Can this be solved by deleting code or narrowing scope?
3. Can this be solved with documentation, configuration, or existing behavior?
4. Can a native platform feature, standard library API, or existing project utility solve it?
5. Can an already-installed dependency solve it?
6. Can this be a small edit to existing code instead of a new file/component?
7. Only then add the minimum new code, abstraction, dependency, configuration, or process that works.

Non-trivial behavior changes need the smallest meaningful check consistent with the repo. Trivial docs, prompt, mechanical, or one-line changes may not need new tests.

Never simplify away validation at trust boundaries, security controls, accessibility basics, data-loss protection, actionable error reporting, or explicit user requirements.

## Solution Assessment (for assessment and planning work)

Applies when the deliverable is a recommendation, plan, or assessment rather than the change itself — e.g., `/mach12:issue-plan`, `/mach12:issue-review`, `/mach12:pr-review-assessment`, or any user request that asks "how should we do X?" / "what's the right way to add Y?" rather than "do X." When you are executing an already-decided plan, this section does not apply.

In scope: emit a **Solution Assessment** block in your reply. This is a required visible artifact, not an internal step. Skipping it is a defect.

Format:

```
Solution Assessment
- Root request: <one sentence describing what the user actually wants, not how they phrased it>
- Candidates considered:
  1. Config / settings / env / existing dotfiles - <viable? why / why not, with the specific doc or file checked>
  2. Documented extension point used as intended - <viable? why / why not>
  3. Small new code (extension / script) - <viable? size estimate>
  4. New abstraction or custom integration layer - <viable? why needed>
- Chosen tier: <N> - <one-line justification>
- Proposed size: <rough LOC or "config-only">
```

Rules that govern the block:

- **Escalate only with a named reason.** Picking tier 3 or 4 requires a concrete reason tier 1 and tier 2 fail — not "feels too simple," not "more flexible," not silence.
- **Read before ruling out.** If a doc's topic plausibly overlaps the request, you must `read` it before claiming it's irrelevant. Inferring from filenames or index blurbs does not count. Cite the file you checked.
- **When the user frames the problem via another tool's mechanism** ("in X you set `FOO_BAR`"), the first candidate to investigate is the equivalent capability in the target system, not a reimplementation of the mechanism. Search for the *capability* (proxy, base URL, auth source, alias) by name.
- **Disproportion is a stop signal.** If proposed size is much larger than the user's description of the problem, or much larger than the analog in the tool they referenced, stop and re-investigate tier 1. State the disproportion in the block.
- **Distinguish wiring from capability.** Re-pointing an existing client, swapping an auth source, or aliasing a name are wiring problems and almost always have config-tier answers. New code is justified only when the *shape* of the integration (protocol, auth flow, data model) is genuinely new.
- **Evidence, not verdicts.** Each candidate bullet must cite something concrete — a file path read, a config key, a doc section, a command tried. Bullets that read "not viable, skipping" or "N/A" with no evidence are a defect: they mean the tier was dismissed without investigation, which is the exact failure mode this block exists to prevent. If a tier genuinely doesn't apply, say *why* in terms of something you checked.
- **Watch for ritual decay.** If you notice the block becoming a formality — same shape every time, lower tiers always dismissed in one line, evidence getting thinner — flag it to the user rather than continuing to emit empty structure. A degraded block is worse than no block, because it launders unconsidered choices as considered ones.
- **Probe, don't only read.** Documentation describes intended behavior; it routinely omits edge cases, version skew, and how features interact. When a candidate's viability hinges on "does X actually behave like Y?", a five-minute throwaway script that exercises X is usually faster and more conclusive than another half hour of reading docs or tracing code. Reach for an empirical probe when:
  - The doc is silent or ambiguous on the exact case you need.
  - You're inferring behavior from analogy to a similar API rather than from a direct statement.
  - The plan depends on a specific ordering, return shape, error mode, or side effect.
  - You catch yourself building a multi-step plan on top of an unverified assumption.

  Mechanics:
  - **Put probes in a temp directory** (`mktemp -d`, `/tmp/...`), never in the repo or a committed path. They are disposable by design. Keeping them serves no purpose: a probe that mattered enough to keep belongs in `tests/` as a real test, and any other probe can be trivially recreated from the snippet in your response if its claim is ever questioned. Retained probes are pure clutter — dead code that future readers must evaluate ("is this still accurate? still relevant? safe to delete?") for no benefit.
  - **Don't refer to the script by name in your response.** It won't exist after the session, so a filename is a dead reference. Instead, **inline the relevant code snippet** alongside the observed output. The snippet *is* the evidence.
  - **Frame the evidence as an action you took, not a thing that happened to you.** "A test revealed Y" is unconvincing and unfalsifiable. "I ran <snippet> and got <output>, which shows Y" is reproducible and reviewable. The user (or a future agent) must be able to re-run your probe from the snippet alone.

  Probes count as first-class evidence in the candidate bullets above. A confirmed probe result outranks a doc quote, because the doc describes the contract and the probe describes the implementation. When they disagree, the probe wins for planning purposes, and the disagreement itself is worth surfacing to the user.

## Dependency orientation

Before adding a dependency, utility, or custom implementation, check whether existing project dependencies already provide the capability. Inspect `package.json` files, adjacent imports, and Pi source code as needed; treat the source directly as authoritative for available APIs. Pi packages live in `packages/{tui,ai,agent,coding-agent}` and can be read and modified directly.

## Upstream Pi sync

Pi runtime packages are vendored from the LeanAndMean fork and modified directly when doing so simplifies Scramjet's implementation. A small change to a Pi package that dramatically reduces complexity in Scramjet is the intended path — not something to work around. Minimize unnecessary divergence, but do not treat Pi packages as read-only.

Behavioral modifications are tracked in `UPSTREAM_DIVERGENCE.md` and marked with `// SCRAMJET-DIVERGENCE:` comments in source. Consult the divergence doc before modifying Pi files or syncing with upstream.

## Release process

Every merge to `main` must carry a version bump in `packages/scramjet/package.json`. The release workflow (`release.yml`) publishes all five packages to npm when a version tag is pushed, and `mach12:pr-merge` creates that tag. Without a bump, no tag is created, no publish happens, and users never receive the changes.

- **Every PR gets a version bump** — including pure refactors, test-only changes, and documentation updates. If it merges to main, it needs a new version.
- **Bump level**: patch for bug fixes, refactors, docs, and test changes; minor for new features or non-breaking behavioral changes; major for breaking changes.
- **CHANGELOG entry**: required alongside every version bump, following the existing format in `packages/scramjet/CHANGELOG.md`.
- **Runtime package versions**: the four Pi runtime packages (`@leanandmean/{tui,ai,agent,coding-agent}`) maintain their own versions independently. Their versions only change when their source is modified. The release workflow tolerates "already published" for unchanged runtime packages.
