---
description: Create a pull request for the current branch with structured description
argument-hint: "[issue-number] [context]"
allowed-tools:
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

## Step 3: Draft PR and get approval

Compose a title under 70 characters in imperative form and this standard GitHub body:

```text
## Summary
- <bullets summarizing the changes>

## Test plan
- [ ] <verification checklist>

Fixes #<issue-number>
```

For a linked PR, include exactly one standalone `Fixes #N` line for the selected issue. For an unlinked PR, omit that line: Zero closing-keyword lines is the valid result when no issue was supplied and none was inferred, or when the user explicitly declined linkage while resolving ambiguity. The initial proposal always has at most one proposed closer.

Format ordinary intentional GitHub relationships so they remain discoverable: same-repository issue or pull-request references use `#N`; cross-repository references use `owner/repo#N` or a canonical URL already obtained from verified GitHub evidence. Artifact-local identifiers use stable labels or plain words—such as `F1`, `S2`, “finding 1,” or “stage 2”—never bare `#N`. Ordinary references must not add closing keywords; exactly one optional standalone `Fixes #N` line remains the only authorized closer. Preserve exact comment URLs and numeric provenance fields when their stronger format is required. Incorporate additional user context into the summary or test plan when relevant.

Before presenting any initial or modified complete body, validate that it contains zero or one closing-keyword occurrence (`Fixes`, `Closes`, or `Resolves`, including accepted GitHub variants). When present, the closer must be a standalone line containing exactly one issue target; reject a line with multiple targets, multiple closing keywords, or any additional closer elsewhere in the body. If invalid, do not present it for approval; explain the invariant and ask the user to choose exactly one linked issue or no linkage.

Present the validated title and complete body, then ask the user to Approve, Modify, or Cancel.

- **Approve:** revalidate the displayed complete body, then continue.
- **Modify:** ask what to change and apply it. If the closing reference was added or changed, treat its canonical positive issue number as a newly selected issue and repeat Step 1's canonical-number validation and `/mach12:gh-issue-read` contract before using it. If that read fails, report the error and do not present or create the revised draft. Then validate the complete body again and only present the complete draft after its selected linkage has been resolved and read. The user may remove linkage without an issue read.
- **Cancel:** stop without creating a PR.

Immediately before creation, validate the final approved body once more. If it no longer contains zero or one valid closing reference, stop and return to complete-body review. Create only the exact validated title and body the user approves.

## Step 4: Push and create

After approval, push the current branch with `git push -u origin <branch-name>`. If the push fails, report the error and stop; never force-push.

Create the PR using a HEREDOC so the complete approved body is preserved exactly:

```text
gh pr create --title "<approved-title>" --body "$(cat <<'EOF'
<approved-body>
EOF
)"
```

If creation fails, report the full error. For an existing PR on the branch, report its URL with `gh pr view`; for authentication errors, suggest `gh auth status`.

## Step 5: Confirm

After successful creation, report the PR number, URL, and whether the approved body links one issue or is unlinked.

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`. Report `status: "incomplete"` if the user cancelled. Reserve `status: "completed"` for a successfully created PR and include these selector-visible next steps in order:

1. `message`: `/mach12:pr-review <pr-number>`, `fresh_session`: `true`; `reason`: the PR was created with its complete approved body and is ready for the recommended automated review.
2. `message`: `/mach12:pr-validation <pr-number>`, `fresh_session`: `true`; `reason`: use the slower, opt-in executable-behavior path when the PR's behavioral risk warrants test-driven regression hunting.

Set `recommended_next_step` to `0`, ordinary PR review. If creation failed or work could not finish, report the matching `blocked` or `incomplete` status. If user input is needed, use `get_scramjet_user_input` instead of reporting status.
