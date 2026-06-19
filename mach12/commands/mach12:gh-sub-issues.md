---
description: Detect sub-issues of a GitHub issue using the two-strategy approach (API, then body-parse)
argument-hint: "<issue-number> [--with-state]"
allowed-tools:
  - bash
  - grep
---

<scramjet-command name="mach12:gh-sub-issues">

# Detect Sub-Issues

You are detecting the sub-issues of a GitHub issue. Two strategies, used in order: the API call first, the body-parse fallback only when the API call fails.

<caller-context>
$ARGUMENTS
</caller-context>

This subroutine is `gh`-specific. A future forge-agnostic command set would substitute an equivalent `glab-sub-issues` (or similar); Strategy B is forge-neutral and would survive a swap.

## Step 1: Parse input

Extract:
- The **issue number** (required, first token).
- An optional **`--with-state`** flag. When present, the return includes each sub-issue's state alongside its number (callers that need to decide based on open/closed status request this).

If no issue number is present, return an error to the caller and stop.

## Step 2: Strategy A -- GitHub sub-issues API

Resolve the repository identifier, then query the sub-issues API. The `--jq` filter depends on whether `--with-state` was requested:

```
REPO=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
# Without --with-state:
gh api --paginate repos/$REPO/issues/<issue-number>/sub_issues --jq '.[].number'
# With --with-state:
gh api --paginate repos/$REPO/issues/<issue-number>/sub_issues --jq '.[] | {number, state}'
```

Three outcomes:
- **API succeeds and returns one or more results:** these are the confirmed sub-issues. Return them. Do NOT fall through to Strategy B.
- **API succeeds but returns no results** (empty array): the issue has no sub-issues. Return an empty list. Do NOT fall through to Strategy B -- the API is authoritative when it answers.
- **API fails** (404, permission error, network timeout, etc.): proceed to Strategy B.

## Step 3: Strategy B -- body-parse fallback

This step runs only when Strategy A failed.

Read the issue body (the caller likely already has it -- if not, call `gh issue view <issue-number>` here). Scan the body for `#<number>` references on GitHub task-list lines:

- A line beginning with optional whitespace.
- Followed by a list marker: `-`, `*`, or `+`.
- Followed by a checkbox: `[ ]`, `[x]`, or `[X]`.
- Followed by text containing `#<number>`.

Exclude any `#<number>` preceded by relational keywords: "Related to", "Blocked by", "See also", "Depends on".

Exclude the parent issue number itself.

If `--with-state` was requested, query each matched number's state individually: `gh issue view <N> --json state --jq .state`. If a per-issue state query fails, treat that sub-issue as open.

## Step 4: Return

Return the list of sub-issues (empty list is a valid answer) and which strategy produced it (`"api"` or `"body-parse"`). Each entry includes the issue number; if `--with-state` was requested, each entry also includes the state.

When Strategy B was used, note this in the return -- the caller may want to flag the list to the user as less reliable than the API.

</scramjet-command>
