# Tasks

Short list of current and next work. Git history records what was done.

## Current state (2026-10-30)

- `scramjet-rs agent` is a usable interactive coding agent through Proxy (`gpt-6-luna` by default): read/write/edit/bash tools, project AGENTS.md/CLAUDE.md context, approvals, resumable sessions, streamed reasoning summaries and replies, and an inline prompt editor (bracketed paste without sending, multi-line input, editing keys, history from the session, type-ahead flushed before approvals). Verified live on scratch projects, including a `--version` change to a clone of this repo.
- The original first-provider goal is met: two fresh `coding` invocations through Proxy wrote a haiku, then wrote and ran a Python reader whose output matched the file.
- Earlier offline demos (runtime, readers, retained TUI probes, haiku, provider validation pair) remain in the tree as development fixtures.

## Current focus

None. `/rs:loop <focus>` records a user-given focus here until it is finished.

## Next candidates

Roughly in priority order; reorder freely as the user's needs become clearer.

1. **Use it and fix what hurts.** Run the agent on real work and fix the first problems found. Observed so far:
   - Slow requests are intermittent upstream latency, not the client: on 2026-10-30 Proxy recorded 130–270s `latency_ms` for 4 of about 40 gpt-6-luna requests (few output tokens, no queue wait), while the same task replayed through a timing relay got first bytes in 0.8–2.7s. A client aid would be showing elapsed seconds in the `… thinking` status.
   - Text typed while a turn runs echoes into the output (the terminal is in cooked mode then).
   - Library tests that fork must hold `coding::tests::PROCESS_GATE.write()`: a fork briefly holds other tests' journal `flock`s. The ungated agent bash test made `failed_material_acknowledgement_…` fail in every full run until gated; other older spawners (`coding::invocation`, `runtime::provider_tests`, `provider_validation_tests`) are still ungated.
2. **Prompt caching key.** Scramjet sends `prompt_cache_key` (the session id) on Responses requests, and Proxy passes it through; the agent sends none. Small change; compare cached-token ratios before and after.
3. **Terminal UI.** The inline prompt editor is done (`src/agent_cli/editor.rs`). Next is the retained alternate-screen experience from the design (browse output while it streams, expand tool details, selection/copy).
4. **Long sessions.** Handle context overflow with compaction or truncation of old tool output. Less urgent for gpt-6-luna (Scramjet's model table: 1.05M context, 922K max input) than for smaller-window models.
5. **More models.** Proxy serves Claude, GPT and DeepSeek models (`/v1/models`); check which work over `/v1/responses`, and add the Anthropic Messages route if Claude models are wanted.
6. **Parallel subagents.** Read-only helper agents sharing the client, as in the design.
7. **Clean up.** Remove specialty code that no longer serves a consumer (haiku journey, fixed validation pair, `coding` invocation store) once nothing depends on it. Ask before deleting large user-visible commands.

## Retired

The S4.13/S4.14 task records (planning, candidate fingerprints, review rounds and the live-call ledger rules) were removed after commit `cfa1ee4`; read them with `git show cfa1ee4:docs/tasks/<id>.md` if needed. The 32-request lifetime ledger cap and stop-on-unknown-usage rule were replaced by per-run limits on 2026-10-30.
