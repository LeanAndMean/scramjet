---
description: Independently review recent scramjet-rs changes for material defects
argument-hint: "[commits or area]"
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
      hint: Continue building once required findings are fixed or recorded.
---

# Review scramjet-rs changes

## Goals

- Find material defects in the named changes, or in a sensible range of recent commits when none are named: bugs, safety problems, or behavior that does not work when actually used.
- Fix or record each required finding so building can continue.

## Scope

<user-context>
$ARGUMENTS
</user-context>

## Review

Follow `CLAUDE.md`. Get an independent perspective from a fresh read-only reviewer subagent, then verify each of its findings yourself against the code and by running it; the reviewer's output is evidence, not authority. Keep only findings with concrete evidence and a real consequence. Optional improvements are not blockers.

Fix required findings that are small and clear, verify, and commit. Put the rest at the top of `docs/TASKS.md`. Stop for the user only as described in `CLAUDE.md`.

## Report

Tell the user briefly what was reviewed, what was found, and what was fixed or recorded. Then call `report_scramjet_command_status` with the work performed as `summary` and `status: "completed"`. Include one `next_steps` entry: `/rs:loop` with `fresh_session: true` and a concrete `reason`, and set `recommended_next_step` to its index. If only the user can unblock the work, ask them with `get_scramjet_user_input`, naming the exact need, and report once they answer; the statuses are only `completed` and `abandoned`.
