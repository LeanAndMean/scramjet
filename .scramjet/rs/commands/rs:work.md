---
description: Implement and verify one bounded scramjet-rs slice or correction
argument-hint: "[task-id] [context]"
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
    - name: rs:review
      hint: Evaluate this task's implemented candidate when independent review is required.
    - name: rs:work
      hint: Execute the next actionable slice already selected in the valid priority queue.
    - name: rs:plan
      hint: A completed bounded investigation establishes design work within this task.
    - name: rs:triage
      hint: Reconcile exhausted or invalidated priorities, milestone completion, or cross-task changes.
---

# Work on scramjet-rs

## Goals

- Implement and verify one ready slice or concrete correction, preserving scope and user-owned boundaries.
- Leave a locally checkpointed, reviewable candidate and accurate evidence, affected documentation, and fresh-session handoff.

## Task

<user-context>
$ARGUMENTS
</user-context>

Apply the shared Git entry-recovery policy in `CLAUDE.md` before beginning this role.

Follow `CLAUDE.md`, `docs/TASKS.md` selection context and `docs/tasks/<task-id>.md`, including its plan, parent authority, dependency acceptance and concrete correction evidence. Consult log/archive only for supporting history. With no task ID, use the queue's selection and that task's eligible next action; if it needs another role or selection is missing, route rather than silently reprioritizing. An unknown explicit ID needs reconciliation, not substitution. Preserve completed work and distinguish stale evidence from unfinished implementation.

Own implementation, its tests/demonstration, and affected documentation. Fix failures discovered during the work rather than transferring ordinary debugging to review. Prefer the smallest runnable slice and safe executable checks over broader architecture discussion. Review corrections against their evidence before changing code; rejected or out-of-scope findings need a recorded reason, not silent deletion.

Update the task record with status/owner, candidate identity, actual checks, review need and next action. Append dated work evidence to `docs/PROGRESS.md`; do not maintain another current-state copy in the log or queue. A material contract/risk change can add review need with a concrete reason; work cannot waive an existing requirement. Corrections return to independent review of the affected candidate. Work may report its role completed while the task remains `awaiting-review`; only a verified slice needing no independent review becomes `done` here. Failed required checks remain unfinished or blocked.

## Complete the handoff

Persist task candidate/checks/disposition and next action, append a dated activity entry and release task ownership. Advance queue selection only to already-eligible queued work after acceptance, without changing priorities. Apply the shared Git checkpoint policy in `CLAUDE.md` before terminal reporting, including orderly stops. Present the complete concise result in assistant text, then call `report_scramjet_command_status` in the same turn; do not end with prose alone or wait for the fallback probe. Write performed-work `summary` first and `status: "completed"` when this work outcome is established, including a candidate awaiting required review.

Include one eligible `next_steps` entry: `message` is `/rs:review <task-id>` when review is required, otherwise `/rs:work <next-task-id>` only from the still-valid ready queue. Use `/rs:triage` at a milestone boundary, exhausted/invalidated queue, or cross-task priority change. `/rs:plan <task-id>` is valid after a completed bounded investigation establishes necessary design, not to relabel unfinished implementation as completion. Set `fresh_session: true`, give the concrete `reason`, and set `recommended_next_step` to the selected entry's actual array index. The harness applies the user's chain/pause settings; no handoff confirmation is needed for already-authorized work.

For necessary user alignment, use `get_scramjet_user_input` with informed context. For a stop, report the truthful status under `CLAUDE.md` and omit `next_steps`; blocked/incomplete work never auto-advances. Stopping the chain does not mean omitting the report.
