---
description: Merge a PR, delete the feature branch, and optionally create a release
argument-hint: "<pr-number> [context]"
allowed-tools:
  - bash
  - read
  - grep
  - glob
  - delegate
---

# Merge and Release

<user-context>
$ARGUMENTS
</user-context>

## Goals

- Merge only a pull request that authoritative readiness evidence shows is safe to merge, without bypassing required checks or review.
- Bring the local default branch and feature-branch cleanup to a truthful final state after the merge.
- Create a release only when explicitly approved, and report merge, cleanup, and release outcomes independently.

This command intentionally declares no next-step policy. Merge is the natural terminus of a feature lifecycle, so Scramjet pauses after a successful merge. If your process has a post-merge follow-up (e.g., a `release:announce` step), add an explicit next-step policy in your local command set.

## Step 1: Parse input

The user's input contains:
- A **PR number** (required)
- Additional **context** or constraints (optional)

Extract the PR number from the input. If the input is ambiguous, ask the user to clarify. If context was provided, note it for use in later steps (e.g., release notes guidance, version tag preference).

## Step 2: Verify readiness

Read ordinary GitHub readiness immediately before merging:

```
gh pr view <pr-number> --json state,isDraft,mergeable,mergeStateStatus,reviewDecision
```

Require the PR to be open, non-draft, free of requested changes or required review, current with the default branch, conflict-free, and passing its required checks. Use `gh pr checks <pr-number> --required` to distinguish required checks; repositories without required checks may continue. If GitHub still reports mergeability as unknown after one brief reread, report incomplete rather than guessing.

No creator, provenance marker, issue linkage, or custom metadata participates in readiness. Do not offer a force merge, force push, or readiness bypass.

## Step 3: Merge

Record the feature branch name, then merge without a force or readiness bypass:

```
gh pr view <pr-number> --json headRefName --jq .headRefName
gh pr merge <pr-number> --delete-branch
```

Before cleanup or release work, confirm that GitHub reports the PR as merged:

```
gh pr view <pr-number> --json state,mergeCommit
```

If the PR is not confirmed merged, report the result and stop. After confirmation, update the local default branch and delete the local feature branch if it still exists:

```
DEFAULT_BRANCH=$(gh repo view --json defaultBranchRef --jq .defaultBranchRef.name)
git checkout "$DEFAULT_BRANCH"
git pull
git branch -d <branch-name>
```

Report cleanup failures accurately without undoing or obscuring the successful merge.

## Step 4: Ask about a release

If the user provided context about release creation, honor it as guidance for this step:

- **Skip directives** (e.g., "skip release", "no release this time"): skip the release question entirely and proceed directly to Step 6. Report in CLI output: "Skipping release per user request." The user has already declined; re-asking is friction without safety benefit.
- **Release-creating directives** (e.g., "tag as v2.0.0", "highlight the auth changes"): still ask the question below, but frame it to acknowledge the user wants to create a release and present "Create release" as the recommended choice in the question text. Stash the specific details (tag, highlights, notes style) for the Step 5 draft. The yes/no gate is preserved because a release is a substantive action -- do not draft and create one without an explicit confirmation, even if the user named a tag.
- **No release-relevant context**: ask the question below as a neutral yes/no.

Step 5's draft-approval gate is the content-review gate for the release itself -- it always runs when a release is being created, regardless of context.

If no skip directive was given, ask the user whether to create a release:

- **Create release**: create a release for this merge.
- **Skip release**: skip release creation.

If the user picks "Create release", proceed to Step 5. If "Skip release", skip to Step 6.

## Step 5: Create a release (if requested)

Release publication is not supported by `create_issue`, `create_pr`, `add_issue_comment`, or `add_pr_comment`. This separate draft approval and `gh release create` path remains an explicit exception to inline forge publication until a release-publication tool exists.

Read recent releases for style consistency:

```
gh release list --limit 5
```

If there are existing releases, read the most recent one for format reference:

```
gh release view <latest-tag>
```

If the user provided context, use it to inform the release draft (e.g., specific tag, highlighted changes, notes style).

Gather context from the PR, linked issues, and commits:

```
gh pr view <pr-number> --json title,body,closingIssuesReferences,commits
```

For each linked issue in `closingIssuesReferences`, delegate to:

```
/mach12:gh-issue-read <issue-number> --marker mach12-plan
```

This retrieves the issue title, body, and implementation plan. If no `mach12-plan` marker is found for an issue, use just its title and body. If there are no linked issues, continue without — this is not an error.

Draft a release using the PR title/body, linked issue context (including plans when available), and commit headlines alongside the existing style reference:
- **Tag**: follow existing tagging convention (e.g., `v1.2.3`, `1.2.3`). If a version bump was done in pre-merge, use that version.
- **Title**: follow existing title convention. If none, use the PR title.
- **Notes**: summarize changes from this PR, informed by the full gathered context. Match the style of previous release notes.

Present the draft to the user and ask:

- **Approve**: create the release as drafted.
- **Modify**: edit the release tag, title, or notes.
- **Skip release**: skip release creation after all.

If the user picks "Modify", ask what they want to change, apply the changes, and present the updated draft for approval again. If the user picks "Skip release", skip to Step 6.

After approval, create the release:

```
gh release create <tag> --title "..." --notes "..."
```

If creation fails, report that the PR was merged but the release was not created. Do not claim success for an operation that failed.

## Step 6: Confirm

Report the merge, branch cleanup, release, and local default-branch outcomes accurately, distinguishing a successful merge from any later cleanup or release failure.

## Status Reporting

After delivering your answer, report command status by calling `report_scramjet_command_status`; summarize the work you performed in `summary`, then choose the status:

- After a successful merge (and optional release): report `status: "completed"` with a brief summary. Omit `next_steps` entirely — this command has no next-step policy and no chaining occurs.
- If merge readiness checks fail (CI, conflicts, review, or branch freshness): report `status: "blocked"` with a summary of the blocking issues. Omit `next_steps`.
- If the command stopped before completing (user cancelled, unexpected error): report `status: "incomplete"` with a summary. Omit `next_steps`.
