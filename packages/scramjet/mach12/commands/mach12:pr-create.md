---
description: Create a pull request for the current branch with structured description
argument-hint: "[issue-number] [context]"
allowed-tools:
  - bash
  - read
  - grep
  - glob
  - delegate
next:
  mode: open
  candidates:
    - name: mach12:pr-review
      hint: |
        Pick when the PR is ready for an automated review pass before
        merge consideration. The common path after PR creation.
---

# Create Pull Request

You are creating a pull request for the current branch, with a structured description that includes a summary, test plan, and exact delivery-unit linkage.

<user-context>
$ARGUMENTS
</user-context>

## Step 1: Parse input

The user may provide:
- An **issue number** to link (e.g., `55`)
- An issue number plus **additional context** (e.g., `55 focus on the API changes`)
- **Nothing** -- infer context from the branch and commits.

Extract the issue number if provided. Note any additional context for use when drafting the PR body. If the input is ambiguous (e.g., it is unclear whether a token is an issue number or context), ask the user to clarify.

## Step 2: Gather context and derive linkage

Determine the current and default branches:

```text
git branch --show-current
gh repo view --json defaultBranchRef --jq .defaultBranchRef.name
```

If the current branch is detached or is the default branch, stop and tell the user to create or check out a feature branch.

Read this branch's commits and diff summary:

```text
git log <default-branch>..HEAD --oneline
git diff <default-branch>...HEAD --stat
```

If both are empty, stop and explain that the branch has no changes relative to the default branch. Suggest checking `git status` for uncommitted work. For complex changes, read enough modified files to draft an accurate summary.

Resolve an issue as follows:

1. If the user supplied an issue number, delegate to `/mach12:gh-issue-read <issue-number>`. If that read fails, report the error and stop.
2. Otherwise, infer an issue only from a clear branch-name pattern such as `feature/issue-55-*`, `fix/issue-55-*`, or `55-some-description`, then delegate to `/mach12:gh-issue-read <inferred-issue-number>`.
3. If no issue can be identified, proceed as explicitly unlinked.

Issue comments may inform the summary and test plan but must not be copied verbatim.

When an issue `D` is identified, delegate to:

```text
/mach12:gh-delivery-unit <D>
```

This is the initial creation-mode derivation. Require `verdict: ok` and retain its exact `delivery-unit`, `classification`, sorted `close-set`, and `part-of` result. Any hold for an identified unit stops PR creation. Explain the reconciliation required by the result; never offer to create an unlinked PR as a workaround.

When no issue is identified, select the explicit unlinked identity `Delivery-unit: none`. Do not run issue derivation, and require no closing or `Part of` linkage.

## Step 3: Draft PR and get approval

Compose a title under 70 characters in imperative form and a body using exactly one of these identity forms immediately after the provenance marker.

For a linked PR:

```text
<!-- mach12-pr -->
<!-- mach12-delivery-unit-v1 -->
Delivery-unit: #<D>
## Summary
- <bullets summarizing the changes>

## Test plan
- [ ] <verification checklist>

Fixes #<D>
Fixes #<additional derived close-set member>
Part of #<direct initiative>
```

Emit exactly one standalone `Fixes #N` line for each derived close-set issue: `D` first, then batch members in ascending numeric order. Emit exactly one standalone `Part of #I` line only when the derivation returns a direct initiative; otherwise emit none. Never close an initiative, sibling, removed source, successor, transitive descendant, or dependency-only issue.

For an explicitly unlinked PR:

```text
<!-- mach12-pr -->
<!-- mach12-delivery-unit-v1 -->
Delivery-unit: none
## Summary
- <bullets summarizing the changes>

## Test plan
- [ ] <verification checklist>
```

Explicitly unlinked means zero closing-keyword lines and zero standalone `Part of` lines. Every Mach 12 PR body carries exactly one delivery identity block; omission is not an unlinked representation.

When referring to numbered findings, suggestions, or stages, use plain words rather than `#<number>` notation so GitHub does not create accidental references. Incorporate additional user context into the summary or test plan when relevant.

Present the title and complete body, then ask the user to Approve, Modify, or Cancel.

- **Approve:** continue with the displayed draft.
- **Modify:** ask what to change, apply it, and present the complete draft again.
- **Cancel:** stop without creating a PR.

The user may modify the title, summary, and test plan. Linkage edits are provisional and cannot waive reconciliation: provenance, identity, exact `Fixes` set, and exact `Part of` cardinality remain subject to mandatory final validation. Removing a closer for partial completion is not allowed; incomplete work requires membership revision or splitting through the delivery-unit workflow and a revised plan before creation.

## Step 4: Push and perform the authoritative pre-create check

After approval, determine whether the branch exists remotely:

```text
git ls-remote --heads origin <branch-name>
```

If it does not, push it with `git push -u origin <branch-name>`. If the push fails, report the full error and stop.

After the push and immediately before `gh pr create`, validate the final approved body from fresh state.

For linked identity, delegate again to:

```text
/mach12:gh-delivery-unit <D>
```

Require `verdict: ok`, then compare the fresh result with the final approved body:

- exactly one consecutive provenance and `Delivery-unit: #D` identity block;
- exactly the fresh derived `Fixes` set, with `D` first and remaining members ascending;
- exactly one fresh direct-initiative `Part of` line or zero when none is expected;
- no duplicate, missing, extra, alternative, or malformed linkage.

For `Delivery-unit: none`, require the exact consecutive provenance/identity block, zero closing-keyword lines, and zero standalone `Part of` lines.

This final fresh derivation and exact final-body comparison are authoritative. If derivation holds or the approved body differs, stop before creation, show the observed/expected diff, and require reconciliation or redrafting followed by fresh user approval. Never silently rewrite the approved body and never convert an identified unit to unlinked.

Only after that check succeeds, create the PR using a HEREDOC:

```text
gh pr create --title "<approved-title>" --body "$(cat <<'EOF'
<approved-body>
EOF
)"
```

If creation fails, report the full error. For an existing PR on the branch, report its URL with `gh pr view`; for authentication errors, suggest `gh auth status`. Do not proceed to confirmation.

## Step 5: Verify GitHub interpretation and confirm

After successful creation, resolve the new PR number and delegate to:

```text
/mach12:gh-delivery-unit --pr <pr-number>
```

Require `verdict: ok` for both linked and explicit-none PRs. For linked PRs this confirms the actual interpreted closing set, exact identity and `Part of` relationship, and that the new PR is the sole claimant. For explicit-none PRs it confirms exact identity with zero closing references and zero standalone `Part of` lines.

If verification holds, report that the PR exists but linkage verification failed, include the exact reason and reconciliation guidance, and do not delete or auto-edit the PR. Leave `next_steps` empty; do not recommend `mach12:pr-review`.

Only after verification succeeds, report the PR number, URL, and linked delivery unit or explicit unlinked status.

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`, then set `status: "completed"` and include a selector-visible next step only when post-create verification returned `ok`:

- `message`: `/mach12:pr-review <pr-number>`, `fresh_session`: `true`
- `reason`: the PR's exact delivery linkage was verified and it is ready for automated review

Set `recommended_next_step` to `0` when included. Leave `next_steps` empty if the user cancelled or post-create verification held. If creation failed or work could not finish, report the matching `blocked` or `incomplete` status. If user input is needed, use `get_scramjet_user_input` instead of reporting status.
