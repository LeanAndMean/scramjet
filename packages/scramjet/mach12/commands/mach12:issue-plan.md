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

<user-context>
$ARGUMENTS
</user-context>

## Goals

- Produce one durable, standalone implementation plan grounded in the complete issue history, current codebase, project guidance, and explicit user decisions.
- Select a minimum-sufficient architecture and proportionate test strategy, divided into dependency-aware stages that each fit one implementation session.
- Publish the verified plan, create its feature branch, and report assignment outcomes before offering next-step routing.

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

The subroutine returns any planning-relevant guidance found in `CONTRIBUTING.md`, `DEVELOPMENT.md`, or `.github/CONTRIBUTING.md`: expected project layers (e.g., models, migrations, API routes, services, UI, documentation), testing expectations (test frameworks, coverage requirements, test types), documented development tools, and any other requirements that should inform the implementation plan.

Record these as **project planning requirements** -- they inform both the exploration focus and the plan drafting in Step 9.

## Step 4: Explore the codebase

Classify the affected work before dispatching advisory subagents:

- **Command-only** work changes executable natural-language surfaces: command or agent Markdown, frontmatter, next-step or delegation contracts, tool scopes, prompt artifacts, command-facing documentation, or tests whose subject is model interpretation. Replace the analogous code exploration with `scramjet:command-set-explorer` to map commands, edges, context boundaries, artifacts, side-effect owners, and complete journeys.
- **Code-only** work changes runtime source or executable implementation tests. Retain `mach12:code-explorer` and dispatch the existing four lenses: similar features, architecture, integration points, and constraints or edge cases.
- **Mixed** work uses both families with disjoint briefs and file/claim partitions. Do not ask either family to review the other's surface. Consolidate related runtime exploration lenses into no more than three runtime exploration calls while keeping each lens's evidence and output explicit.

As part of parent-owned exploration, identify project-provided development tools relevant to the affected artifacts from repository guidance, manifests, adjacent scripts, CI configuration, and established project usage. Establish each tool's authority; classify its relevance as required verification, advisory analysis, or irrelevant, and its execution effect as non-mutating or mutating generation/formatting. Inspect unfamiliar scripts before use; do not install missing tools or run mutating modes without authorization covering their effects. Record exact verified commands, sources, classifications, availability, outputs, and limitations. Treat diagnostics as evidence rather than automatic root-cause or scope decisions, and never treat clean tooling output as behavioral proof.

Across the initial exploration, architecture, and runtime test-design pass in Steps 4, 6, and 8, use a maximum of eight subagent calls across both families. This is a ceiling for code-heavy or mixed work, not a target. User-requested targeted architecture reruns after rejecting the initial options are a separate decision branch and do not count against the completed initial pass. Every brief must carry the task-relevant issue title, body, discussion, requirements, and decisions from Step 2; user context, project planning requirements, and verified project-tool commands, outputs, and limitations relevant to the specialist; relevant parent-established observations; the exact command/runtime partition; the selection reason; and the expected output. Pass concise excerpts or summaries rather than an indiscriminate transcript. Record every selected agent and its evidence-based reason in the synthesis. A better-fit installed agent may replace an advisory role only when this command explicitly names it and defines the required output, or authoritative repository or command guidance establishes compatibility with the responsibility, read-only posture, context needs, required output, and workflow handoff. A catalog-only name or description match is supplementary and cannot replace an applicable named Scramjet specialist or exact Mach 12 role.

For a concrete observed command failure, add `scramjet:command-failure-analyst` only when causal tracing is needed beyond the command-set map. The explorer compresses current behavior; the failure analyst traces one actual execution. Do not add agents merely to cover more quality categories. Missing, failed, or malformed required output remains visible as incomplete evidence rather than triggering substitution.

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

Before escalating a question to the user, attempt to answer it from codebase evidence. When the evidence resolves the matter and the decision is not user-owned, state the finding and proceed while exposing material assumptions so the user can redirect. The user's value is correcting wrong assumptions and providing knowledge that is not in the codebase—not confirming answers the agent can safely establish.

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

If the user delegates judgment with "whatever you think is best", state the selected approach, rationale, and material assumptions, then proceed unless a distinct user-owned or authorization decision remains.

## Step 6: Design architecture

Based on the codebase findings and clarified requirements, route architecture by the Step 4 classification while staying within the shared maximum of eight calls:

- For command-only work, load `writing-scramjet-commands`, then use one `scramjet:command-architect`. Ask for the minimum generalized plan, instruction-justification summary, user-alignment map, details deliberately left to runtime judgment, and a second option only when evidence exposes a material user decision.
- For code-only work, retain three `mach12:code-architect` calls, one for each alternative below.
- For mixed work, give one command architect and the necessary code architects disjoint briefs. The parent integrates their results without asking either family to design the other domain.

A replacement installed agent must satisfy the selected role's exact contract; catalog-only similarity is insufficient. Dispatch independent architecture tasks in one parallel batch. If the user provided context, include it in each brief.

For code architecture, the three alternatives are:

- **Smallest sufficient change**: Design the implementation that satisfies the requirements with the smallest change surface. Walk the minimum-sufficient solution ladder before proposing any new abstractions, files, or dependencies. Maximize reuse of existing patterns.
- **Strongest structural design**: Design the implementation prioritizing clear separation of concerns, maintainability, and well-defined abstractions. Still walk the ladder — justify each new component against a lower rung.
- **Alternative trade-off design**: Design an implementation that optimizes for a different axis (such as performance, extensibility, or a constraint the other lenses deprioritized). Walk the ladder and state what this lens deliberately trades away.

Each code option should produce a full implementation blueprint: files to create or modify, component responsibilities, data flow, and a phased build sequence. The command architect instead returns the concise command design and handoffs defined by its provider contract.

Each architect must assess the technical debt its proposal introduces, retains, reduces, or avoids. An evidence-based “none identified” is valid; do not invent debt. Keep this assessment concise and separate from broader risks and trade-offs: technical debt means likely future maintenance, migration, coupling, testing, or operational cost.

After all results return, review the approaches and form your own recommendation based on the issue's scope, the codebase's conventions, and the user's clarified requirements.

For command work, delete unsupported instructions by default. When the architect returns coaching or another unclassified exception, the parent—not the subagent—must present the exact instruction, intended outcome, real examples and limits, why ordinary judgment and accepted reasons are insufficient, lower-cost alternatives, and prompt/workflow cost. State that coaching exceptions should be uncommon and recommend deletion when real evidence is absent. Retain an exception only after explicit informed user approval, and record the exact decision and evidence status in the existing plan Decision Log as `[user-decided]`; general plan acceptance is not exception approval.

Each code lens must state:
- Which ladder rung it sits on and why lower rungs are insufficient.
- What problem it optimizes for.
- What it deliberately does not build.
- What evidence would make this approach inappropriate.
- The technical debt the option it proposes introduces, retains, reduces, or avoids.

For command-only work, present the command architect's recommended generalized plan, details deliberately left to runtime judgment, handoffs, and debt delta. Include a second option only when the architect identified a material user decision.

For code-only work, present the three options in a narrow Markdown table with these concise, parallel columns, in order: **Option**, **Approach**, **Key difference / trade-off**, and **Debt delta**. Use **Option** only for the short lens or option name. In **Approach**, concisely state what the architecture builds, how it works, and what requirement or problem it solves. Reserve **Key difference / trade-off** for comparative benefits, costs, and sacrifices relative to the other options. Define debt deltas against the current implementation: `+` means debt introduced, while `-` means existing debt reduced or removed. These signs indicate direction, not whether an option is good or bad, and an option does not need to contain both. Use `None identified` when the evidence supports no material delta; never invent debt to make rows symmetrical. Omit retained debt that is decision-immaterial or common to the options from the table cells, but state a materially differentiating retained liability explicitly in words rather than adding another symbol. Summarize common material retained debt outside the compact table.

For code options, always present the detailed trade-offs, concrete implementation differences, **your recommendation with reasoning**, common material debt, and whether every current option has unsatisfactory debt implications. Place those details outside the compact table whenever including them would make table cells verbose. Keep detailed blueprint rationale outside the table as needed. The complete code presentation must still include a brief summary of each approach, a trade-offs comparison, and a concise cross-option technical-debt summary identifying material differences and debt common to all options.

A code recommendation must answer only what the selected lens did not already cover:
- Which lens and ladder rung did you select, citing the lens's lower-rung rationale rather than restating it?
- Why is this not bigger than necessary?
- Why is this not too small to satisfy the requirements?
- Which larger abstractions/dependencies/files were rejected, and why?

Do not default to the middle code option without explaining why both the smaller and more structural options are worse for this issue.

Ask the user to choose only when multiple code options remain materially viable, the command architect returned a material second option, the design changes a user-owned product or safety decision, or an instruction exception needs approval. Before asking, compress the relevant evidence, alternatives, consequences, uncertainty, and recommendation so the user does not need to reconstruct the investigation. Do not frame approval as expected workflow progress. Otherwise present the single supported design and proceed unless the user redirects. If the user rejects the design, incorporate their feedback and return a complete coherent replacement rather than a delta.

## Step 7: Ask architecture questions

After the architect work returns, review its output for unresolved architecture questions — aspects of **how to build it** that remain ambiguous or require user preference.

### Self-assessment

Use the architect output to resolve questions before escalating. When one approach clearly fits the codebase conventions and satisfies the requirements, state your finding rather than asking. Questions deferred from Step 5 may already be answered by the architect analysis — check before presenting them.

### Procedure

1. Review questions deferred from Step 5 against the architect outputs. Drop any that the analysis resolved.
2. Identify new architecture questions surfaced by the design work.
3. For remaining questions, follow the Question Quality Format from Step 5. Include relevant architect findings as context.
4. Present your analysis of how the architecture maps to the clarified requirements, even if no questions remain.
5. **If unresolved questions exist, wait for answers before proceeding.** If the architect analysis resolved everything, present your brief summary and proceed to Step 8.

## Step 8: Design test strategy

Before drafting the plan, decide whether the issue needs a deliberate test strategy.

**Dispatch a test or evaluation designer** when the issue is:
- A bug fix (test-first is particularly valuable here)
- A non-trivial feature (new behavior that needs confidence verification)
- A refactor touching critical paths

For command-only work, do not dispatch a test-design specialist. The parent distinguishes deterministic structural checks from real-use evidence and includes only evidence required by the issue or an observed failure; phrase assertions and synthetic model scenarios cannot establish command value. For code-only work, retain `mach12:test-designer`. For mixed work, use one runtime test-designer brief when runtime behavior warrants it, and let the parent cover the command partition directly. The runtime call counts against the shared eight-call maximum.

**Write a lightweight inline test note instead** when:
- The change is non-executable documentation, configuration, or mechanical metadata
- A command edit is trivial and its test need is obvious enough to state in one sentence (e.g., "update the wiring assertion")
- There is no testable runtime or model-interpreted behavior

### Dispatching the subagent

When a runtime designer is selected, pass a synthesized brief containing:
- Issue classification (bug fix / feature / refactor) and problem statement
- The selected runtime architecture from Step 6
- Relevant codebase findings: existing test patterns, related test files, coverage landscape, and verified project-native checks from Step 4

The subagent treats parent-supplied tool evidence as input rather than independently rerunning project tooling. It returns a runtime test strategy with per-test cost/benefit assessments, coverage intent categorization, and -- for bug fixes -- a test-first recommendation.

### Incorporating the output

- Add a `## Test Strategy` section in the plan, placed before the staged breakdown. For runtime work, include the subagent's classification, test-first recommendation, and proposed tests table. For command work, state deterministic structural checks and any separately justified real-use evidence without presenting either as proof of general effectiveness.
- Distribute per-stage verification directives into each stage's description in the staged breakdown.
- For runtime bug fixes where the test-designer recommends test-first, mark the relevant stages with a test-first directive so `issue-implement` knows to write the failing test before the fix.

### Lightweight path

When skipping the subagent, state the test approach inline in the plan (e.g., "Update wiring test; no behavioral tests needed -- prose-only change"). This satisfies the test coverage planning requirement in Step 9 without a full dispatch.

Place applicable project-native checks in the implementation stages where their evidence is needed. Prefer established checks over improvised substitutes, preserve authorization for mutating generators or formatters, and make unavailable required evidence explicit rather than silently replacing it.

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
