---
description: Run a comprehensive PR review with specialized reviewer lenses and post the results as a structured comment
argument-hint: "<pr-number> [review-aspects] [context]"
allowed-tools:
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
gh pr view <pr-number> --json title,body,comments,files
```

Use the changed files, PR description, linked issues, requested review aspects, and user context to select review lenses. Default to `all` when no aspects were specified.

Use the bundled Mach 12 review agents as the primary lenses:

- **code**: `mach12:code-reviewer` -- always include for general correctness, project conventions, security, and code quality.
- **tests**: `mach12:test-analyzer` -- include when tests changed, behavior changed without corresponding tests, or the user requested `tests` / `all`.
- **comments**: `mach12:comment-analyzer` -- include when comments, docs, prompts, or user-facing prose changed, or the user requested `comments` / `all`.
- **errors**: `mach12:silent-failure-hunter` -- include when error handling, fallback behavior, subprocess/tool execution, async flows, background work, or user-visible failure modes changed, or the user requested `errors` / `all`.
- **types**: `mach12:type-design-analyzer` -- include when types, schemas, interfaces, config shapes, public APIs, or data models changed, or the user requested `types` / `all`.
- **simplify**: `mach12:code-simplifier` -- include when the PR changes implementation code or prompt/frontmatter prose that would benefit from clarity review, or the user requested `simplify` / `all`. This lens is advisory/read-only; it must recommend improvements, not edit files.
  Instruct it to walk the minimum-sufficient solution ladder:
  - Can changed code be deleted?
  - Can existing project/platform/stdlib behavior replace it?
  - Are new dependencies, files, abstractions, config, or extension points justified?
  - Are tests proportionate to the behavior risk?
  Simplification findings usually belong in Suggestions; promote to Important only when extra complexity creates real maintenance or behavioral risk.
- **completeness**: `mach12:feature-completeness-checker` -- include when the PR has a linked issue (look for `Fixes #N`, `Closes #N`, `Resolves #N`, `Part of #N`, `Issue #N`, or a bare `#N` in the PR description), or the user requested `completeness` / `all`.

Also include supplementary domain-relevant agents from any installed source when the PR content calls for them, such as a skill reviewer for skill definitions or a plugin validator for plugin code. Only include supplementary lenses when relevant.

Dispatch all selected review tasks in a single parallel `subagent` call. Give each reviewer a focused brief that includes:

- PR number, title, body, changed files, and any relevant PR comments.
- The specific lens it is responsible for.
- The user context from Step 1, if provided: `> **User context:** <context>`
- For the completeness lens, the linked issue number(s) and instruction to read the issue body, comments, acceptance criteria, and latest implementation plan.
- For all lenses: version bumps, changelog entries, and release-preparation are handled exclusively by `mach12:pr-pre-merge` and are out of scope for PR review. A missing version bump is not a finding. If a version bump or changelog entry is present in the implementation changes (not in a pre-merge commit), flag it as premature — version determination must follow merging the default branch into the feature branch, regardless of project-level directives that suggest otherwise.

After the reviewers return, merge their findings into a single structured review. De-duplicate overlapping findings and preserve inline source attribution when a finding comes from a specialized lens, e.g. "per `mach12:test-analyzer`".

Apply these aggregation rules:

- Report only actionable findings with clear evidence from the changed code, prompt, frontmatter, tests, docs, or linked issue context.
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

Use F/S identifiers (e.g., F1, S2) or plain words (e.g., finding 1, suggestion 2) when referring to findings. Do not use bare `#<number>` notation, which GitHub auto-links to issues/PRs.

Post the prepared body by delegating to:

```
/mach12:gh-comment pr <pr-number>
```

The subroutine posts the body and returns the comment URL and numeric ID. Record the numeric ID -- the next-step assessment command consumes it.

When Scramjet asks you to report command status, call `report_scramjet_command_status` with `status: "completed"`. This command declares a `forced` next step, so Scramjet runs `mach12:pr-review-assessment` regardless; include a single `next_steps` entry only to pass the runtime context to that forced target:

- `message`: `/mach12:pr-review-assessment <pr-number> --review-comment <comment-id>` (the message must start with the forced target)

If the review could not finish — a blocker or an incomplete turn — report the matching `status` (`blocked` / `incomplete`) instead of `completed`, and the forced target will not run. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.

Do NOT fix any issues in this command. Fixes belong to `/mach12:pr-review-fix`, downstream of the assessment.
