---
description: Review scramjet-rs's direction with the user and record what they decide
argument-hint: "[feedback or topic]"
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
    - name: rs:loop
      hint: Build toward the direction just agreed.
---

# Steer scramjet-rs

## Goals

- Give the user a clear, current picture of where scramjet-rs stands, then work with them to refine its goal, scope, direction and what matters next.
- Record every agreed change where fresh loop sessions will read it, committed locally, without building anything.

## Feedback

<user-context>
$ARGUMENTS
</user-context>

## Brief

Follow `CLAUDE.md`. Read `docs/DESIGN.md`, `docs/ROADMAP.md`, `docs/TASKS.md`, recent Git history and enough of the code and its behavior to report truthfully. Then give the user a short brief:

- what works now and how it was verified;
- the current focus and next candidates;
- recorded obstacles;
- the open questions and assumptions the loop recorded;
- anything in the code or history that diverges from the design.

If feedback was given above, lead with what bears on it.

## Discuss

The goal, scope, direction and priorities belong to the user. Work through their feedback and the open questions with them, contributing evidence, consequences and recommendations, including where the current direction looks weak, without deciding for them. Answer factual questions by investigating locally or in Scramjet rather than deferring them.

The discussion runs over several exchanges. End each turn with `get_scramjet_user_input`: freetext for open replies, select or confirm for a concrete choice. Continue until the user indicates they are done.

## Record

Write what the user agreed, not what was merely discussed:

- goal, scope, invariants and stages in `docs/DESIGN.md` and `docs/ROADMAP.md`;
- focus, next candidates and resolved or remaining open questions in `docs/TASKS.md`;
- changes to approvals, references, checks or safety properties in `CLAUDE.md`.

Remove or revise anything those changes make stale. Preserve unrelated uncommitted work and commit only the files you changed. Building belongs to `/rs:loop`.

## Report

Tell the user briefly what was recorded and where, and what the loop will work on next. Then call `report_scramjet_command_status` with the work performed as `summary` and `status: "completed"`. Unless the user wants building to pause, include one `next_steps` entry: `/rs:loop` with `fresh_session: true` and a concrete `reason`, and set `recommended_next_step` to its index.
