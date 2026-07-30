---
description: Independently reassess retained executable PR findings and route validated outcomes
argument-hint: "<pr-number> --review-comment <id> [context]"
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
  mode: closed
  candidates:
    - name: mach12:pr-review-fix
      hint: |
        Pick when independently validated executable findings require
        production fixes before merge.
    - name: mach12:pr-pre-merge
      hint: |
        Pick when no merge-blocking executable finding survives the
        independent assessment.
---

# Assess PR Validation

You are independently reassessing the executable findings retained by `/mach12:pr-validation`, removing rejected proofs, and publishing the final assessment before routing to fixes or pre-merge.

<user-context>
$ARGUMENTS
</user-context>

## Step 1: Parse input

Extract:
- A **PR number** (required).
- A **`--review-comment <id>`** numeric comment ID for the exact validation artifact.
- Additional context (optional).

If the required PR number or exact review comment ID is missing or ambiguous, ask the user to clarify.

## Step 2: Reassess retained proofs

Reacquire authoritative PR, linked-issue, approved-plan, complete-diff, related-test, and prior-artifact context. Independently rerun and classify the exact retained proofs before changing their disposition.

Dispatch bundled `mach12:independent-assessor` and `mach12:code-architect` agents only with `agentScope: "user"` so project-local agents cannot shadow them. Remove second-pass rejected proofs through targeted edits, preserve surviving proof contracts, and post a verified `<!-- mach12-assessment -->` artifact with its numeric comment ID.

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`, then set `status: "completed"` and choose selector-visible `next_steps` from the declared candidates:

- When genuine findings survive, include `/mach12:pr-review-fix <pr-number> --review-comment <review-id> --assessment-comment <assessment-id> <finding-ids>` with `fresh_session: true` and a reason describing the required production fixes.
- When no merge-blocking finding survives, include `/mach12:pr-pre-merge <pr-number>` with `fresh_session: true` and a reason stating that executable assessment is complete.

Set `recommended_next_step` to the zero-based index of the recommended entry. Never include rejected findings in fix arguments. If reassessment or publication could not finish, report the matching `status` (`blocked` / `incomplete`) instead of `completed`. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
