---
description: Tag the release once its changelog section is written
argument-hint: "[version]"
allowed-tools: Read, Bash, add_issue_comment
next:
  mode: open
  candidates:
    - name: release:tag
      hint: another release is ready
---
# Tag the release

## Goals

- The release is tagged, and its changelog section names what it holds.

## Version

<user-context>
$ARGUMENTS
</user-context>

## Tag

Use the `changelog` skill to check the release's section, then tag it with
`git tag` once the user agrees.
