---
description: Read a GitHub issue, analyze the codebase, and create a staged implementation plan
argument-hint: "<issue-number> [context]"
allowed-tools:
  - add_issue_comment
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

The subroutine returns the issue title, body, parent `createdAt` and `updatedAt`, and the full comments stream with comment creation timestamps. Parse and understand:
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

Classify the affected work before dispatching advisory subagents:

- **Command-only** work changes executable natural-language surfaces: command or agent Markdown, frontmatter, next-step or delegation contracts, tool scopes, prompt artifacts, command-facing documentation, or tests whose subject is model interpretation. Replace the analogous code exploration with `scramjet:command-set-explorer` to map commands, edges, context boundaries, artifacts, side-effect owners, and complete journeys.
- **Code-only** work changes runtime source or executable implementation tests. Retain `mach12:code-explorer` and dispatch the existing four lenses: similar features, architecture, integration points, and constraints or edge cases.
- **Mixed** work uses both families with disjoint briefs and file/claim partitions. Do not ask either family to review the other's surface.

Across the initial exploration, architecture, and test-design pass in Steps 4, 6, and 8, use a maximum of eight subagent calls across both families. User-requested targeted architecture reruns after rejecting the initial options are a separate decision branch and do not count against the completed initial pass. Every brief must carry the task-relevant issue title, body, discussion, requirements, and decisions from Step 2; user context and project planning requirements; relevant parent-established observations; the exact command/runtime partition; the selection reason; and the expected output. Pass concise excerpts or summaries rather than an indiscriminate transcript. Record every selected agent and its evidence-based reason in the synthesis. A better-fit installed agent may replace an advisory role only when this command explicitly names it and defines the required output, or authoritative repository or command guidance establishes compatibility with the responsibility, read-only posture, context needs, required output, and workflow handoff. A catalog-only name or description match is supplementary and cannot replace an applicable named Scramjet specialist or exact Mach 12 role.

For command work, add `scramjet:context-flow-analyzer`, `scramjet:authority-state-analyzer`, `scramjet:command-trust-reviewer`, `scramjet:command-failure-analyst`, or `scramjet:command-completeness-checker` only when delegation or fresh-session flow, duplicated authority or partial state, a concrete trust boundary, an observed failure, or explicit requirement coverage makes that responsibility relevant. Never broaden to every specialist. Reserve enough of the eight-call budget for the architecture calls and, when Step 8's dispatch criteria apply, the evaluation or test designer. Missing, failed, or malformed required output remains visible as incomplete evidence rather than triggering silent substitution.

For parallel execution, dispatch all selected exploration tasks in a single batch. Include user context and project planning requirements in each relevant brief. Each exploration should return key files and cited observations; after it completes, read every identified file needed to build deep understanding.

Use the issue and comment timestamps to identify potentially stale factual premises that materially affect the plan, including claimed behavior, file locations, architecture, APIs, constraints, and gaps. Verify those claims against current repository authority before relying on them. Surface material drift and plan from current evidence without discarding historical requirements or decisions that remain supported; age alone does not make them invalid.

Present a comprehensive summary of findings, selection reasons, and patterns discovered. Mention an omitted role only when its omission materially limits confidence.

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

Based on the codebase findings and clarified requirements, route architecture by the Step 4 classification while staying within the shared maximum of eight calls:

- For command-only work, replace the three code architects with three `scramjet:command-architect` calls, one for each alternative below. Each brief asks for one complete selected design under that lens, including rejected additive alternatives, while owning command purpose, responsibility boundaries, side effects, fact authority, partial-state elimination, and total command-set complexity.
- For code-only work, retain three `mach12:code-architect` calls, one for each alternative below.
- For mixed work, give prompt and code architects disjoint briefs, reserve at least one `scramjet:command-architect` as aggregate owner for the command design, and have the parent synthesize three coherent system-level options. All calls count against the same eight-call budget.

A replacement installed agent must satisfy the selected lens's exact blueprint contract; catalog-only similarity is insufficient. Dispatch the selected architecture tasks in one parallel batch. If the user provided context, include it in each brief.

The three alternatives are:

- **Smallest sufficient change**: Design the implementation that satisfies the requirements with the smallest change surface. Walk the minimum-sufficient solution ladder before proposing any new abstractions, files, or dependencies. Maximize reuse of existing patterns.
- **Strongest structural design**: Design the implementation prioritizing clear separation of concerns, maintainability, and well-defined abstractions. Still walk the ladder — justify each new component against a lower rung.
- **Alternative trade-off design**: Design an implementation that optimizes for a different axis (such as performance, extensibility, or a constraint the other lenses deprioritized). Walk the ladder and state what this lens deliberately trades away.

Each option should produce a full implementation blueprint: files to create or modify, component responsibilities, data flow, and a phased build sequence.

Each lens must also assess the technical debt the option it proposes introduces, retains, reduces, or avoids. An evidence-based “none identified” is valid; do not invent debt. Keep this assessment concise and separate from broader risks and trade-offs: technical debt means likely future maintenance, migration, coupling, testing, or operational cost.

After all results return, review the approaches and form your own recommendation based on the issue's scope, the codebase's conventions, and the user's clarified requirements.

Each lens must state:
- Which ladder rung it sits on and why lower rungs are insufficient.
- What problem it optimizes for.
- What it deliberately does not build.
- What evidence would make this approach inappropriate.
- The technical debt the option it proposes introduces, retains, reduces, or avoids.

Present the three options in a narrow Markdown table with these concise, parallel columns, in order: **Option**, **Approach**, **Key difference / trade-off**, and **Debt delta**. Use **Option** only for the short lens or option name. In **Approach**, concisely state what the architecture builds, how it works, and what requirement or problem it solves. Reserve **Key difference / trade-off** for comparative benefits, costs, and sacrifices relative to the other options. Define debt deltas against the current implementation: `+` means debt introduced, while `-` means existing debt reduced or removed. These signs indicate direction, not whether an option is good or bad, and an option does not need to contain both. Use `None identified` when the evidence supports no material delta; never invent debt to make rows symmetrical. Omit retained debt that is decision-immaterial or common to the options from the table cells, but state a materially differentiating retained liability explicitly in words rather than adding another symbol. Summarize common material retained debt outside the compact table.

Always present the detailed trade-offs, concrete implementation differences, **your recommendation with reasoning**, common material debt, and whether every current option has unsatisfactory debt implications. Place those details outside the compact table whenever including them would make table cells verbose. Keep detailed blueprint rationale outside the table as needed. The complete presentation must still include a brief summary of each approach, a trade-offs comparison, and a concise cross-option technical-debt summary identifying material differences and debt common to all options.

The recommendation must answer only what the selected lens did not already cover:
- Which lens and ladder rung did you select, citing the lens's lower-rung rationale rather than restating it?
- Why is this not bigger than necessary?
- Why is this not too small to satisfy the requirements?
- Which larger abstractions/dependencies/files were rejected, and why?

Do not default to the middle option without explaining why both the smaller and more structural options are worse for this issue.

After presenting the complete comparison and recommendation, ask the user to choose either:

- **Accept one approach**; or
- **Reject all current approaches and request revision**, optionally supplying constraints.

If the user rejects all current approaches, incorporate their feedback, refine or rerun the relevant architecture analysis as needed, and present the complete updated option comparison and recommendation again. Do not require all three lenses to rerun when targeted refinement is sufficient, but always present a complete coherent replacement rather than a delta.

**Do not proceed to Step 7 or any later planning work until the user explicitly accepts an approach.**

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

**Dispatch a test or evaluation designer** when the issue is:
- A bug fix (test-first is particularly valuable here)
- A non-trivial feature (new behavior that needs confidence verification)
- A refactor touching critical paths

For command-only work, replace `mach12:test-designer` with `scramjet:command-evaluation-designer`; require the same plan-facing classification, test-first recommendation, proposed-tests table, cost/benefit assessment, and stage directives, while distinguishing structural evidence from provider-expensive or operational evidence. For code-only work, retain `mach12:test-designer`. For mixed work, use disjoint command and runtime briefs when both are justified, and count both against the shared eight-call maximum. A catalog-only match cannot replace either exact role.

**Write a lightweight inline test note instead** when:
- The change is non-executable documentation, configuration, or mechanical metadata
- A command edit is trivial and its test need is obvious enough to state in one sentence (e.g., "update the wiring assertion")
- There is no testable runtime or model-interpreted behavior

### Dispatching the subagent

Pass the selected designer a synthesized brief containing:
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

## Step 9: Load the plan-comment contract and draft the plan

Immediately before drafting the durable artifact, delegate once in this turn to:

```
/mach12:plan-comment-contract initial
```

This loads the canonical artifact policy into the current model context; it does not run an independent formatter. Do not pass candidate Markdown through the delegation arguments.

Apply the contract to the selected architecture, codebase and architect evidence, test strategy, project planning requirements, and user decisions from the preceding steps. Draft the exact, complete post-ready body beginning with `<!-- mach12-plan -->`, then run the contract's final self-check. Resolve defects supported by the available evidence before publication; if evidence is genuinely missing or contradictory, gather it or ask the user rather than finalizing an incomplete marker-bearing body.

## Step 10: Post plan and create branch

After the plan passes the contract self-check, do not display the complete body in assistant prose or ask for a separate approval.

1. **Publish the final plan as a reply comment on the issue.** State the selected architecture, stage count, and publication consequence concisely without repeating the complete plan. Call `add_issue_comment` with the issue number and exact final marker-bearing body. When effective policy requires approval, the approval card presents the exact payload. Regardless of policy, guarded publication and exact verification apply. Continue only after verified publication; cancellation creates no branch or assignment side effects, and ambiguity must be reconciled without automatic retry.

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

Apply the plan-comment contract’s reference policy: intentional same-repository issue or pull-request relationships use `#N`, cross-repository relationships use `owner/repo#N` or an already verified canonical URL, and artifact-local findings, suggestions, and stages use stable labels or plain words rather than bare `#N`. Do not introduce closing keywords for ordinary references.

Confirm all actions to the user (plan posted, branch created, issue assigned, and sub-issues assigned if applicable).

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`, then set `status: "completed"` and include **both** declared candidates in `next_steps` so the user can see all options:

- Always include an entry with `message`: `/mach12:issue-review <issue-number>`, a chosen `fresh_session` value, and `reason`: a brief explanation of the review gate.
- Always include an entry with `message`: `/mach12:issue-implement <issue-number> <first-stage>`, `fresh_session`: `true`, and `reason`: a brief explanation that the plan is ready to implement.
- Set `recommended_next_step` to indicate your preference: recommend `mach12:issue-review` (index 0) when the plan is non-trivial, touches risky areas, or should receive an approval gate; recommend `mach12:issue-implement` (index 1) when the plan is small, uncontroversial, and you are confident in the staged breakdown.
- Leave `next_steps` empty if the appropriate next action is unclear. If the user cancelled, the plan was not posted, or you otherwise did not finish, report the matching `status` (`blocked` / `incomplete`) instead of `completed`. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
