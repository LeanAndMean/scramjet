# Staged roadmap

The strategy is small, complete working features plus early risk probes, not an exhaustive framework followed by a late integration. [Design](DESIGN.md) owns direction and [the task list](TASKS.md) holds current work. Milestones below describe outcomes, not a fixed future file structure.

## Delivery rhythm

Pick the most useful next feature, build the smallest complete version, verify it for real (through Proxy when it involves the provider), and commit. Split a stage into a few tasks only when it becomes active; do not pre-plan every future interface. Revise mechanisms when evidence warrants it without silently changing user-owned scope.

Keep transport/runtime and native TUI work moving in parallel where they are independent. Missing native hardware blocks that acceptance claim, not unrelated headless work. Every stage leaves a usable demonstration, a recorded evidence boundary, and a specific next task. Complete targeted fixes when necessary; defer unrelated improvements rather than widening the session indefinitely.

## S0 — Runnable foundation

**Demonstration:** a minimal Rust CLI/library builds and runs offline; there is a repeatable local development loop.

- Establish the minimum Cargo structure, selected Apache-2.0 license file, ignore rules, and local Git history. Remote repository setup/publication needs separate authorization.
- Use the installed stable toolchain where suitable; do not impose nightly or heavy dependencies without a concrete requirement.
- Add only an initial CLI smoke and a small behavioral test. Record actual build/test commands and toolchain.
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
- Make read-only children and unattended runs genuinely confined with an OS sandbox (Landlock or bubblewrap on Linux, Seatbelt on macOS) that limits writes to declared directories, and network where needed. Approval questions and tool allowlists are not containment.
- Keep descendant histories as references, not recursive snapshots. Establish result/usage ownership and load details only when requested.
- Bound fan-out and storage/output backlog; exercise parent/child admission deadlocks, fairness, late results, cancellation, and cleanup.
- Strengthen journal settlement/recovery and artifact storage as required by this slice. Completed work must not restart on reconstruction.
- Add quota-aware request/token pacing, valid Retry-After cooldowns, and a first damped adaptive controller within hard ceilings. Use a local fixture with changing capacity, overload, proxy timeout, and rate-limit scenarios; verify unresolved remote attempts also bound replacement dispatch after timeout/cancellation.
- Add optional vLLM telemetry only when its deployment semantics are known; test missing/stale data and client-only operation. Do not make telemetry an authority or require a universal optimal queue threshold.

**Exit evidence:** parallel readers complete against known bounds; all ordinary/retry/compaction requests share admission; cancellation and uncertain remote usage are truthful; feedback responds to simulated pressure without unbounded queues or uncontrolled oscillation. Record per-process versus shared quota enforcement.

## S4 — Integrated daily-use Scramjet

**Demonstration:** a project tab runs the headless runtime through the retained TUI, including parallel subagents and representative command-driven work.

- Integrate through bounded typed projections, not direct transcript mutation or a token-to-repaint dependency.
- Provide essential editing, live browsing, expansion, agent summaries/detail, selection, model/effort choice, and structured interaction. Keep the presentation easy to restyle ([design](DESIGN.md#native-interaction)).
- Add representative command/agent/skill discovery, composition, explicit completion, continuation/autonomy, and approval. Preserve user outcomes, not every Pi parser or legacy mode.
- Add useful session resume/compaction and selected history navigation; decide legacy import only from a real need.
- Repeat native copy/paste, focus, resize, hidden-control approval, external-editor, suspend/resume, and exit journeys against the integrated application. Include failure/negative controls that catch no-op Copy and intercepted-but-not-inserted Paste.
- Add real providers with `/login`, GitHub Copilot and OpenAI Codex first (user, 2026-11-04), and use explicit authority for live calls and publication. Exercise normal daily work before expanding catalogs.

**Status (2026-11-03):** the agent is the product and already has sessions with branching, compaction, command sets with next steps, read-only sandboxed subagents and a full-screen conversation view. Agreed order (user, 2026-11-03): remove the demos of S0–S3 and the runtime they exercised (done 2026-11-03; git history keeps them, their exit evidence stays historical, and S3's admission is written for the agent when it resumes); review the code drafted by test-only models; settings; the retained full-screen UI as the main interface; command sets installed by reference with Mach 12 runnable; one `/settings`; then the command lifecycle rebuilt as agreed in the second steer of 2026-11-03 ([design](DESIGN.md#agreed-product-shape-2026-11-03)), before the remaining command-set and small items; then (steer of 2026-11-04) the built-in commands named as in Claude Code and Codex, Scramjet's scrollbar, notes moved out of the editor's box and a short README. Second steer of 2026-11-04: copy and paste, then key remapping ([design](DESIGN.md#copy-paste-and-keys-2026-11-04)), then the rest of themes and a distinct look ([design](DESIGN.md#look-and-themes-2026-11-04)), then packages with Proxy moved out of the public core, providers with `/login`, and MCP ([design](DESIGN.md#packages-providers-and-mcp-2026-11-04)), then the theme editor. Steer of 2026-11-06: panels and the header banner as the defaults, with the theme following the terminal's background and panels' polish; built-in commands acting at once while the agent works; the theme editor's fixes; an opt-in sandbox for the main agent ([design](DESIGN.md#a-sandbox-for-the-main-agent-2026-11-06)); spelling; session chains; a harness that survives a full disk, able to start, reach providers and run tools so that the agent can help free space ([design](DESIGN.md#durable-history-context-and-presentation)). Second steer of 2026-11-06: fresh session shown and toggled on next steps and each response's timing recorded, right after built-in commands at once; a background row in the theme editor; `/usage` for quota, spend and performance ([design](DESIGN.md#usage-2026-11-06)), as polish before S3's remaining parts. S3's remaining parts and S5 wait until the daily driver is solid ([design](DESIGN.md#current-direction-2026-11-03)): lower priority, but kept in mind, so that nothing built before them corners them.

**Exit evidence:** daily-use journeys pass; native acceptance is tied to the exact candidate and tested matrix, not inherited from the probe. Completion of the selected offline milestone does not complete all S4. Known gaps are visible; no platform is described as first-class merely because it compiles.

**Platforms (user decisions, 2026-11-07 and 2026-11-09):** native Windows, in daily use since 2026-11-09, then macOS (Terminal.app, and Homebrew's xterm under XQuartz), both before what is left of S3's parallel work and S5. No platform is called supported until it has been used there.

**Command sets' home (user decision, 2026-11-09):** Mach 12 in a repository of its own, the `scramjet` set, its authoring skill and a built-in lint in this one, in file names that work on Windows and in Claude Code ([design](DESIGN.md#command-sets-home-and-scramjets-sunset-2026-11-09)), so that scramjet-rs needs nothing from Scramjet's repository, which will likely be sunset.

**Scramjet 2 and a clean history (user decision, 2026-11-10):** scramjet-rs becomes Scramjet 2.0 in the public `LeanAndMean/scramjet` once it can go public ([design](DESIGN.md#scramjet-2-and-a-clean-history-2026-11-10)). First, before any release: the history rewritten without the company's details, a check that keeps it so, and the loop's private facts moved out of the tracked files.

**Releases (user decision, 2026-11-09):** after the history is clean (2026-11-10), releases with executables for every supported platform (the Raspberry Pi 5 joins them) on the private repository, checked by CI, for the versions the user asks for ([design](DESIGN.md#releases-updates-and-going-public-2026-11-09)); `scramjet-rs update` updates the program from them, and "What's new" and `/changelog` show the changelog. The repository goes public once every platform works and the issues to fix before publishing are fixed.

**RPC mode (user decision, 2026-11-09):** `scramjet-rs --mode rpc` for front ends built in other repositories ([design](DESIGN.md#rpc-mode-and-off-device-front-ends-2026-11-09)), after the platforms and before what is left of S3's parallel work and S5, now the lowest priority. Its protocol design is signed off by the user before it is built; no network surface and no daemon in this repository.

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

Providers with their logins, packages with executables and MCP moved into S4's order on 2026-11-04. Potential work here includes stronger isolation/worktrees, optional local coordination, or later remote worker support. Choose one from evidence; do not build them all as a final framework stage. No mandatory Python/TypeScript runtime, Rust dylib ABI promise, centralized workflow model, or speculative cluster control plane.

**Exit evidence:** each addition has a named consumer, bounded authority and failure behavior, verified enabled-feature licenses, and a regression check for core standalone use.

## Evidence discipline across stages

Local fixture tests establish client behavior, not backend entitlement or real server capacity. Headless TUI tests establish cells/input dispatch, not desktop gestures or clipboard contents. Native journeys establish only their declared platform/profile/revision. A queued task count establishes neither in-flight requests nor useful throughput.

Keep current work in `TASKS.md` and the record of changes in Git. Raw traces and benchmark outputs stay out of orientation documents. Every commit that changes the program bumps the version and records it in `CHANGELOG.md` ([CLAUDE.md](../CLAUDE.md); user decision, 2026-11-04).
