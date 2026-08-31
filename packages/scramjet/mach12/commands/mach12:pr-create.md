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

<user-context>
$ARGUMENTS
</user-context>

## Goals

- Publish one verified pull request whose title, summary, test plan, draft state, base, and optional single-issue linkage accurately represent the current branch.
- Ensure the branch is safely synchronized to its intended remote without overwriting divergent work.
- Return the verified pull-request identity and offer review paths only after creation succeeds.

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

## Step 4: Synchronize and publish

Invoking this command authorizes ensuring the branch is available on `origin`; do not ask for another push confirmation. Compare local `HEAD`, the configured upstream when present, and a fresh `origin` branch head, treating branch names as quoted data rather than shell source.

- If the fresh remote head equals local `HEAD`, do not push. If the configured upstream does not already name the matching `origin/<branch-name>` branch—including when it is absent or points elsewhere—set that tracking relationship without changing remote content.
- If the remote branch is absent or strictly behind local `HEAD`, run one ordinary `git push -u origin <branch-name>`.
- If the remote branch is ahead or diverged, or any lookup, push, or verification fails, stop and report the observed identities. Never force-push or guess which side should win.

Record whether this invocation pushed the branch or found it already synchronized. Reverify that the fresh remote head equals local `HEAD` before calling `create_pr`, which requires the current-repository head and base branches to exist remotely.

Reconfirm the current head, default base, linkage, and draft state. State those facts and the public publication consequence concisely without repeating the complete title/body. Call `create_pr` once with the final title, body, current branch as `head`, default branch as `base`, and the chosen `draft` boolean. When effective policy requires approval, the approval card presents the exact payload. Regardless of policy, guarded publication and exact verification apply.

Handle the result precisely:

- **Verified:** capture the canonical PR/MR URL and number and continue.
- **Cancelled:** no PR was created; report whether this invocation pushed the branch or found it already synchronized. If revisions are requested, update and revalidate the payload before a new tool call.
- **Definite no-write failure:** surface the actionable prerequisite failure and do not claim creation.
- **Ambiguous:** creation may have occurred. Do not retry automatically; reconcile the branch's PR/MR deliberately before another publication attempt.

## Step 5: Confirm

After successful creation, report the PR number, URL, and whether the published body links one issue or is unlinked.

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`. Report `status: "incomplete"` if the user cancelled. Reserve `status: "completed"` for a successfully created PR and include these selector-visible next steps in order:

1. `message`: `/mach12:pr-review <pr-number>`, `fresh_session`: `true`; `reason`: the PR was created with its complete verified body and is ready for the recommended automated review.
2. `message`: `/mach12:pr-validation <pr-number>`, `fresh_session`: `true`; `reason`: use the slower, opt-in executable-behavior path when the PR's behavioral risk warrants test-driven regression hunting.

Set `recommended_next_step` to `0`, ordinary PR review. If creation failed or work could not finish, report the matching `blocked` or `incomplete` status. If user input is needed, use `get_scramjet_user_input` instead of reporting status.
