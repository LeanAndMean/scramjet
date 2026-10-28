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

- Reconcile the big picture against actual implementation and evidence, then prioritize a short queue of useful bounded outcomes.
- Leave one eligible next action, or an explicit stop/blocker, without implementing tasks or generating an exhaustive plan.

## Context

<user-context>
$ARGUMENTS
</user-context>

Follow the development workflow in `CLAUDE.md`. Read progress, design, and the relevant roadmap stages; inspect current files and Git evidence. Honor explicit user priorities within authorized scope. Reconcile completed, interrupted, review-pending, and blocked work rather than trusting the cursor blindly.

Own task selection and cross-task ordering. Update the short priority queue in `docs/PROGRESS.md` with dependencies, each near-term outcome/acceptance boundary, and the next action. Existing actionable tasks need no planning ritual. A blocker may leave independent work eligible, but selecting that work does not resolve the blocker or grant missing authorization. If a correction loop is making no progress, address its cause or stop that task rather than repeatedly dispatching it.

Route the selected task by evidence: `rs:review <task-id>` for a current candidate awaiting review; `rs:work <task-id>` for ready work or concrete corrections; `rs:plan <task-id>` for material unresolved design or sizing. Do not implement the task, run a full review, or overwrite its acceptance without authority. Ask for user-owned decisions only with the necessary context and consequences.

## Complete the handoff

Persist queue decisions, blockers and the handoff, and release ownership in progress. Present the complete concise result in assistant text, then call `report_scramjet_command_status` in the same turn; do not end with prose alone or wait for the fallback probe. Write performed-work `summary` first and `status: "completed"` when reconciliation is established, even if some tasks remain blocked.

Include one eligible `next_steps` entry: `message` is the evidence-selected `/rs:work <task-id>`, `/rs:plan <task-id>`, or `/rs:review <task-id>`, with `fresh_session: true` and a concrete `reason`. Set `recommended_next_step` to that entry's actual array index. The harness applies the user's chain/pause settings; no handoff confirmation is needed for already-authorized work. Do not chain triage to itself.

For necessary user alignment, use `get_scramjet_user_input` with informed context. For no eligible work or another stop, report the truthful status under `CLAUDE.md` and omit `next_steps`. Stopping the chain does not mean omitting the report.
