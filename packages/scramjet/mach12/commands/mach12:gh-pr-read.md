---
description: Read a GitHub pull request's title, body, and all top-level PR conversation comments; optionally locate an HTML-marker comment
argument-hint: "<pr-number> [--marker <html-marker>]"
delegate-only: true
allowed-tools:
  - bash
---

# Read GitHub Pull Request

<caller-context>
$ARGUMENTS
</caller-context>

## Goals

- Return the caller an authoritative pull-request snapshot containing the parent metadata and complete chronological top-level conversation.
- When requested, identify the most recent matching marker comment and its numeric ID without weakening completeness checks.
- Make incomplete or malformed history explicit rather than presenting partial evidence as authoritative.

This subroutine is `gh`-specific. A future forge-agnostic command set would substitute an equivalent `glab-pr-read` (or similar); the marker-hunt logic stays the same.

## Step 1: Parse input

Extract:
- The **PR number** (required, first token).
- An optional **`--marker <html-marker>`** flag naming an HTML comment marker to locate (e.g., `--marker mach12-review`, `--marker mach12-assessment`).

If no PR number is present, return an error to the caller and stop.

## Step 2: Read the PR and all top-level PR conversation comments

Resolve the canonical `owner/name` with `gh repo view --json nameWithOwner`. Query the PR through `gh api graphql --paginate` with explicit variables for owner, name, PR number, and `$endCursor`. Request parent `title`, `body`, `createdAt`, `updatedAt`, and:

```graphql
comments(first: 100, after: $endCursor) {
  totalCount
  nodes { databaseId body author { login } authorAssociation createdAt url }
  pageInfo { hasNextPage endCursor }
}
```

The query must declare `$endCursor: String` and pass `pageInfo.hasNextPage` plus `pageInfo.endCursor` so `--paginate` follows every page. Accumulate comment nodes in chronological page order. Verify the accumulated node count exactly equals `totalCount`; reject duplicate database IDs. If pagination stops early, a page is malformed, the count differs, or any command fails, surface the full error and report that authoritative history is incomplete; do not return a partial array as complete.

## Step 3: Locate the marker comment (if requested)

If `--marker <html-marker>` was provided, parse the `comments` array and scan from the **end** (most recent first) for the first comment whose body contains the literal HTML marker `<!-- <html-marker> -->`. If multiple comments contain the marker, use the most recent one.

If the marker is not found, return that fact alongside the PR content. The caller decides whether to fall back heuristically (e.g., the last comment with the expected structured format).

## Step 4: Return

Return:
- The PR title, body, `createdAt`, and `updatedAt`.
- The complete accumulated array of top-level PR conversation comments (parsed JSON), including each comment's `createdAt`, its verified `totalCount`, and confirmation that pagination reached `hasNextPage: false`.
- If `--marker` was requested: the matched comment body and its numeric comment ID (parsed from the comment URL -- the number after `issuecomment-`). If the marker was not found, indicate that.

Treat the returned PR and comments as point-in-time evidence. Callers should consider the timestamps and relevant intervening changes, verify potentially stale material claims against current authoritative context, preserve still-supported historical intent and decisions, and never treat age alone as proof of invalidity.
