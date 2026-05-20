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

Walk through the implementation using a structured 7-phase development plan:

1. **Discovery** -- restate the goal: fix the selected findings only; do not fix other findings in the review.
2. **Codebase exploration** -- read every file referenced by the selected findings; trace the relevant code paths.
3. **Clarifying questions** -- if any finding is ambiguous about what the fix should look like, ask the user before implementing.
4. **Architecture design** -- if a fix has non-trivial structural choices, present 2-3 approaches with trade-offs and confirm the user's preference.
5. **Implementation** -- write the code, follow existing codebase conventions strictly.
6. **Quality review** -- dispatch parallel reviewer tasks and address consolidated findings before declaring the fixes complete.
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

When calling `task_complete`, choose the next step using this order:

1. **Continue staged fixing first.** If this session fixed `Stage N` from an assessment comment and that same assessment comment lists `Stage N+1`, set `next_step.name` to `mach12:pr-review-fix` and set `next_step.args` to the same PR/comment arguments plus the next stage label, and set `next_step.fresh_session` to `true`.
   - Example: `mach12:pr-review-fix 36 --review-comment 1234567890 --assessment-comment 1234567891 Stage 2`
2. **After the final planned fix stage, choose the verification path.** Pick `mach12:pr-review` if the fixes were substantive enough that another full review may find issues. Pick `mach12:pr-pre-merge` if the fixes were narrow and confidence is high.
3. **If the next stage is unclear, stop.** Omit `next_step` rather than guessing.
