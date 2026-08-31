---
description: Read a GitHub issue and all comments, review the implementation plan, and present findings
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
        Use when Critical or Important findings remain after revision
        and another review pass is likely to surface genuine problems.
    - name: mach12:issue-implement
      hint: |
        Use when the plan is approved and ready for implementation.
---

# Issue Plan Review

<user-context>
$ARGUMENTS
</user-context>

## Goals

- Independently determine whether the current implementation plan is correct, complete, minimal, testable, and executable from the issue's authoritative context.
- Give every finding an evidence-grounded classification and preserve one aggregate judgment across command and runtime responsibilities.
- Leave a verified replacement plan or required decision record when the user chooses one, and route implementation only after the applicable publication gate passes.

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

Classify the plan as command-only, code-only, or mixed before dispatching review evidence:

- **Command-only** plans change executable natural-language surfaces: command or agent Markdown, frontmatter, next-step or delegation contracts, tool scopes, prompt artifacts, command-facing documentation, or tests whose subject is model interpretation. Replace the six mandatory code-oriented evidence lenses with `scramjet:instruction-semantics-analyzer`, `scramjet:command-operability-reviewer`, and `scramjet:command-simplifier`. Add the command-set explorer, architect, context-flow, authority/state, trust, evaluation, failure, or completeness specialist only when a concrete changed surface or requirement makes that responsibility relevant.
- **Code-only** plans retain the six existing Mach 12 exploration lenses: files referenced in the plan, architecture and patterns, gaps, risks and pitfalls, alternative approaches, and test infrastructure.
- **Mixed** plans use both families with disjoint briefs and file/claim partitions; command specialists replace analogous code lenses rather than being added beside the full code suite.

The review evidence plus assessment pass is capped at seven subagent calls across both families. Reserve one assessment call for command-only or code-only work; for mixed work, reserve two when both finding families are materially plausible, leaving at most five evidence calls. Select proportionally rather than using every available specialist. A better-fit installed agent may replace an advisory role only when this command explicitly names it and defines the required output, or authoritative repository or command guidance establishes compatibility with the responsibility, read-only posture, context needs, required output, and workflow handoff. A catalog-only name or description match is supplementary and cannot replace an applicable named Scramjet specialist or exact Mach 12 role.

Record each selected agent and its evidence-based reason in the dispatch brief and synthesis. Missing, failed, or malformed required output remains visible and narrows the conclusion; never silently substitute another agent or broaden to all specialists. Mention omitted roles only when omission materially limits confidence.

For command-plan evidence, treat commands as goal-oriented instructions whose known-effective common-path process can be legitimate value. Treat plans and procedures as provisional: when an assumption fails, permit the smallest safe adaptation that preserves the goal and durable boundaries, asking only when missing information or user judgment prevents progress. Require recurring observed user friction where the same unresolved question repeatedly reaches users before proposing speculative exception branches, guards, checkpoints, or recovery protocols; exact consumer contracts, demonstrated trust boundaries, and explicit user requirements remain independent justifications. A hypothetical, one review concern, one disposable probe, one isolated incident, or a failure from a superseded design does not establish recurrence.

For parallel execution, dispatch all selected evidence tasks in one batch. Every brief must include the task-relevant issue title, body, discussion, requirements, and decisions; the current plan; user context and project review criteria; relevant parent-established observations; the exact command/runtime partition; the selection reason; and the expected cited output. Pass concise excerpts or summaries rather than an indiscriminate transcript. After the tasks complete, read every identified file needed for the review.

## Step 5: Review the plan

If the user provided context in Step 1, weight the review toward the areas they emphasized -- surface findings on those areas even at Suggestions-level severity, and note in Step 7 how the user's focus shaped the findings (e.g., "User emphasized testing strategy; this raised three suggestions in that area that would otherwise be borderline."). Apply this weighting across all axes below; do not let it crowd out coverage of the other axes.

### Respect user-attributed decisions

The plan's `## Decision Log` may tag entries with a source: `[user-decided]` (the user explicitly directed the decision) or `[agent-proposed]` (planner judgment). A `[user-decided]` entry reflects settled user intent -- do **not** raise it on minimality (axis 7), alternative-approach, or preference grounds; the minimum-sufficient-solution ladder does not override an explicit user requirement. A genuine correctness or feasibility defect in a `[user-decided]` entry (it references a nonexistent file, contradicts a hard codebase constraint, or cannot work as described) is still in scope. `[agent-proposed]` entries and untagged entries (legacy plans posted before this convention) get normal scrutiny -- treat untagged as `[agent-proposed]`, never as `[user-decided]`.

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
   Default severity: Suggestions, unless overbuilding creates significant implementation risk or maintenance burden. Do not apply this axis to `[user-decided]` Decision Log entries -- see "Respect user-attributed decisions" above.
8. **Release-preparation exclusion**: Does the plan include version bumps, changelog entries, or release-preparation as implementation stages? Flag as a defect (severity: Important). Implementation-necessary version changes (e.g., updating a dependency version the code requires) are not excluded.

For every command-process finding, identify the outcome served and whether the process is known-effective common-path guidance or speculative exception machinery. For speculative exception machinery, identify the qualifying evidence category and compare no change, deletion, capable-agent judgment, and one outcome-level invariant before proposing detailed instructions. Count instruction volume, context pressure, branches, calls, and tests in the plan's complexity. Omit unsupported speculative machinery rather than demoting it to Suggestions.

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

Before presenting findings to the user, run an independent assessment pass. Give every finding exactly one verdict owner: command-surface findings go to `scramjet:independent-command-assessor`, runtime-code findings go to `mach12:independent-assessor`, and cross-boundary findings go to one explicitly selected owner based on the alleged behavior and controlling responsibility. Use at most two assessors, within the seven-call review evidence plus assessment cap. Do not ask both assessors to classify the same item.

For mixed work, obtain the runtime assessor's verdicts and unnumbered per-finding plan fragments first. Give the command assessor those accepted results only as non-verdict aggregate context; it must not reclassify them or change their fix approaches. The command assessor classifies only its disjoint command findings and owns one combined-system judgment across the merged accepted set. When accepted runtime results exist but no command findings remain, invoke it in aggregate-only mode: it classifies nothing, preserves every runtime classification, fix approach, and fragment, and returns only the combined-system judgment. The parent validates identifiers and mechanically orders the accepted fragments into one downstream plan when revision is chosen, preserving every assessor-owned classification and fix approach.

A replacement assessor must be explicitly compatible with the caller's exact identifiers, taxonomy, output shape, evidence needs, and workflow handoff; catalog-only similarity is supplementary. Missing or malformed required assessment remains visible and blocks a complete classification rather than triggering silent substitution.

The selected assessor owns classification: do **not** pre-classify its assigned findings, pre-judge their validity, or run the classification yourself after dispatch. Provide each assessor with:

- The issue title/body and full comment stream.
- The current implementation plan.
- The project review criteria from Step 3.
- The key codebase evidence relevant to its assigned surface from Step 4.
- Its assigned initial F/S findings from Step 5, exact command/runtime partition, and evidence-based selection reason.

Each brief should instruct the assessor to preserve every F/S identifier, verify each item against the issue, plan, comments, and relevant artifacts, and classify it using the taxonomy below. Require an unnumbered implementation fragment for every Genuine blocker and Genuine issue; allow an optional fragment for each Useful suggestion; and require no fragment for Nitpick, False positive, or Deferred/out of scope. For command findings, treat known-effective common-path process as legitimate value and plans as provisional, permitting safe goal-preserving adaptation when assumptions fail. A contained or low-risk edit is not sufficient evidence of net improvement: reject speculative exception branches, guards, checkpoints, or recovery protocols that lack recurring observed user friction, an exact consumer contract, a demonstrated trust boundary, or an explicit user requirement, and assess the merged instruction, context, branch, call, and test burden before finalizing item verdicts:

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
- A finding that challenges a `[user-decided]` Decision Log entry purely on minimality, alternative-approach, or preference grounds is classified Deferred/out-of-scope (or False positive if it misreads the plan) -- never Critical or Important. A genuine correctness or feasibility defect in a `[user-decided]` entry remains in scope and is classified on its merits.
- Preserve the original F/S identifiers when reclassifying so later discussion can reference stable items.

## Step 7: Present findings and execute decision

Present your review to the user, organized as:

1. **Plan summary**: Brief restatement of what the plan proposes.
2. **Strengths**: What the plan gets right.
3. **Assessment summary**: Counts by classification (e.g., genuine blockers, genuine issues, useful suggestions, nitpicks, false positives, deferred/out-of-scope).
4. **Selection and aggregate evidence**: Selected specialists and evidence-based reasons, material omissions only, the aggregate owner, and the net change in responsibilities, artifacts, and recovery paths.
5. **Issues**: Problems found, classified by severity and labeled with stable identifiers:
   - **Critical**: Genuine blockers that will cause the implementation to fail or produce incorrect results.
   - **Important**: Genuine issues or significant risks that should be addressed before implementation.
   - **Suggestions**: Useful improvements or explicitly deferred/out-of-scope concerns that are not blockers.
6. **Questions**: Any clarifying questions that came up during your review.
7. **Pitfalls for implementation**: Consolidate risk findings from Steps 4 and 5 into concrete, actionable warnings for the implementation session. Draw from the "Risks and pitfalls" exploration lens and the "Risks" assessment axis. Each item should be specific enough that an implementation session can act on it without re-exploring.
8. **Recommendation**: State whether the plan should be approved, revised, discussed further, or abandoned.

Ask the user how they want to proceed:

- **Create revised plan**: dispatch the architect to produce a revised plan addressing the findings.
- **Proceed as-is**: continue with the current plan despite findings.
- **Discuss findings**: explore specific findings in more detail before deciding.
- **Cancel**: stop here without updating or proceeding (a brief audit note will be posted).

If the user picks "Create revised plan", enter the revision loop:

### Revision loop

1. **Architect dispatch.** This single revision-architect call is a separate decision branch and does not count against the completed seven-call review/assessment pass. Use `scramjet:command-architect` for command-only revisions and `mach12:code-architect` for code-only revisions. For mixed revisions, select one architect from the findings' controlling domain and scope its advice to that domain; do not ask it to design the other domain or dispatch a second architect. The parent retains the other-domain evidence and assessed fix approaches for later integration. A replacement installed architect requires established compatibility with the selected domain's revision-advice contract; catalog-only similarity is supplementary.

   Dispatch the selected architect with a brief containing:
   - The issue title, body, requirements, and authoritative decisions from Step 2.
   - The controlling-domain partition and evidence-based reason for selecting this architect.
   - The current implementation plan being revised (original plan on first iteration, or most recent revision on subsequent iterations).
   - The full findings list from Step 5 (with F/S identifiers and current classifications from Step 6), identifying which are Critical, Important, and Suggestions.
   - The raw exploration context from Step 4 — key files, observations, and codebase patterns discovered by the exploration subagents.
   - Any contribution guidelines or project planning requirements from Step 3.
   - The existing plan's `## Pitfalls and Gotchas` section (if present). Instruct the architect to preserve existing pitfalls unless the revision makes them irrelevant, and to add any new pitfalls discovered during review.
   - If this is a subsequent revision iteration, include the prior revised plan and the delta assessment that prompted re-revision.

   Instruct the architect to propose revision advice for its controlling domain that addresses the applicable Critical and Important findings while preserving the strengths identified in Step 7. Suggestions are optional improvements to incorporate where they fit naturally. Treat the proposal as architect evidence, not yet as the post-ready artifact. The parent must integrate that advice with preserved other-domain evidence, assessor-owned fix approaches, and prior decisions into the complete replacement through the plan-comment contract in the next step.

2. **Load and apply the plan-comment contract.** After the architect returns and immediately before producing the final revised artifact, delegate once in this turn to:

   ```
   /mach12:plan-comment-contract revision
   ```

   This loads the canonical artifact policy into the current model context; it does not run an independent formatter. Apply it to the prior plan, classified findings and deltas, architect proposal and exploration evidence, test strategy, project constraints, pitfalls, and decisions. Produce the exact, complete standalone replacement beginning with `<!-- mach12-plan -->`, and run the contract's final self-check. Resolve supported defects before publication; surface genuinely missing or contradictory evidence instead of producing a publication-eligible marker-bearing body.

3. **Delta assessment.** Assess the finalized candidate, not the architect's raw proposal. Perform a lightweight delta assessment (not a full 6-lens re-exploration). For each finding from the original review (referencing stable F/S identifiers) and each N-prefixed item from prior iteration deltas, classify into one of three categories:
   - **Addressed**: The revised plan resolves this finding. State how in one sentence.
   - **Remaining**: The revised plan does not resolve this finding, or only partially addresses it. State what is still missing.
   - **New issue**: The revised plan introduces a concern not present in the original review. Label with N-prefixed identifiers continuing from the highest prior N-number (e.g., if prior delta had N1–N3, new issues start at N4) and classify severity (Critical/Important/Suggestion) using the same criteria as Step 6.

   Additionally, assess **pitfalls completeness**: does the revised plan's `## Pitfalls and Gotchas` section preserve pitfalls from the prior version (unless the corresponding plan aspect was removed) and incorporate any new pitfalls surfaced by the review? Flag dropped pitfalls or missing new ones.

   Precise criteria: A finding is "addressed" only when the revised plan's structure, staging, or approach concretely resolves the concern — not when the plan merely acknowledges it or adds a vague note. A "new issue" is a concern about the revised plan's structure, completeness, or correctness that did not exist in the original plan or any prior iteration's delta — not a restatement of an existing finding under a different framing.

   If any finding is **Remaining** at Critical or Important severity, or any **New issue** is Critical or Important, treat the candidate as invalid and do not call `add_issue_comment`. Present the delta assessment, explain that correction and reassessment are required, and offer only **Revise again** or **Discuss findings**. On **Revise again**, return to the architect dispatch in the next revision turn with the invalid candidate and delta assessment, then load `/mach12:plan-comment-contract revision` once before producing and reassessing a corrected replacement. Repeat this gate until no Critical or Important delta remains. Suggestions stay visible but are optional and do not block publication.

4. **Publish the revision.** Only after the Critical/Important delta gate passes, present the delta assessment and summary outside the post-ready body. State the issue target, revision consequence, and finding-resolution counts concisely without repeating the complete replacement plan. Call `add_issue_comment` with the issue number and exact complete marker-bearing replacement. When effective policy requires approval, the approval card presents the exact payload. Regardless of policy, guarded publication and exact verification apply.

   - **Verified:** retain the canonical comment URL; only this result counts as an updated plan and makes implementation routing eligible.
   - **Cancelled:** retain the candidate in conversation, report that the plan was not updated, and stop without implementation routing.
   - **Definite no-write failure:** surface the actionable failure, report that the plan was not updated, and stop without implementation routing.
   - **Ambiguous:** the replacement may have been posted; preserve previously verified artifacts, do not retry automatically, and block later mutation and routing pending deliberate reconciliation.

   Never treat an unverified replacement as the current plan.

If the user picks "Discuss findings", walk through the specific findings they want to explore, then ask again how to proceed. This step remains active across all discussion iterations until the user picks a terminal option (Create revised plan, Proceed, or Cancel).

In every decision comment authored here, format intentional GitHub relationships so they remain discoverable: same-repository issue or pull-request references use `#N`; cross-repository references use `owner/repo#N` or a canonical URL already obtained from verified GitHub evidence. Artifact-local identifiers use stable labels or plain words—such as `F1`, `S2`, `N3`, “finding 1,” or “stage 2”—never bare `#N`. Do not introduce closing keywords for ordinary references. Preserve exact comment URLs and numeric provenance fields when their stronger format is required.

If the user picks "Proceed as-is" and at least one Critical or Important finding exists, post a decision comment on the issue to record the user's choice. Prepare a body with this shape:
- First line: `<!-- mach12-decisions -->`
- A note that a plan review was conducted and the user chose to proceed without changes
- Each Critical and Important finding on its own line (one sentence each)
- Keep the entire comment body under 15 lines

State the decision consequence concisely, then call `add_issue_comment` with the issue number and exact decision body. When effective policy requires approval, the approval card presents the exact payload. Regardless of policy, guarded publication and exact verification apply. Treat this comment as required before implementation routing: verified publication retains the canonical decision-comment URL and permits routing; cancellation or definite no-write failure reports that the decision was not recorded and stops without routing; ambiguity blocks later mutation and routing pending deliberate reconciliation and must not be retried automatically. Preserve any already verified plan artifact; never republish it because this later decision comment failed.

If the user picks "Proceed as-is" and no Critical or Important finding remains—including a clean review, false positives, nitpicks, or Suggestions only—do NOT post a decision comment; proceeding is the expected path and implementation routing is eligible without publication.

If the user picks "Cancel":

1. Confirm that no changes were made to the plan.
2. Post a lightweight decision comment on the issue. Prepare a body with this shape:
   - First line: `<!-- mach12-decisions -->`
   - A note that a plan review was conducted and the session ended without updating or proceeding
   - Finding counts by severity (e.g., "2 Critical, 1 Important, 3 Suggestions")
   - Keep the entire comment body to 5 lines or fewer

   State the audit consequence concisely, then call `add_issue_comment` with the issue number and exact decision body. When effective policy requires approval, the approval card presents the exact payload. Regardless of policy, guarded publication and exact verification apply. Retain its URL only when verified; report cancellation or definite no-write failure as no audit comment; and treat ambiguity as acceptance unknown, without automatic retry and pending deliberate reconciliation. The user cancelled, so no result from this branch permits workflow routing.

After delivering your answer, call `report_scramjet_command_status` and summarize the work you performed in `summary`. Set `status: "completed"` and include **both** declared candidates in `next_steps` only after the selected path's routing gate passes:

- Always include an entry with `message`: `/mach12:issue-review <issue-number>`, `fresh_session`: `true`, and `reason`: a brief explanation of when another review pass would be warranted.
- Always include an entry with `message`: `/mach12:issue-implement <issue-number> 1`, `fresh_session`: `true`, and `reason`: a brief explanation that the plan is ready to implement.
- Set `recommended_next_step` only after the selected path's publication gate passes: recommend `mach12:issue-review` (index 0) when a verified revised plan still warrants another review; recommend `mach12:issue-implement` (index 1) when a revised plan was verified and addresses all blockers, when a required Proceed-as-is decision comment was verified, or when a no-Critical/Important path required no comment.
- If a required revised-plan or Proceed-as-is publication is cancelled, definitely fails before writing, or remains ambiguous, leave `next_steps` empty and report `status: "incomplete"` or `"blocked"` rather than `"completed"`. Discussion, Cancel, unresolved user decisions, and unclear outcomes likewise emit no routing. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
