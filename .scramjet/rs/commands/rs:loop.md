---
description: Build out scramjet-rs one verified increment at a time, then continue
argument-hint: "[focus]"
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
      hint: Continue with the next most useful increment.
    - name: rs:review
      hint: This increment changed process isolation, credential handling or destructive file operations and should be independently checked before building on it.
    - name: rs:steer
      hint: Recorded open questions now block further useful work and need the user's direction.
---

# Build scramjet-rs

## Goals

- Deliver one increment that makes scramjet-rs more useful to the user, verified for real and committed.
- Leave `docs/TASKS.md` holding only the work that remains, so a fresh session can continue; what was done and how it was verified belong in the commit.
- Stop only for decisions, permissions or information that belong to the user.

## Focus

<user-context>
$ARGUMENTS
</user-context>

## Build one increment

Follow `CLAUDE.md`. Preserve unrelated uncommitted work; if its ownership is unclear, ask before touching it.

A focus given above replaces any current focus recorded in `docs/TASKS.md`; record it there until it is finished, so later fresh sessions keep it. Work toward the recorded focus if there is one, otherwise on the most useful item in `docs/TASKS.md` in light of what you find in the code. Add problems discovered along the way to the list. When the list runs low or a milestone completes, refill it from `docs/DESIGN.md` and `docs/ROADMAP.md`.

The increment is done when it works when actually used, through the company's proxy when the provider is involved, and formatting, Clippy and the relevant tests pass. Commit it, with what changed and how it was verified in the message, and remove what it finished from `docs/TASKS.md`. If the item is too large for one session, commit working partial progress and record the continuation in `docs/TASKS.md`.

When an item stops making progress, change the approach or switch to another item within this run, and record the obstacle in one line in `docs/TASKS.md`. Do not repeat an approach that an earlier recorded obstacle shows failed. A run that only documents a blocker is not progress.

Stop for the user only as described in `CLAUDE.md`, using `get_scramjet_user_input` with the context the user needs to decide. When such a decision does not block the current item, do not stop: record it under `## Open questions` in `docs/TASKS.md` and continue with work that does not depend on it. Record smaller product choices you made that the user may want to revisit as `Choice:` trailers in the commit that makes them (`CLAUDE.md`). The user reviews both with `/rs:steer`. Stop only when the answer is needed now.

## Report

Tell the user briefly what changed, how it was verified, the commit, any open questions or choices recorded, and what comes next. Then call `report_scramjet_command_status` with the work performed as `summary`:

- `status: "completed"` when this run committed real progress, including partial progress with its continuation recorded. Include one `next_steps` entry with `fresh_session: true` and a concrete `reason`: `/rs:review <commits>` when the increment changed process isolation, credential handling or destructive file operations; `/rs:steer` when the remaining useful work depends on recorded open questions; otherwise `/rs:loop`. Set `recommended_next_step` to that entry's index.

The statuses are only `completed` and `abandoned`. When only the user can unblock the work, ask them with `get_scramjet_user_input`, naming the exact need; the command waits for their answer. When nothing usable could be saved, say what happened and ask whether to try another way or stop; report `status: "abandoned"`, without `next_steps`, only when they want to stop.
