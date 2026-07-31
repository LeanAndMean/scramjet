---
description: Read a GitHub issue's title, body, and all comments; optionally locate an HTML-marker comment
argument-hint: "<issue-number> [--marker <html-marker>]"
delegate-only: true
allowed-tools:
  - bash
---

# Read GitHub Issue

You are reading a GitHub issue and optionally locating a specific HTML-marker comment within its thread.

<caller-context>
$ARGUMENTS
</caller-context>

This subroutine is `gh`-specific. A future forge-agnostic command set would substitute an equivalent `glab-issue-read` (or similar); the marker-hunt logic stays the same.

## Step 1: Parse input

Extract:
- The **issue number** (required, first token).
- An optional **`--marker <html-marker>`** flag naming an HTML comment marker to locate (e.g., `--marker mach12-plan`, `--marker mach12-decisions`).

If no issue number is present, return an error to the caller and stop.

## Step 2: Read the issue and complete comment stream

Resolve the canonical `owner/name` with `gh repo view --json nameWithOwner`. Query the issue through `gh api graphql --paginate` with explicit variables for owner, name, issue number, and `$endCursor`. Request `title`, `body`, and:

```graphql
comments(first: 100, after: $endCursor) {
  totalCount
  nodes { databaseId body author { login } authorAssociation createdAt url }
  pageInfo { hasNextPage endCursor }
}
```

The query must declare `$endCursor: String` and pass `pageInfo.hasNextPage` plus `pageInfo.endCursor` so `--paginate` follows every page. Accumulate comment nodes in chronological page order. Verify the accumulated node count exactly equals `totalCount`; reject duplicate database IDs. If pagination stops early, a page is malformed, the count differs, or any command fails, surface the full error and report that authoritative history is incomplete; do not return a partial array as complete. The caller decides whether the workflow can proceed without the issue.

## Step 3: Locate the marker comment (if requested)

If `--marker <html-marker>` was provided, parse the `comments` array and scan from the **end** (most recent first) for the first comment whose body contains the literal HTML marker `<!-- <html-marker> -->` (e.g., `<!-- mach12-plan -->`).

If the marker is not found, return that fact alongside the issue content -- the caller decides whether absence is fatal or a fallback (e.g., the most recent substantive comment with the expected structure).

## Step 4: Return

Return:
- The issue title and body.
- The complete accumulated comments array (parsed JSON), its verified `totalCount`, and confirmation that pagination reached `hasNextPage: false`.
- If `--marker` was requested: the matched comment body and its numeric comment ID (parsed from the comment URL -- the number after `issuecomment-`). If the marker was not found, indicate that.
