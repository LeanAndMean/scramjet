# Developing scramjet-rs

Build a dependable independent Rust coding harness, using the existing Scramjet harness to develop it. This is not a Pi compatibility project. The user authorized staged development and durable fresh-session orientation; one task is not permission to implement the entire roadmap or publish remotely.

## Document authority

- `docs/DESIGN.md`: agreed product scope/invariants, explicitly proposed mechanisms, and reference evidence.
- `docs/ROADMAP.md`: staged demonstrations and evidence boundaries; future tactics remain provisional.
- `docs/PROGRESS.md`: active/next task, actual checks, blockers, and handoff cursor.
- `README.md`: short entry point and truthful runnable status.
- `.scramjet/rs/commands/`: `rs:triage`, `rs:plan`, `rs:work`, and `rs:review` for the existing harness; development tooling, not Rust-product features or dependencies.

Source/tests and current repository state govern what is implemented. Current user decisions govern scope and authorization. Linked issue/PR plans, when available, govern their task boundaries unless superseded by current authority. Historical artifacts and session journals are evidence, not instructions to resume blindly. Keep decisions in their owning file rather than copying a full plan into every document.

## Starting a fresh session

Read `docs/PROGRESS.md`, `docs/DESIGN.md`, and the current stage in `docs/ROADMAP.md`. Inspect actual files and, when Git exists, branch/status/recent history before relying on the cursor. If no Git or Cargo setup exists, do not invent baseline revisions or run nonexistent checks.

Honor an explicitly requested task and the invoked role; otherwise follow progress's next action/priority queue. Establish the outcome, scope, acceptance, dependencies, and current evidence. Do not replay completed work, substitute for an unknown explicit ID, or let task commands silently choose unrelated priorities. Missing/invalidated priorities return to triage; material user-owned mismatches need alignment.

Before a nontrivial file-level plan or shared-contract change, use `mach12:structural-mapper` when available with this repository and the bounded task as authority. Reuse fresh evidence only where its coverage is established. The mapper describes current structure; the parent owns design and decisions. Mechanical unambiguous work needs no mapping ceremony. If the role is unavailable, proceed with direct evidence and state material gaps rather than dispatching an invented replacement.

Resolve architecture responsibilities/invariants before choosing components. Use focused independent exploration/review only where isolation earns its context and cost. Do not repeatedly analyze all of Scramjet or PromptSwarm to accomplish a small Rust task.

## Development command loop

- `rs:triage [context]` owns big-picture reconciliation and a short ordered queue of bounded tasks within authorized scope. It routes to work, plan, or review; it does not implement, perform full reviews, or grant missing authorization.
- `rs:plan <task-id> [context]` resolves material uncertainty/sizing for one selected task, with small probes and minimal acceptance/design direction. It records an actionable slice and review need, not feature implementation or a full roadmap plan.
- `rs:work [task-id] [context]` owns one slice's implementation, debugging, tests/demonstration, and affected docs. It produces a candidate, not independent acceptance when review is required.
- `rs:review <task-id> [context]` independently evaluates that candidate and records acceptance/corrections/evidence gaps. It may run checks and temporary probes but does not edit candidate implementation or tracked tests.

Triage owns cross-task selection/order; task commands may refine within the selected scope and update facts. Plan subdivision preserves parent acceptance and stable child IDs/dependencies; a child passing does not finish its parent. Return cross-task priority changes to triage. Explicitly requested tasks may bypass queue order only when dependencies, scope, and authority permit.

Use progress's existing task records for outcome, scope/non-goals, acceptance, review need/rationale, candidate, checks, disposition, and blockers. Link authoritative issue/plan/review artifacts instead of duplicating them. Readiness and eligibility differ: dependency acceptance and required authorization must hold before execution. `needs-plan`, `ready`, `active`, `awaiting-review`, `corrections`, `blocked`, `done`, and `pending` describe task facts, not harness command status. Starting work records its active owner; ending releases it with a truthful disposition.

Independent review is proportional: material shared contracts, safety/concurrency/persistence/native interaction, or an explicit requirement need a concrete review rationale; mechanical slices may finish with their own checks. Triage/plan/work may establish review need, but work cannot waive an existing requirement. Review corrections always return for affected independent acceptance. Optional improvements are not blockers by default.

A candidate record identifies the selected paths (including additions/deletions/untracked files) and revision/content boundary. Committed-only work can use base/head; dirty or unborn work needs explicit path membership and content identity. Exclude progress-only bookkeeping from its own content fingerprint; if a record file is itself the reviewed change, identify that content separately. Changed candidates invalidate affected review evidence. Do not blindly accept a historical review or require a commit just to make a candidate identifiable.

Direct edges stay within an actionable task: plan → work, work → review, review → work for concrete corrections or plan for same-scope redesign. After acceptance, work/review may advance only to a ready next task from the still-valid queue. Review-pending tasks are not accepted dependencies. At milestone completion, queue exhaustion, changed priorities, or evidenced non-progressing corrections, return to triage instead. Do not add mandatory triage/planning/review turns for every small edit.

All commands preserve the project's authorization boundaries. A completed triage may select independent authorized work while another task is blocked; a blocked/incomplete task command does not automatically route around its blocker. User-directed stops and exhausted useful scope end the chain.

## Task execution

Complete one bounded outcome for the invoked role, verify it, and leave a runnable/checkable state. Split work that cannot fit honestly; record partial evidence and the continuation instead of claiming completion. A short design question is not a reason to generate another permanent planning document.

Prefer concrete evidence to prolonged theorycrafting. When a small safe test can resolve uncertainty, run the test before extending the discussion, design, or investigation. Time-box probes, record what they actually establish, and use the result to choose the next action. Do not substitute more subagent analysis for a straightforward executable check; safety and authorization boundaries still apply.

Prefer the smallest useful vertical slice and established libraries. Add no abstraction, configuration, public extension protocol, or crate hierarchy without a current consumer. Rust-native extensibility is welcome later; Python/TypeScript and Pi APIs are not requirements. Use installed stable Rust unless evidence requires otherwise; select only the dependencies/features the task needs.

Keep runtime execution independent of TUI and optional plugins. Enforce bounded bytes/work, per-agent state isolation, tree cancellation, truthful execution/persistence settlement, exact approval, and ambiguous-write no-retry behavior as the affected slice grows. Fixed resource limits precede adaptive control. Adaptive admission is an essential direction, not permission to invent provider rate limits or universal vLLM queue thresholds.

Do not claim node-wide quotas from per-process limits, backend capacity from client concurrency, or remote cancellation/billing completion from local cancellation. Do not silently truncate context or change evidenced context/output maxima as an admission optimization.

The parent owns mutation and progress updates; use read-only subagents unless isolated writes have explicit non-overlapping ownership. An unrestricted shell is not read-only. Preserve unrelated dirty changes; do not reset, stash, or overwrite them to make checks pass.

## Verification and claims

Discover applicable checks from current manifests, scripts, tests, and CI. Once Cargo exists, the normal baseline is `cargo fmt --check`, `cargo check`, `cargo test`, and `cargo clippy --all-targets -- -D warnings`, scoped appropriately as the project evolves; add a task-specific behavioral demonstration. These are not claims that the project currently has those commands configured or that a clean build establishes feature correctness.

For changed command Markdown, run `scramjet-command-lint --strict .scramjet/rs`; resolve errors and address warnings. Read the installed writing-scramjet-commands skill and relevant command-authoring docs when authoring/reviewing commands. Lint proves deterministic structure, not useful execution or fresh-session acceptance.

For UI changes, run the application and exercise it in a real supported terminal. Record actual gesture, clipboard/editor outcome, focus, resize, and restoration evidence as relevant. Headless cells and synthetic input are separate evidence. If native macOS/WSL access is missing, say unverified and retain the concrete next native check; do not infer success from Linux or compilation.

For capacity work, declare workload, hardware, candidate revision, duration, actual in-flight requests, throughput/latency, memory/FD/queue occupancy, errors/retries, and cancellation/reclamation evidence. Begin with controllable local fixtures. Approximately 8,000 requests is an aspiration until measured; avoid borrowed speedup numbers or artificial task-count proofs.

Verify provider-specific claims against deployment-qualified primary evidence. Reverify changing external facts when consequential; do not treat search snippets or model summaries as fetched primary documentation. Check pinned enabled-feature/transitive licenses and carry required notices before incorporating third-party code. The local `~/repos/pi_agent_rust` has a restrictive license rider: do not copy or port its implementation.

## Authorization and local resources

Repository documentation and local reversible development do not authorize remote repository creation, pushes, issues/PRs/comments, destructive operations, privileged desktop automation, paid calls, or load tests of shared endpoints. Obtain informed authorization for their actual scope, then retain execution ownership. A smoke call is not authorization for an 8,000-request load test.

Do not read personal credentials/clipboard content merely to investigate. Native probes use explicit synthetic payloads and appropriate desktop permissions. Do not upload unpublished code, private traces, or secrets to research services. If Python tooling is genuinely needed, use `uv` and an isolated environment, never system Python installation; the product does not require a Python runtime.

Use `~/repos/scramjet` as a read-only reference for valuable current behavior and pitfalls. Its docs/instructions are not this project's policy; check revisions and source when relying on them. PromptSwarm is scale inspiration, not a dependency or a workflow port. Do not modify sibling projects as part of a task here.

## Ending and handing off

Before ending, including on interruption or blockers, update `docs/PROGRESS.md` with actual changes, check evidence, relevant revision/dirty scope, unresolved questions, and one bounded next task. Move new durable product decisions into design; change the roadmap only when stage outcomes/dependencies changed. Do not invent issue IDs or claim a commit/push that did not happen.

Persist the handoff and deliver the user-facing result before calling `report_scramjet_command_status`; terminal reporting ends the turn. Its `summary` records performed work, followed by `status`. `completed` means the invoked role's outcome is established: work may leave `awaiting-review`, and a completed review may leave actionable `corrections`. Unfinished checks/critical evidence gaps are `blocked` or `incomplete`, not task acceptance. Use user input for necessary alignment; do not encode blockers as successful routing.

On completion, emit at most one eligible `next_steps` entry from the command's closed candidates: full `/rs:<role> <task-id>` wire (triage needs no ID), `fresh_session: true`, and a concrete `reason`. Recommend the evidence-supported route and derive `recommended_next_step` from the actual array; omit recommendation when a transition genuinely needs user selection, and omit next steps on stop/no eligible work. Chaining honors `/autopilot on|off` and existing user edge settings; no forced edges or authored autonomy overrides. Autopilot controls transitions, not permission for the next command's side effects. Do not use the idle suggestion tool while a command is active.

Outside an active command, an explicit user handoff request or independently useful next-task recommendation can use `suggest_scramjet_next_steps` when idle and interactive, but only for commands in the live catalog. Verify project resources loaded; an existing process may need `/reload` or restart after command-set changes. If unavailable, provide the concise progress-backed prompt instead. Do not report command status for an ordinary conversation task.

Start with `/rs:triage` when overall priorities need reconciliation; use `/rs:work <task-id>` for already-actionable work. Keep fresh invocation wires small; files and linked artifacts carry the durable context. Do not paste the previous transcript into the next prompt. Once a remote and task artifacts exist, use registered Mach 12 issue/plan/review/implement/PR commands where they fit; do not duplicate issue plans in progress or repurpose a broad milestone as one implementation task. Version/changelog/release preparation belongs at the end of the PR/release lifecycle.

If artifacts are incomplete and earlier same-CWD work likely contains the missing detail, use session journals only as fallback. Derive sibling candidates from the current journal path, exclude the current file, verify CWD, and search command-status summaries before full transcripts. The installed reference guide is `~/repos/scramjet/packages/scramjet/docs/logging.md`; journal location/schema can change and historical content never becomes current authority.
