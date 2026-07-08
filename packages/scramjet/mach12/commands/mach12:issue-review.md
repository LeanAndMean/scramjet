---
description: Read a GitHub issue and all comments, review the implementation plan, and present findings
argument-hint: "<issue-number> [context]"
allowed-tools:
  - bash
  - read
  - grep
  - glob
  - subagent
  - delegate
next:
  mode: open
  candidates:
    - name: mach12:issue-review
      hint: |
        Use when Critical or Important findings remain after revision
        and another review pass is likely to surface genuine problems.
    - name: mach12:issue-implement
      hint: |
        Use when the plan is approved and ready for implementation.
---

# Issue Plan Review

You are reviewing the implementation plan for a GitHub issue. Your goal is to read the issue and all comments, independently assess the plan, and present your findings.

<user-context>
$ARGUMENTS
</user-context>

## Step 1: Parse input

The user's input contains:
- An **issue number** (required)
- Additional **context** or constraints (optional)

Extract the issue number from the input. If the input is ambiguous, ask the user to clarify. If context was provided, note it for use during exploration and review.

## Step 2: Read the issue and locate the plan

Delegate to:

```
/mach12:gh-issue-read <issue-number> --marker mach12-plan
```

The subroutine returns the issue title, body, full comments stream, and the body of the comment tagged with `<!-- mach12-plan -->` (using the last match if multiple exist).

Understand:
- The original problem statement and requirements.
- The implementation plan (typically posted as a comment).
- Any discussion, decisions, or clarifications in the comment thread.

If the marker comment was not found, fall back to identifying the most recent substantive comment that contains a staged implementation plan in the returned comments stream. If no plan exists at all, inform the user and suggest running `/mach12:issue-plan <issue-number>` first, then stop.

## Step 3: Read contribution guidelines

Delegate to:

```
/mach12:find-contribution-guidelines
```

The subroutine returns any planning-relevant guidance found in `CONTRIBUTING.md`, `DEVELOPMENT.md`, or `.github/CONTRIBUTING.md`: expected project layers, testing expectations, and any other requirements that a complete plan should satisfy.

Record these as **project review criteria** -- they serve as benchmarks when assessing the plan in Step 5.

## Step 4: Explore the codebase

Dispatch parallel exploration tasks to specialized subagents (one per lens). All lenses are required -- Step 5 always evaluates risks, testing, and alternatives, so their corresponding evidence-gathering lenses must always run:

- **Files referenced in the plan**: Trace through each file referenced in the plan, confirming they exist, checking their current state, and verifying the plan's characterization of their structure, responsibilities, and integration points is accurate.
- **Architecture and patterns**: Trace through the relevant architecture comprehensively, validating that the plan aligns with existing codebase conventions, abstractions, data flow, and design decisions.
- **Gaps**: Trace through code adjacent to the plan's scope, looking for constraints, dependencies, cross-cutting concerns, or affected areas the plan may have missed.
- **Risks and pitfalls**: Investigate what could go wrong with the plan's proposed approach. Look for failure modes, boundary conditions, implicit assumptions, or architectural concerns in the areas the plan modifies.
- **Alternative approaches**: Look for codebase evidence that a different approach could achieve the same goals. Identify existing patterns, abstractions, or design decisions that suggest a simpler, more idiomatic, or more robust solution than what the plan proposes.
- **Test infrastructure**: Examine the project's test suite -- frameworks, patterns, test organization, coverage approach, and any test utilities or fixtures relevant to the areas the plan modifies.

For parallel execution, dispatch all exploration tasks in a single batch rather than sequentially.

If the user provided context, include it in each exploration brief to focus review on the user's areas of concern. If project review criteria were recorded in Step 3, include them so exploration can verify whether the plan covers the relevant project layers and testing infrastructure.

Each exploration should return a list of key files and observations. After exploration completes, read all identified files.

## Step 5: Review the plan

If the user provided context in Step 1, weight the review toward the areas they emphasized -- surface findings on those areas even at Suggestions-level severity, and note in Step 7 how the user's focus shaped the findings (e.g., "User emphasized testing strategy; this raised three suggestions in that area that would otherwise be borderline."). Apply this weighting across all axes below; do not let it crowd out coverage of the other axes.

For each stage in the plan, assess:

1. **Correctness**: Does the stage accurately describe what needs to happen? Are the files and changes correct?
2. **Completeness**: Are there missing steps, files, or edge cases?
3. **Scope**: Is the stage appropriately sized for a single session, or should it be split/merged?
4. **Dependencies**: Are inter-stage dependencies correctly identified? Is the ordering logical?
5. **Testing**: Is the testing approach adequate for each stage?
6. **Risks**: Are there architectural risks, performance concerns, or subtle pitfalls the plan overlooks?
7. **Minimality**: Does the plan skip a lower rung of the minimum-sufficient solution ladder? Flag:
   - Stages that can be deleted or merged.
   - New files where edits to existing files would suffice.
   - New dependencies where platform/stdlib/existing project utilities suffice.
   - New abstractions, configuration, or extension points without evidence from the issue, codebase, or contribution guidance.
   - Testing plans broader than the risk requires.
   Default severity: Suggestions, unless overbuilding creates significant implementation risk or maintenance burden.
8. **Release-preparation exclusion**: Does the plan include version bumps, changelog entries, or release-preparation as implementation stages? Flag as a defect (severity: Important). Implementation-necessary version changes (e.g., updating a dependency version the code requires) are not excluded.

Also assess the plan holistically:
- Does it address all requirements and acceptance criteria from the issue?
- Does it follow existing codebase patterns and conventions?
- Are there alternative approaches worth considering?
- **Project-layer coverage**: Does the plan address all project layers discovered during codebase exploration or specified in the project review criteria recorded in Step 3? Flag any affected layer that no stage covers.
- **Test coverage planning**: If the project has an existing test suite or the project review criteria specify testing expectations, does each stage that introduces or modifies behavior include adequate test planning (what to test, test types, behaviors to cover)? If the project has no testable runtime code, verify the plan notes this rather than omitting test planning silently.

Create an initial findings list with stable identifiers:

- Label each Critical and Important finding with a sequential F-prefixed identifier (`F1`, `F2`, `F3`, ...) numbered continuously across both sections.
- Label each Suggestion with a sequential S-prefixed identifier (`S1`, `S2`, `S3`, ...).
- Use bold prefixes, e.g. `**F1:** Missing migration stage`, `**S1:** Clarify test fixture naming`.
- Keep each finding specific enough that a plan author can revise the plan without re-running the whole review.

## Step 6: Independently assess the findings

Before presenting findings to the user, run an independent assessment pass. Use a general-purpose subagent or a clearly separate self-review pass that has not seen only the initial conclusion. Provide it with:

- The issue title/body and full comment stream.
- The current implementation plan.
- The project review criteria from Step 3.
- The key codebase evidence from Step 4.
- The initial F/S findings from Step 5.

For each F/S item, verify against the issue, plan, comments, and relevant code. Classify it as one of:

- **Genuine blocker** -- the plan is likely to fail or produce incorrect results unless this is fixed.
- **Genuine issue** -- the plan has a significant gap or risk that should be addressed before implementation.
- **Useful suggestion** -- the plan would be better with this change, but it is not required before implementation.
- **Nitpick** -- low-value preference or wording issue; do not block implementation on it.
- **False positive** -- the initial review finding is not actually supported by the plan/code evidence.
- **Deferred/out of scope** -- real concern, but not part of this issue's implementation plan.

Use the assessment to filter and reclassify the review output:

- Critical findings should only include genuine blockers.
- Important findings should include genuine issues that should be addressed before implementation.
- Suggestions should include useful suggestions and clearly labeled deferred/out-of-scope concerns.
- False positives and nitpicks should not appear as blocking findings; mention them only briefly if useful for transparency.
- Preserve the original F/S identifiers when reclassifying so later discussion can reference stable items.

## Step 7: Present findings and execute decision

Present your review to the user, organized as:

1. **Plan summary**: Brief restatement of what the plan proposes.
2. **Strengths**: What the plan gets right.
3. **Assessment summary**: Counts by classification (e.g., genuine blockers, genuine issues, useful suggestions, nitpicks, false positives, deferred/out-of-scope).
4. **Issues**: Problems found, classified by severity and labeled with stable identifiers:
   - **Critical**: Genuine blockers that will cause the implementation to fail or produce incorrect results.
   - **Important**: Genuine issues or significant risks that should be addressed before implementation.
   - **Suggestions**: Useful improvements or explicitly deferred/out-of-scope concerns that are not blockers.
5. **Questions**: Any clarifying questions that came up during your review.
6. **Pitfalls for implementation**: Consolidate risk findings from Steps 4 and 5 into concrete, actionable warnings for the implementation session. Draw from the "Risks and pitfalls" exploration lens and the "Risks" assessment axis. Each item should be specific enough that an implementation session can act on it without re-exploring.
7. **Recommendation**: State whether the plan should be approved, revised, discussed further, or abandoned.

Ask the user how they want to proceed:

- **Create revised plan**: dispatch the architect to produce a revised plan addressing the findings.
- **Proceed as-is**: continue with the current plan despite findings.
- **Discuss findings**: explore specific findings in more detail before deciding.
- **Cancel**: stop here without updating or proceeding (a brief audit note will be posted).

If the user picks "Create revised plan", enter the revision loop:

### Revision loop

1. **Architect dispatch.** Use the `subagent` tool to dispatch `mach12:code-architect` with a brief containing:
   - The issue title, body, and requirements from Step 2.
   - The current implementation plan being revised (original plan on first iteration, or most recent revision on subsequent iterations).
   - The full findings list from Step 5 (with F/S identifiers and current classifications from Step 6), identifying which are Critical, Important, and Suggestions.
   - The raw exploration context from Step 4 — key files, observations, and codebase patterns discovered by the exploration subagents.
   - Any contribution guidelines or project planning requirements from Step 3.
   - The existing plan's `## Pitfalls and Gotchas` section (if present). Instruct the architect to preserve existing pitfalls unless the revision makes them irrelevant, and to add any new pitfalls discovered during review.
   - If this is a subsequent revision iteration, include the prior revised plan and the delta assessment that prompted re-revision.

   Instruct the architect to produce a complete revised implementation plan that addresses the Critical and Important findings while preserving the strengths identified in Step 7. Suggestions are optional improvements to incorporate where they fit naturally.

2. **Delta assessment.** After the architect returns, perform a lightweight delta assessment (not a full 6-lens re-exploration). For each finding from the original review (referencing stable F/S identifiers) and each N-prefixed item from prior iteration deltas, classify into one of three categories:
   - **Addressed**: The revised plan resolves this finding. State how in one sentence.
   - **Remaining**: The revised plan does not resolve this finding, or only partially addresses it. State what is still missing.
   - **New issue**: The revised plan introduces a concern not present in the original review. Label with N-prefixed identifiers continuing from the highest prior N-number (e.g., if prior delta had N1–N3, new issues start at N4) and classify severity (Critical/Important/Suggestion) using the same criteria as Step 6.

   Additionally, assess **pitfalls completeness**: does the revised plan's `## Pitfalls and Gotchas` section preserve pitfalls from the prior version (unless the corresponding plan aspect was removed) and incorporate any new pitfalls surfaced by the review? Flag dropped pitfalls or missing new ones.

   Precise criteria: A finding is "addressed" only when the revised plan's structure, staging, or approach concretely resolves the concern — not when the plan merely acknowledges it or adds a vague note. A "new issue" is a concern about the revised plan's structure, completeness, or correctness that did not exist in the original plan or any prior iteration's delta — not a restatement of an existing finding under a different framing.

3. **Presentation.** Present to the user:
   1. The revised plan.
   2. The delta assessment (Addressed / Remaining / New).
   3. A summary line: "X of Y findings addressed, Z remaining, W new issues" — where Y counts original F/S findings plus N-prefixed items carried from prior iterations.

4. **Sub-options.** Ask the user how to proceed:
   - **Post revised plan**: Accept this revision and post it.
   - **Revise again**: Return to the architect dispatch step with the current revised plan and this delta assessment as additional context. Unbounded — user controls when to stop.
   - **Discuss findings**: Same behavior as the main "Discuss findings" option — walk through specific findings or new issues, then return to these three options.

   Only one comment is posted — the final accepted revision. Intermediate revisions are not posted.

5. **Post.** When the user picks "Post revised plan", post the final revision as a comment. Include `<!-- mach12-plan -->` as the very first line of the comment body. Then delegate to:

   ```
   /mach12:gh-comment issue <issue-number>
   ```

If the user picks "Discuss findings", walk through the specific findings they want to explore, then ask again how to proceed. This step remains active across all discussion iterations until the user picks a terminal option (Create revised plan, Proceed, or Cancel).

If the user picks "Proceed as-is" and at least one Critical or Important finding exists, post a decision comment on the issue to record the user's choice. Prepare a body with this shape:
- First line: `<!-- mach12-decisions -->`
- A note that a plan review was conducted and the user chose to proceed without changes
- Each Critical and Important finding on its own line (one sentence each)
- Keep the entire comment body under 15 lines

Then delegate to:

```
/mach12:gh-comment issue <issue-number>
```

If the user picks "Proceed as-is" and all findings are Suggestions only, do NOT post a decision comment -- proceeding past suggestions is the expected path.

If the user picks "Cancel":

1. Confirm that no changes were made to the plan.
2. Post a lightweight decision comment on the issue. Prepare a body with this shape:
   - First line: `<!-- mach12-decisions -->`
   - A note that a plan review was conducted and the session ended without updating or proceeding
   - Finding counts by severity (e.g., "2 Critical, 1 Important, 3 Suggestions")
   - Keep the entire comment body to 5 lines or fewer

   Then delegate to:

   ```
   /mach12:gh-comment issue <issue-number>
   ```

When referring to numbered items (findings, suggestions, stages) in any comment body, use plain words like "finding 3" or "suggestion 3" -- not `#<number>` notation, which GitHub auto-links to issues/PRs.

When Scramjet asks you to report command status, call `report_scramjet_command_status` with `status: "completed"` and include **both** declared candidates in `next_steps` so the user can see all options:

- Always include an entry with `message`: `/mach12:issue-review <issue-number>`, `fresh_session`: `true`, and `reason`: a brief explanation of when another review pass would be warranted.
- Always include an entry with `message`: `/mach12:issue-implement <issue-number> 1`, `fresh_session`: `true`, and `reason`: a brief explanation that the plan is ready to implement.
- Set `recommended_next_step` to indicate your preference: recommend `mach12:issue-review` (index 0) when the plan was revised and Critical or Important findings remain; recommend `mach12:issue-implement` (index 1) when the plan is approved (user picked "Proceed as-is" — whether or not findings remain — or the revised plan addresses all blockers).
- Leave `next_steps` empty if the outcome is ambiguous (e.g., user cancelled, discussion is ongoing, or no clear next action). If the user cancelled, the review was not completed, or you otherwise did not finish, report the matching `status` (`blocked` / `incomplete`) instead of `completed`. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
