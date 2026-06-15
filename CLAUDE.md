# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
npm run typecheck    # tsc --noEmit
npm run build        # tsc -p tsconfig.build.json -> dist/
npm test             # vitest --run
npm run lint         # biome check .
npx vitest run tests/command-status.test.ts   # single test file
```

CI runs typecheck, build, test, lint, an `npm pack` round-trip smoke (installs the produced tarball globally and probes `scramjet --help`), and a postinstall smoke (against a temporary `XDG_DATA_HOME`) on ubuntu and macos.

## Local development

Scramjet ships as an npm package since Stage 8. The `scramjet` bin on PATH runs the compiled `dist/index.js`, NOT the TypeScript source. That introduces two staleness traps; both have one-time setups to avoid them.

**One-time dev setup (after `npm install`):**

```sh
npm run build          # produce dist/ so the bin has something to import
npm link               # install `scramjet` globally as a symlink to this tree
ln -sfn "$(pwd)/mach12" "${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/mach12"
                       # so edits to mach12/*.md files are picked up live
```

If you previously ran the (now-deleted) `./install.sh`, you may have dangling symlinks at `~/.local/bin/scramjet` (target gone) and `~/.pi/agent/extensions/scramjet` (would double-load on top of `npm link`). Remove both before `npm link`.

**Iteration:**

- **Edited a `.ts` file under the source tree** → `npm run build` (or run `tsc -p tsconfig.build.json --watch` in a separate terminal). **This is the most common "am I testing my changes?" confusion** — if you edit, run `scramjet`, and see old behavior, suspect a stale `dist/` before suspecting anything else.
- **Edited `mach12/*.md`** → no rebuild needed if the mach12 symlink is in place (loader reads .md at every Pi startup).
- **Edited `bin/scramjet.js` or `scripts/postinstall.js`** → no rebuild needed; they're `.js` and run as-is.
- **To verify which scramjet is on PATH:** `readlink -f "$(which scramjet)"` should resolve into this repo's working tree (via npm's global `lib/node_modules/@leanandmean/scramjet/bin/scramjet.js`). If it resolves elsewhere, you're running an old or published copy.

## Formatting

Biome: tabs, indent width 3, line width 120. Run `npx biome check --write .` to auto-fix.

## Documentation sync

When modifying command frontmatter schema (`commands/loader.ts`, `commands/parse-next-step.ts`), delegation behavior (`delegate.ts`), tool scoping (`delegate.ts`, `tool-scope-advisory.ts`), status reporting (`command-status.ts`), or next-step dispatch (`auto-continue.ts`, `commands/validator.ts`), update `docs/command-authoring.md` to reflect the change. The authoring doc is agent-facing — inaccurate guidance produces malformed commands.

## Architecture

Scramjet is a compact Pi extension distributed as the npm package `@leanandmean/scramjet`. The `bin/scramjet.js` entry point calls Pi's library `main(argv, { extensionFactories: [scramjetExtension] })`; Pi instantiates scramjet alongside any disk-discoverable extensions in the user's Pi agent dir. The package ships compiled output in `dist/` produced by `tsconfig.build.json` (vanilla `tsc` with `--rewriteRelativeImportExtensions` to rewrite the source's `.ts` import extensions to `.js` in the emit).

`index.ts` constructs a single `ScramjetState` (registry, agent registry, delegate stack, sidebar log, `/scramjet on/off` flag, pending forced dispatch — see `types.ts`) and threads it through every register-call. Each capability is its own file; nothing in the harness imports anything outside `types.ts` and Pi's API.

**Vision MVP harness modules** (issue 23 buildout):

- `commands/loader.ts` + `commands/parse-next-step.ts` + `commands/validator.ts` — pure-function parser, next-step policy reader, and pre-dispatch validator. `commands/index.ts` is the discovery wiring: a single `resources_discover` hook that scans the seeded global root and the per-cwd project root, builds the command registry (returned to Pi via `promptPaths`) and the agent registry (consumed locally by the bridge), then calls `ensureAgentBridge` to wire agents into Pi's subagent dispatch path.
- `commands/agent-bridge.ts` — symlinks each registered subagent into `<getAgentDir()>/agents/<name>.md` so Pi's upstream subagent example extension can discover them. Pi has no `agents_discover` hook or `registerAgent` API; the subagent example scans `~/.pi/agent/agents/` directly, and the documented install method (per its README) is symlinking. Idempotent, ownership-tracked (a symlink is only treated as scramjet-owned when its resolved target falls under one of the scanned scramjet roots), and self-pruning of dangling scramjet-owned symlinks. Skipped on native Windows for the same reason as `scripts/postinstall.js` (symlinks need admin/developer mode).
- `delegate.ts` — registers the `delegate` tool. The tool looks up a command in the registry, intersects `allowed-tools` with the active top-level command scope for the first delegate (or caller frame for nested delegates), pushes a frame onto the delegate stack (latched scoping; frames never pop within a turn), journals an indented `agent`-origin history entry, and returns the substituted command body as tool result content. Detects cycles. Resets the stack on `before_agent_start`.
- `next-step.ts` — pure block builder. Exports `buildNextStepBlock`, which renders the `<scramjet-next-step>` policy block for a given `next:` policy (one branch per `forced` / `closed` / `open` / `ask` mode, with close-tag escaping for prompt-injection safety), and `buildProbeMessage`, which wraps that block in the hardcoded status-check preamble that asks the agent to call `scramjet_command_status`. The probe message is sent (deferred) by `auto-continue.ts`; the `agent_end` dispatcher that reads the reported status/pick and fires the next command also lives in `auto-continue.ts` (see below).
- `history.ts` — appends sidebar entries (`▸` user, `●` agent, `■` forced; delegated entries currently use `agent` origin at `depth > 0`) to the persistent history journal and replays the journal at `session_start` so `/scramjet on/off`, the active top-level command, and the log survive `pi --resume`. Depth > 0 replay entries do not replace `activeTopLevelCommand`. It also journals each `scramjet_command_status` report as a `COMMAND_STATUS_TYPE` (`scramjet:command-status`) entry via `recordCommandStatus`; `replayHistory` reads those entries (last-status-wins, scoped to the active command) to reconstruct the resumable `waiting` phase on resume (issue 88 Stage 2 — see the auto-continuation bullet).
- `tool-scope-advisory.ts` — `tool_call` hook that emits `console.warn` when the active frame's `effectiveAllowedTools` excludes the called tool. Advisory only; never blocks. Hard enforcement is deferred (see "Design philosophy" below).
- `subagent-output-advisor.ts` — `tool_result` hook that emits `console.warn` and appends a session-only sidebar entry when the upstream subagent example tool returns a literal `(no output)` payload. Surfaces the silent-failure mode the upstream tool produces when a subprocess exits cleanly with no assistant text (crash before stdout flush, unknown model id, config error). Advisory only; never modifies the tool result.

**Auto-continuation core** (originated pre-MVP; reshaped into the two-phase command-status protocol by issue 84):

- `command-status.ts` + `auto-continue.ts` — the harness mechanism that drives next-step dispatch via a two-phase protocol. The command produces its normal user-facing answer first; nothing about completion is injected into that turn. `auto-continue.ts` listens on `agent_end`: when an active top-level command with a `next:` policy goes idle (`commandPhase === "running"`), it advances the phase to `probing` and **defers** (via `setTimeout(0)`, after the run settles and `isStreaming` clears when `agent.prompt()` resolves) a TUI-hidden status-check message built by `buildProbeMessage`, sent through `pi.sendMessage({ display: false }, { triggerTurn: true })` to start a short probe turn. `command-status.ts` registers the `scramjet_command_status` tool; its `execute` is harness-phase-gated — outside `probing` it returns a helpful error with no `terminate`; inside, it stores the `CommandStatusPayload` (`status` ∈ `completed`/`waiting_for_user`/`blocked`/`incomplete`, plus `summary`, optional `user_prompt`, and a `next_steps[]` array), advances the phase to `reported`, and returns `terminate: true`. On the probe turn's `agent_end` (`reported`), `auto-continue.ts` routes by status: `completed` (→ `idle`) validates the pick via the policy and dispatches mode-by-mode (`forced` fires unconditionally even under `/scramjet off`; `closed`/`open` honor the first valid `next_steps` entry when `/scramjet on`; `ask` pauses); `blocked` (→ `idle`) warns; `incomplete` (→ `idle`) pauses quietly; `waiting_for_user` parks at the resumable `waiting` phase (issue 88) and echoes the `user_prompt` if supplied. `waiting` is the only resting phase besides `idle`: a paused interactive command stays associated with its invocation, and an **interactive, non-slash** reply (handled in `history.ts`'s `input` handler) flips `waiting → running`, re-arming the existing `running → probing` probe so the resumed command can later report `completed` and offer its declared next step. Chaining still requires an explicit `completed` report, so an accidental resume can only re-probe — never mis-chain; a stray `agent_end` while `waiting` is a defensive no-op (no probe without a user reply); and exiting the workflow via an unknown slash drops `waiting → idle`. A `probing` `agent_end` with no report self-heals to `idle` (no loop). On `rebuild` (resume / branch switch) the transient phases (`running`/`probing`/`reported`) are never restored, but the stable `waiting` halt is (issue 88 Stage 2): `command-status.ts` journals every report as a `COMMAND_STATUS_TYPE` entry and `replayHistory` reconstructs `waiting` iff the active command's **last** journaled status was `waiting_for_user`, so a paused interactive command survives `pi --resume`. Journaling *all* statuses (not just `waiting_for_user`) is what makes a command that waited, was answered, then completed **without** chaining reconstruct to `idle` — never resurrected — since that path writes no subsequent command-start. A stale post-resume tool call (any non-`waiting` reconstruction) still hits the phase guard rather than mis-dispatching; only the phase is reconstructed, never `latestCommandStatus`.

**Independent capabilities:**

- `scramjet-command.ts` — the `/scramjet on|off` slash command.
- `clear-alias.ts` — `/clear` alias.
- `diagram/` — detects `mmdc` / `dot` / `plantuml` at startup and registers `draw_diagram` only if at least one renderer is installed.
- `pr-indicator.ts` — ambient footer hint showing the current branch's active GitHub PR number (`PR #<n>`) via `ctx.ui.setStatus` when exactly one open PR matches; shows nothing in every other case (no/multiple PRs, unsupported remote, missing/unauthenticated `gh`, not a git repo). Resolves on `session_start` / `session_tree` / `agent_end` (the `gh` call on `agent_end` is gated behind a cheap local branch-diff). An opportunistic hint, not workflow state: nothing is journaled, nothing is added to `ScramjetState`, and it shows regardless of `/scramjet on|off`. This footer `setStatus` text is a different Pi primitive from — and not a violation of — the sidebar UI panel deferred in `docs/scramjet-vision.md` section 5. A commented forge-swap seam marks where a future `glab` (`MR !<iid>`) branch would slot in.
- `agent-catalog.ts` — injects the available subagent catalog into the system prompt via `before_agent_start`, enabling the calling agent to discover agent names and descriptions before dispatching. Reads `state.agentRegistry` (populated by the command-set loader's agent bridge), formats a sorted list of `name: description` entries, and appends only when the registry is non-empty. Returns only `systemPrompt` (no `message`), unconditional (flag-independent, same posture as `base-directives.ts`). Exports a pure `buildAgentCatalogBlock(registry)` for testability alongside the hook registration function.
- `user-input.ts` — registers the `scramjet_user_input` tool, which lets agents request structured user input mid-turn without ending the turn. Supports three interaction types: `confirm` (yes/no via `MultiLineSelectList`, distinguishing explicit No from Escape/cancel), `select` (structured options with descriptions and `recommended` index via `MultiLineSelectList`), and `freetext` (open-ended input via `ctx.ui.input()`). Phase-gated to `running` and `probing`; returns a helpful non-terminating error outside those phases or in non-TUI mode. Runtime-validates type-specific required fields (e.g., `options` for select, non-empty `message`). When called during `probing`, suspends the probe watchdog via `state.suspendProbeWatchdog()` before awaiting UI and re-arms via `state.rearmProbeWatchdog()` after the response (coordinated with `auto-continue.ts` which populates those callbacks). Cancellation (Escape) returns `{ cancelled: true }`. Each interaction is journaled as a `scramjet:user-input` custom entry type via `pi.appendEntry()`. The tool never returns `terminate: true` — the agent continues executing after the response. Advertised via `promptSnippet` on the tool definition.
- `base-directives.ts` — appends a general coding-agent quality block (`SCRAMJET_BASE_DIRECTIVES`) to Pi's assembled system prompt on every turn via `before_agent_start`. The prose is adopted from a captured Claude Code CLI system prompt and product-neutralized (issue 78). It returns only `systemPrompt` (no `message`, so it composes cleanly with any other extension's `before_agent_start.message` return), appends on top of any user SYSTEM.md, and is unconditional (flag-independent, same posture as `pr-indicator.ts`, so no `ScramjetState` is threaded). Two Scramjet-specific reference blocks (orientation + feedback routing) are conditional self-knowledge with doc pointers resolved from the installed package root via a module-relative `package.json` walk.

**Bundled Mach 12 command set** (`mach12/`):

The tenant of the harness — `mach12/commands/*.md` are command files using the next-step declarations and delegation. Ten top-level commands (`mach12:issue-create`, `mach12:issue-plan`, …, `mach12:pr-merge`) with top-level `next:` blocks where chaining is intended (`forced`/`closed`/`open`/`ask`); `mach12:pr-merge` intentionally has no `next` and is the default terminus. Seven delegate-only subroutines (`mach12:push`, `mach12:find-contribution-guidelines`, `mach12:gh-issue-read`, `mach12:gh-pr-read`, `mach12:gh-sub-issues`, `mach12:gh-assign`, `mach12:gh-comment`) are invoked via `delegate` from the top-level commands. Subroutines have no `next:` block — the caller's `next:` controls chaining. `mach12/agents/*.md` ships nine bundled subagents (exploration, architecture, code review, comment analysis, test analysis, silent-failure analysis, type-design analysis, feature-completeness checking, code simplification) that the multi-lens commands dispatch to. The npm `postinstall` script seeds the whole `mach12/` tree into `${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/mach12/` on install; the command-set loader picks it up at runtime via `resources_discover`. `gh-*` subroutines are flagged in their prose as forge-swap points for the deferred `glab-*` family.

**Distribution:**

`bin/scramjet.js` is a small Node entry that imports the compiled `dist/index.js` default export and calls Pi's `main(argv, { extensionFactories: [scramjetExtension] })`. `scripts/postinstall.js` runs on `npm install` and idempotently seeds the bundled Mach 12 tree (skipped on native Windows with a notice; failure prints a warning but never blocks the install). The previous bash-shim distribution (`install.sh`, `uninstall.sh`, `bin/scramjet`) and the Claude Code plugin compat layer (`src/install/transform.mjs`, `src/tool-aliases/`) were removed at Stage 8 of the vision MVP.

## Project direction

The architecture section above describes the **current** shape of the code. The **target** shape is laid out in `docs/scramjet-vision.md`, which is the source-of-truth design document for the next major rewrite (the "vision MVP"). Consult the vision doc when:

- Planning work that introduces, removes, or reshapes a harness capability (command sets, next-step declarations, delegation, the `/scramjet on/off` flag, history journaling).
- Deciding what is in scope vs. deferred for the MVP. The vision doc carries the MVP-vs-post-MVP boundaries and the per-section deferrals (sidebar UI, hard tool-scoping enforcement, authoring loop).
- Resolving "should we add X?" questions about the harness — the vision doc states the non-goals as well as the goals, and several common asks (workflow DAG, conditional next-step DSL, prose-replacement abstractions) are explicit non-goals.
- Reviewing a design decision and wanting to know what was already considered and rejected, and why.
- Reviewing or planning work that touches design principles, harness behavior, or command-set conventions — the elaborated principles section grounds decisions with context, examples, and counterexamples.

The MVP buildout shipped under GitHub issue 23 (umbrella). Subissues 24-33 carried the individual stages; the staged plan and per-stage progress comments live on issue 23 for the historical record. Post-MVP work (sidebar UI, hard tool-scoping enforcement, authoring loop) is tracked as separate issues — consult the vision doc for the deferred-scope catalog. The CLAUDE.md design-philosophy section below was rewritten to match the vision (commands declare their edges, MVP-specific rationales are explicit); when those bullets reference design decisions you don't recognize, the vision doc is where the long-form reasoning lives.

## Design philosophy

These principles override default instincts. Do not add complexity that violates them.

- **Emergent over prescribed.** Workflows emerge from edges in each command's instructions, not from centralized definitions. Don't add workflow registries, DAG configs, or state machines.
- **Zero lock-in.** The user can press Escape at any transition and be back in normal Pi. No workflow state persists. Don't add resumable state, queues, or progress tracking across sessions. *Exception (issue 88): a command paused at `waiting_for_user` parks at a stable `waiting` phase that is reconstructed on `pi --resume` / branch switch, so the user can answer and resume the in-flight command. This is the one authorized resumable halt — only the phase is reconstructed (never `latestCommandStatus`), the transient mid-turn phases are deliberately not journaled, and a command that already completed reconstructs to `idle` and never re-fires. See the auto-continuation bullet in the architecture section and `docs/scramjet-vision.md` §2.1. Don't generalize this into broader cross-session workflow persistence.*
- **Invisible when idle.** If Scramjet has nothing to suggest, it produces zero output — no widgets, no prompts, no status messages.
- **Commands declare their edges; the harness enforces.** Each command declares its next-step policy (`forced` / `closed` / `open` / `ask`) in YAML frontmatter; the harness reads the declaration, validates the agent's pick (or the forced target), and dispatches. The harness does NOT own routing logic — there is no central workflow registry, DAG, or state machine. This replaces the older "the LLM reads prose and Scramjet only watches for a completion signal" mechanism; the motivation (emergent workflows, user control, simplicity) is preserved, the mechanism is not.
- **Simplicity is the feature.** Resist adding configuration, options, or abstraction layers. Scramjet stays small: one extension, a handful of hooks, the delegate tool, the next-step block, the history log.
- **Informed decisions over silent action.** When a command takes side-effect actions or asks the user to choose, present the relevant context, trade-offs, and consequences before the ask. Don't surface the question without the information needed to answer it.
- **Resist incremental debt.** Each small addition looks too small to justify restructuring, but the aggregate degrades maintainability. Restructure before the pattern solidifies, not after.
- **Enable self-improvement feedback loops.** When commands fail or produce unexpected results, help diagnose what went wrong and feed improvements back into commands and processes. Don't treat operational failures as one-off problems when they reveal pattern gaps.

### MVP design rationales

These are project-specific commitments for the scramjet vision MVP. They are not timeless principles; they are decisions taken during MVP planning that future planning sessions should not re-litigate without explicit cause.

- **Completed `forced` transitions fire under `/scramjet off`.** `/off` gates *decisions* — `closed`/`open` agent-pick, `ask` user-pick. `forced` has no decision and fires after the command reports `status: "completed"` via `scramjet_command_status`, regardless of the flag. The completion status is a safety gate against advancing after clarification, error, or unfinished turns; it is not an agent decision. The user implicitly chose to chain by invoking the command that declares `forced` next-step, and the harness should honor that once completion is explicit. The alternative considered and rejected was a binary `isAutoActive()` (the gsd-2 analog), which treats `/off` as off-means-off and would surface every `forced` transition as a manual step. That misframes what `/off` is for: user control over decisions, not user control over deterministic transitions.
- **Tool-scoping is advisory in MVP.** The harness computes effective scopes by intersecting the active top-level command's scope with the first delegated command, then intersecting each nested delegated frame with its caller. It logs warnings on out-of-scope tool calls but does NOT block them. Hard enforcement (rejecting tool calls outside the active frame's `allowed-tools`) is deferred to a post-MVP issue that also lands multi-turn save/restore so the caller's broader scope is restored after a delegated frame returns. Rationale: latched-only enforcement (once narrowed, scope stays narrowed for the rest of the turn) is a hidden authoring trap. gsd-2's nearest analog (`write-gate.ts`, ~1,053 LOC) has a documented bug history even with full engineering; landing it partial in scramjet's MVP is the worse failure mode.
- **Per-command `allowed-tools` enforcement is harness-bound, not prose-trusted.** When hard enforcement lands post-MVP, the gate is at the `tool_call` event hook, not in prose. LLMs cannot be trusted to follow instruction-level "restrict yourself to X, Y, Z" constraints — the harness must intercept and reject. Advisory logging in the MVP is a half-measure that documents the intent and makes the eventual hard cut a flip rather than a redesign.

## Solution Assessment (for assessment and planning work)

Applies when the deliverable is a recommendation, plan, or assessment rather than the change itself — e.g., `/mach12:issue-plan`, `/mach12:issue-review`, `/mach12:pr-review-assessment`, or any user request that asks "how should we do X?" / "what's the right way to add Y?" rather than "do X." When you are executing an already-decided plan, this section does not apply.

In scope: emit a **Solution Assessment** block in your reply. This is a required visible artifact, not an internal step. Skipping it is a defect.

Format:

```
Solution Assessment
- Root request: <one sentence describing what the user actually wants, not how they phrased it>
- Candidates considered:
  1. Config / settings / env / existing dotfiles — <viable? why / why not, with the specific doc or file checked>
  2. Documented extension point used as intended — <viable? why / why not>
  3. Small new code (extension / script) — <viable? size estimate>
  4. New abstraction or custom integration layer — <viable? why needed>
- Chosen tier: <N> — <one-line justification>
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

## Version pinning

Scramjet intentionally consumes a LeanAndMean-patched Pi coding-agent package through an npm alias:

```json
"@earendil-works/pi-coding-agent": "npm:@leanandmean/pi-coding-agent@0.74.0-scramjet.1"
```

The alias preserves existing imports while installing the patched package at the upstream dependency key. The patched package is based on upstream Pi `pi.piBaseVersion` with Scramjet patch flavor `pi.piPatchFlavor`; `pi.piTestedVersion` is the combined `${piBaseVersion}-${piPatchFlavor}`. `@earendil-works/pi-tui` remains the upstream `pi.piBaseVersion` package. CI enforces that all of these fields stay in sync — bump the base version, patch flavor/tested version, coding-agent alias, and pi-tui pin together.

## Release process

Every merge to `main` must carry a version bump. The release workflow (`release.yml`) publishes to npm when a version tag is pushed, and `mach12:pr-merge` creates that tag from `package.json`'s version. Without a bump, no tag is created, no publish happens, and users never receive the changes.

- **Every PR gets a version bump** — including pure refactors, test-only changes, and documentation updates. If it merges to main, it needs a new version.
- **Bump level**: patch for bug fixes, refactors, docs, and test changes; minor for new features or non-breaking behavioral changes; major for breaking changes.
- **CHANGELOG entry**: required alongside every version bump, following the existing format in `CHANGELOG.md`.
