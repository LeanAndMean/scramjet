# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
npm run typecheck    # tsc --noEmit
npm test             # vitest --run
npm run lint         # biome check .
npx vitest run tests/task-complete.test.ts   # single test file
```

CI runs typecheck, test, lint, and install/uninstall smoke tests on ubuntu and macos.

## Formatting

Biome: tabs, indent width 3, line width 120. Run `npx biome check --write .` to auto-fix.

## Architecture

Scramjet is a compact Pi extension. Pi loads `index.ts` directly via jiti — no compilation step. The imports `@earendil-works/pi-coding-agent`, `@earendil-works/pi-tui`, and `typebox` are virtual modules provided by Pi at runtime; they exist in `devDependencies` only for type checking.

The auto-continuation core is one tool (`task_complete`) plus an `agent_end` listener that drives the countdown widget; the rest of the extension (the `/scramjet` toggle, the `/clear` alias, the diagram tool, the Claude Code tool-name aliases) is independent and can be reasoned about in isolation. The auto-continuation flow is:

1. `task-complete.ts` injects a system prompt snippet (via `before_agent_start`) telling the agent to call `task_complete` when done, with an optional `next_step` if the command's instructions suggest one. The tool sets `terminate: true` to end the agent loop and stores the completion signal.
2. `auto-continue.ts` listens to `agent_end`. If the stored signal has a `next_step`, it shows a countdown widget. Any keypress cancels. Countdown expiry sends the next command as a user message (optionally in a fresh session via the internal `/scramjet-exec-fresh` command).
3. If no `task_complete` was called, nothing happens. Scramjet is invisible.

The diagram tool (`diagram/`) is independent — it detects installed renderers (`mmdc`, `dot`, `plantuml`) and registers `draw_diagram` only if at least one is available.

Plugin wiring is install-time, not runtime. `install.sh` symlinks Pi's bundled subagent example (`node_modules/@earendil-works/pi-coding-agent/examples/extensions/subagent`) into the agent dir unchanged — not forked — and clones the Mach 10 and Anthropic marketplace plugins into `$HOME/.local/share/scramjet/`.

For each plugin, command files are symlinked into `<agent-dir>/prompts/<plugin>:<basename>.md`, while agent files are transformed copies under `<agent-dir>/agents/<plugin>:<basename>.md` (strip `model: inherit`, convert `tools:` YAML arrays to comma-strings). The originals in `$HOME/.local/share/scramjet/` are never modified.

`src/tool-aliases/` registers PascalCase Claude Code tool names (`Read`, `Bash`, `Edit`, `Write`, `Grep`, `Glob`, `LS`) as wrappers around Pi's native lowercase tools so plugin agents' `tools:` restrictions function natively. A `.scramjet-manifest` file in the agent dir tracks every installed plugin path for clean uninstall via `./uninstall.sh --clear-manifest`.

## Project direction

The architecture section above describes the **current** shape of the code. The **target** shape is laid out in `docs/scramjet-vision.md`, which is the source-of-truth design document for the next major rewrite (the "vision MVP"). Consult the vision doc when:

- Planning work that introduces, removes, or reshapes a harness capability (command sets, next-step declarations, delegation, the `/scramjet on/off` flag, history journaling).
- Deciding what is in scope vs. deferred for the MVP. The vision doc carries the MVP-vs-post-MVP boundaries and the per-section deferrals (sidebar UI, hard tool-scoping enforcement, authoring loop).
- Resolving "should we add X?" questions about the harness — the vision doc states the non-goals as well as the goals, and several common asks (workflow DAG, conditional next-step DSL, prose-replacement abstractions) are explicit non-goals.
- Reviewing a design decision and wanting to know what was already considered and rejected, and why.

The MVP buildout is tracked under GitHub issue 23 (umbrella) with one staged implementation plan in its comments. Subissues 24-33 carry the individual stages; the umbrella's `<!-- mach10-plan -->` comment is the current execution plan. The CLAUDE.md design-philosophy section below has been rewritten to match the vision (commands declare their edges, the plugin compat layer is time-bounded, MVP-specific rationales are explicit); when those bullets reference design decisions you don't recognize, the vision doc is where the long-form reasoning lives.

## Design philosophy

These principles override default instincts. Do not add complexity that violates them.

- **Emergent over prescribed.** Workflows emerge from edges in each command's instructions, not from centralized definitions. Don't add workflow registries, DAG configs, or state machines.
- **Zero lock-in.** The user can press Escape at any transition and be back in normal Pi. No workflow state persists. Don't add resumable state, queues, or progress tracking across sessions.
- **Invisible when idle.** If Scramjet has nothing to suggest, it produces zero output — no widgets, no prompts, no status messages.
- **Commands declare their edges; the harness enforces.** Each command declares its next-step policy (`forced` / `closed` / `open` / `ask`) in YAML frontmatter; the harness reads the declaration, validates the agent's pick (or the forced target), and dispatches. The harness does NOT own routing logic — there is no central workflow registry, DAG, or state machine. This replaces the older "the LLM reads prose and Scramjet only watches for `task_complete`" mechanism; the motivation (emergent workflows, user control, simplicity) is preserved, the mechanism is not.
- **Simplicity is the feature.** Resist adding configuration, options, or abstraction layers. Scramjet stays small: one extension, a handful of hooks, the delegate tool, the next-step block, the history log.
- **Plugin compat layer is kept through Stage 7, removed at Stage 8.** Scramjet wires plugins authored for Claude Code CLI (Mach 10, feature-dev, pr-review-toolkit, …) while Mach 12 is being built. Plugin files must keep working under Claude Code during the MVP build; upstream changes to those plugins are limited to pure prose tweaks. Once Mach 12 is feature-complete at the end of Stage 7, Stage 8 tears the compat layer out and switches Scramjet to an npm-distributed CLI that embeds Pi via its library API. Mach 10 plugins keep functioning in users' existing setups; they are no longer cloned or wired by Scramjet itself.

### MVP design rationales

These are project-specific commitments for the scramjet vision MVP. They are not timeless principles; they are decisions taken during MVP planning that future planning sessions should not re-litigate without explicit cause.

- **`forced` fires under `/scramjet off`.** `/off` gates *decisions* — `closed`/`open` agent-pick, `ask` user-pick. `forced` has no decision and fires immediately regardless of the flag. The user implicitly chose to chain by invoking the command that declares `forced` next-step; the harness should honor that. The alternative considered and rejected was a binary `isAutoActive()` (the gsd-2 analog), which treats `/off` as off-means-off and would surface every `forced` transition as a manual step. That misframes what `/off` is for: user control over decisions, not user control over deterministic transitions.
- **Tool-scoping is advisory in MVP.** The harness logs warnings on out-of-scope tool calls but does NOT block them. Hard enforcement (rejecting tool calls outside the active frame's `allowed-tools`) is deferred to a post-MVP issue that also lands multi-turn save/restore so the caller's broader scope is restored after a delegated frame returns. Rationale: latched-only enforcement (once narrowed, scope stays narrowed for the rest of the turn) is a hidden authoring trap. gsd-2's nearest analog (`write-gate.ts`, ~1,053 LOC) has a documented bug history even with full engineering; landing it partial in scramjet's MVP is the worse failure mode.
- **Per-command `allowed-tools` enforcement is harness-bound, not prose-trusted.** When hard enforcement lands post-MVP, the gate is at the `tool_call` event hook, not in prose. LLMs cannot be trusted to follow instruction-level "restrict yourself to X, Y, Z" constraints — the harness must intercept and reject. Advisory logging in the MVP is a half-measure that documents the intent and makes the eventual hard cut a flip rather than a redesign.

## Solution Assessment (for assessment and planning work)

Applies when the deliverable is a recommendation, plan, or assessment rather than the change itself — e.g., `/mach10:issue-assessment`, `/mach10:issue-plan`, `/mach10:issue-plan-review`, or any user request that asks "how should we do X?" / "what's the right way to add Y?" rather than "do X." When you are executing an already-decided plan, this section does not apply.

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

`pi.piTestedVersion` in `package.json` must match the pinned versions of `@earendil-works/pi-coding-agent` and `@earendil-works/pi-tui` in `devDependencies`. CI enforces this — bump all three together.
