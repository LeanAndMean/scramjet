---
description: Run a comprehensive PR review with specialized reviewer lenses and post the results as a structured comment
argument-hint: "<pr-number> [review-aspects] [context]"
allowed-tools:
  - add_pr_comment
  - bash
  - read
  - grep
  - glob
  - subagent
  - delegate
next:
  mode: forced
  target: mach12:pr-review-assessment
---

# Review PR

You are running a comprehensive review of a pull request and posting the results as a structured comment. The post-turn forced next-step runs `/mach12:pr-review-assessment`, which independently assesses each finding before any fixes happen. Review and assessment are deliberately split: this command performs only the review.

<user-context>
$ARGUMENTS
</user-context>

## Step 1: Parse input

The user's input typically contains:
- A **PR number** (required)
- Optional **review aspects**: `comments`, `tests`, `errors`, `types`, `code`, `simplify`, `completeness`, or `all`
- Additional **context**, focus areas, or constraints (optional)

Example inputs:
- `108`
- `108 error handling and test coverage`
- `108 focus on the new API endpoints`
- `108 tests errors`
- `108 all`

Extract the PR number. If recognized review aspects were provided, note them for lens selection in Step 3. Treat any remaining text as user context. If the input is ambiguous, ask the user to clarify.

## Step 2: Check out PR branch

Ensure you are on the correct branch:

```
gh pr checkout <pr-number>
git pull
```

## Step 3: Run the review

Determine the changed files and PR context before launching reviewers:

```
git diff --name-only origin/main...HEAD
gh pr view <pr-number> --json title,body,createdAt,updatedAt,comments,files
```

Identify linked issues from explicit relationship forms (`Fixes`, `Closes`, `Resolves`, `Part of`, or `Issue`) and contextually relevant bare `#<number>` references in the PR body. Treat references found only in the conversation as candidates and establish their relevance to the PR before considering them linked; do not treat quoted material, review finding identifiers, or incidental references as links. Deduplicate issue numbers.

Before briefing reviewers, delegate to `/mach12:gh-issue-read <issue-number>` for each linked issue so its current body, complete discussion, and timestamps are available alongside plans and prior reviews in the PR comments. If any linked issue cannot be read completely, surface the failed issue and error, stop before reviewer dispatch, and report the review blocked or incomplete; do not silently continue with reduced authoritative context.

Treat the PR description, comments, linked issues, plans, and prior reviews as point-in-time evidence. Use their timestamps and relevant intervening changes to identify material historical claims, then verify potentially stale claims against the checked-out PR head, current diff, tests, linked-issue evidence, and repository guidance. Preserve still-supported historical intent and decisions; neither age, status, nor recent activity proves current validity or invalidity.

Use the changed files, PR description, linked issues, requested review aspects, and user context to classify the work as command-only, code-only, or mixed, then select every applicable lens rather than defaulting unspecified aspects to `all`. An explicit aspect requests emphasis but does not make an irrelevant lens applicable.

For command-only changes, prompt specialists replace analogous code lenses. Choose proportionally among `scramjet:instruction-semantics-analyzer`, `scramjet:command-operability-reviewer`, and `scramjet:command-simplifier`; add the command-set explorer, architect, context-flow, authority/state, trust, evaluation, or completeness specialist only when concrete changed surfaces require that responsibility. For mixed changes, give prompt and code specialists disjoint briefs. Command files, agent definitions, frontmatter, next-step and delegation contracts, tool scopes, prompt artifacts, command-facing documentation, and tests about model interpretation are command surfaces; runtime source and executable implementation tests are code surfaces.

Review command prose as goal-oriented instructions whose known-effective common-path process can be legitimate value, not as an exhaustive program. Treat plans and procedures as provisional: when an assumption fails, permit the smallest safe adaptation that preserves the goal and durable boundaries, asking only when missing information or user judgment prevents progress. Require recurring observed user friction where the same unresolved question repeatedly reaches users before proposing speculative exception branches, guards, checkpoints, or recovery protocols; exact consumer contracts, demonstrated trust boundaries, and explicit user requirements remain independent justifications. A hypothetical, one review concern, one disposable probe, one isolated incident, or a failure from a superseded design does not establish recurrence.

For code surfaces, retain the bundled Mach 12 lenses proportionally:

- **code**: `mach12:code-reviewer` for general correctness, project conventions, security, and code quality.
- **tests**: `mach12:test-analyzer` when tests changed, behavior changed without corresponding tests, or the user requested `tests` / `all`.
- **comments**: `mach12:comment-analyzer` when ordinary code comments, runtime docs, or user-facing non-command prose changed, or the user requested `comments` / `all`.
- **errors**: `mach12:silent-failure-hunter` when error handling, fallback behavior, subprocess/tool execution, async flows, background work, or user-visible failure modes changed, or the user requested `errors` / `all`.
- **types**: `mach12:type-design-analyzer` when types, schemas, interfaces, config shapes, public APIs, or data models changed, or the user requested `types` / `all`.
- **simplify**: `mach12:code-simplifier` when implementation code would benefit from clarity review or the user requested `simplify` / `all`.
- **completeness**: `mach12:feature-completeness-checker` when code behavior must be reconciled with a linked issue or the user requested `completeness` / `all`.

A better-fit installed agent may replace an analogous role only when this command explicitly names it or authoritative repository or command guidance establishes compatibility with the responsibility, read-only posture, context needs, output shape, and review handoff. A catalog-only name or description match is supplementary evidence, never replacement authority. Supplementary installed lenses remain relevance-gated, and missing required output narrows the review instead of triggering an all-agent fallback.

Dispatch at most a combined maximum of seven reviewers across prompt and code families in one parallel `subagent` call. Preserve required issue coverage, prefer the most decision-relevant lenses when more than seven apply, and record each selected agent's evidence-based reason in its brief and the synthesis. All reviewers are advisory and read-only; the parent exclusively owns repository mutation, test execution, user interaction, and publication.

Give each reviewer a focused brief that includes:

- PR number, title, body, changed files, and any relevant PR comments.
- The task-relevant linked-issue body, discussion, acceptance criteria, latest plan, and authoritative decisions already fetched by the parent; never pass only an issue number to an isolated reviewer.
- The specific lens, exact command/runtime surface partition, and evidence-based selection reason it is responsible for.
- Relevant parent-established observations and the expected cited output.
- The user context from Step 1, if provided: `> **User context:** <context>`
- Relevant artifact timestamps, identified freshness caveats, and which claims were checked against current authority.
- For proposed command process: the outcome it serves, whether it is known-effective common-path guidance or speculative exception machinery, and any safe goal-preserving adaptation when assumptions fail. For speculative exception machinery, include the qualifying evidence category and compare capable-agent judgment, no change, deletion, and one outcome-level invariant.
- For all lenses: if a version bump or changelog entry is present in the diff but was not introduced by a pre-merge commit, flag it as premature.

After the reviewers return, merge their findings into a single structured review. De-duplicate overlapping findings and preserve inline source attribution when a finding comes from a specialized lens, e.g. "per `mach12:test-analyzer`".

Apply these aggregation rules:

- Report only actionable findings with clear evidence from the changed code, prompt, frontmatter, tests, docs, or linked issue context.
- Reject speculative command exception machinery that lacks qualifying evidence; do not preserve it as a Suggestion merely because the proposed edit is small.
- When synthesizing command findings, count added instructions, context pressure, branches, calls, and tests as aggregate complexity.
- Group findings into Critical, Important, Suggestions, and Strengths.
- Label each Critical and Important finding with a sequential F-prefixed identifier (F1, F2, F3, ...) numbered continuously across both sections.
- Label each Suggestion with a sequential S-prefixed identifier (S1, S2, S3, ...) using a separate counter.
- Use bold prefixes, e.g. `**F1:** Missing null check`, `**S1:** Consider extracting helper`.

Do NOT attempt to fix any issues -- this command is for review only. Fixes happen in a later command.

## Step 4: Post review comment

Prepare the review comment body. It must include:
- `<!-- mach12-review -->` as the very first line of the comment body (this invisible HTML marker enables reliable identification in future sessions).
- The complete review findings (Critical, Important, Suggestions, Strengths), including any findings from supplementary lenses merged into the appropriate severity categories with inline source attribution (e.g., "per skill reviewer").
- F/S identifiers on every finding -- Critical and Important findings use `F<n>` numbered sequentially across both sections, Suggestions use `S<n>` with a separate counter (e.g., `**F1:** ...`, `**F2:** ...`, `**S1:** ...`).
- Model attribution at the bottom -- use the model attribution from the Model Identity section of your system prompt (e.g., "Reviewed by <model name>").
- A note that this is an automated review.

Format the comment as a well-structured markdown document that can serve as input to a future `/mach12:pr-review-fix` session.

Format intentional GitHub relationships in the review body so they remain discoverable: same-repository issue or pull-request references use `#N`; cross-repository references use `owner/repo#N` or a canonical URL already obtained from verified GitHub evidence. Artifact-local identifiers use stable labels or plain words—such as `F1`, `S2`, “finding 1,” or “stage 2”—never bare `#N`. Do not introduce closing keywords for ordinary references. Preserve exact comment URLs and numeric provenance fields when their stronger format is required.

State the reviewed head, selected lens coverage, finding counts, and forced-assessment consequence concisely without repeating the complete review. Call `add_pr_comment` with the PR number and complete final body. Continue only when publication is verified, then extract and retain the numeric GitHub comment ID from the verified canonical URL for the next-step assessment. If the ID cannot be extracted, block the transition without retrying publication. Cancellation or definite no-write prevents the forced transition; ambiguity prohibits automatic retry and requires deliberate reconciliation.

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`, then set `status: "completed"`. This command declares a `forced` next step, so Scramjet runs `mach12:pr-review-assessment` regardless; include a single `next_steps` entry only to pass the runtime context to that forced target:

- `message`: `/mach12:pr-review-assessment <pr-number> --review-comment <comment-id>` (the message must start with the forced target)

If the review could not finish — a blocker or an incomplete turn — report the matching `status` (`blocked` / `incomplete`) instead of `completed`, and the forced target will not run. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.

Do NOT fix any issues in this command. Fixes belong to `/mach12:pr-review-fix`, downstream of the assessment.
