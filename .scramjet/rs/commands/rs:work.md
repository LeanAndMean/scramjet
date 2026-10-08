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
- Leave a reviewable candidate and accurate evidence, affected documentation, and fresh-session handoff.

## Task

<user-context>
$ARGUMENTS
</user-context>

Follow `CLAUDE.md` and the selected task record. With no task ID, use the queue's eligible next action; if it needs another role or priorities are missing, route rather than silently reprioritizing. An unknown explicit ID needs reconciliation, not substitution. Preserve completed work and distinguish stale evidence from unfinished implementation.

Own implementation, its tests/demonstration, and affected documentation. Fix failures discovered during the work rather than transferring ordinary debugging to review. Prefer the smallest runnable slice and safe executable checks over broader architecture discussion. Review corrections against their evidence before changing code; rejected or out-of-scope findings need a recorded reason, not silent deletion.

Record candidate identity, actual checks, and review need in progress. A material contract/risk change can add review need with a concrete reason; work cannot waive an existing requirement. Corrections return to independent review of the affected candidate. Work may report its role completed while the task remains `awaiting-review`; only a verified slice needing no independent review becomes `done` here. Failed required checks remain unfinished or blocked.

Persist the handoff and deliver the result before status reporting under `CLAUDE.md`. Prefer `/rs:review <task-id>` when required, otherwise `/rs:work <next-task-id>` only from the still-valid queue. Return to `/rs:triage` at a milestone boundary, exhausted/invalidated queue, or cross-task priority change. `/rs:plan <task-id>` is valid after a completed bounded investigation establishes necessary design, not as a way to declare unfinished implementation completed. Blocked/incomplete work never auto-advances.
