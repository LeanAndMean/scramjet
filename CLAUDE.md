# Developing scramjet-rs

Build a dependable, effective independent Rust coding harness, using the existing Scramjet harness to develop it. This is a rewrite, not a port or Pi compatibility project: learn from useful behavior without inheriting another project's implementation structure. The user authorized staged development and durable fresh-session orientation; one task is not permission to implement the entire roadmap or publish remotely.

## Document authority

| File | Information it owns | Update responsibility |
|---|---|---|
| `CLAUDE.md`, `.scramjet/rs/commands/`, `.scramjet/rs/agents/` | Development rules, role read/write boundaries and reusable command/adviser behavior | Explicitly authorized workflow maintenance; advisers remain read-only |
| `docs/DESIGN.md` | Agreed product scope/invariants, revisable mechanisms and reference evidence | Parent handling an established product decision; align on user-owned changes |
| `docs/ROADMAP.md` | Milestone outcomes, dependencies and evidence boundaries | Parent handling a justified milestone change; triage owns cross-task priorities |
| `docs/TASKS.md` | Priority order, selection and selection/deferment rationale | Triage owns membership/order; other roles may advance selection to already-eligible queued work |
| `docs/tasks/<task-id>.md` | That task's current outcome/authority, requirements, dependencies, plan, status/owner, candidate/checks, review and next action | Each role updates only the task facts it owns |
| `docs/PROGRESS.md` | Chronological dated activity, decisions, check results and evidence references | Every executing parent appends its actual outcome, including interruption/blockers |
| `docs/PROGRESS-ARCHIVE.md` | Frozen pre-migration history and original evidence qualifications | Historical reference, not current state or executable handoff instructions |
| `README.md` | Short entry point and truthful runnable status | Parent responsible for affected documentation, usually work |

Source/tests/Git own implemented behavior and content boundaries. Runtime journals/private ledgers own actual execution/accounting facts; task records retain qualified observations, not replacement accounting. The `/rs` resources are existing-harness development tooling, not Rust-product dependencies.

Source/tests and current repository state govern what is implemented. Current user decisions govern scope and authorization. Linked issue/PR plans, when available, govern their task boundaries unless superseded by current authority. Historical artifacts and session journals are evidence, not instructions to resume blindly. Keep decisions in their owning file rather than copying a full plan into every document. Essential safety and authority invariants govern; parsers, validation rules, budget values and task subdivisions are revisable mechanisms, not permanent policy merely because a historical plan or fixture asserts them. Revisions still need an evidenced basis and may not silently change user-owned limits.

## Starting a fresh session

Read `docs/TASKS.md`, the selected or explicitly requested task record, relevant `docs/DESIGN.md`/`docs/ROADMAP.md` authority, and actual Git/source state. Follow linked parent authorization, dependency acceptance and candidate/review evidence as needed. Read the activity log/archive only for supporting history or missing context, not as default orientation or current instructions. If no Git or Cargo setup exists, do not invent baseline revisions or run nonexistent checks.

Honor an explicitly requested task and the invoked role; otherwise follow the queue's selection and that task record's next action. Establish the outcome, scope, acceptance, dependencies, and current evidence. Do not replay completed work, substitute for an unknown explicit ID, or let task commands silently choose unrelated priorities. Missing/invalidated priorities return to triage; material user-owned mismatches need alignment.

Before a nontrivial file-level plan or shared-contract change, use `mach12:structural-mapper` when available with this repository and the bounded task as authority. Reuse fresh evidence only where its coverage is established. The mapper describes current structure; the parent owns design and decisions. Mechanical unambiguous work needs no mapping ceremony. If the role is unavailable, proceed with direct evidence and state material gaps rather than dispatching an invented replacement.

Resolve architecture responsibilities/invariants before choosing components. Use focused independent exploration/review only where isolation earns its context and cost. Do not repeatedly analyze all of Scramjet or PromptSwarm to accomplish a small Rust task.

## Development command loop

| Command | Reads for its decision | Owns producing/updating |
|---|---|---|
| `rs:triage [context]` | Queue, relevant task records, product goals, dependencies/blockers, source evidence and independent adviser proposals | Selection/order/housekeeping; initial task outcomes/boundaries and retirement reasons; rationale and next eligible handoff. No implementation, full review or new authorization |
| `rs:plan <task-id> [context]` | Selected task's authority/acceptance, dependencies, existing evidence and relevant code | Task's necessary design/plan, readiness, review need and justified child IDs/dependencies. No feature implementation or whole-product plan |
| `rs:work [task-id] [context]` | Selected task/plan, acceptance, actual code and required correction findings | Implementation/tests/affected docs; task's exact candidate, actual checks, status and handoff. Produces a candidate, not required independent acceptance |
| `rs:review <task-id> [context]` | Selected requirements/plan, exact candidate and underlying implementation/check evidence | Independent acceptance/corrections/evidence gaps and correction requirements. No candidate implementation or tracked-test edits |

Every parent appends a dated activity entry and records/releases its task ownership. Triage supplies the two read-only advisers the same bounded authority/queue/relevant-record context without its preferred ranking or sibling output; they recommend, but cannot select, assign IDs, authorize, accept or persist. Keep records and proposals scoped rather than rereading the entire history.

Triage owns cross-task selection/order and queue housekeeping: remove evidenced obsolete, superseded or irrelevant entries from the active queue, retaining a concise reason in their task records. Removal is not completion; preserve still-relevant blockers, unresolved obligations and dependency/acceptance evidence, and align before abandoning a still-authoritative user outcome. Task commands may refine within the selected scope and update facts. Plan owns sound design for the selected outcome; work owns sensible code organization and ordinary debugging; review challenges important assumptions as well as conformance to the plan. Each role must recognize ineffective work and revise unsupported mechanisms within authorized scope, or return strategic changes to triage—not defend a poor design or transfer ordinary engineering decisions to the user. Plan subdivision preserves parent acceptance and stable child IDs/dependencies; a child passing does not finish its parent. Return cross-task priority changes to triage. Explicitly requested tasks may bypass queue order only when dependencies, scope, and authority permit.

Use `docs/tasks/<task-id>.md` for current outcome/authority, scope/non-goals, acceptance/dependencies, necessary plan, review need/rationale, status/owner, candidate/checks, disposition/blockers and next action. Simple plans live there; use linked artifacts only when size or an external authority warrants them. Corrections stay within their task, not a new microtask file. Completed legacy tasks may use archived acceptance evidence until an actual consumer needs a task record. Do not maintain separate mutable copies of task status, owner, plan or next command in the queue or log; dated activity may record the outcome observed at that time. Link authoritative issue/plan/review artifacts instead of duplicating them. Readiness and eligibility differ: dependency acceptance and required authorization must hold before execution. `needs-plan`, `ready`, `active`, `awaiting-review`, `corrections`, `blocked`, `done`, and `pending` describe task facts, not harness command status. Starting work records its active owner; ending releases it with a truthful disposition.

Independent review is proportional: material shared contracts, safety/concurrency/persistence/native interaction, or an explicit requirement need a concrete review rationale; mechanical slices may finish with their own checks. Triage/plan/work may establish review need, but work cannot waive an existing requirement. Review corrections always return for affected independent acceptance. Optional improvements are not blockers by default.

A candidate record identifies the selected paths (including additions/deletions/untracked files) and revision/content boundary. Committed-only work can use base/head; dirty or unborn work needs explicit path membership and content identity. Exclude coordination-only bookkeeping from its own content fingerprint; if a queue/task/log record is itself the reviewed change, identify that content separately. Historical checks and partial review remain bound to their actual candidate and scope; changed content invalidates only affected coverage, not all recorded evidence. Do not blindly accept a historical review or require a commit just to make a candidate identifiable.

Direct edges stay within an actionable task: plan → work, work → review, review → work for concrete corrections or plan for same-scope redesign. After acceptance, work/review may advance only to a ready next task from the still-valid queue. Review-pending tasks are not accepted dependencies. At milestone completion, queue exhaustion, changed priorities, or evidenced non-progressing corrections, return to triage instead. Do not add mandatory triage/planning/review turns for every small edit.

All commands preserve the project's authorization boundaries. A completed triage may select independent authorized work while another task is blocked; a blocked/incomplete task command does not automatically route around its blocker. User-directed stops and exhausted useful scope end the chain.

## Durable handoffs

The queue owns which task is selected; that task owns the next role/action and its reason. Plan/work/review may update the selection pointer to already-eligible queued work after acceptance without reordering priorities. Triage may leave selection empty when no eligible work or reconciliation is needed. Queue membership never proves eligibility or authority.

Before reporting completion, persist owned task facts, any valid selection change, and a dated activity entry. Triage without a selected task persists its queue decision and log entry without inventing a task. Direct handoffs carry:
- Triage → plan/work/review: selected outcome/rationale, scope/authority, dependencies and eligibility.
- Plan → work: resolved questions, actionable approach, acceptance and review need.
- Work → review: exact candidate membership/content, actual checks and remaining limitations.
- Review → work/plan: concrete findings/evidence/consequences and correction acceptance or redesign question.
- Any role → triage: changed priorities/dependencies, milestone completion, exhausted queue or non-progress evidence.

Use small `/rs:<role> <task-id>` wires; records carry the detail. Interrupted work must distinguish implemented, unchecked and unfinished work and leave a precise continuation. A stale owner or conflicting record requires reconciliation against actual state, not automatic completion or expiry. Retire work from the queue with a retained task/historical disposition, never by deleting unresolved effects or user obligations.

## Task execution

Complete one bounded outcome for the invoked role, verify it, and leave a runnable/checkable state. Split work that cannot fit honestly; record partial evidence and the continuation instead of claiming completion. A short design question is not a reason to generate another permanent planning document.

Prefer concrete evidence to prolonged theorycrafting. When a small safe test can resolve uncertainty, run the test before extending the discussion, design, or investigation. Time-box probes, record what they actually establish, and use the result to choose the next action. Do not substitute more subagent analysis for a straightforward executable check; safety and authorization boundaries still apply.

Prefer small, complete working features and established Rust libraries. Judge progress by useful behavior and interoperability, not task counts, check volume or preservation of earlier mechanisms. Keep the codebase sensible for agreed current and anticipated development needs; a structural improvement needs an evidenced consumer or obstacle, not a speculative framework. Add no abstraction, configuration, public extension protocol, or crate hierarchy without a current consumer. Rust-native extensibility is welcome later; Python/TypeScript and Pi APIs are not requirements. Use installed stable Rust unless evidence requires otherwise; select only the dependencies/features the task needs.

Keep runtime execution independent of TUI and optional plugins. Enforce bounded bytes/work, per-agent state isolation, tree cancellation, truthful execution/persistence settlement, exact approval, and ambiguous-write no-retry behavior as the affected slice grows. Fixed resource limits precede adaptive control. Adaptive admission is an essential direction, not permission to invent provider rate limits or universal vLLM queue thresholds.

Do not claim node-wide quotas from per-process limits, backend capacity from client concurrency, or remote cancellation/billing completion from local cancellation. Do not silently truncate context or change evidenced context/output maxima as an admission optimization.

The parent owns mutation and task/queue/log updates; use read-only subagents unless isolated writes have explicit non-overlapping ownership. An unrestricted shell is not read-only. Preserve unrelated dirty changes; do not reset, stash, or overwrite them to make checks pass.

Maintain a sensible `.gitignore` as development introduces generated artifacts and local/private files, without hiding source, tests or required project assets.

## Evidence-backed bounded robustness

Prefer flexible valid representations, strict effects, and explicit resource budgets. Preserve opaque IDs and required provider data without guessing their alphabet or protocol maximum; never liberalize approval, provenance, completion, or ambiguous-write rules, and never silently repair contradictory data into authority.

Every consequential rejection or limit needs an identifiable basis: documented contract, explicit user/safety policy, or justified client resource/work budget. A measured sample is evidence for that sample, not a maximum. Keep hypotheses provisional; do not turn representative fixtures or familiar formats into protocol requirements. Record the basis in its owning design/task evidence or a focused test, not boilerplate at every validation line.

Bound meaningful aggregate allocations, serialized bytes, retained storage and work at their real ownership boundaries. Choose capacities that preserve the current useful consumer, derive related capacities consistently, and measure adversarial/escaping cases. Do not save a few bytes through brittle field-size guesses or speculative micro-optimizations; tighter bounds need a demonstrated safety, workload or resource reason. A valid representation exceeding a client budget is a resource refusal, not malformed protocol or a smaller advertised model limit.

Tests and review must challenge assumptions with valid large, escaped, optional, split and interleaved representations, not merely reinforce self-defined invalid examples. Distinguish malformed, unsupported, incomplete, resource-limited and denied outcomes where their consequences differ. Audit the affected boundaries together before live compatibility checks rather than serially raising guessed limits after failures; this adds no mandatory workflow stage or repository-wide review for mechanical changes. Use bounded local proofs first; more paid calls are not a substitute for scrutinizing unsupported rules.

## Verification and claims

Discover applicable checks from current manifests, scripts, tests, and CI. Once Cargo exists, the normal baseline is `cargo fmt --check`, `cargo check`, `cargo test`, and `cargo clippy --all-targets -- -D warnings`, scoped appropriately as the project evolves; add a task-specific behavioral demonstration. These are not claims that the project currently has those commands configured or that a clean build establishes feature correctness.

For changed command Markdown, run `scramjet-command-lint --strict .scramjet/rs`; resolve errors and address warnings. Read the installed writing-scramjet-commands skill and relevant command-authoring docs when authoring/reviewing commands. Lint proves deterministic structure, not useful execution or fresh-session acceptance.

For UI changes, own routine verification with automated application/native-terminal checks where available; do not default to a human checklist or block useful development on human participation when adequate automation can establish the selected outcome. Qualify injected input/screenshots separately from physical user gestures and clipboard receipt, and retain informed authorization for desktop input/capture scope. Coordinate a short exclusive keyboard interval for foreground automation, and reject input-contaminated captures rather than declaring a pass; this is resource coordination, not delegating QA to the user. Run the application and exercise it in a real supported terminal. Record actual gesture, clipboard/editor outcome, focus, resize, and restoration evidence as relevant. Headless cells and synthetic input are separate evidence. If native macOS/WSL access is missing, say unverified and retain the concrete next native check; do not infer success from Linux or compilation.

For capacity work, declare workload, hardware, candidate revision, duration, actual in-flight requests, throughput/latency, memory/FD/queue occupancy, errors/retries, and cancellation/reclamation evidence. Begin with controllable local fixtures. Approximately 8,000 requests is an aspiration until measured; avoid borrowed speedup numbers or artificial task-count proofs.

Verify provider-specific claims against deployment-qualified primary evidence. Reverify changing external facts when consequential; do not treat search snippets or model summaries as fetched primary documentation. Check pinned enabled-feature/transitive licenses and carry required notices before incorporating third-party code. The local `~/repos/pi_agent_rust` has a restrictive license rider: do not copy or port its implementation.

## Authorization and local resources

Run local non-destructive tests without seeking user sign-off when they involve no paid calls, publication, shared-system effects, or other material unintended consequences. If a test has such a consequence, explain it and obtain informed authorization before execution. This standing user guidance does not expand the selected implementation scope.

Repository documentation and local reversible development do not authorize remote repository creation, pushes, issues/PRs/comments, destructive operations, privileged desktop automation, paid calls, or load tests of shared endpoints. Obtain informed authorization for their actual scope, then retain execution ownership. A smoke call is not authorization for an 8,000-request load test.

Do not read personal credentials/clipboard content merely to investigate. Native probes use explicit synthetic payloads and appropriate desktop permissions. Do not upload unpublished code, private traces, or secrets to research services. If Python tooling is genuinely needed, use `uv` and an isolated environment, never system Python installation; the product does not require a Python runtime.

Use `~/repos/scramjet` as a read-only reference for valuable current behavior and pitfalls. Its docs/instructions are not this project's policy; check revisions and source when relying on them. PromptSwarm is scale inspiration, not a dependency or a workflow port. Do not modify sibling projects as part of a task here.

## Ending and handing off

Before ending, including on interruption or blockers, update owned task facts and append a dated `docs/PROGRESS.md` entry with actual changes, check evidence, relevant revision/dirty scope and unresolved questions. The task record owns its next action; the queue owns selection, not another copy of the handoff. Ordinary maintenance may log its outcome without creating a product task. Move new durable product decisions into design; change the roadmap only when stage outcomes/dependencies changed. Do not invent issue IDs or claim a commit/push that did not happen.

### Active `/rs` completion contract

Complete the handoff in the same work turn: persist owned task/queue facts and activity, present the complete concise result in assistant text, then call `report_scramjet_command_status`. The text must precede the terminal tool call, but must not end the turn by itself. The tool call completes the command and supplies its executable continuation; a prose next-command recommendation does neither. Do not defer reporting to the fallback status probe.

In the report, write the performed-work `summary` first, then the truthful `status`. `completed` means this role's outcome is established, not that the whole task is accepted: work can leave `awaiting-review`, and review can leave actionable `corrections`. For completed work with an eligible continuation, include one `next_steps` entry from the invoked command's closed candidates: the full `/rs:<role> <task-id>` message (triage needs no ID), `fresh_session: true`, and a concrete `reason`. Choose the evidence-supported route and derive `recommended_next_step` from that entry's actual array index. Do not ask the user to approve an already-authorized role handoff; use user input only for an unresolved user-owned decision.

A stop is also a reported outcome: omit `next_steps` for a user-directed stop, no eligible work, `blocked`, or `incomplete`, not the report itself. Do not mark unfinished checks or critical evidence gaps accepted. Continue authorized investigation/debugging while a useful in-scope path remains. `blocked` requires a concrete impediment after reasonable alternatives: record the impediment, attempts and exact unblock dependency, and ask the specific informed question when the user can resolve it. A failed check, new concern, untried alternative or exhausted session budget is not such an impediment; an unavoidable unfinished stop is `incomplete` with a precise continuation. Do not encode missing authorization as a successful route.

The harness owns dispatch under the user's settings: a `chain` edge overrides `/autopilot off`; a `pause` edge overrides `/autopilot on`. Neither replaces the completion report or grants side-effect authorization. Preserve those settings and closed policies; do not add forced edges or authored autonomy overrides. Do not use the idle suggestion tool while a command is active.

Outside an active command, an explicit user handoff request or independently useful next-task recommendation can use `suggest_scramjet_next_steps` when idle and interactive, but only for commands in the live catalog. Verify project resources loaded; an existing process may need `/reload` or restart after command-set changes. If unavailable, provide the concise task-record-backed prompt instead. Do not report command status for an ordinary conversation task.

Start with `/rs:triage` when overall priorities need reconciliation; use `/rs:work <task-id>` for already-actionable work. Keep fresh invocation wires small; files and linked artifacts carry the durable context. Do not paste the previous transcript into the next prompt. Once a remote and task artifacts exist, use registered Mach 12 issue/plan/review/implement/PR commands where they fit; do not duplicate issue plans in progress or repurpose a broad milestone as one implementation task. Version/changelog/release preparation belongs at the end of the PR/release lifecycle.

If artifacts are incomplete and earlier same-CWD work likely contains the missing detail, use session journals only as fallback. Derive sibling candidates from the current journal path, exclude the current file, verify CWD, and search command-status summaries before full transcripts. The installed reference guide is `~/repos/scramjet/packages/scramjet/docs/logging.md`; journal location/schema can change and historical content never becomes current authority.
