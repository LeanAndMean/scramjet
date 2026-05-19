---
description: Read a GitHub issue, analyze the codebase, and create a staged implementation plan
argument-hint: "<issue-number> [context]"
allowed-tools:
  - bash
  - read
  - grep
  - glob
  - delegate
next:
  mode: open
  candidates:
    - name: mach12:issue-review
      hint: |
        Use when the plan is non-trivial, touches risky areas
        (concurrency, security, large refactors), or you have low
        confidence in any step.
    - name: mach12:issue-implement
      hint: |
        Use when the plan is small, uncontroversial, and you are
        confident in the staged breakdown.
---

# Issue Plan

You are creating a staged implementation plan for a GitHub issue. Your goal is to deeply understand the issue, explore the relevant codebase, and produce a plan where each stage can be implemented within a single session.

**User input:** $ARGUMENTS

## Guardrails

This command is strictly for **planning**. Do NOT:

- Implement any code changes -- no file edits, no file writes
- Attempt to execute the plan within this session

Implementation happens in separate sessions via `/mach12:issue-implement`.

## Step 1: Parse input

The user's input contains:
- An **issue number** (required)
- Additional **context** or constraints (optional)

Extract the issue number from the input. If the input is ambiguous, ask the user to clarify. If context was provided, note it for use during exploration and planning.

## Step 2: Read the issue

Read the issue title and body:

```
gh issue view <issue-number>
```

Then read all comments (`--comments` returns only comments and drops the title and body, so both calls are required):

```
gh issue view <issue-number> --comments
```

Parse and understand:
- The problem statement
- Any constraints or requirements mentioned
- Prior discussion or decisions in the comments
- Acceptance criteria (if specified)

## Step 3: Read contribution guidelines

Before exploring the code, delegate to:

```
/mach12:find-contribution-guidelines
```

The subroutine returns any planning-relevant guidance found in `CONTRIBUTING.md`, `DEVELOPMENT.md`, or `.github/CONTRIBUTING.md`: expected project layers (e.g., models, migrations, API routes, services, UI, documentation), testing expectations (test frameworks, coverage requirements, test types), and any other requirements that should inform the implementation plan.

Record these as **project planning requirements** -- they inform both the exploration focus and the plan drafting in Step 7.

## Step 4: Explore the codebase

Dispatch parallel exploration tasks to specialized subagents (one per lens). All lenses are required -- Step 5 always evaluates constraints and edge cases, and Step 6 requires constraint awareness for sound architecture design.

- **Similar features**: Find existing code that solves related problems. Trace through their implementation comprehensively, identifying patterns and conventions the new work should follow.
- **Architecture**: Map the architecture and abstractions for the relevant area, tracing through the code to understand the layers, data flow, and design decisions.
- **Integration points**: Identify where new code would connect to existing systems, including extension surfaces, testing infrastructure, and cross-cutting concerns.
- **Constraints and edge cases**: Investigate constraints and edge cases that the issue does not mention but the codebase reveals. Look for boundary conditions, implicit assumptions, error paths, or environmental requirements in the affected areas.

For parallel execution, dispatch all exploration tasks in a single batch rather than sequentially.

If the user provided context, include it in each exploration brief to guide focus. If project planning requirements were identified in Step 3, include them so exploration covers the relevant project layers and testing infrastructure.

Each exploration should return a list of 5-10 key files. After exploration completes, read all identified files to build deep understanding.

Present a comprehensive summary of findings and patterns discovered.

## Step 5: Ask clarifying questions

**CRITICAL**: This is one of the most important steps. DO NOT SKIP.

Review the codebase findings from Step 4 against the issue requirements. Identify all underspecified aspects:

1. Present a clear analysis of the problem based on what you found in the codebase.
2. Identify ambiguities, underspecified scope, unstated constraints, edge cases, integration concerns, and design preferences that will affect the implementation plan.
3. **Present all questions to the user in a clear, organized list.**
4. **Wait for answers before proceeding to architecture design.**

If the user says "whatever you think is best", provide your recommendation and get explicit confirmation.

## Step 6: Design architecture

Based on the codebase findings and clarified requirements, dispatch parallel architecture tasks to specialized subagents under three different lenses:

- **Minimal changes**: Design the implementation with the smallest change surface. Maximize reuse of existing patterns. Minimize new abstractions.
- **Clean architecture**: Design the implementation prioritizing clear separation of concerns, maintainability, and well-defined abstractions.
- **Pragmatic balance**: Design the implementation balancing speed with code quality and extensibility.

For parallel execution, dispatch all architecture tasks in a single batch rather than sequentially.

If the user provided context, include it in each brief so architecture designs account for the user's constraints or preferences.

Each lens should produce a full implementation blueprint: files to create or modify, component responsibilities, data flow, and a phased build sequence.

After all results return, review the approaches and form your own recommendation based on the issue's scope, the codebase's conventions, and the user's clarified requirements.

Present to the user: brief summary of each approach, trade-offs comparison, **your recommendation with reasoning**, and concrete implementation differences.

**Ask the user which approach they prefer.**

## Step 7: Draft the plan

Using the architecture selected in Step 6 as the structural foundation, draft a **staged implementation plan** with:
- Clear stages (numbered, with descriptive names)
- What each stage accomplishes
- Which files will be created or modified
- Dependencies between stages
- Testing approach for each stage

### Planning requirements

Before finalizing the plan, verify it satisfies the following:

**Project-layer coverage:** Cross-check the plan against the project layers discovered during codebase exploration and any layers specified in the project planning requirements recorded in Step 3. Every affected layer should be addressed by at least one stage. If a discovered layer is not affected by this change, it may be omitted -- but if a layer is affected and no stage addresses it, add the missing work to the appropriate stage or create a new one.

**Test coverage planning:** Identify what testing is appropriate for this project and codebase. If the project has an existing test suite or the project planning requirements specify testing expectations, each stage that introduces or modifies behavior must specify: what tests to add or modify, what test types are needed (unit, integration, end-to-end, etc.), and what behaviors or interfaces to cover. If the project has no testable runtime code (e.g., plugin definitions, documentation, configuration), note this in the plan and skip test planning.

**Each stage must be scoped to what can be implemented within a single session.** A stage that is too large should be split. Consider:
- The amount of codebase exploration needed
- The number of files to create or modify
- The complexity of the logic involved
- The testing surface area

## Step 8: Post plan and create branch

Present the plan to the user and ask:

- **Approve**: post the plan and create the feature branch
- **Request changes**: suggest changes to the plan before posting
- **Cancel**: abort without posting the plan or creating a branch

If the user requests changes, discuss their feedback, revise the plan, and present it for approval again. If the user cancels, stop and confirm that the plan was not posted and no branch was created.

After the user approves the plan:

1. **Post the plan as a reply comment on the issue** using `gh issue comment <issue-number> --body "..."`. Format the comment so it serves as input to future sessions. Include:
   - `<!-- mach12-plan -->` as the very first line of the comment body (this invisible HTML marker enables reliable identification in future sessions)
   - The full implementation plan
   - The staged breakdown
   - A `## Decision Log` section appended after the staged breakdown. This section captures the reasoning behind key decisions made during planning:
     - **Clarifying Questions (Step 5):** For each question asked and answered, include the question and a synthesized answer. Only include exchanges where the answer changed or constrained the plan. Omit exchanges where the user confirmed a default or said "whatever you think is best."
     - **Architecture Choice (Step 6):** The selected approach, the rationale for choosing it, and the alternatives considered with brief reasons for rejection.
     - **Omission condition:** Skip the Decision Log section entirely if Step 5 produced no questions AND Step 6 had no meaningful differentiation between approaches.
   - A note that this comment will guide staged implementation

2. **Create a feature branch**:
   - Derive a short slug from the issue title (lowercase, hyphens, 3-5 words max)
   - Branch name format: `feature/issue-<issue-number>-<slug>`
   - Example: `feature/issue-55-fix-analytics-url`
   - Push the branch to remote with `-u` flag

3. **Assign the issue** to the current user:
   - Check existing assignees: `gh issue view <issue-number> --json assignees --jq '[.assignees[].login] | join(",")'`
   - Check current user: `gh api user --jq .login`
   - If the current user is already assigned, skip silently.
   - If there are no assignees, run `gh issue edit <issue-number> --add-assignee @me`.
   - If other assignees exist (not including the current user), warn the user and ask how to proceed:
     - **Add me**: add yourself as an additional assignee -- run `gh issue edit <issue-number> --add-assignee @me`
     - **Skip**: leave the current assignee(s) unchanged
     - **Replace**: remove existing assignee(s) and assign only yourself -- run `gh issue edit <issue-number> --remove-assignee <existing-logins> --add-assignee @me`
   - If the assignment command fails, warn the user in CLI output and continue -- assignment failure must not block the workflow.

4. **Assign sub-issues** to the current user:

   Detect sub-issues using the two-strategy approach:

   - **Strategy A (API):** First resolve the repository identifier, then query the GitHub sub-issues API:
     ```
     REPO=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
     gh api --paginate repos/$REPO/issues/<issue-number>/sub_issues --jq '.[].number'
     ```
     - If the API call **succeeds and returns one or more numbers**, use them as the confirmed sub-issue list.
     - If the API call **succeeds but returns no results** (empty array), the issue has no sub-issues. Do NOT fall through to Strategy B -- treat the sub-issue list as empty.
     - If the API call **fails** (e.g., 404, permission error, network timeout), proceed to Strategy B.

   - **Strategy B (body-parse fallback):** This strategy runs only when Strategy A **failed**. Scan the issue body for sub-issue references. Match `#<number>` references that appear on GitHub task list lines -- lines beginning with optional whitespace followed by a list marker (`-`, `*`, or `+`) and a checkbox (`[ ]`, `[x]`, or `[X]`). Exclude any `#<number>` preceded by relational keywords: "Related to", "Blocked by", "See also", or "Depends on". Collect the matched issue numbers, excluding the parent issue number itself.

   If sub-issues are found, get the current user login via `gh api user --jq .login`. For each sub-issue, check assignees via `gh issue view <sub-issue> --json assignees --jq '[.assignees[].login] | join(",")'`. Three paths:
   - Current user already assigned: skip silently.
   - No assignees: auto-assign with `gh issue edit <sub-issue> --add-assignee @me`.
   - Other assignees exist: collect into a "conflicting" list.

   If the conflicting list is non-empty, ask the user how to proceed with a single bulk decision:
   - **Add me**: add yourself as an additional assignee on all conflicting sub-issues.
   - **Skip**: leave the current assignee(s) unchanged on all sub-issues.
   - **Replace**: remove existing assignee(s) and assign only yourself on all sub-issues.

   Assignment failures are non-blocking (warn and continue).

When referring to numbered items (findings, suggestions, stages) in the comment body, use plain words like "finding 3" or "suggestion 3" -- not `#<number>` notation, which GitHub auto-links to issues/PRs.

Confirm all actions to the user (plan posted, branch created, issue assigned, and sub-issues assigned if applicable).
