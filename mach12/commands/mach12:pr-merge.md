---
description: Merge a PR, delete the feature branch, and optionally create a release
argument-hint: "<pr-number> [context]"
allowed-tools:
  - bash
  - read
  - grep
  - glob
next:
  mode: open
  candidates: []
---

# Merge and Release

You are merging a PR that has passed review and the pre-merge checklist, then optionally creating a release.

**User input:** $ARGUMENTS

This command's next-step policy is `open` with no Mach 12 candidates -- merge is the natural terminus of a feature lifecycle. Project-local or other command sets may declare follow-up commands here (e.g., a `release:announce` step) by extending the wiring in a downstream set. By default, the chain stops after a successful merge.

## Step 1: Parse input

The user's input contains:
- A **PR number** (required)
- Additional **context** or constraints (optional)

Extract the PR number from the input. If the input is ambiguous, ask the user to clarify. If context was provided, note it for use in later steps (e.g., release notes guidance, version tag preference).

## Step 2: Verify readiness

Confirm the PR is ready to merge:

```
gh pr view <pr-number> --json state,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup
```

If there are blocking issues, report them to the user and stop. Do NOT force-merge.

- **Failed CI checks**: suggest running a CI-fix flow to diagnose and address the failures.
- **Merge conflicts**: suggest resolving conflicts manually or rebasing the branch.
- **Missing review approval**: suggest requesting a review.
- **Branch behind main**: when `mergeStateStatus` is `BEHIND`, suggest running `/mach12:pr-pre-merge <pr-number>` to update the branch before merging.

## Step 3: Merge

Merge the PR using the repository's default merge strategy, and delete the remote feature branch in the same command:

```
gh pr merge <pr-number> --delete-branch
```

Then update local default branch:

```
DEFAULT_BRANCH=$(gh repo view --json defaultBranchRef --jq .defaultBranchRef.name)
git checkout "$DEFAULT_BRANCH" && git pull
```

Clean up the local feature branch if it still exists. Get the branch name from the PR:

```
gh pr view <pr-number> --json headRefName --jq .headRefName
```

Then delete the local branch:

```
git branch -d <branch-name-from-above>
```

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

Read recent releases for style consistency:

```
gh release list --limit 5
```

If there are existing releases, read the most recent one for format reference:

```
gh release view <latest-tag>
```

If the user provided context, use it to inform the release draft (e.g., specific tag, highlighted changes, notes style).

Draft a release:
- **Tag**: follow existing tagging convention (e.g., `v1.2.3`, `1.2.3`). If a version bump was done in pre-merge, use that version.
- **Title**: follow existing title convention. If none, use the PR title.
- **Notes**: summarize changes from this PR. Match the style of previous release notes.

Present the draft to the user and ask:

- **Approve**: create the release as drafted.
- **Modify**: edit the release tag, title, or notes.
- **Skip release**: skip release creation after all.

If the user picks "Modify", ask what they want to change, apply the changes, and present the updated draft for approval again. If the user picks "Skip release", skip to Step 6.

After approval, create the release:

```
gh release create <tag> --title "..." --notes "..."
```

## Step 6: Confirm

Report to the user:
- PR merged (with merge commit hash).
- Feature branch deleted.
- Release created (if applicable, with link).
- Current state of the default branch.
