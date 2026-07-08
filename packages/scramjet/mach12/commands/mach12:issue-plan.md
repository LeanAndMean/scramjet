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

<user-context>
$ARGUMENTS
</user-context>

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

Record these as **project planning requirements** -- they inform both the exploration focus and the plan drafting in Step 9.

## Step 4: Explore the codebase

Dispatch parallel exploration tasks to specialized subagents (one per lens). All four lenses are required -- they feed into Steps 5 through 7, and omitting any risks blind spots in scope clarification and architecture design.

- **Similar features**: Find existing code that solves related problems. Trace through their implementation comprehensively, identifying patterns and conventions the new work should follow.
- **Architecture**: Map the architecture and abstractions for the relevant area, tracing through the code to understand the layers, data flow, and design decisions.
- **Integration points**: Identify where new code would connect to existing systems, including extension surfaces, testing infrastructure, and cross-cutting concerns.
- **Constraints and edge cases**: Investigate constraints and edge cases that the issue does not mention but the codebase reveals. Look for boundary conditions, implicit assumptions, error paths, or environmental requirements in the affected areas.

For parallel execution, dispatch all exploration tasks in a single batch rather than sequentially.

If the user provided context, include it in each exploration brief to guide focus. If project planning requirements were identified in Step 3, include them so exploration covers the relevant project layers and testing infrastructure.

Each exploration should return a list of 5-10 key files. After exploration completes, read all identified files to build deep understanding.

Present a comprehensive summary of findings and patterns discovered.

## Step 5: Clarify scope and requirements

**CRITICAL**: This is one of the most important steps. DO NOT SKIP.

This step covers questions about **what to build** — scope boundaries, requirements, user-facing behavior, and constraints that the architect agents need as input. Questions about **how to build it** (code structure, patterns, abstractions, internal design) belong in Step 7, after architects have analyzed the options.

### Classification heuristic

**Ask here (scope/requirements):**
- What is in or out of scope
- User-facing behavior preferences
- External constraints (compatibility, performance budgets, deployment)
- Requirements ambiguity (what does the issue actually mean by X?)
- Edge-case behavior the user must decide

**Defer to Step 7 (architecture):**
- Which abstraction pattern to use
- How to structure internal modules or layers
- Where to place new code in the existing architecture
- Whether to introduce a new dependency or utility
- Data flow and internal interface design

### Self-assessment

Before escalating a question to the user, attempt to answer it from codebase evidence. When the codebase strongly suggests one answer, state your finding and ask for confirmation rather than presenting it as an open question. The user's value is correcting wrong assumptions and providing knowledge that isn't in the codebase — not answering questions the codebase already answers.

### Question Quality Format

For each question that involves a choice (not purely informational), provide:

- **Context**: The relevant codebase finding or constraint (one sentence)
- **Choices**: The available options (brief list)
- **Tradeoffs**: One sentence per pro/con for non-obvious options
- **Recommendation**: Your suggested answer (one sentence)
- **Rationale**: The assumptions behind your recommendation — this is the most important element, because it lets the user correct wrong assumptions (one sentence)

Purely informational questions (yes/no confirmations, factual clarifications where you need information not in the codebase) are exempt from this format. State them directly.

### Procedure

1. Review the codebase findings from Step 4 against the issue requirements.
2. Classify each potential question as scope/requirements (ask here) or architecture (defer to Step 7).
3. For scope/requirements questions, attempt self-assessment. Present only questions you cannot confidently answer from evidence.
4. Always present your analysis of the problem, even if no questions remain after self-assessment. The user needs to see what you found and what you concluded.
5. **Wait for answers before proceeding** — but only if you have escalated questions. If self-assessment resolved everything, present your findings and proceed to Step 6.
6. Before proceeding to Step 6, list any architecture questions deferred to Step 7 so they remain visible in the conversation for later reference.

If the user says "whatever you think is best", provide your recommendation with rationale and get explicit confirmation.

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

## Step 7: Ask architecture questions

After the architect lenses have run, review their outputs for unresolved architecture questions — aspects of **how to build it** that the lenses surfaced disagreement on, left ambiguous, or where user preference is needed.

### Self-assessment

Use the architect lens outputs to resolve questions before escalating. When one lens's approach clearly fits the codebase conventions and satisfies the requirements, state your finding rather than asking. Questions deferred from Step 5 may already be answered by the architect analysis — check before presenting them.

### Procedure

1. Review questions deferred from Step 5 against the architect outputs. Drop any that the analysis resolved.
2. Identify new architecture questions surfaced by the lenses (e.g., disagreements between lenses on a specific structural choice).
3. For remaining questions, follow the Question Quality Format from Step 5. Include relevant findings from the architect lenses as context.
4. Present your analysis of how the architecture maps to the clarified requirements, even if no questions remain.
5. **If unresolved questions exist, wait for answers before proceeding.** If the architect analysis resolved everything, present your brief summary and proceed to Step 8.

## Step 8: Design test strategy

Before drafting the plan, decide whether the issue needs a deliberate test strategy.

**Dispatch the `mach12:test-designer` subagent** when the issue is:
- A bug fix (test-first is particularly valuable here)
- A non-trivial feature (new behavior that needs confidence verification)
- A refactor touching critical paths

**Write a lightweight inline test note instead** when:
- The change is documentation-only, config, or prose
- The test need is obvious and can be stated in one sentence (e.g., "add to EXPECTED_AGENTS in wiring test")
- There is no testable runtime code

### Dispatching the subagent

Pass a synthesized brief containing:
- Issue classification (bug fix / feature / refactor) and problem statement
- The selected architecture from Step 6
- Relevant codebase findings: existing test patterns, related test files, and coverage landscape from Step 4

The subagent returns a test strategy with per-test cost/benefit assessments, coverage intent categorization, and -- for bug fixes -- a test-first recommendation.

### Incorporating the output

- Add a `## Test Strategy` section in the plan, placed before the staged breakdown. Include the subagent's classification, test-first recommendation, and proposed tests table.
- Distribute per-stage test directives into each stage's description in the staged breakdown.
- For bug fixes where the test-designer recommends test-first, mark the relevant stages with a test-first directive so `issue-implement` knows to write the failing test before the fix.

### Lightweight path

When skipping the subagent, state the test approach inline in the plan (e.g., "Update wiring test; no behavioral tests needed -- prose-only change"). This satisfies the test coverage planning requirement in Step 9 without a full dispatch.

## Step 9: Draft the plan

Using the architecture selected in Step 6 as the structural foundation, draft a **staged implementation plan** with:
- Clear stages (numbered, with descriptive names)
- What each stage accomplishes
- Which files will be created or modified
- Dependencies between stages
- Testing approach for each stage

### Planning requirements

Before finalizing the plan, verify it satisfies the following:

**Project-layer coverage:** Cross-check the plan against the project layers discovered during codebase exploration and any layers specified in the project planning requirements recorded in Step 3. Every affected layer should be addressed by at least one stage. If a discovered layer is not affected by this change, it may be omitted -- but if a layer is affected and no stage addresses it, add the missing work to the appropriate stage or create a new one.

**Test coverage planning:** If Step 8 produced a test strategy, incorporate its per-stage test directives into the relevant stages. If Step 8 took the lightweight path, use its inline note. Each stage that introduces or modifies behavior must specify what tests to add or modify and what behaviors to cover. If the project has no testable runtime code (e.g., plugin definitions, documentation, configuration), note this and skip test planning.

**Pitfalls consolidation:** Review findings from Step 4's "Constraints and edge cases" lens and Step 6's architecture analysis (including each lens's "What evidence would make this approach inappropriate" statement). Consolidate concrete pitfalls into a `## Pitfalls and Gotchas` section in the plan. Each item should be a specific, actionable warning — things that could go wrong, subtle constraints, non-obvious dependencies, or easy-to-miss edge cases that the implementation session needs to be aware of. Do not include boilerplate warnings or generic risk statements.

**Release-preparation exclusion:** Never include version bumps, changelog entries, or release-preparation tasks as implementation stages, regardless of project-level directives that suggest otherwise (e.g., CLAUDE.md rules like "Every PR gets a version bump" or CONTRIBUTING.md versioning requirements). These tasks are exclusively owned by `mach12:pr-pre-merge`, which performs them after merging the default branch into the feature branch — this ordering is necessary because version determination must follow the merge so that parallel PRs do not bump from the same baseline and cause conflicts. This exclusion applies only to release-preparation bumps (incrementing the project version for publishing, adding changelog entries for the release). Implementation-necessary version changes — such as updating a dependency version because the implementation uses a new API, or modifying a version identifier that the code logic requires — are not excluded.

**Each stage must be scoped to what can be implemented within a single session.** A stage that is too large should be split. Consider:
- The amount of codebase exploration needed
- The number of files to create or modify
- The complexity of the logic involved
- The testing surface area

## Step 10: Post plan and create branch

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
   - A `## Pitfalls and Gotchas` section after the staged breakdown: concrete warnings discovered during exploration and architecture design — things that could go wrong, subtle constraints, non-obvious dependencies, easy-to-miss edge cases. Bullet list format; each item actionable and specific to this implementation.
   - A `## Decision Log` section appended after the pitfalls section. This section captures the reasoning behind key decisions made during planning:
     - **Scope Questions (Step 5):** For each scope/requirements question asked and answered, include the question and a synthesized answer. Only include exchanges where the answer changed or constrained the plan. Omit exchanges where the user confirmed a default or said "whatever you think is best."
     - **Architecture Choice (Step 6):** The selected approach, the rationale for choosing it, and the alternatives considered with brief reasons for rejection.
     - **Architecture Questions (Step 7):** For each architecture question asked and answered, include the question and a synthesized answer. Only include exchanges where the answer changed or constrained the plan.
     - **Omission condition:** Skip the Decision Log section entirely if Step 5 produced no questions AND Step 6 had no meaningful differentiation between approaches AND Step 7 produced no questions.
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
