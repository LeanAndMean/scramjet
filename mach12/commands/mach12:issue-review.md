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

**User input:** $ARGUMENTS

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

If the user provided context in Step 1, weight the review toward the areas they emphasized -- surface findings on those areas even at Suggestions-level severity, and note in Step 7 how the user's focus shaped the findings (e.g., "User emphasized testing strategy; this raised three suggestions in that area that would otherwise be borderline."). Apply this weighting across all six axes below; do not let it crowd out coverage of the other axes.

For each stage in the plan, assess:

1. **Correctness**: Does the stage accurately describe what needs to happen? Are the files and changes correct?
2. **Completeness**: Are there missing steps, files, or edge cases?
3. **Scope**: Is the stage appropriately sized for a single session, or should it be split/merged?
4. **Dependencies**: Are inter-stage dependencies correctly identified? Is the ordering logical?
5. **Testing**: Is the testing approach adequate for each stage?
6. **Risks**: Are there architectural risks, performance concerns, or subtle pitfalls the plan overlooks?

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
6. **Recommendation**: State whether the plan should be approved, revised, discussed further, or abandoned.

Ask the user how they want to proceed:

- **Update the plan**: post a revised plan addressing the findings.
- **Proceed as-is**: continue with the current plan despite findings.
- **Discuss findings**: explore specific findings in more detail before deciding.
- **Cancel**: stop here without updating or proceeding (a brief audit note will be posted).

If the user picks "Update the plan", draft a revised plan incorporating the findings, and present it for review before posting. When posting the revised plan as a comment, include `<!-- mach12-plan -->` as the very first line of the comment body.

If the user picks "Discuss findings", walk through the specific findings they want to explore, then ask again how to proceed. This step remains active across all discussion iterations until the user picks a terminal option (Update, Proceed, or Cancel).

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

When Scramjet asks you to report command status, call `scramjet_command_status` with `status: "completed"` and choose selector-visible `next_steps` entries based on the review outcome:

- If the plan was revised and Critical or Important findings remain, include an entry with `name`: `mach12:issue-review`, `args`: `<issue-number>`, `fresh_session`: `true`, and `reason`: a brief explanation that additional review is warranted.
- If the plan is approved (user picked "Proceed as-is" — whether or not findings remain — or the revised plan addresses all blockers), include an entry with `name`: `mach12:issue-implement`, `args`: `<issue-number> 1`, `fresh_session`: `true`, and `reason`: a brief explanation that the plan is ready to implement.
- Set `recommended_next_step` to the zero-based index of the entry you recommend Scramjet route to automatically. Leave `next_steps` empty if the outcome is ambiguous (e.g., user cancelled, discussion is ongoing, or no clear next action). If the user cancelled, the review was not completed, or you otherwise did not finish, report the matching `status` (`waiting_for_user` / `blocked` / `incomplete`) instead of `completed`.
