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

Follow `CLAUDE.md`, `docs/TASKS.md` selection context and `docs/tasks/<task-id>.md` requirements/plan, parent/dependency authority and linked candidate/work evidence. Consult log/archive only for necessary history. Identify the exact candidate, including relevant uncommitted/untracked changes; confirm it is reviewable rather than reviewing a claimed result or unrelated dirty tree. Changed candidate contents cannot inherit an earlier acceptance.

Use a fresh independent read-only perspective for material review, with the smallest suitable specialist and bounded brief; review commands with the command specialist, code with the code specialist. When reviewer findings are returned, prefer chaining them into a fresh read-only independent assessment (`scramjet:independent-command-assessor` for command findings, `mach12:independent-assessor` for code findings). Give the assessor the candidate, controlling acceptance and reviewer claims; ask it to re-derive each claim from evidence and distinguish required corrections, optional nitpicks, rejected claims and evidence gaps before disposition. This is a reliability default, not a mandatory extra pass. The parent owns synthesis, executable checks, disposition, interaction, and the ledger; neither specialist's verdict is authority. Do not repeat checks solely for ceremony or equate a clean build with behavioral acceptance.

Evaluate requirements from their authority, not from hypothetical future perfection. Use focused tests/probes to challenge important behavior. Do not edit implementation or its tracked tests; temporary probes and generated check artifacts are permissible within authority. Update the task's independent review/disposition, candidate-bound findings/coverage, blockers and next action; append dated review evidence to `docs/PROGRESS.md`. Do not duplicate current status in the queue/log. Candidate changes by another actor invalidate affected review evidence.

Required findings need concrete evidence, consequence, and correction acceptance. Distinguish optional improvements; they do not prevent acceptance unless current authority requires them. Assess disputed findings independently rather than automatically fixing or accepting them. Critical requirements that cannot be verified leave the task blocked, never silently accepted. Accepted candidates become `done`; required bounded corrections become `corrections`; redesign becomes `needs-plan`. A completed review with actionable findings can report `completed` even though the task is not done.

## Complete the handoff

Persist task review/disposition/next action and a dated activity entry; release task ownership. Advance queue selection only to already-eligible queued work after acceptance, without changing priorities. Present the complete concise result in assistant text, then call `report_scramjet_command_status` in the same turn; do not end with prose alone or wait for the fallback probe. Write performed-work `summary` first and `status: "completed"` when this independent review outcome is established, including actionable corrections rather than task acceptance.

Include one eligible `next_steps` entry: `message` is `/rs:work <task-id>` for bounded corrections, `/rs:plan <task-id>` for same-scope redesign, or `/rs:work <next-task-id>` from the still-valid ready queue after acceptance. Use `/rs:triage` when priorities change, a milestone finishes, the queue is exhausted, or repeated corrections show no progress. Set `fresh_session: true`, give the concrete `reason`, and set `recommended_next_step` to the selected entry's actual array index. The harness applies the user's chain/pause settings; no handoff confirmation is needed for already-authorized work.

For necessary user alignment, use `get_scramjet_user_input` with informed context. Unresolved critical evidence/authorization gaps or unfinished review do not automatically advance: report the truthful stop status under `CLAUDE.md` and omit `next_steps`. Stopping the chain does not mean omitting the report.
