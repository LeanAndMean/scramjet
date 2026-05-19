---
description: Assign the current user to one or more GitHub issues, handling existing assignees
argument-hint: "<issue-number> [<issue-number> ...]"
allowed-tools:
  - bash
---

# Assign GitHub Issues

You are assigning the current user to one or more GitHub issues. The subroutine handles three assignment states per issue (already assigned, no assignees, other assignees) and aggregates conflicts into a single bulk prompt at the end.

**Caller input:** $ARGUMENTS

This subroutine is `gh`-specific. A future forge-agnostic command set would substitute an equivalent `glab-assign` (or similar); the three-way decision logic stays the same.

Assignment failures are **non-blocking**. Warn the user in CLI output and continue -- a failed assignment must not block the caller's workflow.

## Step 1: Parse input

Extract one or more **issue numbers** (space-separated). If no issue numbers are present, return an error to the caller and stop.

## Step 2: Resolve the current user

```
gh api user --jq .login
```

Record the login. If the call fails, warn the caller and stop -- the assignment cannot proceed without a target user.

## Step 3: Classify each issue

For each issue number, read its current assignees:

```
gh issue view <issue-number> --json assignees --jq '[.assignees[].login] | join(",")'
```

Classify each issue into one of three buckets:

- **Already assigned to the current user:** skip silently (the user need not be told). This is the expected case when returning for subsequent stages.
- **No assignees:** auto-assign immediately with `gh issue edit <issue-number> --add-assignee @me`. Record the success or failure.
- **Other assignees (not including the current user):** collect into a **conflicting** list along with the existing assignees.

## Step 4: Resolve conflicts

If the conflicting list is empty, skip to Step 5.

If the conflicting list has one or more entries, present a single bulk decision prompt to the user. The same choice applies to every conflicting issue -- callers that pass a parent and its sub-issues in one invocation get one prompt covering all of them, by design. Per-issue decisions are not supported; if a caller needs them, it should call the subroutine separately for each issue.

Show the list of conflicting issues with their existing assignees, then ask:

- **Add me:** add the current user as an additional assignee on every conflicting issue. For each, run `gh issue edit <issue-number> --add-assignee @me`.
- **Skip:** leave every conflicting issue's assignees unchanged.
- **Replace:** remove the existing assignees and assign only the current user on every conflicting issue. For each, run `gh issue edit <issue-number> --remove-assignee <existing-logins> --add-assignee @me`.

Record successes and failures per issue.

## Step 5: Report

Return a summary to the caller listing, for each issue, the resolution (already-assigned, auto-assigned, conflict-add, conflict-skip, conflict-replace) and any failures. The caller decides whether to surface this to the user or continue silently.
