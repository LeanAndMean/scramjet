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
- Create only an exactly approved release targeted at the confirmed merge commit, under current repository authority, and report every applicable outcome independently.

This command intentionally declares no next-step policy. Merge is the natural terminus of a feature lifecycle, so Scramjet pauses after a successful merge. If a process has a post-merge follow-up, add an explicit next-step policy in the local command set.

## Step 1: Parse input

Extract the required PR number and any additional context or constraints. If the input is ambiguous, ask the user to clarify. Classify only unambiguous release intent: the user may explicitly decline a release, explicitly request one, or leave release creation undecided. Treat tag, title, notes, or emphasis details as draft guidance.

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

Before cleanup or release work, confirm that GitHub reports the PR as merged and retain its full 40-character merge commit SHA as `MERGED_SHA`:

```
gh pr view <pr-number> --json state,mergeCommit
MERGED_SHA=$(gh pr view <pr-number> --json mergeCommit --jq '.mergeCommit.oid')
```

If the PR is not confirmed merged or `mergeCommit.oid` is not a full commit SHA, report the result and stop. After confirmation, update the local default branch and delete the local feature branch if it still exists:

```
DEFAULT_BRANCH=$(gh repo view --json defaultBranchRef --jq .defaultBranchRef.name)
git checkout "$DEFAULT_BRANCH"
git pull
git branch -d <branch-name>
```

Report cleanup failures accurately without undoing or obscuring the successful merge.

## Step 4: Resolve release intent

If the user context explicitly declines a release, skip release work without asking and proceed to Step 6. If it explicitly requests a release, treat that intent as settled and proceed to Step 5 without asking again. Otherwise, give the user the current merge and cleanup facts, then ask whether to create a release:

- **Create release**: proceed to the release authority and exact-draft gate.
- **Skip release**: finish without release creation.

The exact-draft approval in Step 5 remains mandatory whenever a release is requested; it authorizes the concrete payload rather than duplicating the intent decision.

## Step 5: Create a release (if requested)

Release publication is not supported by `create_issue`, `create_pr`, `add_issue_comment`, or `add_pr_comment`. This separate draft approval and `gh release create` path remains an explicit exception to inline forge publication until a release-publication tool exists.

Reacquire all applicable contribution and release authority before drafting by delegating to:

```
/mach12:find-contribution-guidelines
```

Resolve every policy-owned tag, version, target, preflight, and downstream verification requirement from the returned source paths. If authority is absent or leaves a detail unspecified, use current repository evidence and established release style without inventing an ecosystem-specific gate. Repositories without applicable release authority retain generic release behavior.

Read recent releases for style consistency and gather the PR, linked-issue, and commit context needed for accurate notes. For linked issues, delegate to `/mach12:gh-issue-read <issue-number> --marker mach12-plan`; continue without linked issues when none exist. User context may choose or modify the optional title and notes, but cannot override policy-owned tag, version, target, preflight, or proof requirements.

Before asking for approval, explain that release creation is immutable and can trigger irreversible, nontransactional publication. Identify the mandatory authority-defined read-only preflight, the consequences of failure or ambiguity, and the downstream proof outcomes that release creation does not itself establish. Present the exact draft with:

- **Tag**: the authority-compliant release tag.
- **Target**: the exact `MERGED_SHA`.
- **Title**: the proposed optional release title.
- **Notes**: the complete proposed release notes.

Present the exact draft and ask:

- **Approve**: authorize exactly the displayed release.
- **Modify**: change the optional title or notes, or resolve an authority-permitted tag choice, then present the complete updated draft again.
- **Skip release**: finish without creating a release.

After exact approval, immediately require the current checkout's `HEAD` to equal `MERGED_SHA`, run the authority-defined read-only preflight against `MERGED_SHA`, then reread that the intended remote tag and GitHub release are still absent. Only after every check succeeds, create the approved release with its explicit target:

```
gh release create <tag> --target "$MERGED_SHA" --title "..." --notes "..."
```

A failed or ambiguous preflight, conflict check, or release creation must stop release work without retry while preserving the successful merge fact. After verified release creation, observe each authority-defined downstream outcome within its stated bounds. Never infer downstream publication, provenance, or installation success from GitHub release creation.

## Step 6: Confirm

Report the merge, local and remote branch cleanup, local default-branch update, GitHub release, publication, provenance, and installation outcomes independently when applicable. Mark non-applicable outcomes explicitly, and distinguish determinate failure from pending, timed-out, ambiguous, or otherwise indeterminate state.

## Status Reporting

After delivering your answer, report command status by calling `report_scramjet_command_status`; summarize the work you performed in `summary`, omit `next_steps`, and choose the status from the complete outcome:

- Report `status: "completed"` only when the merge and every other applicable required outcome succeeded or were explicitly non-applicable. When release was skipped, its outcomes are non-applicable; when release was requested, every authority-defined release outcome must succeed.
- Report `status: "blocked"` for a determinate failed required outcome that needs user or repository action, while preserving any successful merge or cleanup facts.
- Report `status: "incomplete"` for a pending, ambiguous, timed-out, or otherwise indeterminate release state, cancellation, or execution failure that prevents a trustworthy completed or blocked result.
