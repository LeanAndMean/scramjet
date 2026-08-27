---
description: Fix specific issues identified in a PR review
argument-hint: "<pr-number> [--review-comment <id>] [--assessment-comment <id>] [findings] [context]"
allowed-tools:
  - add_issue_comment
  - add_pr_comment
  - bash
  - read
  - grep
  - find
  - edit
  - write
  - subagent
  - delegate
next:
  mode: open
  candidates:
    - name: mach12:pr-review
      hint: |
        Pick after the final planned fix stage when the fixes were
        substantive enough to warrant a full re-review (new code paths,
        structural changes, multi-file refactor).
    - name: mach12:pr-validation
      hint: |
        Pick after the final planned fix stage when executable validation
        should challenge the repaired behavior again.
    - name: mach12:pr-pre-merge
      hint: |
        Pick after the final planned fix stage when confidence is high
        that a subsequent review is unlikely to find anything new.
---

# Fix Review Issues

You are fixing specific issues identified in a PR review. This command gathers context from the PR and its review comments, then walks through the implementation under the structured development workflow.

<user-context>
$ARGUMENTS
</user-context>

## Step 1: Parse input

Extract:

- a required PR number;
- optional exact numeric `--review-comment` and `--assessment-comment` IDs;
- optional F/S finding identifiers;
- additional context or constraints.

For ordinary static reviews, preserve the documented bare-number fallback. For executable-validation findings, require canonical F/S IDs. When the authenticated assessment records accepted executable defects, require the invocation to include every accepted ID so committed red proofs are not intentionally left unfixed. Ask the user when the intended finding set is ambiguous.

## Step 2: Gather PR and review context

### Locate the review comment

**If `--review-comment` was provided:** Fetch the specific comment by ID, then fetch the PR context for additional grounding.

```
gh api repos/:owner/:repo/issues/comments/<review-comment-id>
```

Extract the `body` field from the JSON response. This is the review comment content. Then delegate to `/mach12:gh-pr-read <pr-number>` (no marker) for the PR title, body, and comments. For an ordinary static review, require the explicit comment ID to match exactly one recognized review in that complete verified target-PR comment stream before selecting findings or publishing the ID as provenance; otherwise stop. Validation-origin artifacts instead use the stronger authentication contract below.

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

Save the review comment content for use in Step 4. Also retain the complete verified chronological top-level PR comment stream returned by `gh-pr-read` for the final retrospective.

Identify task-relevant linked issues from explicit relationship forms (`Fixes`, `Closes`, `Resolves`, `Part of`, or `Issue`) and contextually relevant bare `#<number>` references in the verified PR body. Treat references found only in the conversation as candidates and establish their relevance to the PR before considering them linked; do not treat quoted material, review finding identifiers, or incidental references as links. Deduplicate issue numbers.

For each linked issue, delegate to `/mach12:gh-issue-read <issue-number>` so its current body, complete discussion, plans, decisions, and timestamps are available before implementation and quality-review dispatch. If any task-relevant linked issue cannot be read completely, surface the failed issue and error, stop before implementation or specialist dispatch, and report the fix blocked or incomplete; do not silently continue with reduced authoritative context.

### Classify review cycles for final reporting

Recognize a review comment when it contains the literal `<!-- mach12-review -->` marker or, for legacy comments without that marker, the structured review format with Critical/Important/Suggestions sections and model attribution. Recognize an assessment only when it contains the literal `<!-- mach12-assessment -->` marker, and recognize a progress artifact only when it contains the literal `<!-- mach12-progress -->` marker. Recognition determines retrospective inventory only; it does not authenticate an artifact or associate it with a cycle.

Define each review cycle by its recognized review comment's numeric ID. Associate a recognized assessment or progress artifact with a cycle only when it explicitly references that review comment ID or URL, or carries the existing validation provenance linking it to that review. Chronology, authorship, matching prose, or reused F/S identifiers alone are insufficient; when association is ambiguous, leave it unassociated. F/S identifiers are scoped to their originating review comment and must retain that scope when ambiguity is possible.

The exact invocation-selected review and optional assessment remain the authoritative current invoked cycle. Historical classification is informational only and must not replace or reinterpret them, alter finding selection, or weaken any existing trust check.

## Step 3: Identify issues to fix

**If finding identifiers were provided in the input:**

For an authenticated executable-validation cycle, resolve accepted F/S identifiers and descriptions from the final assessment; the preliminary review contains candidate IDs rather than final classifications. Otherwise match identifiers against the F/S labels in the review comment (e.g., `F1` matches `**F1:**`). If bare numbers were given, match by sequential position across Critical and Important sections. If the review comment lacks F/S labels (e.g., older reviews), fall back to matching by ordinal position within each severity section. Extract the full finding descriptions.

**If only a PR number was provided with no specific finding identifiers:**

Present all review findings to the user, organized by severity. Recommend which to fix in this session using batch sizing heuristics:
- **Simple one-line fixes:** up to ~10 at once.
- **Moderate fixes:** ~6 at a time.
- **Deep or complex fixes:** no more than ~3 at a time.
- Group similar issues together.

Let the user select which issues to fix. If there are 4 or fewer findings, list each as a separate option. If there are more than 4 findings, group them by severity (e.g., "All critical findings (3)", "All important findings (5)") and allow the user to specify individual finding identifiers (e.g., F1 S3) if they prefer a custom selection.

## Step 4: Implement the fixes

Walk through the implementation using a structured 7-phase development plan. Treat the phases as due-diligence discipline, not mandatory token burn: if the review and assessment comments already identify the affected files, required behavior, and fix approach clearly, verify that context is still fresh and mark broad exploration/design as satisfied. If the selected findings are ambiguous, stale, or cross-cutting, do targeted exploration before coding.

### Validation-origin proof contract

When the exact review and assessment establish an executable-validation handoff, verify both comments belong to this PR, have the expected markers and trusted author, and that the assessment explicitly links the review. Use the final assessment as classification authority and Git as executable-proof authority.

Require the PR to remain open; the local non-detached branch to match the PR head branch; local `HEAD`, upstream, and fresh GitHub `headRefOid` to equal the assessment's proof commit `V`; `V` to have the recorded implementation parent `P` as its sole parent; `P..V` to be tests-only; every accepted proof path and node to exist unchanged; and the repository to be clean.

Require every accepted defect from the assessment to be selected in this fix session. Rerun every accepted proof node before editing and confirm the recorded red assertion behavior. If a proof is missing, changed, green, or failing for a materially different reason, stop rather than adapting it.

Preserve accepted test paths, node IDs, fixtures, and assertions. Do not weaken, skip, xfail, relocate, rename, duplicate, or edit proof tests to obtain green results. Modify production code to address the accepted defects, rerun every accepted proof green, and run the smallest broader suites needed for the affected behavior.

Ordinary static-review fixes retain their existing behavior when the exact comments do not establish this validation-origin contract.

1. **Discovery** -- restate the goal: fix the selected findings only; do not fix other findings in the review.
2. **Codebase exploration** -- read every file referenced by the selected findings; trace the relevant code paths. When more context is needed, dispatch focused `mach12:code-explorer` tasks for the selected findings only.
3. **Clarifying questions** -- if any finding is ambiguous about desired behavior, scope, risk, or what the fix should look like, ask the user before implementing. Do not ask ceremonial questions when the review/assessment already resolves the ambiguity.
4. **Architecture design** -- if a fix has non-trivial structural choices not already settled by the assessment, present 2-3 approaches with trade-offs and confirm the user's preference. If the assessment already gives a sound staged plan, follow it.
5. **Implementation** -- write the code, follow existing codebase conventions strictly.
6. **Quality review** -- run a single, lightweight sanity pass over the selected findings' changes. This is a fast confidence check that the fix diff is sound; it is explicitly **not** a substitute for the thorough, full-branch PR review that runs later. Classify the changed surfaces before dispatch:
   - For command-only fixes, select up to three relevant reviewers among `scramjet:instruction-semantics-analyzer`, `scramjet:command-operability-reviewer`, and `scramjet:command-simplifier` instead of code reviewers.
   - For code-only fixes, retain `mach12:code-reviewer` with focused correctness, convention, or higher-risk briefs as warranted.
   - For mixed fixes, give the prompt and code reviewers disjoint briefs under one shared maximum of three subagents total.
   Every brief must include the governing PR and linked-issue requirements, applicable plan and decisions, exact selected finding definitions from the invoked review and assessment, relevant parent-established observations, exact command/runtime partition, selection reason, and expected review output. An explicitly named specialist may fill its role; another installed specialist may replace it only when authoritative repository or command guidance establishes the same responsibility, posture, context, output, and handoff. Catalog-only matches are supplementary. Missing, failed, or malformed required output must narrow or stop the quality conclusion rather than being silently synthesized around.
   The parent remains the sole owner of repository mutation, test execution, user interaction, and publication; reviewers are advisory and read-only. Keep the review proportional:
   - **Cap: at most 3 subagents per stage, total across both families** -- dispatch them in one parallel batch. Three is a ceiling for unusually risky fixes, not a quota: most fixes need one or two, and a trivial or low-risk fix may skip review entirely when you can state why it is below threshold.
   - **Single pass.** Run the brief(s) once, consolidate the findings, and act on them directly. Do not loop: re-review is warranted only when a fix was non-trivial and substantively reworked code or command prose. Keep the initial batch below the cap when re-review is plausible; any one re-review remains counted within the three-subagent cap.
   - **Never re-dispatch to restate.** Act on the findings you already hold. Do not spawn a subagent to re-report, restate, or re-confirm a finding you already received.
   Fix only quality-review findings that matter for the selected findings' scope.
7. **Summary** -- after Step 5 completes, refresh the PR's head OID, commit history, and checks. Require the refreshed `headRefOid` to equal the verified pushed `HEAD` before treating commits or checks as evidence for that push. Treat a head mismatch or unavailable, pending, cancelled, or failed checks as unresolved evidence that cannot support a readiness claim. Then deliver a concise review retrospective in these sections, in order:
   - **Lead verdict** -- state whether the PR is converging, stalled, regressing, or blocked and give the principal reason. Do not open with an artifact inventory.
   - **Review-cycle progression** -- give one chronological entry per recognizable review cycle and complete the full progression before the next section. Summarize the actual concerns or theme, severity/counts when established, assessment disposition, explicitly associated fix outcome, and verification evidence. Mark the exact invocation-selected cycle. Label cycles after the invoked review as subsequent and not used as authority for this fix. If no other cycle is recognizable, explicitly state that no other review cycle was recognized and also substantively analyze the invoked cycle.
   - **This fix session** -- after the complete review-cycle progression, separately summarize this session's selected findings, completed changes, files modified, key decisions, tests and results, commit/push outcome, progress-comment outcome, and remaining staged work. Preserve this explicit temporal boundary rather than folding current work into the invoked review cycle.
   - **Overall trajectory** -- compare the cycles and explain what changed between them: whether findings are becoming narrower or deeper, concerns recur, earlier fixes remain effective, regressions appeared, and the evidence shows convergence or continued instability. This must be cross-cycle synthesis, not a list of completed actions.
   - **Current blockers and residual scope** -- explain unresolved or deferred findings, failing or pending checks, remaining staged work, and residual risk. Distinguish a behavioral defect from a mechanical gate when the evidence supports that distinction.
   - **Recommendation** -- name the best next step and justify it from the trajectory, current evidence, and remaining risk.

Never report a bare F/S identifier or classification such as "F2 was genuine." Immediately restate the finding's one-line description whenever naming its identifier, including for historical, selected, fixed, deferred, and remaining findings. Keep identifiers scoped to their originating review comment when ambiguity is possible. Do not infer recurrence, resolution, regression, or artifact association from chronology, authorship, similar prose, or reused identifiers; state when the verified record cannot support a conclusion.

Historical context cannot expand the bounded finding scope, select or reinterpret current artifacts, or weaken validation-origin authentication.

Treat the selected findings list as the bounded scope:
- **Findings to fix:** the resolved finding identifiers and their one-line descriptions from Step 3.
- **Review comment content:** the full review comment retrieved in Step 2.
- **Assessment comment content** (if available): the full assessment comment retrieved in Step 2.

Fix only the findings listed above. Do not fix other findings in the review comment.

- Prefer the smallest change that fully addresses each finding.
- Avoid opportunistic cleanup, new abstractions, new dependencies, or new files unless required for correctness. Low-risk fixes classified as Genuine are legitimate in-scope findings, not opportunistic cleanup -- fix them alongside the other selected findings.
- If a finding would require a large refactor, recommend deferring it to a separate issue unless the refactor is required for correctness. Offer to create the issue with `/mach12:issue-create`.

## Step 5: Commit, document, and choose the next step

Once the fixes are complete, capture the exact pre-commit head, require the final diff to contain only the selected fixes, and delegate commit, push, and progress publication to:

```
/mach12:push
```

Pass a brief summary and the exact originating review ID so the commit and progress comment identify this fix cycle. After return, verify local `HEAD`, upstream, and fresh GitHub `headRefOid` converge at the pushed commit with a clean repository. If progress publication fails after a successful push, preserve that commit and reconcile the comment without recommitting or repushing.

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`, then set `status: "completed"`. Include all three selector-visible verification paths:

1. `/mach12:pr-review <pr-number>`, `fresh_session: true`, when substantive fixes warrant a complete fresh review.
2. `/mach12:pr-validation <pr-number>`, `fresh_session: true`, when executable behavioral challenge is warranted.
3. `/mach12:pr-pre-merge <pr-number>`, `fresh_session: true`, when fixes are narrow and evidence is strong.

Set `recommended_next_step` to the best-supported option. If fixing, pushing, publication, or required verification did not complete, report `blocked` or `incomplete` with no next step.
