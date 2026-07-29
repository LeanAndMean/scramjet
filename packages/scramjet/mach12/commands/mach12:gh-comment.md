---
description: Post a comment on a GitHub issue or pull request and capture its URL
argument-hint: "<issue|pr> <number>"
delegate-only: true
allowed-tools:
  - bash
---

# Post Issue or PR Comment

You are posting a comment on either a GitHub issue or pull request. The comment body has already been prepared by the caller and is present in your conversation context.

<caller-context>
$ARGUMENTS
</caller-context>

This subroutine is `gh`-specific. A future forge-agnostic command set would substitute an equivalent `glab-comment` (or similar); the immutable body transport and URL capture stay the same.

## Step 1: Parse input

Extract:
- The **kind** (`issue` or `pr`) -- required, first token.
- The **number** -- required, second token.

If either is absent or the kind is not exactly `issue` or `pr`, return an error to the caller and stop.

## Step 2: Post the comment

Treat the body the caller prepared as immutable. Do not rewrite, normalize, summarize, or otherwise edit it before posting.

A HEREDOC contributes the newline immediately before its delimiter. Before posting, verify that the prepared body is already newline-terminated, so that newline is part of the approved body rather than a transport mutation. If it is not newline-terminated or its final-newline state cannot be verified, return an error without posting so the caller can prepare and, when applicable, reapprove a preservable body.

Choose a HEREDOC delimiter only after confirming that it does not occur as a standalone line anywhere in the prepared body. If it collides, choose and check a different delimiter. Use a quoted delimiter and pass the body through standard input with `--body-file -`:

```
gh <kind> comment <number> --body-file - <<'MACH12_COMMENT_BODY'
<prepared body>
MACH12_COMMENT_BODY
```

Substitute `<kind>` as `issue` or `pr` based on the parsed input. Insert the verified body exactly between the delimiter lines; do not apply any body-shaping guidance after preparation.

If the post fails, surface the full error to the caller. The caller decides whether to retry or surface the failure to the user.

## Step 3: Capture the URL

Retrieve the URL of the just-posted comment, again picking the subcommand by kind:

```
gh <kind> view <number> --json comments --jq '.comments[-1].url'
```

The numeric comment ID is the number after `issuecomment-` in the URL (e.g., if the URL ends with `#issuecomment-1234567890`, the ID is `1234567890`). Note: GitHub uses the `issuecomment-` URL fragment prefix for both issue and PR comments, so the parsing rule is the same for both kinds.

## Step 4: Return

Return the full comment URL and the numeric comment ID to the caller.
