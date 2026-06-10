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
---

# Create Pull Request

You are creating a pull request for the current branch, with a structured description that includes a summary, test plan, and issue linkage.

**Context (optional):** $ARGUMENTS

## Step 1: Parse input

The user may provide:
- An **issue number** to link (e.g., `55`)
- An issue number plus **additional context** (e.g., `55 focus on the API changes`)
- **Nothing** -- infer context from the branch and commits.

Extract the issue number if provided. Note any additional context for use when drafting the PR body. If the input is ambiguous (e.g., it's unclear whether a token is an issue number or context), ask the user to clarify.

## Step 2: Gather context

Determine the current branch:

```
git branch --show-current
```

If the output is empty (detached HEAD state), stop and tell the user to create or checkout a branch first.

Determine the default branch of the repository:

```
gh repo view --json defaultBranchRef --jq .defaultBranchRef.name
```

If the current branch is the default branch, stop and tell the user to create a feature branch first.

Read recent commits on this branch vs the default branch:

```
git log <default-branch>..HEAD --oneline
```

Read the diff summary to understand the scope of changes:

```
git diff <default-branch>...HEAD --stat
```

If both the commit log and diff are empty, stop and tell the user there are no changes on this branch relative to the default branch. Suggest checking `git status` for uncommitted work.

For complex changes, read specific modified files and understand the changes in enough detail to write an accurate summary.

**Issue resolution:**

1. If an issue number was provided in $ARGUMENTS, delegate to:

   ```
   /mach12:gh-issue-read <issue-number>
   ```

   The subroutine returns the issue title, body, and full comments stream. If the call fails (issue not found, permission denied, etc.), report the error to the user. Do NOT proceed without the issue when the user explicitly provided an issue number.

   Comment content (implementation plans, decisions, assessment findings, progress notes) should inform the Summary bullets and Test plan when drafting the PR, but should not be copy-pasted verbatim into the PR body.
2. If no issue number was provided, try to infer one from the branch name (e.g., `feature/issue-55-*`, `fix/issue-23-*`, or `55-some-description`). If found, delegate to `/mach12:gh-issue-read <inferred-issue-number>` to read it and its comments.
3. If no issue can be identified, proceed without one.

**Sub-issue detection:**

If an issue was identified, detect any sub-issues so closing keywords can be included for them in the PR body. Skip this block entirely if no issue was identified.

Delegate to:

```
/mach12:gh-sub-issues <issue-number> --with-state
```

The subroutine returns each sub-issue with its number and state, plus which strategy produced the list (`api` or `body-parse`). State is needed for the closing-keywords rule below.

If the sub-issue list came from the body-parse fallback, flag it to the user when presenting the PR draft -- the fallback is less reliable than the API.

## Step 3: Draft PR and get approval

Compose a PR title and body based on the gathered context.

**Title:**
- Short, under 70 characters.
- Imperative form (e.g., "Add validation for bulk solvent inputs").

**Body:**

```
## Summary
- <bullet points summarizing the changes>

## Test plan
- [ ] <bulleted checklist of how to verify the changes>

Fixes #<issue-number>
```

**Closing keywords inclusion rule:**
- **No issue identified:** Omit all closing keywords.
- **Issue without sub-issues:** Include only `Fixes #<issue-number>`.
- **Issue with sub-issues:** Include `Fixes #<issue-number>` for the parent, followed by one `Fixes #<N>` line for each sub-issue. If any sub-issues are already closed, present a note above the `Fixes` lines listing those sub-issues and their closed state (adding `Fixes` for an already-closed issue is harmless but adds noise). When presenting the draft, tell the user they can remove unwanted closing keywords via the "Modify" option (e.g., for already-closed sub-issues, or if this PR only addresses some of the sub-issues). If the sub-issues were detected via Strategy B (body-parse fallback), note this to the user so they can verify the list is correct.

- When referring to numbered items (findings, suggestions, stages) in the body, use plain words like "finding 3" or "suggestion 3" -- not `#<number>` notation, which GitHub auto-links to issues/PRs. (`Fixes #<issue-number>` is an intentional GitHub reference and should be kept as-is.)
- If the user provided additional context in $ARGUMENTS, incorporate it into the summary or test plan as appropriate.

Present the draft title and body to the user and ask:

- **Approve**: create the PR as drafted.
- **Modify**: edit the PR title or body.
- **Cancel**: abort without creating a PR.

If the user picks "Modify", ask what they want to change, apply the changes, and present the updated draft for approval again. If the user picks "Cancel", stop and confirm that no PR was created.

## Step 4: Create the pull request

After user approval, ensure the branch has been pushed to the remote. Check if the branch exists on the remote:

```
git ls-remote --heads origin <branch-name>
```

If the branch does not exist on the remote, push it:

```
git push -u origin <branch-name>
```

If the push fails, report the error and stop.

Then create the PR using `gh`. Use a HEREDOC for the body to ensure correct formatting:

```
gh pr create --title "<approved-title>" --body "$(cat <<'EOF'
<approved-body>
EOF
)"
```

If `gh pr create` fails:
- **PR already exists for this branch**: Report the existing PR URL using `gh pr view`.
- **Permission or authentication errors**: Suggest checking `gh auth status`.
- **Other errors**: Report the full error to the user.

Do NOT proceed to confirmation if PR creation failed.

## Step 5: Confirm PR

Report to the user:
- PR number and URL.
- Linked issue (if any).

When Scramjet asks you to report command status, call `scramjet_command_status` with `status: "completed"` and include a selector-visible `next_steps` entry if the PR is ready for automated review:

- `message`: `/mach12:pr-review <pr-number>`
- `reason`: a brief explanation that the PR is ready for automated review

Set `recommended_next_step` to `0` when you include this entry so Scramjet can route to it automatically.

Leave `next_steps` empty if the user cancelled or the PR should not be reviewed yet. If PR creation failed or you could not finish, report the matching `status` (`blocked` / `waiting_for_user` / `incomplete`) instead of `completed`.
