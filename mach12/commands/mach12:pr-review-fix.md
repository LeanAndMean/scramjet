---
description: Fix specific issues identified in a PR review
argument-hint: "<pr-number> [--review-comment <id>] [--assessment-comment <id>] [findings] [context]"
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
  mode: open
  candidates:
    - name: mach12:pr-review-fix
      hint: |
        Pick when this session fixed Stage N from a staged assessment
        plan and Stage N+1 remains. Re-run this command in a fresh
        session with the same PR/comment arguments and the next stage
        label.
    - name: mach12:pr-review
      hint: |
        Pick after the final planned fix stage when the fixes were
        substantive enough to warrant a full re-review (new code paths,
        structural changes, multi-file refactor).
    - name: mach12:pr-pre-merge
      hint: |
        Pick after the final planned fix stage when confidence is high
        that a subsequent review is unlikely to find anything new.
---

# Fix Review Issues

You are fixing specific issues identified in a PR review. This command gathers context from the PR and its review comments, then walks through the implementation under the structured development workflow.

**User input:** $ARGUMENTS

## Step 1: Parse input

The user's input typically contains:
- A **PR number** (required)
- **`--review-comment <id>`** flag with a numeric comment ID (optional)
- **`--assessment-comment <id>`** flag with a numeric comment ID (optional)
- **Finding identifiers to fix** -- space-separated F/S identifiers (e.g., `F1 F2 S3`) or bare numbers as fallback (optional)
- Additional context or constraints (optional)

Example inputs:
- `108` (PR only -- read the PR and determine which review findings to fix)
- `108 F1 F2 S3`
- `108 --review-comment 1234567890 --assessment-comment 1234567891 F1 F2 S3`
- `108 --review-comment 1234567890 F1 S2 focus on error handling`

Extract the PR number. Parse `--review-comment` and `--assessment-comment` flags if present (each followed by a numeric ID). If finding identifiers are provided (F/S prefixed or bare numbers), note them. If the input is ambiguous, ask the user to clarify.

## Step 2: Gather PR and review context

### Locate the review comment

**If `--review-comment` was provided:** Fetch the specific comment by ID, then fetch the PR context for additional grounding.

```
gh api repos/:owner/:repo/issues/comments/<review-comment-id>
```

Extract the `body` field from the JSON response. This is the review comment content. Then delegate to `/mach12:gh-pr-read <pr-number>` (no marker) for the PR title, body, and comments.

**If `--review-comment` was NOT provided (fallback):** Delegate to:

```
/mach12:gh-pr-read <pr-number> --marker mach12-review
```

The subroutine returns the PR title, body, comments array, and the matched review comment body and numeric ID (most recent marker match). If no comment contains the marker, the subroutine reports that and the caller falls back to the last comment with the structured review format (Critical/Important/Suggestions sections and model attribution).

### Locate the assessment comment (optional)

**If `--assessment-comment` was provided:** Fetch it by ID:

```
gh api repos/:owner/:repo/issues/comments/<assessment-comment-id>
```

**If not provided:** This is optional context. Do not attempt to locate the assessment heuristically -- proceed without it.

Save the review comment content for use in Step 4.

## Step 3: Identify issues to fix

**If finding identifiers were provided in the input:**

Match identifiers against the F/S labels in the review comment (e.g., `F1` matches `**F1:**`). If bare numbers were given, match by sequential position across Critical and Important sections. If the review comment lacks F/S labels (e.g., older reviews), fall back to matching by ordinal position within each severity section. Extract the full finding descriptions.

**If only a PR number was provided with no specific finding identifiers:**

Present all review findings to the user, organized by severity. Recommend which to fix in this session using batch sizing heuristics:
- **Simple one-line fixes:** up to ~10 at once.
- **Moderate fixes:** ~6 at a time.
- **Deep or complex fixes:** no more than ~3 at a time.
- Group similar issues together.

Let the user select which issues to fix. If there are 4 or fewer findings, list each as a separate option. If there are more than 4 findings, group them by severity (e.g., "All critical findings (3)", "All important findings (5)") and allow the user to specify individual finding identifiers (e.g., F1 S3) if they prefer a custom selection.

## Step 4: Implement the fixes

Walk through the implementation using a structured 7-phase development plan. Treat the phases as due-diligence discipline, not mandatory token burn: if the review and assessment comments already identify the affected files, required behavior, and fix approach clearly, verify that context is still fresh and mark broad exploration/design as satisfied. If the selected findings are ambiguous, stale, or cross-cutting, do targeted exploration before coding.

1. **Discovery** -- restate the goal: fix the selected findings only; do not fix other findings in the review.
2. **Codebase exploration** -- read every file referenced by the selected findings; trace the relevant code paths. When more context is needed, dispatch focused `mach12:code-explorer` tasks for the selected findings only.
3. **Clarifying questions** -- if any finding is ambiguous about desired behavior, scope, risk, or what the fix should look like, ask the user before implementing. Do not ask ceremonial questions when the review/assessment already resolves the ambiguity.
4. **Architecture design** -- if a fix has non-trivial structural choices not already settled by the assessment, present 2-3 approaches with trade-offs and confirm the user's preference. If the assessment already gives a sound staged plan, follow it.
5. **Implementation** -- write the code, follow existing codebase conventions strictly.
6. **Quality review** -- run a single, lightweight sanity pass over the selected findings' changes. This is a fast confidence check that the fix diff is sound; it is explicitly **not** a substitute for the thorough, full-branch PR review that runs later (`mach12:pr-review` re-covers correctness, tests, error handling, type design, and simplification across the whole branch). Keep it proportional to the fix:
   - **Cap: at most 3 `mach12:code-reviewer` subagents per stage, total** -- dispatched in a single parallel batch, each given a focused brief for the selected findings' changes (e.g., bugs/correctness, project conventions and abstraction fit, and -- only when the fix warrants it -- a higher-risk angle such as error/fallback paths, type/interface design, or test coverage). Three is a ceiling for unusually risky fixes, not a quota: most fixes need one or two, and a trivial or low-risk fix (mechanical rename, comment/text edit, config tweak with no logic change) may skip review entirely when you can state why the change is below threshold.
   - **Single pass.** Run the brief(s) once, consolidate the findings, and act on them directly. Do not loop: re-review is warranted only when a fix you made was non-trivial and substantively reworked code a reviewer flagged -- and then only the one brief covering that area, dispatched once more and **counted against the three-subagent per-stage cap** (keep the initial batch small when a re-review is plausible so you reserve headroom). A reviewer returning findings is not itself a reason to re-review.
   - **Never re-dispatch to restate.** Act on the findings you already hold. Do not spawn a subagent to re-report, restate, or re-confirm a finding you already received -- you carry the finding; a fresh subagent does not.
   Fix only quality-review findings that matter for the selected findings' scope.
7. **Summary** -- list what was fixed, key decisions, files modified.

Treat the selected findings list as the bounded scope:
- **Findings to fix:** the resolved finding identifiers and their one-line descriptions from Step 3.
- **Review comment content:** the full review comment retrieved in Step 2.
- **Assessment comment content** (if available): the full assessment comment retrieved in Step 2.

Fix only the findings listed above. Do not fix other findings in the review comment.

If a fix is out of scope or would require significant refactoring, recommend deferring it to a new GitHub issue rather than fixing it inline. Offer to create the issue with `/mach12:issue-create`.

## Step 5: Commit, document, and choose the next step

Once the fixes are complete, commit, push, and post a progress comment on the PR by delegating to:

```
/mach12:push
```

Pass a brief summary of the findings addressed as `$ARGUMENTS` so the commit message and PR progress comment speak specifically to the fixes.

Each fix session should be **fresh** to maximize available context.

When Scramjet asks you to report command status, call `scramjet_command_status` with `status: "completed"` and choose selector-visible `next_steps` entries using this order:

1. **Continue staged fixing first.** If this session fixed `Stage N` from an assessment comment and that same assessment comment lists `Stage N+1`, include an entry with `name`: `mach12:pr-review-fix`, `args`: the same PR/comment arguments plus the next stage label, `fresh_session`: `true`, and `reason`: a brief explanation that the next planned fix stage remains.
   - Example: `name: mach12:pr-review-fix`, `args`: `36 --review-comment 1234567890 --assessment-comment 1234567891 Stage 2`, `reason`: `Stage 2 is the next planned fix stage.`
2. **After the final planned fix stage, include both verification-path candidates** so the user can see all options. Always include an entry with `name`: `mach12:pr-review`, `args`: `<pr-number>`, `fresh_session`: `true`, and `reason`: a brief explanation of when a full re-review is warranted. Always include an entry with `name`: `mach12:pr-pre-merge`, `args`: `<pr-number>`, `fresh_session`: `true`, and `reason`: a brief explanation that the PR is ready for the merge checklist. Set `recommended_next_step` to the index of `mach12:pr-review` when the fixes were substantive enough that another full review may find issues, or to the index of `mach12:pr-pre-merge` when the fixes were narrow and confidence is high.
3. **If the next stage is unclear, stop.** Leave `next_steps` empty rather than guessing. When you include any `next_steps`, set `recommended_next_step` to the zero-based index of the entry you recommend Scramjet route to automatically. If fixing hit a blocker or needs user input, report the matching `status` (`blocked` / `waiting_for_user` / `incomplete`) instead of `completed`.
