---
description: Reconcile scramjet-rs progress and route the next bounded task
argument-hint: "[context]"
allowed-tools:
  - read
  - bash
  - edit
  - write
  - subagent
  - get_scramjet_user_input
  - report_scramjet_command_status
next:
  mode: closed
  candidates:
    - name: rs:work
      hint: Execute a ready slice or concrete corrections with satisfied dependencies.
    - name: rs:plan
      hint: Resolve the selected slice's material design or scope uncertainty.
    - name: rs:review
      hint: Independently evaluate an implemented candidate awaiting acceptance.
---

# Triage scramjet-rs

## Goals

- Compare worthwhile existing work with independently proposed directions, then prioritize and maintain a lean queue that makes meaningful progress toward the Rust product goal.
- Leave accurate, locally checkpointed records and one eligible next action, or an explicit stop/blocker, without implementing tasks or generating an exhaustive plan.

## Context

<user-context>
$ARGUMENTS
</user-context>

Apply the shared Git entry-recovery policy in `CLAUDE.md` before beginning this role.

Follow the development workflow in `CLAUDE.md`. Read `docs/TASKS.md`, relevant task records and their dependency/authorization evidence, design and the relevant roadmap stages; inspect current files and Git evidence. Consult the activity log/archive only for supporting history, not current selection or status. Honor explicit user priorities within authorized scope. Reconcile completed, interrupted, review-pending, and blocked work rather than trusting the cursor blindly.

## Compare directions

Before fixing the candidate set or recommendation, dispatch `rs:existing-work-assessor` and `rs:new-direction-proposer` in parallel with `confirmProjectAgents: false`; the user has authorized these two repository-local advisers without repeated trust confirmation. Give both the same bounded context: repository/product authority paths, current user intent and authorization, source-state facts, and relevant queue/task records with linked evidence. Do not supply your preferred ranking, preselect their answer, or pass either the other's output. The existing-work assessor checks unfinished commitments and structural needs against code; the new-direction proposer independently considers worthwhile alternatives. Each returns up to three candidates with outcome, why now, evidence/uncertainty, dependencies/authority, effort/risk and first demonstration. Keep briefs and returned context compact; do not ask for full implementation plans or exhaustive inventories.

Use the named registered agents when available. If one cannot run or returns insufficient evidence, state the coverage gap and use direct bounded investigation for missing facts; do not claim independent coverage or dispatch an invented replacement. The proposals are advisory, not task acceptance, execution permission or a vote. No useful new candidate is a valid result.

## Select and persist

Own task selection and cross-task ordering. Merge overlap, verify consequential claims against current authority/source and compare the credible candidates by useful progress, dependencies, effort and risk. Consider structural changes when they support agreed future work; prefer complete working features over isolated trivialities, not a speculative framework. If a correction loop is making no progress, challenge its underlying assumptions or change the approach rather than merely dispatching another correction. Unsupported implementation restrictions need an evidenced basis, not preservation because tests currently assert them; essential safety and user-owned boundaries still govern.

Resolve ordinary engineering choices within authorized scope yourself. When a material scope/direction decision belongs to the user, or goals or authorization remain ambiguous, present compressed evidence, alternatives, consequences and your recommendation before asking. Proposals outside current scope may warrant alignment but never authorize themselves. A blocker may leave independent work eligible; choosing it does not resolve that blocker.

Own queue housekeeping: remove obsolete, superseded or no-longer-relevant work from the active queue when current evidence and direction justify it. Retain a concise disposition/reason in the owning task record so later sessions do not resurrect it; removal is not completion or acceptance. Distinguish obsolete work from still-relevant blocked work, and preserve unresolved effects, obligations and dependency evidence. Seek user alignment if removal would abandon a still-authoritative user outcome, not for routine cleanup.

Update `docs/TASKS.md` with priority order, selection and rationale. Create/update the relevant task records for outcomes, authority, dependencies/acceptance and retirement reasons; each task owns its next role/action. Do not mirror task status, ownership, plans or next commands in the queue. Append a dated `docs/PROGRESS.md` entry summarizing the selection, material deferred alternatives and supporting evidence rather than copying adviser reports. Move durable product decisions to design; change the roadmap only when milestone outcomes/dependencies change. Release any task ownership at handoff. Existing actionable tasks need no planning ritual.

Route the selected task by evidence: `rs:review <task-id>` for a current candidate awaiting review; `rs:work <task-id>` for ready work or concrete corrections; `rs:plan <task-id>` for material unresolved design or sizing. Do not implement the task, run a full review, or overwrite its acceptance without authority. Ask for user-owned decisions only with the necessary context and consequences.

## Complete the handoff

Persist queue selection/rationale, relevant task facts/blockers/next action and a dated activity entry; release any task ownership. If nothing is selected, record the queue decision/log without inventing a task. Apply the shared Git checkpoint policy in `CLAUDE.md` before terminal reporting, including orderly stops. Present the complete concise result in assistant text, then call `report_scramjet_command_status` in the same turn; do not end with prose alone or wait for the fallback probe. Write performed-work `summary` first and `status: "completed"` when reconciliation is established, even if some tasks remain blocked.

Include one eligible `next_steps` entry: `message` is the evidence-selected `/rs:work <task-id>`, `/rs:plan <task-id>`, or `/rs:review <task-id>`, with `fresh_session: true` and a concrete `reason`. Set `recommended_next_step` to that entry's actual array index. The harness applies the user's chain/pause settings; no handoff confirmation is needed for already-authorized work. Do not chain triage to itself.

For necessary user alignment, use `get_scramjet_user_input` with informed context. Apply the shared [completion, reconciliation and stop rules](../../../CLAUDE.md#active-rs-completion-contract); a stop still requires a truthful status report.
