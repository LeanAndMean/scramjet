---
description: Challenge a PR through independently validated executable tests
argument-hint: "<pr-number> [context]"
allowed-tools:
  - bash
  - read
  - grep
  - glob
  - edit
  - write
  - subagent
  - delegate
next:
  mode: forced
  target: mach12:pr-validation-assessment
---

# Validate PR Behavior

You are challenging a pull request through executable tests and publishing independently admitted failing proofs for a fresh assessment session. This command owns repository mutation; subagents provide read-only analysis or independently rerun evidence.

<user-context>
$ARGUMENTS
</user-context>

## Step 1: Parse input

Extract:
- A **PR number** (required).
- Additional context, focus areas, or constraints (optional).

If the PR number is missing or ambiguous, ask the user to clarify.

## Step 2: Validate executable behavior

Gather the authoritative PR, linked-issue, approved-plan, complete-diff, related-test, and prior-review context before generating hypotheses. Dispatch bundled `mach12:test-designer`, `mach12:independent-assessor`, and `mach12:code-architect` agents only with `agentScope: "user"` so project-local agents cannot shadow them.

Designers return hypotheses and fixture guidance only. The main agent alone creates, edits, moves, removes, and executes tests sequentially. Keep candidate-specific limits and mutation rules in this command rather than adding them to the global designer contract.

Independently assess executable evidence before retaining it. Leave only validated failing proofs in permanent behavioral test suites, post a verified `<!-- mach12-review -->` artifact, and record its numeric comment ID for the forced handoff.

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`, then set `status: "completed"` only after the review artifact is posted and verified. This command declares a forced next step; include one selector-visible context entry:

- `message`: `/mach12:pr-validation-assessment <pr-number> --review-comment <comment-id>`
- `fresh_session`: `true`
- `reason`: "Independently reassess the retained executable proofs in a fresh session."

Set `recommended_next_step` to `0`. If validation or publication could not finish, report the matching `status` (`blocked` / `incomplete`) instead of `completed`; the forced target will not run. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
