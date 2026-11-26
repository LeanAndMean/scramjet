---
description: Write an AGENTS.md that tells coding agents how to work in this project
argument-hint: "[what it should cover]"
allowed-tools:
  - Read
  - Bash
  - Grep
  - Glob
  - Write
  - Edit
---

# Initialize the project's instructions

<user-context>
$ARGUMENTS
</user-context>

## Goals

- The project's directory (the current working directory) holds an `AGENTS.md` that an agent starting a fresh session there can work from: what the project is, how it is laid out, the commands that build, test, lint and run it, and the conventions and pitfalls its code and history show that an agent would not guess. The user's words above say what else it should cover.
- Every fact in it comes from the repository, and every command it names was run here or is one the project's own files define; what could not be established is left out or marked as such, not guessed.
- It is short enough to read whole at each session's start: what an agent needs, not what it finds by reading the code.

Scramjet reads both `AGENTS.md` and `CLAUDE.md`, so a project that already has either in this directory gets no second file: propose the changes that would make the existing one meet the goals, and change it only once the user approves them, since it may hold decisions only they can judge.

Keep credentials, tokens and personal data out of it, also those found in the project's files. Leave it uncommitted, so the user reviews it in their own diff, and the rest of the project as it was: remove what trying its commands left behind.

Report `completed` with what was written or proposed, and what could not be established.
