---
description: Run a comprehensive PR review with independent reviewers and post the results as a structured comment
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

<user-context>
$ARGUMENTS
</user-context>

## Goals

- Produce a comprehensive, evidence-grounded review of every relevant command and runtime surface without modifying the pull request.
- Publish one verified structured review whose actionable findings, strengths, scope, and stable identifiers are consumable by independent assessment.
- Hand the exact review artifact to the forced assessment step so no finding is treated as fix authority before independent classification.

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

Extract the PR number. If recognized review aspects were provided, note them as review focus in Step 3. Treat any remaining text as user context. If the input is ambiguous, ask the user to clarify.

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

Collect current CI/check evidence. Identify project-provided tools relevant to the changed artifacts from repository guidance, manifests, adjacent scripts, CI configuration, and established usage; establish each tool's authority; classify its relevance as required verification, advisory analysis, or irrelevant, and its execution effect as non-mutating or mutating generation/formatting. When current CI evidence is unavailable or insufficient, safely run applicable non-mutating checks. Inspect unfamiliar scripts before use; do not install missing tools or run mutating modes without authorization. Record exact commands, outputs, and limitations. Treat failures as evidence rather than automatic root causes, warnings as bounded diagnostics rather than expanded review scope, and clean output as insufficient behavioral proof.

Identify linked issues from explicit relationship forms (`Fixes`, `Closes`, `Resolves`, `Part of`, or `Issue`) and contextually relevant bare `#<number>` references in the PR body. Treat references found only in the conversation as candidates and establish their relevance to the PR before considering them linked; do not treat quoted material, review finding identifiers, or incidental references as links. Deduplicate issue numbers.

Before briefing reviewers, delegate to `/mach12:gh-issue-read <issue-number>` for each linked issue so its current body, complete discussion, and timestamps are available alongside plans and prior reviews in the PR comments. If any linked issue cannot be read completely, surface the failed issue and error, stop before reviewer dispatch, and report the review blocked or incomplete; do not silently continue with reduced authoritative context.

Treat the PR description, comments, linked issues, plans, and prior reviews as point-in-time evidence. Use their timestamps and relevant intervening changes to identify material historical claims, then verify potentially stale claims against the checked-out PR head, current diff, tests, linked-issue evidence, and repository guidance. Preserve still-supported historical intent and decisions; neither age, status, nor recent activity proves current validity or invalidity.

Use the changed files, PR description, linked issues, requested review aspects, and user context to classify the work as command-only, code-only, or mixed. An explicit aspect requests emphasis but does not make an unrelated reviewer necessary.

For command-only changes adding or materially altering instructions, responsibility, handoffs, framing, or user gates, use one `scramjet:command-reviewer`. Use `scramjet:instruction-semantics-analyzer` alone only for narrow analysis or a clarification that adds no procedure, responsibility, or gate; use both only for explicitly disjoint questions. Add `scramjet:command-set-explorer` before review only when a large multi-command set must be compressed; its descriptive map is context, not another source of findings.

For mixed changes, give the one selected command reviewer and code specialists disjoint briefs. Command files, agent definitions, frontmatter, next-step and delegation contracts, tool scopes, prompt artifacts, command-facing documentation, and tests about model interpretation are command surfaces; runtime source and executable implementation tests are code surfaces. Command specialists load the `writing-scramjet-commands` skill as their shared authoring authority.

For code surfaces, retain the bundled Mach 12 lenses proportionally:

- **code**: `mach12:code-reviewer` for general correctness, project conventions, security, and code quality.
- **tests**: `mach12:test-analyzer` when tests changed, behavior changed without corresponding tests, or the user requested `tests` / `all`.
- **comments**: `mach12:comment-analyzer` when ordinary code comments, runtime docs, or user-facing non-command prose changed, or the user requested `comments` / `all`.
- **errors**: `mach12:silent-failure-hunter` when error handling, fallback behavior, subprocess/tool execution, async flows, background work, or user-visible failure modes changed, or the user requested `errors` / `all`.
- **types**: `mach12:type-design-analyzer` when types, schemas, interfaces, config shapes, public APIs, or data models changed, or the user requested `types` / `all`.
- **simplify**: `mach12:code-simplifier` when implementation code would benefit from clarity review or the user requested `simplify` / `all`.
- **completeness**: `mach12:feature-completeness-checker` when code behavior must be reconciled with a linked issue or the user requested `completeness` / `all`.

A better-fit installed agent may replace a named role only when authoritative repository or command guidance establishes compatible responsibility, read-only posture, context needs, output, and handoff. Catalog similarity alone is insufficient, and missing output narrows the review rather than triggering substitution.

Dispatch at most seven finding reviewers across both families, primarily for code-heavy mixed work. Command-only work uses one finding reviewer; an explorer used for context compression does not become another reviewer. If an explorer is needed, run it first, then dispatch the finding reviewers in one parallel call. All subagents are read-only; the parent owns tooling, synthesis, interaction, and publication.

Give each reviewer a focused brief containing the PR and changed surface, task-relevant issue authority and decisions, exact surface partition, selection reason, parent-established observations, verified CI/project-tool evidence and limitations, user context, freshness caveats, expected cited output, and any claimed coaching/exception evidence plus the exact context presented before user approval. Do not ask reviewers to rerun project tools. If a version bump or changelog entry is present before pre-merge, flag it as premature.

After the reviewers return, merge disjoint code and command results into one structured review and preserve source attribution.

Apply these aggregation rules:

- For command surfaces, preserve the selected reviewer's candidate claims, acceptable-reason analysis, user-gate or exception evidence, and uncertainty without inventing additional findings or fix designs. Unapproved coaching, purported approval without adequate context, missing user-owned gates, and ceremonial gates are material defects. Reject a candidate before publication when its required evidence is absent.
- Report only material findings grounded in the changed artifact or linked authority; a possible future edge case is not a finding.
- Treat `No material findings` as a normal successful command review.
- Group findings into Critical, Important, Suggestions, and Strengths.
- Label each Critical and Important finding with a sequential F-prefixed identifier (F1, F2, F3, ...) numbered continuously across both sections.
- Label each Suggestion with a sequential S-prefixed identifier (S1, S2, S3, ...) using a separate counter.
- Use bold prefixes, e.g. `**F1:** Missing null check`, `**S1:** Consider extracting helper`.

Do NOT attempt to fix any issues -- this command is for review only. Fixes happen in a later command.

## Step 4: Post review comment

Prepare the review comment body. It must include:
- `<!-- mach12-review -->` as the very first line of the comment body (this invisible HTML marker enables reliable identification in future sessions).
- The complete review findings (Critical, Important, Suggestions, Strengths), preserving source attribution for each selected reviewer.
- F/S identifiers on every finding -- Critical and Important findings use `F<n>` numbered sequentially across both sections, Suggestions use `S<n>` with a separate counter (e.g., `**F1:** ...`, `**F2:** ...`, `**S1:** ...`).
- Model attribution at the bottom -- use the model attribution from the Model Identity section of your system prompt (e.g., "Reviewed by <model name>").
- A note that this is an automated review.

Format the comment as a well-structured markdown document that can serve as input to a future `/mach12:pr-review-fix` session.

Format intentional GitHub relationships in the review body so they remain discoverable: same-repository issue or pull-request references use `#N`; cross-repository references use `owner/repo#N` or a canonical URL already obtained from verified GitHub evidence. Artifact-local identifiers use stable labels or plain words—such as `F1`, `S2`, “finding 1,” or “stage 2”—never bare `#N`. Do not introduce closing keywords for ordinary references. Preserve exact comment URLs and numeric provenance fields when their stronger format is required.

State the reviewed head, selected reviewer coverage, finding counts, and forced-assessment consequence concisely without repeating the complete review. Call `add_pr_comment` with the PR number and complete final body. Continue only when publication is verified, then extract and retain the numeric GitHub comment ID from the verified canonical URL for the next-step assessment. If the ID cannot be extracted, block the transition without retrying publication. Cancellation or definite no-write prevents the forced transition; ambiguity prohibits automatic retry and requires deliberate reconciliation.

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`, then set `status: "completed"`. This command declares a `forced` next step, so Scramjet runs `mach12:pr-review-assessment` regardless; include a single `next_steps` entry only to pass the runtime context to that forced target:

- `message`: `/mach12:pr-review-assessment <pr-number> --review-comment <comment-id>` (the message must start with the forced target)

If the review could not finish — a blocker or an incomplete turn — report the matching `status` (`blocked` / `incomplete`) instead of `completed`, and the forced target will not run. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.

Do NOT fix any issues in this command. Fixes belong to `/mach12:pr-review-fix`, downstream of the assessment.
