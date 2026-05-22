---
description: Run a comprehensive PR review and post the results as a structured comment
argument-hint: "<pr-number> [context]"
allowed-tools:
  - bash
  - read
  - grep
  - glob
  - delegate
next:
  mode: forced
  target: mach12:pr-review-assessment
---

# Review PR

You are running a comprehensive review of a pull request and posting the results as a structured comment. The post-turn forced next-step runs `/mach12:pr-review-assessment`, which independently assesses each finding before any fixes happen. Review and assessment are deliberately split: this command performs only the review.

**User input:** $ARGUMENTS

## Step 1: Parse input

The user's input typically contains:
- A **PR number** (required)
- Additional **context**, focus areas, or constraints (optional)

Example inputs:
- `108`
- `108 error handling and test coverage`
- `108 focus on the new API endpoints`

Extract the PR number. If context was provided, note it for use in Step 3. If the input is ambiguous, ask the user to clarify.

## Step 2: Check out PR branch

Ensure you are on the correct branch:

```
gh pr checkout <pr-number>
git pull
```

## Step 3: Run the review

Dispatch a comprehensive PR review through parallel reviewer subagents. Each reviewer should target a different lens (code quality, correctness, conventions, test coverage, security, comment accuracy, type design, error-handling adequacy, feature completeness against any linked issue), then merge their findings into a single structured output.

For parallel execution, dispatch all review tasks in a single batch rather than sequentially.

Include the following constraints in the review brief:

- Use review-relevant subagents from any installed source -- include domain-relevant lenses for the content being reviewed (e.g., a skill reviewer when reviewing skill definitions, a plugin validator when reviewing plugin code). Only include supplementary lenses when relevant to the content.
- If the PR has a linked issue (look for issue references like `Fixes #N`, `Closes #N`, `Resolves #N`, `Part of #N`, `Issue #N`, or a bare `#N` in the PR description), include a feature-completeness lens alongside the other reviewers. This lens verifies that the PR fully implements the requirements from the linked issue's acceptance criteria and implementation plan. Do not include this lens if no linked issue is detected.
- Label each Critical and Important finding with a sequential F-prefixed identifier (F1, F2, F3, ...) numbered continuously across both sections. Label each Suggestion with a sequential S-prefixed identifier (S1, S2, S3, ...) using a separate counter. Use bold prefixes (e.g., `**F1:** Missing null check`, `**S1:** Consider extracting helper`).
- If user context was provided in Step 1, append it to each review brief: `> **User context:** <context>`

Do NOT attempt to fix any issues -- this command is for review only. Fixes happen in a later command.

## Step 4: Post review comment

Prepare the review comment body. It must include:
- `<!-- mach12-review -->` as the very first line of the comment body (this invisible HTML marker enables reliable identification in future sessions).
- The complete review findings (Critical, Important, Suggestions, Strengths), including any findings from supplementary lenses merged into the appropriate severity categories with inline source attribution (e.g., "per skill reviewer").
- F/S identifiers on every finding -- Critical and Important findings use `F<n>` numbered sequentially across both sections, Suggestions use `S<n>` with a separate counter (e.g., `**F1:** ...`, `**F2:** ...`, `**S1:** ...`).
- Model attribution at the bottom -- identify yourself by your actual model name (e.g., "Reviewed by <model name>").
- A note that this is an automated review.

Format the comment as a well-structured markdown document that can serve as input to a future `/mach12:pr-review-fix` session.

Use F/S identifiers (e.g., F1, S2) or plain words (e.g., finding 1, suggestion 2) when referring to findings. Do not use bare `#<number>` notation, which GitHub auto-links to issues/PRs.

Post the prepared body by delegating to:

```
/mach12:gh-comment pr <pr-number>
```

The subroutine posts the body and returns the comment URL and numeric ID. Record the numeric ID -- the next-step assessment command consumes it.

When you call `task_complete`, include the forced next-step handoff so the assessment command receives the runtime context:

- `next_step.name`: `mach12:pr-review-assessment`
- `next_step.args`: `<pr-number> --review-comment <comment-id>`
- `next_step.fresh_session`: `false`

Do NOT fix any issues in this command. Fixes belong to `/mach12:pr-review-fix`, downstream of the assessment.
