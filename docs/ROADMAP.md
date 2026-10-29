# Staged roadmap

The strategy is small, complete working features plus early risk probes, not an exhaustive framework followed by a late integration. [Design](DESIGN.md) owns direction; [the queue](TASKS.md) owns selection/order, task records own current state, and [progress](PROGRESS.md) records dated activity/evidence. Milestones below describe outcomes, not a fixed future file structure or an authorization to implement the entire stage.

## Delivery rhythm

Triage owns a short priority queue; plan resolves only material task uncertainty; work implements/checks one slice; review independently evaluates candidates when needed. Their exact responsibilities and routing live in `CLAUDE.md`, not a central workflow definition. Select one bounded task that fits a session and has an observable result. Split a stage into a few tasks only when it becomes active; do not pre-plan every future interface. Design enough to preserve essential responsibilities and invariants, then implement and verify the smallest integrated slice. Revise proposed mechanisms when evidence warrants it without silently changing user-owned scope.

Keep transport/runtime and native TUI work moving in parallel where they are independent. Missing native hardware blocks that acceptance claim, not unrelated headless work. Every stage leaves a usable demonstration, a recorded evidence boundary, and a specific next task. Complete targeted fixes when necessary; defer unrelated improvements rather than widening the session indefinitely.

## S0 — Runnable foundation

**Demonstration:** a minimal Rust CLI/library builds and runs offline; there is a repeatable local development loop.

- Establish the minimum Cargo structure, selected Apache-2.0 license file, ignore rules, and local Git history. Remote repository setup/publication needs separate authorization.
- Use the installed stable toolchain where suitable; do not impose nightly or heavy dependencies without a concrete requirement.
- Add only an initial CLI smoke and a small behavioral test. Record actual build/test commands and toolchain as dated progress evidence; the selected task record owns current acceptance.
- Establish dependency/license checking proportionally; do not build a plugin SDK or speculative crate hierarchy.

**Exit evidence:** CLI smoke, formatting, build, and tests succeed on the available host; unsupported/untested hosts remain explicit. Documentation reflects actual commands and layout.

## S1 — Two early feasibility demonstrations

### Transport/runtime track

**Demonstration:** multiple streamed requests to a controllable local fixture, under fixed admission limits, with cancellation and slow-consumer pressure.

- Implement the minimum provider-neutral request/result boundary and a first compatible HTTP streaming path.
- Exercise fragmented UTF-8/framing, failed/incomplete responses, bounded buffering, and terminal settlement through the actual transport path.
- Give every request the same fixed admission owner; establish separate request versus pending-work/byte bounds. Leave slots for quota identity and feedback without designing the final controller.
- Record a small reproducible baseline and start increasing concurrency inexpensively. Do not target 8,000 before the small workload is correct.

### Native TUI track

**Demonstration:** an isolated retained transcript/editor skeleton under synthetic streaming, without live model calls.

- Test browsing, editing, selection, copy, multiline paste at an interior caret, resize, focus transfer, and terminal cleanup.
- Challenge normal Command+C/Command+V on actual macOS terminals early. A copy API call or simulated key is not native acceptance.
- Establish representative Ubuntu and Windows Terminal/WSL journeys alongside macOS. Record unavailable environments and exact terminal/profile/revision qualifications.
- Keep this probe small and reusable in later integration; do not port arbitrary renderers, images, themes, or Pi components.

**Exit evidence:** transport behavior and resource bounds are demonstrated; native feasibility is either evidenced per platform or explicitly unresolved with a concrete next experiment. Unresolved native feasibility must be resolved before claiming the corresponding interactive replacement ready.

## S2 — Minimal headless coding vertical slice

**Demonstration:** one agent reads a disposable project, performs a controlled edit, invokes a bounded tool, and returns an accountable final result.

- Build the minimum completed-response/tool loop, schemas, allowlists, route configuration, and cancellation through S1 admission.
- Make working-directory and credential ownership explicit. Gate mutation before testing on valuable work; an unrestricted shell must not be classified as read-only.
- Preserve enough material history and operation certainty to diagnose interruptions. Do not defer all persistence/ambiguous-write semantics to a later framework.
- Exercise failed responses, invalid tool arguments, denied capabilities, tool errors, and cancellation. Inject an interruption after a disposable write is dispatched but before its result settles; restart and verify uncertain state is retained without repeating the write. Required authorization records precede the effect, and durable artifact references resolve.
- Avoid paid calls by using the fixture; a real provider smoke needs separately established endpoint authority.

**Exit evidence:** the integrated offline coding journey, interruption/reconstruction journey, and targeted failure tests pass; every model attempt enters admission, and tool execution/usage outcomes remain correlated. No uncertain dispatched write is automatically retried.

## S3 — Lightweight parallel agents and feedback

**Demonstration:** a parent uses parallel isolated readers without launching a complete CLI per child; cancellation, request accounting, and pressure feedback remain correct.

- Share immutable definitions/client infrastructure while isolating agent context, route, cwd, authority, and cancellation.
- Keep descendant histories as references, not recursive snapshots. Establish result/usage ownership and load details only when requested.
- Bound fan-out and storage/output backlog; exercise parent/child admission deadlocks, fairness, late results, cancellation, and cleanup.
- Strengthen journal settlement/recovery and artifact storage as required by this slice. Completed work must not restart on reconstruction.
- Add quota-aware request/token pacing, valid Retry-After cooldowns, and a first damped adaptive controller within hard ceilings. Use a local fixture with changing capacity, overload, proxy timeout, and rate-limit scenarios; verify unresolved remote attempts also bound replacement dispatch after timeout/cancellation.
- Add optional vLLM telemetry only when its deployment semantics are known; test missing/stale data and client-only operation. Do not make telemetry an authority or require a universal optimal queue threshold.

**Exit evidence:** parallel readers complete against known bounds; all ordinary/retry/compaction requests share admission; cancellation and uncertain remote usage are truthful; feedback responds to simulated pressure without unbounded queues or uncontrolled oscillation. Record per-process versus shared quota enforcement.

## S4 — Integrated daily-use Scramjet

**Demonstration:** a project tab runs the headless runtime through the retained TUI, including parallel subagents and representative command-driven work.

- Integrate through bounded typed projections, not direct transcript mutation or a token-to-repaint dependency.
- Provide essential editing, live browsing, expansion, agent summaries/detail, selection, model/effort choice, and structured interaction.
- Add representative command/agent/skill discovery, composition, explicit completion, continuation/autonomy, and approval. Preserve user outcomes, not every Pi parser or legacy mode.
- Add useful session resume/compaction and selected history navigation; decide legacy import only from a real need.
- Repeat native copy/paste, focus, resize, hidden-control approval, external-editor, suspend/resume, and exit journeys against the integrated application. Include failure/negative controls that catch no-op Copy and intercepted-but-not-inserted Paste.
- Select initial real providers/auth routes with the user, and use explicit authority for live calls and publication. Exercise normal daily work before expanding catalogs.

**Selected offline milestone (2026-10-26):** S4.13 now contains the authorized retained project request → parallel readers → exact-approved edit → explicit read-only verification journey, with useful history and cancellation/restoration. Its historical coding→companion outcome remains required, but is no longer a separate tiny prerequisite. Deliver two substantial batches: headless retained-project execution/history/continuation, then the consuming retained application and integrated journeys. Plan material project/runtime/evidence ownership uncertainty before the first batch; independently accept each material candidate. The application batch depends on accepted headless contracts; both are necessary for the parent outcome. Bounded offline fixtures establish harness behavior, not general model intelligence. No live/paid providers, publication, arbitrary shell, shared-endpoint load or new desktop/clipboard automation belongs to this authorization.

**Selected first-provider outcome (clarified 2026-10-29):** S4.14 prioritizes a reusable headless prompt plus optional prior-conversation-record invocation, with ordinary scripting orchestrating successive fresh processes. Demonstrate two natural-language messages: create Markdown, then write and exact-approve/execute the Python reader using the first record as context. Deliver real prompt/context/tool behavior, not a fixed-request haiku workflow or another throwaway consumer. Reuse provider/effect/isolation foundations; plan bounded record continuity and fresh authority, and preserve unresolved live-call obligations. The specialty retained haiku application is removed from this short-term acceptance sequence, not from Git; wider retained/native interaction goals remain. Offline proofs precede eligible deployed evidence and final headless user QA. [S4.14](tasks/S4.14.md) owns exact authorization and acceptance; [S4.14b](tasks/S4.14b.md) owns the same-task redesign.

**Exit evidence:** daily-use journeys pass; native acceptance is tied to the exact candidate and tested matrix, not inherited from the probe. Completion of the selected offline milestone does not complete all S4. Known gaps are visible; no platform is described as first-class merely because it compiles.

## S5 — Capacity and admission hardening

**Demonstration:** reproducible sustained high-concurrency workloads with responsive control, bounded resources, accountable failures, and explainable admission adjustments.

- Progress through measured concurrency levels toward approximately 8,000 actual in-flight requests on a declared host/workload. Distinguish synthetic transport capacity, stateful agent capacity, and actual inference-server behavior.
- Record throughput, TTFT/queue latency, tail latency, CPU, peak/steady memory, file descriptors, queue/byte occupancy, tool activity, cancellation/reclamation, and error/retry classes. Preserve the reproduction recipe and candidate identity.
- Challenge slow consumers, changing capacity, rate/token limits, proxy deadlines, backend queues, preemption, large contexts, disk pressure, and long soak. Compare fixed and adaptive admission on the same workloads.
- Validate proxy/replica telemetry and client-only behavior on authorized deployments; do not deliberately flood or stress shared endpoints without a separate bounded load-test authorization.
- Introduce cross-tab quota coordination only if required by measured usage. Keep standalone use available; do not claim global enforcement from local limits.
- Add regression thresholds only after stable baseline evidence. Benchmark claims must identify workload, hardware, revision, duration, uncertainty, and what was actually measured.

**Exit evidence:** publish only the capacity actually established. Failure to reach 8,000 is a measured engineering result, not a reason to manufacture a pass or indefinitely prevent useful adoption.

## S6 — Selective adoption and extensibility

**Demonstration:** the mature core gains an integration justified by actual use without making it essential to ordinary execution.

Potential work includes additional providers/auth routes, optional Rust extension executables, stronger isolation/worktrees, optional local coordination, or later remote worker support. Choose one from evidence; do not build them all as a final framework stage. No mandatory Python/TypeScript runtime, Rust dylib ABI promise, centralized workflow model, or speculative cluster control plane.

**Exit evidence:** each addition has a named consumer, bounded authority and failure behavior, verified enabled-feature licenses, and a regression check for core standalone use.

## Evidence discipline across stages

Local fixture tests establish client behavior, not backend entitlement or real server capacity. Headless TUI tests establish cells/input dispatch, not desktop gestures or clipboard contents. Native journeys establish only their declared platform/profile/revision. A queued task count establishes neither in-flight requests nor useful throughput.

Keep priority order/selection in `TASKS.md`, current requirements/state/handoff in the selected task record, and dated activity/evidence in `PROGRESS.md`. Once substantive work uses issues/PRs, link those artifacts instead of duplicating their full plans. Keep raw traces and benchmark outputs outside normal orientation documents; create focused permanent evidence files only when their size/reuse earns it. Release versioning and changelog preparation happen at the end of the release/PR lifecycle, not in feature tasks.
