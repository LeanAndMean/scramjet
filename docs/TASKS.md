# Tasks

Short list of current and next work. Git history records what was done.

## Current state (2026-10-30)

- `scramjet-rs agent` is a usable interactive coding agent through Proxy (`gpt-6-luna` by default): read/write/edit/bash tools, project AGENTS.md/CLAUDE.md context, approvals, resumable sessions. Verified live on scratch projects.
- The original first-provider goal is met: two fresh `coding` invocations through Proxy wrote a haiku, then wrote and ran a Python reader whose output matched the file.
- Earlier offline demos (runtime, readers, retained TUI probes, haiku, provider validation pair) remain in the tree as development fixtures.

## Current focus

None. `/rs:loop <focus>` records a user-given focus here until it is finished.

## Next candidates

Roughly in priority order; reorder freely as the user's needs become clearer.

1. **Use it and fix what hurts.** Run the agent on real work and fix the first problems found.
2. **Streaming output.** Show assistant text and reasoning summaries as they arrive instead of after each response.
3. **Long sessions.** Handle context overflow (the provider's error today) with compaction or truncation of old tool output.
4. **Terminal UI.** A proper input editor (multi-line, history), then the retained alternate-screen experience from the design.
5. **More models.** Check which models Proxy serves, and add the Anthropic Messages route if Claude models are wanted.
6. **Parallel subagents.** Read-only helper agents sharing the client, as in the design.
7. **Clean up.** Remove specialty code that no longer serves a consumer (haiku journey, fixed validation pair, `coding` invocation store) once nothing depends on it. Ask before deleting large user-visible commands.

## Retired

The S4.13/S4.14 task records (planning, candidate fingerprints, review rounds and the live-call ledger rules) were removed after commit `cfa1ee4`; read them with `git show cfa1ee4:docs/tasks/<id>.md` if needed. The 32-request lifetime ledger cap and stop-on-unknown-usage rule were replaced by per-run limits on 2026-10-30.
