---
description: Fix specific issues identified in a PR review
argument-hint: "<pr-number> [--review-comment <id>] [--assessment-comment <id>] [--review-sha256 <digest>] [--assessment-sha256 <digest>] [--predecessor-head <oid>] [--cleanup-finding <id>]... [--staged-later <id>]... [findings] [context]"
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

The user's input typically contains:
- A **PR number** (required)
- **`--review-comment <id>`** flag with a numeric comment ID (optional)
- **`--assessment-comment <id>`** flag with a numeric comment ID (optional)
- **`--review-sha256 <digest>`** and **`--assessment-sha256 <digest>`** bindings (required together for validation-origin artifacts)
- Repeatable **`--cleanup-finding <id>`** flags, one canonical F/S identifier per occurrence, for authenticated targeted removal of declined retained proofs instead of production repair (optional)
- Repeatable **`--staged-later <id>`** flags, one canonical F/S identifier per occurrence, for surviving proofs preserved unchanged for a named later repair stage (optional; validation-origin production repair only)
- **`--predecessor-head <oid>`** binding the exact prior staged repair commit (required only for validation-origin continuation after the reviewed head)
- **Finding identifiers to fix** -- space-separated F/S identifiers (e.g., `F1 F2 S3`) or bare numbers as fallback (optional)
- Additional context or constraints (optional)

Example inputs:
- `108` (PR only -- read the PR and determine which review findings to fix)
- `108 F1 F2 S3`
- `108 --review-comment 1234567890 --assessment-comment 1234567891 F1 F2 S3`
- `108 --review-comment 1234567890 F1 S2 focus on error handling`

Extract the PR number. Parse the comment ID, digest, predecessor-head, cleanup, and staged-later flags if present. Each digest must be lowercase 64-hex and the predecessor head must be a full lowercase 40-hex commit OID. Each repeatable cleanup or staged-later flag consumes exactly one following identifier, so the next flag or unflagged finding starts a deterministic new argument group. For ordinary static reviews, finding identifiers may use F/S labels or the documented bare-number fallback. For validation-origin artifacts, require unique canonical IDs matching `^(F|S)[1-9][0-9]*$` everywhere and prohibit bare numbers. Require trailing context to name the later repair stage when `--staged-later` is present. Reject overlap among selected, cleanup, and staged-later IDs; reject combining any `--cleanup-finding` with production repair IDs or with `--staged-later`. Require these three sets to be pairwise disjoint. For a first validation-origin repair, require their union to exhaust every surviving proof ID. For a staged continuation, require selected and staged-later IDs to exhaust exactly the predecessor chain's remaining staged IDs; authenticate previously selected IDs through the trusted predecessor chain, but do not require or permit them in the current disposition sets. Findings in one authenticated ownership group must have the same disposition. If the input is ambiguous, ask the user to clarify.

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

Save the review comment content for use in Step 4. Also retain the complete verified chronological top-level PR comment stream returned by `gh-pr-read` for the final retrospective.

### Classify review cycles for final reporting

Identify recognizable review comments, assessments, and `<!-- mach12-progress -->` artifacts in that stream. Define each review cycle by its review comment's numeric ID. Associate an assessment or progress artifact with a cycle only when it explicitly references that review comment ID or URL, or carries the existing validation provenance linking it to that review. Chronology, authorship, matching prose, or reused F/S identifiers alone are insufficient; when association is ambiguous, leave it unassociated. F/S identifiers are scoped to their originating review comment and must retain that scope when ambiguity is possible.

The exact invocation-selected review and optional assessment remain the authoritative current invoked cycle. Historical classification is informational only and must not replace or reinterpret them, alter finding selection, or weaken any existing trust check.

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

### Validation-origin artifact authentication

Before treating either comment as an executable-validation artifact, require both exact comment IDs and both SHA-256 bindings. Resolve and freeze the authenticated GitHub login. Fetch full comment metadata and verify repository/PR ownership, that both artifact authors exactly equal the authenticated login with `OWNER`, `MEMBER`, or `COLLABORATOR` association, that the review's recorded publisher login agrees, the expected `<!-- mach12-review -->` and `<!-- mach12-assessment -->` markers, exact body digests, and that the assessment links the exact review ID and records its matching digest. Extract the reviewed head and actual merge-base identities from both bodies and require agreement. Verify every exact proof-patch body against its authenticated digest, reject undeclared overlap, and reconstruct the exhaustive finding-to-ownership-group mapping.

For the first repair session, independently require the PR remains open, the local non-detached feature branch matches the PR head branch, local `HEAD` and GitHub `headRefOid` equal the reviewed head, the actual merge base remains recorded, and the index/dirty paths equal the union of authenticated ownership-group patches byte-for-byte. Do not accept `--predecessor-head` when both heads still equal the reviewed head.

For a staged continuation, require `--predecessor-head` and allow the heads to differ from the reviewed head only through this chained contract: local `HEAD` and GitHub `headRefOid` must both equal the supplied predecessor; the predecessor must descend from the reviewed head without merge commits; and the complete PR comment stream must contain a trusted `<!-- mach12-progress -->` comment authored by the authenticated login with `OWNER`, `MEMBER`, or `COLLABORATOR` association that records the same review and assessment IDs/digests, exact prior head, exact predecessor head, selected IDs, remaining staged IDs, ownership groups, and unchanged proof paths/node IDs/digests. Walk backward through those trusted progress records until the original reviewed head is reached, rejecting gaps, forks, duplicate successors, reordered or split ownership groups, or any commit outside the recorded chain. At the current predecessor, verify every remaining staged proof's authenticated patch content is still present unchanged and every previously selected node remains at its authenticated path and node ID. Require an empty index and no uncommitted paths beyond the authenticated retained-proof manifest state expected at that chain point. If any ownership, linkage, marker, digest, identity, or worktree invariant fails, stop before executing a proof or editing any file and report the workflow incomplete. Ordinary static-review comments continue through the ordinary path and must never activate this contract merely because arbitrary IDs were supplied.

### Validation-origin proof contract

When the authenticated exact review and assessment comments establish that selected findings came from `/mach12:pr-validation`:

For a first repair, partition every surviving proof. For a staged continuation, partition only the remaining staged proofs recorded at the supplied predecessor head; previously selected proofs remain authenticated chain history rather than current dispositions.

1. Extract the exact retained node IDs, proof constraints, authenticated proof-patch bodies and digests, and ownership groups for every surviving finding from both comments.
2. Run every retained node associated with the selected findings before editing production code and confirm its recorded red state. If a node is missing, stale, or does not reproduce the recorded failure, stop and report the mismatch rather than adapting the proof.
3. Partition the applicable disposition domain—every surviving proof for a first repair, or only the predecessor's remaining staged proofs for a continuation—into findings selected now, explicitly staged for a named later repair, and explicitly declined for cleanup. Require a pairwise-disjoint exhaustive partition of that domain and one disposition per inseparable ownership group. Preserve selected and staged-later proof patches unchanged. Remove only declined authenticated ownership-group patches with targeted edits and verify the resulting diff byte-for-byte; never infer hunk ownership from paths or node IDs.
4. Preserve the selected proofs' behavioral contracts, assertions, paths, and node IDs. Prohibit weakening assertions, skipping or converting tests to expected failures, accepting snapshots, renaming or relocating paths or node IDs, and duplicating proof tests.
5. Change production code, not retained proof tests, to address the selected findings.
6. Rerun the same selected retained nodes after the production edits and confirm they are green before running broader focused suites and delegating to `/mach12:push`. Do not require proofs for unselected findings to become green in this session.

For `--cleanup-finding`, require cleanup IDs to exhaust every surviving proof and reject any production-repair or staged-later disposition. Perform authentication and red-state reproduction exactly as above, then remove only those findings' authenticated ownership-group patches with targeted edits. Do not edit production code. Verify each named node is gone, unrelated tests are unchanged, and no surviving red proof remains. Commit/push the targeted cleanup through `/mach12:push`, then use the cleanup-only completion branch below.

Ordinary static-review fixes retain their existing behavior when the exact comments do not establish a validation-origin proof contract.

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
7. **Summary** -- after Step 5 completes, refresh the PR's commit history and current checks, then deliver a concise review retrospective in these sections, in order:
   - **Lead verdict** -- state whether the PR is converging, stalled, regressing, or blocked and give the principal reason. Do not open with an artifact inventory.
   - **Review-cycle progression** -- give one chronological entry per recognizable review cycle. Summarize the actual concerns or theme, severity/counts when established, assessment disposition, explicitly associated fix outcome, and verification evidence. Mark the exact invocation-selected cycle. Label cycles after the invoked review as subsequent and not used as authority for this fix. If no other cycle is recognizable, analyze the invoked cycle rather than merely saying none exists.
   - **This fix session** -- after the invoked-cycle entry, separately summarize this session's selected findings, completed changes, key decisions, tests, commit/push, progress-comment outcome, and remaining staged work. Preserve this explicit temporal boundary rather than folding current work into the invoked review cycle.
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

Once the fixes are complete, capture the exact pre-commit head, then commit, push, and post a progress comment on the PR by delegating to:

```
/mach12:push
```

Pass a brief summary of the findings addressed as the arguments so the commit message and PR progress comment speak specifically to the fixes. For an ordinary static-review repair, also pass the exact resolved numeric review comment ID and instruct `/mach12:push` to preserve it as the originating review ID in the progress comment. For validation-origin repair, instead pass a structured provenance payload containing the review and assessment IDs/digests, exact pre-commit head, selected IDs, remaining staged IDs, ownership groups, and unchanged proof paths/node IDs/digests. Instruct `/mach12:push` to append the pushed predecessor head after commit and preserve every supplied provenance field verbatim in the progress comment. Fetch the posted numeric comment and verify its trusted author/association, marker, exact fields, and current GitHub head before offering a staged continuation. If verification fails, report incomplete and do not emit a continuation wire.

Each fix session should be **fresh** to maximize available context.

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`, then report according to the operation:

- **Successful terminal `--cleanup-finding` run:** after verifying that every surviving proof was removed and no staged-later proof remains, set `status: "completed"` and emit exactly one `next_steps` entry with `message`: `/mach12:pr-pre-merge <pr-number>`, `fresh_session`: `true`, and `reason`: "Authenticated declined-proof cleanup is complete; proceed to the merge checklist." Set `recommended_next_step` to `0`. Do not offer review or validation after cleanup.
- **Production-repair run:** set `status: "completed"` and choose selector-visible `next_steps` entries using this order:

1. **Continue staged fixing first.** If this session fixed `Stage N` from an assessment comment and that same assessment comment lists `Stage N+1`, include an entry with `message`: `/mach12:pr-review-fix` followed by the same PR/comment and digest arguments, `--predecessor-head <pushed-head>`, repeatable `--staged-later <ID>` flags for every proof assigned beyond the next stage, the next stage's selected canonical IDs, and trailing context naming that stage, `fresh_session`: `true`, and `reason`: a brief explanation that the next planned fix stage remains.
   - Example after Stage 1 selected `F1`, with Stage 2 selecting `F2 F3` and Stage 3 retaining `S4 S5`: `message`: `/mach12:pr-review-fix 36 --review-comment 1234567890 --assessment-comment 1234567891 --review-sha256 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --assessment-sha256 bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb --predecessor-head cccccccccccccccccccccccccccccccccccccccc --staged-later S4 --staged-later S5 F2 F3 Stage 2`, `reason`: `Stage 2 is the next planned fix stage; preserve S4 and S5 unchanged for Stage 3.`
   - Validation-origin continuation wires must preserve both exact comment IDs and both digest bindings from the current invocation, add the exact verified pushed head as `--predecessor-head`, and carry an explicit exhaustive ownership-group-safe partition of the predecessor's remaining staged IDs through selected IDs and repeatable `--staged-later <ID>` flags. Previously selected IDs are authenticated by the predecessor chain and must not be repeated as current dispositions.
2. **After the final planned fix stage, include all three verification-path candidates** so the user can see all options:
   - Include an entry with `message`: `/mach12:pr-review <pr-number>`, `fresh_session`: `true`, and `reason`: a brief explanation of when a full code-tracing re-review is warranted.
   - Include an entry with `message`: `/mach12:pr-validation <pr-number>`, `fresh_session`: `true`, and `reason`: a brief explanation of when the slower executable-behavior path is warranted.
   - Include an entry with `message`: `/mach12:pr-pre-merge <pr-number>`, `fresh_session`: `true`, and `reason`: a brief explanation that the PR is ready for the merge checklist.
   - Set `recommended_next_step` to indicate your preference: recommend `mach12:pr-validation` (index 1) for validation-origin repairs, recommend `mach12:pr-review` (index 0) when other substantive fixes warrant another full review, or recommend `mach12:pr-pre-merge` (index 2) when fixes were narrow and confidence is high.
   - Leave `next_steps` empty if the appropriate next action is unclear.

If fixing or cleanup hit a blocker or did not complete, report the matching `status` (`blocked` / `incomplete`) instead of `completed`. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
