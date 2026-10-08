---
description: Make one uncertain scramjet-rs task actionable
argument-hint: "<task-id> [context]"
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
      hint: Implement the now-actionable selected slice with satisfied dependencies.
    - name: rs:triage
      hint: Subdivision or new evidence requires cross-task priority reconciliation.
---

# Plan one scramjet-rs slice

## Goals

- Resolve only the selected task's material uncertainty and establish the smallest useful implementation slice with checkable acceptance.
- Persist enough direction for work to begin in a fresh session without implementing the feature or planning the whole harness.

## Task

<user-context>
$ARGUMENTS
</user-context>

Follow `CLAUDE.md` and the selected progress task's authority. Verify actual structure before a nontrivial file-level plan. Use small safe probes when they can settle uncertainty; prefer evidence to further theory. Essential responsibilities and invariants govern component choices. User-owned scope or architecture decisions require informed alignment; ordinary implementation choices do not.

Record the outcome, scope/non-goals, acceptance demonstration, necessary design decisions, and independent-review need in the existing task record or linked authoritative plan. Keep simple plans in `docs/PROGRESS.md`; do not create another document by default. Design owns durable product decisions, not detailed task tactics. Planning may edit those records and disposable probes, not feature implementation.

If subdivision is necessary, preserve the original task's acceptance and assign stable child IDs/dependencies; accepting one child does not complete its parent. Triage owns unrelated ordering. Return to triage when decomposition or evidence changes cross-task priorities; otherwise hand the actionable slice directly to work. Unresolved required decisions leave the task blocked, not ready.

Persist the plan and handoff, then deliver the result. Follow `CLAUDE.md`'s reporting contract: completed actionable planning recommends `/rs:work <task-id>` in a fresh session; completed priority-changing analysis recommends `/rs:triage`. Missing required decisions or unfinished planning do not automatically advance.
