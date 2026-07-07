---
description: Read a GitHub pull request's title, body, and all comments; optionally locate an HTML-marker comment
argument-hint: "<pr-number> [--marker <html-marker>]"
delegate-only: true
allowed-tools:
  - bash
---

# Read GitHub Pull Request

You are reading a GitHub pull request and optionally locating a specific HTML-marker comment within its thread.

<caller-context>
$ARGUMENTS
</caller-context>

This subroutine is `gh`-specific. A future forge-agnostic command set would substitute an equivalent `glab-pr-read` (or similar); the marker-hunt logic stays the same.

## Step 1: Parse input

Extract:
- The **PR number** (required, first token).
- An optional **`--marker <html-marker>`** flag naming an HTML comment marker to locate (e.g., `--marker mach12-review`, `--marker mach12-assessment`).

If no PR number is present, return an error to the caller and stop.

## Step 2: Read the PR and comments

Read the PR title, body, and all comments in one call:

```
gh pr view <pr-number> --json title,body,comments
```

If the call fails (PR not found, authentication error, network), surface the full error to the caller and stop.

## Step 3: Locate the marker comment (if requested)

If `--marker <html-marker>` was provided, parse the `comments` array and scan from the **end** (most recent first) for the first comment whose body contains the literal HTML marker `<!-- <html-marker> -->`. If multiple comments contain the marker, use the most recent one.

If the marker is not found, return that fact alongside the PR content. The caller decides whether to fall back heuristically (e.g., the last comment with the expected structured format).

## Step 4: Return

Return:
- The PR title and body.
- The full comments array (parsed JSON).
- If `--marker` was requested: the matched comment body and its numeric comment ID (parsed from the comment URL -- the number after `issuecomment-`). If the marker was not found, indicate that.
