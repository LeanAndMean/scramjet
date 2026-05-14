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

Scramjet is a Pi extension (~450 lines). Pi loads `index.ts` directly via jiti — no compilation step. The imports `@earendil-works/pi-coding-agent`, `@earendil-works/pi-tui`, and `typebox` are virtual modules provided by Pi at runtime; they exist in `devDependencies` only for type checking.

The extension registers one tool (`task_complete`), one event listener (`agent_end`), and one command (`/scramjet`). The auto-continuation flow is:

1. `task-complete.ts` injects a system prompt snippet (via `before_agent_start`) telling the agent to call `task_complete` when done, with an optional `next_step` if the command's instructions suggest one. The tool sets `terminate: true` to end the agent loop and stores the completion signal.
2. `auto-continue.ts` listens to `agent_end`. If the stored signal has a `next_step`, it shows a countdown widget. Any keypress cancels. Countdown expiry sends the next command as a user message (optionally in a fresh session via the internal `/scramjet-exec-fresh` command).
3. If no `task_complete` was called, nothing happens. Scramjet is invisible.

The diagram tool (`diagram/`) is independent — it detects installed renderers (`mmdc`, `dot`, `plantuml`) and registers `draw_diagram` only if at least one is available.

## Design philosophy

These principles override default instincts. Do not add complexity that violates them.

- **Emergent over prescribed.** Workflows emerge from edges in each command's instructions, not from centralized definitions. Don't add workflow registries, DAG configs, or state machines.
- **Zero lock-in.** The user can press Escape at any transition and be back in normal Pi. No workflow state persists. Don't add resumable state, queues, or progress tracking across sessions.
- **Invisible when idle.** If Scramjet has nothing to suggest, it produces zero output — no widgets, no prompts, no status messages.
- **Commands own their edges.** The next step comes from Claude reading a command's instructions, not from Scramjet. Don't move flow logic into Scramjet.
- **Simplicity is the feature.** Resist adding configuration, options, or abstraction layers. The entire system is one tool, one event listener, one widget.
- **Preserve Claude Code plugin compatibility.** Scramjet wires plugins authored for Claude Code CLI (Mach 10, feature-dev, pr-review-toolkit, …). Plugin files must keep working under Claude Code: fix cross-harness gaps on the scramjet side. Upstream changes to those plugins are limited to pure prose tweaks; don't strip frontmatter fields, restructure agents, or rename constructs.

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

## Version pinning

`pi.piTestedVersion` in `package.json` must match the pinned versions of `@earendil-works/pi-coding-agent` and `@earendil-works/pi-tui` in `devDependencies`. CI enforces this — bump all three together.
