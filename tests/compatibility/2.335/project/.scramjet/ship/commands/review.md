---
description: Review the uncommitted changes and fix what matters
argument-hint: "[what to look at most]"
next:
  mode: closed
  candidates:
    - name: ship:commit
      hint: the changes are ready
    - name: ship:review
      hint: the fixes need another look
---
# Review the changes

## Goals

- The uncommitted changes hold no defect the user would want fixed before committing.

## Focus

<user-context>
$ARGUMENTS
</user-context>

## Review

Ask the `ship:reviewer` agent to review the changes, with any focus given above.
Check each finding against the code yourself: fix those that hold, and tell the
user which you set aside and why.
