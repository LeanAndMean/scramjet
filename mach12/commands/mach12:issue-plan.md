---
description: Read a GitHub issue, analyze the codebase, and create a staged implementation plan
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

Delegate to:

```
/mach12:gh-issue-read <issue-number>
```

The subroutine returns the issue title, body, and the full comments stream. Parse and understand:
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

- **Smallest sufficient change**: Design the implementation that satisfies the requirements with the smallest change surface. Walk the minimum-sufficient solution ladder before proposing any new abstractions, files, or dependencies. Maximize reuse of existing patterns.
- **Strongest structural design**: Design the implementation prioritizing clear separation of concerns, maintainability, and well-defined abstractions. Still walk the ladder — justify each new component against a lower rung.
- **Alternative trade-off design**: Design an implementation that optimizes for a different axis (such as performance, extensibility, or a constraint the other lenses deprioritized). Walk the ladder and state what this lens deliberately trades away.

For parallel execution, dispatch all architecture tasks in a single batch rather than sequentially.

If the user provided context, include it in each brief so architecture designs account for the user's constraints or preferences.

Each lens should produce a full implementation blueprint: files to create or modify, component responsibilities, data flow, and a phased build sequence.

After all results return, review the approaches and form your own recommendation based on the issue's scope, the codebase's conventions, and the user's clarified requirements.

Each lens must state:
- Which ladder rung it sits on and why lower rungs are insufficient.
- What problem it optimizes for.
- What it deliberately does not build.
- What evidence would make this approach inappropriate.

Present to the user: brief summary of each approach, trade-offs comparison, **your recommendation with reasoning**, and concrete implementation differences.

The recommendation must answer only what the selected lens did not already cover:
- Which lens and ladder rung did you select, citing the lens's lower-rung rationale rather than restating it?
- Why is this not bigger than necessary?
- Why is this not too small to satisfy the requirements?
- Which larger abstractions/dependencies/files were rejected, and why?

Do not default to the middle option without explaining why both the smaller and more structural options are worse for this issue.

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

1. **Post the plan as a reply comment on the issue.** Format the body so it serves as input to future sessions. Include:
   - `<!-- mach12-plan -->` as the very first line of the comment body (this invisible HTML marker enables reliable identification in future sessions).
   - The full implementation plan.
   - The staged breakdown.
   - A `## Decision Log` section appended after the staged breakdown. This section captures the reasoning behind key decisions made during planning:
     - **Clarifying Questions (Step 5):** For each question asked and answered, include the question and a synthesized answer. Only include exchanges where the answer changed or constrained the plan. Omit exchanges where the user confirmed a default or said "whatever you think is best."
     - **Architecture Choice (Step 6):** The selected approach, the rationale for choosing it, and the alternatives considered with brief reasons for rejection.
     - **Omission condition:** Skip the Decision Log section entirely if Step 5 produced no questions AND Step 6 had no meaningful differentiation between approaches.
   - A note that this comment will guide staged implementation.

   Then delegate to:

   ```
   /mach12:gh-comment issue <issue-number>
   ```

   The subroutine posts the prepared body and returns the comment URL and numeric ID.

2. **Create a feature branch**:
   - Derive a short slug from the issue title (lowercase, hyphens, 3-5 words max).
   - Branch name format: `feature/issue-<issue-number>-<slug>`.
   - Example: `feature/issue-55-fix-analytics-url`.
   - Push the branch to remote with `-u` flag.

3. **Detect sub-issues** for the assignment step below. Delegate to:

   ```
   /mach12:gh-sub-issues <issue-number>
   ```

   The subroutine returns the list of sub-issue numbers (possibly empty) and which strategy produced them.

4. **Assign the issue and any sub-issues** to the current user. Delegate to:

   ```
   /mach12:gh-assign <issue-number> [<sub-issue-number> ...]
   ```

   Pass the parent issue number followed by every sub-issue number detected in step 3. The subroutine resolves the current user, classifies each issue (already assigned, no assignees, other assignees), auto-assigns where safe, and aggregates conflicts into a single bulk prompt at the end (Add me / Skip / Replace). Assignment failures are non-blocking.

When referring to numbered items (findings, suggestions, stages) in the comment body, use plain words like "finding 3" or "suggestion 3" -- not `#<number>` notation, which GitHub auto-links to issues/PRs.

Confirm all actions to the user (plan posted, branch created, issue assigned, and sub-issues assigned if applicable).

When Scramjet asks you to report command status, call `report_scramjet_command_status` with `status: "completed"` and include **both** declared candidates in `next_steps` so the user can see all options:

- Always include an entry with `message`: `/mach12:issue-review <issue-number>`, a chosen `fresh_session` value, and `reason`: a brief explanation of the review gate.
- Always include an entry with `message`: `/mach12:issue-implement <issue-number> <first-stage>`, `fresh_session`: `true`, and `reason`: a brief explanation that the plan is ready to implement.
- Set `recommended_next_step` to indicate your preference: recommend `mach12:issue-review` (index 0) when the plan is non-trivial, touches risky areas, or should receive an approval gate; recommend `mach12:issue-implement` (index 1) when the plan is small, uncontroversial, and you are confident in the staged breakdown.
- Leave `next_steps` empty if the appropriate next action is unclear. If the user cancelled, the plan was not posted, or you otherwise did not finish, report the matching `status` (`blocked` / `incomplete`) instead of `completed`. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
