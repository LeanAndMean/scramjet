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

You are merging a PR that has passed review and the pre-merge checklist, then optionally creating a release.

<user-context>
$ARGUMENTS
</user-context>

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

Evaluate the response in this safety order:

1. If `state` is not `OPEN`, report blocked and stop.
2. If `isDraft` is `true`, report blocked and stop.
3. If `reviewDecision` is `CHANGES_REQUESTED`, report blocked and stop.
4. If `reviewDecision` is `REVIEW_REQUIRED`, report blocked and stop. Empty or null `reviewDecision` is not blocking by itself.
5. Read required checks with `gh pr checks <pr-number> --required --json name,state,bucket,link`, capturing stdout, stderr, and exit status separately. When stdout is valid check JSON, classify every returned bucket regardless of exit status: `pass` and `skipping` are settled and nonfailing; any `pending`, `fail`, or `cancel` bucket is blocked; any unrecognized bucket is indeterminate, so report incomplete and stop. When stdout is not valid check JSON, a nonzero exit is acceptable only when stderr is the exact no-required-check diagnostic `no required checks reported on the '<branch>' branch`; every other command or parse failure is an execution failure, so report incomplete and stop.
6. Classify `mergeStateStatus` exhaustively: `CLEAN` and `HAS_HOOKS` are ready; `BEHIND` is blocked and should route to `/mach12:pr-pre-merge <pr-number>`; `UNSTABLE`, `BLOCKED`, and `DRAFT` are blocked; `DIRTY` is a confirmed conflict and blocked; `UNKNOWN` is indeterminate.
7. If `mergeable` is `CONFLICTING`, report blocked and stop. If it is `MERGEABLE`, proceed only when the state classification above is ready.
8. If `mergeable` is `UNKNOWN`, `mergeStateStatus` is `UNKNOWN`, or either field has an unrecognized value, wait briefly and perform one bounded reread with the same `gh pr view` command. Proceed only if the reread maps to a determinate ready outcome. If it is still indeterminate, report incomplete and stop.

No creator, provenance marker, issue linkage, or custom metadata participates in readiness. Do not offer a force merge, force push, or readiness bypass.

## Step 3: Merge

After all normal readiness checks pass, record the feature branch name:

```
gh pr view <pr-number> --json headRefName --jq .headRefName
```

Capture stdout, stderr, and exit status. Require the pre-merge `headRefName` lookup to succeed and return one non-empty branch name; otherwise report incomplete and stop without merging. Then merge without a force or readiness bypass, again capturing stdout, stderr, and exit status:

```
gh pr merge <pr-number> --delete-branch
```

After the merge attempt, independently confirm GitHub's authoritative state before any cleanup or release work:

```
gh pr view <pr-number> --json state,mergeCommit
```

If this verification fails or does not report `state: MERGED`, report the merge command result and verification evidence, report incomplete, and stop without checking out, pulling, deleting a local branch, or creating a release. A failed merge command followed by authoritative `MERGED` state means the merge completed despite the CLI error; state that distinction explicitly and continue using the verified merge commit.

After confirmed merge, resolve the default branch, check it out, and pull it as separate gated operations:

```
DEFAULT_BRANCH=$(gh repo view --json defaultBranchRef --jq .defaultBranchRef.name)
git checkout "$DEFAULT_BRANCH"
git pull
```

Stop on the first failure, report that the PR is already merged but local cleanup is incomplete, and report `status: "incomplete"`; do not claim the failed operation succeeded or proceed to release creation. If the feature branch still exists locally, delete the recorded branch with `git branch -d <branch-name>`. Gate that deletion the same way: on failure, preserve the branch, report the merged-but-not-cleaned-up state, report incomplete, and stop.

Verify remote cleanup with `git ls-remote --heads origin <recorded-branch-name>`, capturing stdout, stderr, and exit status separately. A successful empty result confirms deletion. A lookup error or a returned branch means remote cleanup is failed or indeterminate: report that the PR is merged but remote cleanup is incomplete, report `status: "incomplete"`, and stop without claiming deletion or proceeding to release creation. Only ask about a release after local and remote cleanup are both verified.

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

After approval, create the release and capture stdout, stderr, and exit status:

```
gh release create <tag> --title "..." --notes "..."
```

If creation fails, report that the PR merge and cleanup succeeded but release creation did not, include the full error, report incomplete, and stop without claiming a release exists. On success, verify the exact tag with `gh release view <tag> --json url,tagName`. If verification fails or returns a different tag, report the partial-success state and report incomplete. Claim release creation and show its URL only after this verification succeeds.

## Step 6: Confirm

Report only outcomes verified by their preceding commands:
- PR merged (with the authoritative `mergeCommit` hash).
- Remote and local feature-branch cleanup results separately; do not claim deletion when a cleanup command failed.
- Release created (if applicable, with the verified link).
- Current state of the default branch after its checkout and pull succeeded.

If any operation after the authoritative merge confirmation failed, distinguish the successful merge from the failed cleanup or release and report `status: "incomplete"`.

## Status Reporting

After delivering your answer, report command status by calling `report_scramjet_command_status`; summarize the work you performed in `summary`, then choose the status:

- After a successful merge (and optional release): report `status: "completed"` with a brief summary. Omit `next_steps` entirely — this command has no next-step policy and no chaining occurs.
- If merge readiness checks fail (CI, conflicts, review, or branch freshness): report `status: "blocked"` with a summary of the blocking issues. Omit `next_steps`.
- If the command stopped before completing (user cancelled, unexpected error): report `status: "incomplete"` with a summary. Omit `next_steps`.
