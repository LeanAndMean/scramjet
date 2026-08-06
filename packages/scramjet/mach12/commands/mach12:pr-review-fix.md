---
description: Fix specific issues identified in a PR review
argument-hint: "<pr-number> [--review-comment <id>] [--assessment-comment <id>] [findings] [context]"
allowed-tools:
  - bash
  - read
  - grep
  - find
  - edit
  - write
  - subagent
  - delegate
  - get_scramjet_user_input
  - read_pr
  - add_pr_comment
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

Use `read_pr` and continue every returned range with the unchanged snapshot until the complete PR document is visible.

**If `--review-comment` was provided:** Require the explicit comment ID to match exactly one recognized review in that verified target-PR comment stream and use that comment's body before selecting findings or publishing the ID as provenance; otherwise stop. Do not fetch a second raw copy of content that could drift from the aggregate read. Validation-origin artifacts later apply the stronger authentication contract below.

**If `--review-comment` was NOT provided (fallback):** Scan the chronological comments for `<!-- mach12-review -->` and use the last matching comment and its opaque ID. If no comment contains the marker, fall back to the last comment with the structured review format (Critical/Important/Suggestions sections and model attribution).

### Locate the assessment comment (optional)

**If `--assessment-comment` was provided:** Require it to match exactly one comment in the same complete `read_pr` stream and use that body.

**If not provided:** This is optional context. Do not attempt to locate the assessment heuristically—proceed without it.

Save the review comment content for use in Step 4. Also retain the complete verified chronological top-level PR comment stream returned by `read_pr` for the final retrospective.

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
6. **Quality review** -- run a single, lightweight sanity pass over the selected findings' changes. This is a fast confidence check that the fix diff is sound; it is explicitly **not** a substitute for the thorough, full-branch PR review that runs later (`mach12:pr-review` re-covers correctness, tests, error handling, type design, and simplification across the whole branch). Keep it proportional to the fix:
   - **Cap: at most 3 `mach12:code-reviewer` subagents per stage, total** -- dispatched in a single parallel batch, each given a focused brief for the selected findings' changes (e.g., bugs/correctness, project conventions and abstraction fit, and -- only when the fix warrants it -- a higher-risk angle such as error/fallback paths, type/interface design, or test coverage). Three is a ceiling for unusually risky fixes, not a quota: most fixes need one or two, and a trivial or low-risk fix (mechanical rename, comment/text edit, config tweak with no logic change) may skip review entirely when you can state why the change is below threshold.
   - **Single pass.** Run the brief(s) once, consolidate the findings, and act on them directly. Do not loop: re-review is warranted only when a fix you made was non-trivial and substantively reworked code a reviewer flagged -- and then only the one brief covering that area, dispatched once more and **counted against the three-subagent per-stage cap** (keep the initial batch small when a re-review is plausible so you reserve headroom). A reviewer returning findings is not itself a reason to re-review.
   - **Never re-dispatch to restate.** Act on the findings you already hold. Do not spawn a subagent to re-report, restate, or re-confirm a finding you already received -- you carry the finding; a fresh subagent does not.
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
