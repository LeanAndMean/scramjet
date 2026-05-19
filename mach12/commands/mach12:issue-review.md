---
description: Read a GitHub issue and all comments, review the implementation plan, and present findings
argument-hint: "<issue-number> [context]"
allowed-tools:
  - bash
  - read
  - grep
  - glob
next:
  mode: ask
  hint: |
    Decide whether the plan is ready to implement, needs revision, or
    the issue should be abandoned. Suggested follow-ups when ready:
    /mach12:issue-implement to begin work, or /mach12:issue-plan to
    re-plan after substantive revision.
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

Read the issue title and body:

```
gh issue view <issue-number>
```

Then read all comments (`--comments` returns only comments and drops the title and body, so both calls are required):

```
gh issue view <issue-number> --comments
```

Understand:
- The original problem statement and requirements
- The implementation plan (typically posted as a comment)
- Any discussion, decisions, or clarifications in the comment thread

Locate the implementation plan comment by searching all issue comments for the `<!-- mach12-plan -->` HTML marker. If multiple comments contain the marker, use the last one (the most recent revision). If no comment contains the marker, fall back to identifying the most recent substantive comment that contains a staged implementation plan. If no plan exists at all, inform the user and suggest running `/mach12:issue-plan <issue-number>` first, then stop.

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

If the user provided context in Step 1, weight the review toward the areas they emphasized -- surface findings on those areas even at Suggestions-level severity, and note in Step 6 how the user's focus shaped the findings (e.g., "User emphasized testing strategy; this raised three suggestions in that area that would otherwise be borderline."). Apply this weighting across all six axes below; do not let it crowd out coverage of the other axes.

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

## Step 6: Present findings and execute decision

Present your review to the user, organized as:

1. **Plan summary**: Brief restatement of what the plan proposes.
2. **Strengths**: What the plan gets right.
3. **Issues**: Problems found, classified by severity:
   - **Critical**: Will cause the implementation to fail or produce incorrect results.
   - **Important**: Significant gaps or risks that should be addressed before implementation.
   - **Suggestions**: Improvements that would make the plan better but are not blockers.
4. **Questions**: Any clarifying questions that came up during your review.

Ask the user how they want to proceed:

- **Update the plan**: post a revised plan addressing the findings.
- **Proceed as-is**: continue with the current plan despite findings.
- **Discuss findings**: explore specific findings in more detail before deciding.
- **Cancel**: stop here without updating or proceeding (a brief audit note will be posted).

If the user picks "Update the plan", draft a revised plan incorporating the findings, and present it for review before posting. When posting the revised plan as a comment, include `<!-- mach12-plan -->` as the very first line of the comment body.

If the user picks "Discuss findings", walk through the specific findings they want to explore, then ask again how to proceed. This step remains active across all discussion iterations until the user picks a terminal option (Update, Proceed, or Cancel).

If the user picks "Proceed as-is" and at least one Critical or Important finding exists, post a decision comment on the issue to record the user's choice:

```
gh issue comment <issue-number> --body "..."
```

Comment format:
- First line: `<!-- mach12-decisions -->`
- A note that a plan review was conducted and the user chose to proceed without changes
- Each Critical and Important finding on its own line (one sentence each)
- Keep the entire comment body under 15 lines

If the user picks "Proceed as-is" and all findings are Suggestions only, do NOT post a decision comment -- proceeding past suggestions is the expected path.

If the user picks "Cancel":

1. Confirm that no changes were made to the plan.
2. Post a lightweight decision comment on the issue:

   ```
   gh issue comment <issue-number> --body "..."
   ```

   Comment format:
   - First line: `<!-- mach12-decisions -->`
   - A note that a plan review was conducted and the session ended without updating or proceeding
   - Finding counts by severity (e.g., "2 Critical, 1 Important, 3 Suggestions")
   - Keep the entire comment body to 5 lines or fewer

When referring to numbered items (findings, suggestions, stages) in any comment body, use plain words like "finding 3" or "suggestion 3" -- not `#<number>` notation, which GitHub auto-links to issues/PRs.
