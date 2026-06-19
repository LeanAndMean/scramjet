---
description: Read a GitHub issue's title, body, and all comments; optionally locate an HTML-marker comment
argument-hint: "<issue-number> [--marker <html-marker>]"
allowed-tools:
  - bash
---

<scramjet-command name="mach12:gh-issue-read">

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

## Step 2: Read the issue and comments

Read the title, body, and all comments in one call:

```
gh issue view <issue-number> --json title,body,comments
```

If the call fails (issue not found, authentication error, network), surface the full error to the caller and stop. The caller decides whether the workflow can proceed without the issue.

## Step 3: Locate the marker comment (if requested)

If `--marker <html-marker>` was provided, parse the `comments` array and scan from the **end** (most recent first) for the first comment whose body contains the literal HTML marker `<!-- <html-marker> -->` (e.g., `<!-- mach12-plan -->`).

If the marker is not found, return that fact alongside the issue content -- the caller decides whether absence is fatal or a fallback (e.g., the most recent substantive comment with the expected structure).

## Step 4: Return

Return:
- The issue title and body.
- The full comments array (parsed JSON).
- If `--marker` was requested: the matched comment body and its numeric comment ID (parsed from the comment URL -- the number after `issuecomment-`). If the marker was not found, indicate that.

</scramjet-command>
