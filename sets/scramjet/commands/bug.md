---
description: Report a Scramjet bug or request as an anonymized GitHub issue, shown whole before it is posted
argument-hint: "[what went wrong, or what you want]"
allowed-tools:
  - Read
  - Bash
  - Grep
  - Glob
---

# Report a Scramjet issue

<user-context>
$ARGUMENTS
</user-context>

## Goals

- An issue in `https://github.com/LeanAndMean/scramjet` that Scramjet's maintainers can act on: what happened or is wanted, what was expected, steps to reproduce, Scramjet's version, the platform and the terminal.
- Nothing in it identifies the user or their work.
- The user sees the whole issue before it is posted, and it is posted only with their approval.

The report is about Scramjet itself. Take what happened from the user's words above and, if they are missing or unclear, ask them. Find the version by running the `scramjet` program the Scramjet documentation line of your instructions names with `--version`, the platform from the system, and the terminal from variables that name it (`TERM_PROGRAM`, `WT_SESSION`, `TERM`; a sandbox removes `TMUX`, so its absence says nothing); read only those you need, never the whole environment.

The issue is public, so anonymize it: home directories as `~`, and no user, host, company or colleague names, private repository names, internal URLs or hostnames, or code and file contents from the user's projects. Put nothing from this session (messages, tool output, errors) in it unless the user agrees to the exact excerpt, anonymized like the rest.

Post it with `create_issue`, giving the repository URL above: its approval shows the user the whole issue, and they approve or decline it there. If they decline, ask what to change. If it cannot be posted, show the user the whole draft with `https://github.com/LeanAndMean/scramjet/issues/new`, where they can post it themselves, and say why.

Report `completed` once the issue is posted, or the draft is given to the user or declined by them, with the issue's URL in the summary when there is one.
