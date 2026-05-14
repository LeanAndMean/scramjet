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

## Version pinning

`pi.piTestedVersion` in `package.json` must match the pinned versions of `@earendil-works/pi-coding-agent` and `@earendil-works/pi-tui` in `devDependencies`. CI enforces this — bump all three together.
