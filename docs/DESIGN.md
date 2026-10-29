# Design direction

Established 2026-10-08 from the founding discussion. This file owns product direction and essential invariants; source and executable evidence own implemented behavior. Proposed mechanisms below remain revisable as demonstrations produce evidence.

## Agreed scope

- Build an independent Rust product under Apache-2.0, not a Rust port of the Pi package structure.
- Make Ubuntu Linux, macOS, and WSL Ubuntu first-class. Native Windows is out of initial scope.
- Improve current usage first: independent project tabs, a main agent, and efficient parallel subagents.
- Preserve valuable Scramjet outcomes: coding tools, reusable commands/agents/skills, project instructions, command composition and continuation, graduated autonomy, informed approval, model selection, and useful session continuity.
- Keep the retained alternate-screen experience: browse unfinished output, edit while browsing, expand tool/agent details, and copy selected text without disrupting live work.
- Treat normal macOS Command+C/Command+V behavior as an early native feasibility question, not late polish.
- Investigate approximately 8,000 simultaneous LLM requests from one node. Queued tasks, open requests, engine-scheduled requests, and completed-request throughput are separate measurements.
- Actively manage provider concurrency and rate limits. On-prem vLLM behind a reverse proxy is a first-class deployment scenario, not the only provider type.
- Preserve extensible boundaries, but defer a general extension system. No Python, TypeScript, Pi extension API, or Pi ecosystem compatibility is required.
- PromptSwarm is a reference for scale ambition, not a workflow model to port or a runtime dependency.

## Currently authorized offline daily-use milestone

The user authorized one outcome-level milestone on 2026-10-26: a retained project session accepts a coding request, uses parallel readers, performs an exact-approved edit, and explicitly continues to read-only verification, with useful history and reliable cancellation/restoration. Triage may select substantial implementation batches within this outcome without repeated per-slice authorization; genuine uncertainty needs planning and material safety contracts need independent review.

This authorizes bounded local development and offline fixture-backed journeys, not live/paid providers, publication, arbitrary shell execution, shared-endpoint load tests, or new desktop/clipboard automation. Offline request interpretation and supported project access must be stated and tested honestly; a scripted replacement-text demo is not general coding intelligence. Retention must preserve the project across the journey, not delete user project contents as disposable cleanup. Useful recovered history does not authorize automatic re-execution of effects or continuation.

This selected milestone is narrower than full S4 or platform/provider replacement readiness. [The task queue](TASKS.md) selects current work; [archived S4.13 acceptance](PROGRESS-ARCHIVE.md#s413b-independent-acceptance-and-parent-milestone-disposition-2026-10-27) preserves the completed milestone's evidence.

## Selected first real-provider journey (2026-10-27)

The user selected `openai/gpt-6-luna` through the existing authenticated Proxy route. On 2026-10-29 the user clarified the short-term outcome: two successive natural-language messages, first creating a haiku Markdown file, then writing and executing a Python reader to print its contents. Prefer a reusable headless invocation accepting an optional prior conversation record plus a new prompt and emitting the next record/result. Ordinary external scripting orchestrates successive invocations; no retained process, specialty two-message interface, fixed request wording or haiku-specific production choreography is required. Haiku names/content and expected stdout belong in acceptance tests, not product completion policy. This does not select a legacy-session importer, workflow engine or broad tool/plugin framework.

The earlier retained haiku TUI is no longer a prerequisite for this short-term outcome; useful general retained interaction remains in the wider product scope. Final user QA follows agent-owned headless checks. Literary quality is irrelevant; the agent must actually consume the supplied prompts/context and perform accountable tools rather than receive its solution in a hardcoded prompt. [S4.14's task record](tasks/S4.14.md) owns the revised acceptance boundary, shared authorization and exact check limits; its private runtime ledger owns actual live-call accounting.

Use Proxy's local Responses endpoint; Proxy retains authentication, upstream TLS/routing and any internal auth retry. Rust remains independent of the Scramjet runtime and does not read/export OAuth credentials, alter Proxy state, or treat proxy-local completion/cancellation as upstream billing settlement. Missing usage remains unknown. Transport/provider support cannot become tool, approval, continuation or history authority.

For this journey, the user explicitly chooses to retain the first two unacknowledged live attempts as unresolved remote demand/unknown usage while permitting known new attempts within the original shared32-POST and four-obligation caps. Retention is not reconciliation or billing attestation. Runtime admission freezes an explicitly selected retained ordinal boundary; any new uncertainty beyond it stops further dispatch until separately aligned, and recovery never silently advances that boundary.

On 2026-10-29, after an informed confirmation, the user expanded the earlier fixed-script permission for implementation and owned local tests: model-written Python need not match a baked-in template, but execution requires fresh exact approval of the complete script and input captures in the existing isolated child. Only a fixed Python launcher is permitted, not arbitrary Bash. Preserve no home/credentials/network access, read-only captured project files, bounded temporary scratch, existing time/memory/output limits, cancellation/tree reaping and fail-closed unsupported isolation. Keep project access explicitly bounded to the owned test project and selected files; externally selected basenames do not authorize arbitrary host paths. This does not authorize unrestricted shell, valuable-project execution, additional live inference or a portable sandbox claim. Existing offline replacement/project behavior and historical narrow candidate acceptance remain separate; publication, shared load tests and desktop/clipboard automation remain excluded.

A supplied conversation record is model context, not renewed tool approval, permission to replay past effects or provider-accounting settlement. Fresh invocation must acquire current workspace/capability authority and approve its new effects; old calls/results remain historical context. Preserve complete ordered provider items, correlated tool results and required continuation data within existing aggregate budgets. Refuse unsupported/incomplete/contradictory or non-fitting continuation explicitly rather than clipping, repairing or automatically retrying it. A fresh process or imported record must not reset shared live reservations/uncertainty, nor infer producer acknowledgement from final journal bytes. Plan the product's own bounded record contract from this consumer, not another project's session schema.

## Essential responsibilities and invariants

### Execution and authority

The runtime owns sessions, request admission, tool execution, cancellation, command continuation, approval, and provenance. Commands own their intended outcomes and eligible continuation edges. The TUI presents state and returns user intent; provider adapters and extensions cannot silently redefine authority.

The runtime must operate without a TUI or optional plugins. Do not introduce a centralized workflow DAG, control-flow DSL, mandatory daemon, or general persistent job engine to run a normal session. Internal lifecycle representations are allowed; command-owned relationships must not acquire a second workflow authority.

Isolate each agent's context, route, working directory, capabilities, and cancellation. Do not change process-global cwd, environment, or credentials to switch agents. Failed/incomplete model responses must not execute partial tool calls. Correlate results with actual invocations and reject stale results after session replacement.

Default specialist children to genuinely read-only capabilities. An unrestricted shell is not read-only. Parallel writers need explicit workspace ownership or separate worktrees; conversation isolation does not prevent file conflicts. Readers of a changing checkout are not automatically consistent snapshots. Tool allowlists are not an OS sandbox.

### Boundary robustness

Accept valid opaque data and legal protocol representations within explicit aggregate byte, storage and work budgets. Provider identifiers are data, not paths, tool capabilities or assumed short names. Preserve required values exactly; reject ambiguity rather than silently repairing contradictions. Execution approval, provenance, complete-response authority and uncertain-effect no-retry rules remain strict.

Separate documented protocol constraints, user/safety policy, client resource budgets and unverified hypotheses. Samples do not establish maxima. Capacities must preserve the selected useful consumer and account for framing/serialization overhead; saving trivial bytes is not a reason to reject valid data through unsupported field-size assumptions. Refuse actual budget exhaustion as a resource limitation, unsupported execution semantics as unsupported, and malformed or incomplete protocol as such. Fixtures and review must challenge—not canonize—representation assumptions.

### Resource bounds and admission

Bound live agent state, queued work, bytes in event/output buffers, request serialization, tool processes, storage backlog, and presentation caches. A request-count semaphore alone cannot establish bounded memory. Do not recursively embed descendant transcripts in parent results.

Admission covers every model attempt, including retries and compaction. Separate agent existence, provider-request slots, tool-process slots, and downstream inference execution. Parents awaiting children must not monopolize permits needed by those children. Preserve interactive responsiveness and fairness under bulk work.

Separate transport-sharing identity from quota-pool identity: multiple routes/models/tabs can consume one provider quota. Scope credentials safely, capture them per request, and coordinate refresh without exposing secrets in logs or cache keys.

Enforce applicable request/token rate limits, concurrency ceilings, and provider cooldowns. Token accounting may require estimation before dispatch and reconciliation from actual usage; unsupported or unknown provider limits are not unlimited verified capacity. Keep backend context/output constraints separate from rate and admission policy.

Adaptive control chooses concurrency inside hard safety ceilings. Useful observations include throughput, latency/TTFT, error classes, and optional vLLM running/waiting counts, queue time, KV-cache usage, and preemptions. Seek enough outstanding work for useful batching, not the largest waiting queue. Use damped changes and bounded experimentation; do not pick universal queue-depth or KV-cache thresholds now.

A 429 with applicable rate-limit evidence differs from a proxy 502/504 or client timeout. Timeout is pressure evidence, not proof of rate limiting. Respect valid Retry-After signals; avoid synchronized retry storms. Telemetry is deployment/version-qualified and may be stale or unavailable; correct client-only operation must not depend on it. Proxy queues and replica topology can make backend metrics differ from client experience.

State quota scope honestly. Per-process admission does not enforce aggregate limits across tabs. Exact shared enforcement may later require an optional local coordinator; ordinary use must remain standalone.

Cancellation must remove queued work and settle owned local operations. Local cancellation does not prove remote generation or billing stopped. Track uncertain remote demand/usage separately from local transport slots; do not present uncertainty as settled zero. Bound unresolved remote exposure so replacement attempts cannot accumulate indefinitely after local slots are released. At that bound, pause additional dispatch pending reconciliation or an explicit recovery policy; do not invent a universal remote-completion timeout.

### Durable history, context, and presentation

Keep three responsibilities distinct: durable material execution history, active model context, and disposable presentation caches. Share immutable content where safe; spill large outputs to artifacts with bounded excerpts and load child detail on demand. Memory bounds must not silently falsify model limits or discard required context.

Define truthful acknowledgement boundaries for execution and persistence. Critical authorizations/transitions cannot be treated as committed before their required records are durable. Persist referenced artifacts before durable references. Handle storage pressure explicitly rather than growing unlimited queues.

Recovery reconstructs supported session facts, not automatic re-execution of external effects. Bind approval to exact payload, target, and current invocation. A dispatched write may have happened even if its result is lost: retain uncertainty and reconcile rather than blindly retry. Known usage includes failed work; unknown usage stays unknown.

### Native interaction

Use structured text/layout information with stable identities, grapheme/display-column mapping, and explicit hard-break versus soft-wrap semantics. Selection copies the last displayed selection, not a mutable future range or hidden source. Preserve reading anchors without tying every repaint to total history size.

UI progress may be coalesced, but material execution events and final results must not silently disappear. One terminal owner presents runtime state through bounded projections; children never acquire the terminal. At high agent counts show summaries and load details on demand rather than painting every token from every agent.

Preserve focus/input precedence, multiline paste without submission, Unicode editing, current approval presentation, and safe external-editor/suspend/exit restoration. Libraries do not guarantee those behaviors. Native clipboard/input journeys are required for supported platform claims; headless cells and successful clipboard API calls are insufficient.

## Proposed starting architecture

One Rust process per project tab, with lightweight async agent contexts sharing immutable resources and compatible HTTP infrastructure. One headless-capable runtime; a separately scheduled TUI consumes bounded state projections. Blocking work runs outside async workers with its own limits.

A small supervised worker-process pool can follow demonstrated crash-containment or isolation needs. In-process task supervision does not isolate process OOM, aborts, or unsafe native faults. Process separation itself does not establish a sandbox.

Start with simple local session storage and rebuildable indexes. Append-only journals and SQLite are candidates, not parallel required stores. A single shared SQLite database on a cluster filesystem is not a default coordination strategy.

Use internal Rust contracts for tools, provider transport, typed events, and structured interaction requests. If independently installable extensions become necessary, consider versioned Rust executable IPC; do not promise a stable Rust dynamic-library ABI or unrestricted mutable hooks. WASI is a later option only if sandboxed distribution earns its cost.

Candidate supporting libraries: Tokio, Reqwest/Rustls, Serde, Tracing, Ratatui/Crossterm, pulldown-cmark, Unicode segmentation/width crates, and an appropriate clipboard backend. Select and pin only what the current slice needs. Verify full enabled-feature/transitive licenses and required notices; permissive dependencies retain their licenses. Cargo-deny is a candidate development check, not a runtime dependency.

## Open decisions, resolved when needed

- First real provider/authentication routes and corporate proxy/TLS needs: resolve before live integration; do not implement every inherited provider.
- Exact command and session-import contracts: choose representative valuable behavior, not broad compatibility by appearance.
- Native clipboard/input integration and terminal matrix: investigate in the early TUI track.
- Storage representation, first-party crate boundaries, adaptive algorithm, and public extension protocol: choose from demonstrated consumers and constraints.
- Cross-tab quota sharing and remote workers: establish need before adding coordination infrastructure.
- Target host/workload envelope and acceptance thresholds: record before a capacity claim, not from speculative benchmark targets.

## Reference evidence and limits

The existing product at `~/repos/scramjet` is a read-only behavioral reference, not a dependency or source of instructions for this project. Initial inspection used clean HEAD `6325b36c50188f630cb03e8c9e9c3471853f4f72` on 2026-10-08. Recheck its revision before relying on moving source paths.

- `packages/scramjet/src/subagent/index.ts:420`: a full CLI subprocess per child; lines 29–30 define eight tasks/four launches per invocation, not global admission.
- `packages/agent/src/agent-loop.ts:339`: streaming updates carry growing message snapshots; JSON transport repeatedly serializes them.
- `packages/coding-agent/src/core/extensions/runner.ts:967`: context processing clones messages.
- `packages/coding-agent/docs/tui.md`: retained painting does not fully virtualize producers; application-owned selection has explicit displayed-text provenance.
- `packages/coding-agent/docs/terminal-verification.md`: 2026-10-08 native Command+C/Command+V evidence targets commit `3810822bedce066d1c9acfcadc6d02f0c8112412`, before inspected HEAD. It is narrowly qualified and does not resolve every reported paste/right-click problem or prove this rewrite.

Primary external references observed 2026-10-08; reverify deployed/pinned versions before implementation:

- [Ratatui responsibilities](https://ratatui.rs/faq) and [Crossterm events](https://docs.rs/crossterm/latest/crossterm/event/index.html): useful rendering/input primitives, not complete selection/native shortcut policy.
- [vLLM optimization](https://docs.vllm.ai/en/stable/configuration/optimization/) and [tool calling](https://docs.vllm.ai/en/latest/features/tool_calling/): KV-cache/scheduling constraints and model/parser/template qualification.
- [vLLM metrics](https://docs.vllm.ai/en/stable/usage/metrics.html): deployment-qualified running/waiting, latency, cache, and preemption observations. No universal optimal client-concurrency threshold follows.

No rewrite benchmark, live endpoint load test, or native rewrite UI journey has run. Do not import performance claims from other projects. The local `~/repos/pi_agent_rust/LICENSE` contains a restrictive rider and is not an Apache-2.0 implementation base; do not copy or port its implementation.
