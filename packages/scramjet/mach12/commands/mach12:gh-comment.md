---
description: Post a comment on a GitHub issue or pull request and capture its URL
argument-hint: "<issue|pr> <number>"
allowed-tools:
  - bash
---

# Post Issue or PR Comment

You are posting a comment on either a GitHub issue or pull request. The comment body has already been prepared by the caller and is present in your conversation context.

<caller-context>
$ARGUMENTS
</caller-context>

This subroutine is `gh`-specific. A future forge-agnostic command set would substitute an equivalent `glab-comment` (or similar); the body-shaping rules and URL capture stay the same.

## Step 1: Parse input

Extract:
- The **kind** (`issue` or `pr`) -- required, first token.
- The **number** -- required, second token.

If either is absent or the kind is not exactly `issue` or `pr`, return an error to the caller and stop.

## Step 2: Post the comment

Use the body the caller prepared. Use a HEREDOC to preserve formatting and avoid shell quoting issues. Pick the `gh` subcommand by kind:

```
gh <kind> comment <number> --body "$(cat <<'EOF'
<prepared body>
EOF
)"
```

(With `<kind>` substituted as `issue` or `pr` based on the parsed input -- e.g., `gh issue comment 55 --body "..."` or `gh pr comment 108 --body "..."`.)

When referring to numbered items (findings, suggestions, stages) in the body, use plain words like "finding 3" or "stage 2" -- not `#<number>` notation, which GitHub auto-links to issues/PRs.

If the post fails, surface the full error to the caller. The caller decides whether to retry or surface the failure to the user.

## Step 3: Capture the URL

Retrieve the URL of the just-posted comment, again picking the subcommand by kind:

```
gh <kind> view <number> --json comments --jq '.comments[-1].url'
```

The numeric comment ID is the number after `issuecomment-` in the URL (e.g., if the URL ends with `#issuecomment-1234567890`, the ID is `1234567890`). Note: GitHub uses the `issuecomment-` URL fragment prefix for both issue and PR comments, so the parsing rule is the same for both kinds.

## Step 4: Return

Return the full comment URL and the numeric comment ID to the caller.
