---
description: Resolve pull-request CI failures with a lightweight plan, reviewed fixes, and durable PR records
argument-hint: "<pr-number> [context]"
allowed-tools:
  - add_issue_comment
  - add_pr_comment
  - bash
  - read
  - grep
  - edit
  - write
  - subagent
  - delegate
  - get_scramjet_user_input
  - report_scramjet_command_status
---

# Fix PR CI

<user-context>
$ARGUMENTS
</user-context>

## Goals

- Resolve unsuccessful CI for the selected pull request through legitimate, evidence-backed remediation in one session.
- Preserve the agreed plan, pushed-fix progress, and material conclusions on the PR for collaborators and later sessions.
- Return a truthful current-head CI outcome and unresolved work; CI success alone is not merge readiness.

## Investigate and align

Identify the PR number and requested CI outcome. Delegate to read the PR and complete conversation:

```
/mach12:gh-pr-read <pr-number>
```

Establish its current head, unsuccessful checks, and relevant logs or provider evidence. Treat historical diagnoses as evidence to verify against the current result.

Establish a bounded remediation plan from the supported diagnosis and repository authority. Explain the evidence and uncertainty, intended scope, verification, and consequential effects before obtaining user agreement through `get_scramjet_user_input`. Ordinary tactical choices remain yours; material changes to scope, behavior, risk, or authorization require renewed alignment.

Record the agreed plan on the PR using `add_pr_comment`. Explain the publication consequence concisely and put the complete final comment only in the tool arguments. Require verified publication before implementing the plan; never automatically retry an ambiguous write.

## Remediate and publish

Work toward the agreed CI outcome: implement corrections, run relevant project-native checks, review the changed surface proportionately, and address material findings. Use an existing read-only reviewer when a fresh perspective is useful; for material command-surface changes, load `writing-scramjet-commands` and follow its shared review and independent-assessment requirements. The main agent owns mutation, checks, user interaction, and publication.

Delegate each reviewed fix batch to:

```
/mach12:push CI fix: <summary> for PR #<pr-number>
```

Supply the bounded changes and verification context. Push owns staging, commit, push, convergence verification, and the ordinary progress comment, including its declared `add_issue_comment` capability. Consume its verified head and progress-artifact result before correlating later CI evidence with that push. Preserve a successful push when progress publication is incomplete; reconcile the missing record without repeating completed mutation or automatically retrying an ambiguous write.

## Establish the result

Verify CI against the resulting current PR head, correlating checks and provider results with that exact commit. Use evidence-led, progress-aware troubleshooting toward the agreed outcome. Pending checks, absent checks, unavailable logs, or uncorrelated results cannot establish success. Preserve material revised plans and unresolved conclusions on the PR with `add_pr_comment` when collaborators or later sessions need them, rather than recording every tool action.

Return a compact handoff: outcome, verified head, CI results and evidence, fix summary, plan and push/progress references, and unresolved work. Completion requires the requested CI outcome and verified required durable records.

For a **delegated invocation**, return control without calling `report_scramjet_command_status` or ending the caller's lifecycle.

For a **direct invocation**: After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`, then report `status: "completed"` only when the completion requirements above are met. Report `status: "blocked"` for a concrete blocker or `status: "incomplete"` for an unresolved or indeterminate result. Omit `next_steps`; this command is a terminus. When user input is needed, use `get_scramjet_user_input` instead of reporting terminal status.
