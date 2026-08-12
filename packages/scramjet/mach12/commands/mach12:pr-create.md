---
description: Create a pull request for the current branch with structured description
argument-hint: "[issue-number] [context]"
allowed-tools:
  - create_pr
  - bash
  - read
  - grep
  - glob
  - delegate
next:
  mode: open
  candidates:
    - name: mach12:pr-review
      hint: |
        Pick when the PR is ready for an automated review pass before
        merge consideration. The common path after PR creation.
    - name: mach12:pr-validation
      hint: |
        Pick for an opt-in executable-behavior pass when the PR changes
        high-risk lifecycle, concurrency, persistence, or protocol behavior.
---

# Create Pull Request

You are creating a pull request for the current branch, with a structured description that includes a summary, test plan, and optional issue linkage.

<user-context>
$ARGUMENTS
</user-context>

## Step 1: Parse input and resolve linkage

The user may provide an issue number, an issue number plus context, context alone, or nothing.

Resolve issue-linkage ambiguity before constructing a draft:

- An explicit canonical positive issue number is the proposed linked issue.
- If user input could be either an issue number or general context, ask which interpretation is intended.
- If multiple plausible issue candidates exist in the input or branch name, present them and ask the user to select exactly one issue or explicitly decline linkage.
- Do not resolve ambiguity by silently producing an unlinked draft or by deferring the decision to draft approval.
- If no issue was supplied and exactly one issue is unambiguously encoded by a branch pattern such as `feature/issue-55-*`, `fix/issue-55-*`, or `55-some-description`, use that issue.
- If no issue was supplied and the branch yields no candidate, proceed unlinked.

When one issue is selected, delegate to `/mach12:gh-issue-read <issue-number>`. If the read fails, report the error and stop. Issue comments may inform the summary and test plan but must not be copied verbatim. Never expand a parent issue, sub-issues, or other relationships into additional closers.

## Step 2: Gather branch context

Determine the current and default branches:

```text
git branch --show-current
gh repo view --json defaultBranchRef --jq .defaultBranchRef.name
```

If the current branch is detached or is the default branch, stop and tell the user to create or check out a feature branch.

Read this branch's commits and diff summary:

```text
git log <default-branch>..HEAD --oneline
git diff <default-branch>...HEAD --stat
```

If both are empty, stop and explain that the branch has no changes relative to the default branch. Suggest checking `git status` for uncommitted work. For complex changes, read enough modified files to draft an accurate summary.

## Step 3: Draft and validate the PR payload

Compose an imperative title under 70 characters and this body:

```text
## Summary
- <bullets summarizing the changes>

## Test plan
- [ ] <verification checklist>

Fixes #<issue-number>
```

For a linked PR, include exactly one standalone `Fixes #N` line for the selected issue. Zero closing-keyword lines is valid for an unlinked PR. The proposal always has at most one closer. Validate that the body contains zero or one closing-keyword occurrence and that any closer is a standalone line with exactly one canonical issue target. If linkage changes, repeat Step 1's canonical-number validation and `/mach12:gh-issue-read` contract before finalizing the replacement payload.

For ordinary intentional relationships, same-repository issue or pull-request references use `#N`; cross-repository relationships use `owner/repo#N` or an already verified canonical URL. Artifact-local identifiers use stable labels or plain words rather than bare `#N`. Ordinary references must not add closing keywords.

Internally review the complete title/body against the branch diff, selected linkage, and user context. Correct and revalidate it without separately displaying or approving the complete payload in assistant prose.

## Step 4: Push and publish

Push the current branch with `git push -u origin <branch-name>` before opening publication approval because `create_pr` verifies that the current-repository head and base branches exist. If the push fails, report the error and stop; never force-push. A later cancellation may therefore leave a pushed branch but creates no PR.

Reconfirm the current head, default base, linkage, and draft state. State those facts and the public publication consequence concisely without repeating the complete title/body. Call `create_pr` once with the final title, body, current branch as `head`, default branch as `base`, and the chosen `draft` boolean.

Handle the result precisely:

- **Verified:** capture the canonical PR/MR URL and number and continue.
- **Cancelled:** no PR was created; report the already-pushed branch accurately. If revisions are requested, update and revalidate the payload before a new tool call.
- **Definite no-write failure:** surface the actionable prerequisite failure and do not claim creation.
- **Ambiguous:** creation may have occurred. Do not retry automatically; reconcile the branch's PR/MR deliberately before another publication attempt.

## Step 5: Confirm

After successful creation, report the PR number, URL, and whether the published body links one issue or is unlinked.

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`. Report `status: "incomplete"` if the user cancelled. Reserve `status: "completed"` for a successfully created PR and include these selector-visible next steps in order:

1. `message`: `/mach12:pr-review <pr-number>`, `fresh_session`: `true`; `reason`: the PR was created with its complete tool-approved body and is ready for the recommended automated review.
2. `message`: `/mach12:pr-validation <pr-number>`, `fresh_session`: `true`; `reason`: use the slower, opt-in executable-behavior path when the PR's behavioral risk warrants test-driven regression hunting.

Set `recommended_next_step` to `0`, ordinary PR review. If creation failed or work could not finish, report the matching `blocked` or `incomplete` status. If user input is needed, use `get_scramjet_user_input` instead of reporting status.
