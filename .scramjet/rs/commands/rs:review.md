---
description: Independently evaluate a scramjet-rs candidate and route acceptance or corrections
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
      hint: Correct this candidate or execute the next ready task after acceptance.
    - name: rs:plan
      hint: Evidence requires redesign within the same authorized task.
    - name: rs:triage
      hint: Reconcile milestone completion, changed priorities, or an exhausted queue.
---

# Review one scramjet-rs candidate

## Goals

- Independently assess the current candidate against the selected task's acceptance and controlling invariants, using focused executable evidence where useful.
- Record acceptance, required corrections, or critical evidence gaps with enough detail for the next session, without fixing the implementation or widening scope.

## Task

<user-context>
$ARGUMENTS
</user-context>

Follow `CLAUDE.md` and the task's authoritative plan/evidence. Identify the exact candidate, including relevant uncommitted/untracked changes; confirm it is reviewable rather than reviewing a claimed result or unrelated dirty tree. Changed candidate contents cannot inherit an earlier acceptance.

Use a fresh independent read-only perspective for material review, with the smallest suitable specialist and bounded brief; review commands with the command specialist, code with the code specialist. The parent owns synthesis, executable checks, disposition, interaction, and the ledger. Do not repeat checks solely for ceremony or equate a clean build with behavioral acceptance.

Evaluate requirements from their authority, not from hypothetical future perfection. Use focused tests/probes to challenge important behavior. Do not edit implementation or its tracked tests; temporary probes and generated check artifacts are permissible within authority. Persist only review/handoff records and necessary evidence; candidate changes by another actor invalidate affected review evidence.

Required findings need concrete evidence, consequence, and correction acceptance. Distinguish optional improvements; they do not prevent acceptance unless current authority requires them. Assess disputed findings independently rather than automatically fixing or accepting them. Critical requirements that cannot be verified leave the task blocked, never silently accepted. Accepted candidates become `done`; required bounded corrections become `corrections`; redesign becomes `needs-plan`. A completed review with actionable findings can report `completed` even though the task is not done.

Persist the disposition and deliver the result before reporting under `CLAUDE.md`. Recommend `/rs:work <task-id>` for bounded corrections, `/rs:plan <task-id>` for same-scope redesign, or the next queued `/rs:work <next-task-id>` after acceptance. Return to `/rs:triage` when priorities change, a milestone finishes, the queue is exhausted, or repeated corrections show no progress. Unresolved critical evidence/authorization gaps or unfinished review stop rather than auto-advance.
