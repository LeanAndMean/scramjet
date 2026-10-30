# Tasks

Short list of current and next work. Git history records what was done.

## Current state (2026-10-30)

- `scramjet-rs agent` is a usable interactive coding agent through Proxy (`gpt-6-luna` by default): read/write/edit/bash tools, project AGENTS.md/CLAUDE.md context, approvals, resumable sessions, retries of transient provider failures, streamed reasoning summaries and replies, and an inline prompt editor (bracketed paste without sending, multi-line input, editing keys, history from the session, type-ahead flushed before approvals). Text typed during a turn is hidden and kept for the next prompt. Tool results show a short preview: the bash output tail, a numbered diff for edits. A dim status line says what the agent waits for, with elapsed seconds (`thinking`, `preparing <tool>` while a call streams, `running` after a second of tool time). Reasoning summary parts are separate paragraphs, and streamed reasoning and replies wrap between words, with list items continuing under their text. The retry wait counts down on the status line. Approval questions show control and text-direction characters as escapes; answering `a` allows all later tool calls until exit. Ctrl-C at an approval question cancels the turn (recorded for the model as cancelled before the call ran). Requests carry the session id as `prompt_cache_key`. Verified live on scratch projects, including a `--version` change to a clone of this repo.
- The original first-provider goal is met: two fresh `coding` invocations through Proxy wrote a haiku, then wrote and ran a Python reader whose output matched the file.
- Earlier offline demos (runtime, readers, retained TUI probes, haiku, provider validation pair) remain in the tree as development fixtures.

## Current focus

None. `/rs:loop <focus>` records a user-given focus here until it is finished.

## Next candidates

Roughly in priority order; reorder freely as the user's needs become clearer.

1. **Use it and fix what hurts.** Run the agent on real work and fix the first problems found. Observed so far:
   - Done 2026-10-30 from dogfood runs (details in `git log`): retries of transient upstream failures (an 18-request task through Proxy hit 4, all retried), bash tail / edit diff / write head previews with escape sequences dropped, the elapsed and retry-countdown status line, separate reasoning paragraphs, escaped approval text, an `a` (allow all until exit) approval answer after a task asked eight times for read-only commands, word wrapping of streamed text, which the terminal had split mid-word, plain bash output on success with failures marked (`Command exited with code N`, red), and the read range on the call line (`read f:120-124`). The retry countdown was written by the agent itself in a clone of this repo.
   - Next dogfood candidates: show a status after reasoning streams and the model goes quiet (needs care not to split a paragraph that resumes; the wrapper holds the last word, so a quiet model also hides it); reasoning summaries print in full and take much of the screen (Scramjet can hide them with a setting and a key); the prompt editor wraps long prompts mid-word (`editor::Layout` breaks by cells); a failed bash preview spends a line on the blank separator before `Command exited with code N`; the usage line wraps mid-word in narrow windows.
   - Slow requests are intermittent upstream latency, not the client: on 2026-10-30 Proxy recorded 130–270s `latency_ms` for 4 of about 40 gpt-6-luna requests (few output tokens, no queue wait), while the same task replayed through a timing relay got first bytes in 0.8–2.7s; a 17-request dogfood task took 19 minutes, and a later 18-request task about 30 minutes with single requests of 4–6 minutes. The elapsed status now makes such waits visible.
   - Tests that create per-pid scratch directories in `/tmp` without removing them can collide with a reused pid (`provider_capture_transport` did, fixed 2026-10-30); 9 test files use `temp_dir()`, check the rest leave nothing behind.
   - Library tests that fork must hold `coding::tests::PROCESS_GATE.write()`: a fork briefly holds other tests' journal `flock`s. The ungated agent bash test made `failed_material_acknowledgement_…` fail in every full run until gated; other older spawners (`coding::invocation`, `runtime::provider_tests`, `provider_validation_tests`) are still ungated.
2. **Terminal UI.** The inline prompt editor is done (`src/agent_cli/editor.rs`). Next is the retained alternate-screen experience from the design (browse output while it streams, expand tool details, selection/copy).
3. **Long sessions.** Handle context overflow with compaction or truncation of old tool output. Less urgent for gpt-6-luna (Scramjet's model table: 1.05M context, 922K max input) than for smaller-window models.
4. **More models.** Proxy serves Claude, GPT and DeepSeek models (`/v1/models`). Proxy rejects Claude models on `/v1/responses` (`guardUnsupportedProtocol` in `~/repos/proxy/proxy.ts`), so Claude needs an Anthropic Messages client (`/v1/messages`); ask the user before adding that route. On 2026-10-30 `gpt-6.1-sol` and `deepseek-v4.1-flash` ran a bash call and replied over `/v1/responses` (`--model`); `deepseek-v4-pro` is Messages-only too (Proxy answers with a synthetic assistant message saying so, which the agent shows as a reply).
5. **Parallel subagents.** Read-only helper agents sharing the client, as in the design.
6. **Clean up.** Remove specialty code that no longer serves a consumer (haiku journey, fixed validation pair, `coding` invocation store) once nothing depends on it. Ask before deleting large user-visible commands.

## Retired

The S4.13/S4.14 task records (planning, candidate fingerprints, review rounds and the live-call ledger rules) were removed after commit `cfa1ee4`; read them with `git show cfa1ee4:docs/tasks/<id>.md` if needed. The 32-request lifetime ledger cap and stop-on-unknown-usage rule were replaced by per-run limits on 2026-10-30.
