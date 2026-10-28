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
- Persist and locally checkpoint enough direction for work to begin in a fresh session without implementing the feature or planning the whole harness.

## Task

<user-context>
$ARGUMENTS
</user-context>

Apply the shared Git entry-recovery policy in `CLAUDE.md` before beginning this role.

Follow `CLAUDE.md`, `docs/TASKS.md` selection context and `docs/tasks/<task-id>.md` authority, including relevant parent/dependency records and linked evidence. Read log/archive only when supporting history is needed. Verify actual structure before a nontrivial file-level plan. Use small safe probes when they can settle uncertainty; prefer evidence to further theory. Essential responsibilities and invariants govern component choices. User-owned scope or architecture decisions require informed alignment; ordinary implementation choices do not.

Own the selected task's plan: update its outcome, scope/non-goals, acceptance demonstration, necessary design decisions, readiness and review need. Simple plans remain in the task record; link a separate authoritative plan only when size or external authority warrants it. Design owns durable product decisions, not detailed task tactics. Append dated planning evidence to `docs/PROGRESS.md`, not another mutable plan/status there. Planning may edit those records and disposable probes, not feature implementation.

If subdivision is necessary, preserve parent acceptance and create stable substantial child records/IDs/dependencies; accepting one child does not finish its parent. Triage owns cross-task queue membership/order. Return to triage when decomposition or evidence changes those priorities; otherwise hand actionable work directly to work. Unresolved required decisions leave the task blocked, not ready.

## Complete the handoff

Persist the task's plan/readiness/review need and next action, append a dated activity entry and release task ownership. Apply the shared Git checkpoint policy in `CLAUDE.md` before terminal reporting, including orderly stops. Present the complete concise result in assistant text, then call `report_scramjet_command_status` in the same turn; do not end with prose alone or wait for the fallback probe. Write performed-work `summary` first and `status: "completed"` when this planning outcome is established.

Include one eligible `next_steps` entry: `message` is `/rs:work <task-id>` for actionable planning or `/rs:triage` for needed queue/selection reconciliation under the shared completion contract, with `fresh_session: true` and a concrete `reason`. Set `recommended_next_step` to that entry's actual array index. The harness applies the user's chain/pause settings; no handoff confirmation is needed for already-authorized work.

For necessary user alignment, use `get_scramjet_user_input` with informed context. Apply the shared [completion, reconciliation and stop rules](../../../CLAUDE.md#active-rs-completion-contract); a stop still requires a truthful status report.
