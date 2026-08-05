---
description: Commit, push, and post a progress comment using session context or branch detection
argument-hint: "[context]"
delegate-only: true
allowed-tools:
  - bash
  - read
  - grep
  - glob
  - read_issue
  - read_pr
  - add_issue_comment
  - add_pr_comment
---

# Push

You are finalizing a batch of work: committing changes, pushing to remote, and documenting progress on the associated PR or issue.

<caller-context>
$ARGUMENTS
</caller-context>

This command is delegate-only. The next step belongs to the caller's `next:` declaration -- do not embed routing suggestions in the progress comment or CLI output.

## Step 1: Determine what to commit

When the caller supplies a structured validation-origin provenance payload, validate it before staging, committing, or pushing. Require review and assessment IDs/digests, pre-commit head, selected IDs, remaining staged IDs, ownership groups, and unchanged proof paths/node IDs/digests; require every field to be unambiguous and internally complete. If validation fails, stop before any repository or remote mutation and report the missing or malformed fields.

Run `git status` and `git diff --staged` to understand the current state.

Staging rules:
- If you have context from this session about which files were modified, stage those specific files by name. Do NOT use `git add -A` or `git add .`.
- If files are already staged and the staging looks correct based on session context, proceed with those.
- If it is unclear what should be staged (e.g., this is a fresh session with no prior context), present the untracked and unstaged files to the user and ask for guidance.
- Never stage files that likely contain secrets (`.env`, `credentials.json`, key material, etc.).

## Step 2: Commit

Review recent commit messages for style consistency:

```
git log --oneline -10
```

Generate a commit message that:
- Follows the repository's existing style.
- Summarizes the nature of the changes (new feature, bug fix, refactor, etc.).
- Focuses on the "why" rather than the "what".
- If context was provided above: if it reads like a commit message, use it verbatim; otherwise treat it as guidance.

Create the commit using a HEREDOC for the message to preserve formatting:

```
git commit -m "$(cat <<'EOF'
<commit message>
EOF
)"
```

Do not append model-identity or tooling co-author footers unless the repository's existing commit history demonstrates that convention.

## Step 3: Push

```
git push
```

If no upstream is set, push with `-u` to the current branch name.

## Step 4: Post progress comment

Determine the comment target using this priority order:

### 1. Session context

Check the conversation for signals about what was being worked on. If an earlier command targeted a specific issue or PR, use that as the comment target.

- **Issue-oriented signals** (post on the issue): `mach12:issue-implement`, `mach12:issue-plan`, `mach12:issue-review` invoked with an issue number.
- **PR-oriented signals** (post on the PR): `mach12:pr-review-fix`, `mach12:pr-review`, `mach12:pr-pre-merge` invoked with a PR number.

If session context points to an issue but a PR also exists on the current branch (`gh pr view --json number,url` succeeds), prefer the PR -- it supersedes the issue as the active work context.

### 2. Detection fallback

If session context is ambiguous or unavailable (fresh session, standalone push):

1. **Try PR first:** `gh pr view --json number,url` on the current branch. If a PR exists, comment on it.
2. **Fall back to issue:** if no PR, check the branch name for an issue-number pattern (e.g., `feature/issue-55-*`, `fix/issue-23-*`). If found, comment on that issue.

### 3. Skip gracefully

If neither session context nor detection yields a target, skip commenting and inform the user.

### Comment content

Include `<!-- mach12-progress -->` as the very first line of the comment body (this invisible HTML marker enables reliable identification in future sessions).

Prepare a brief progress comment covering:
- Summary of changes in this batch.
- Commit hash(es) included.
- Notable decisions or deviations from the plan.
- For an ordinary static-review repair, the exact numeric review comment ID supplied by the caller, labeled as the originating review ID.

Preserve an ordinary originating review ID exactly as supplied; it associates the progress artifact with that review cycle but does not constitute validation provenance.

When the caller supplies a structured validation-origin provenance payload, preserve every field and value verbatim in a dedicated `Validation repair provenance` section and append the exact pushed `HEAD` as the predecessor head. Require the payload to include review and assessment IDs/digests, pre-commit head, selected IDs, remaining staged IDs, ownership groups, and unchanged proof paths/node IDs/digests. Do not summarize, reorder, omit, or rewrite these fields; if the payload is incomplete, stop before posting the progress comment and report the push workflow incomplete.

Do not include next-step suggestions in the comment body. The caller's `next:` block surfaces follow-ups -- a duplicate suggestion here would compete with the harness.

Format intentional GitHub relationships in the progress comment so they remain discoverable: same-repository issue or pull-request references use `#N`; cross-repository references use `owner/repo#N` or a canonical URL already obtained from verified GitHub evidence. Artifact-local identifiers use stable labels or plain words—such as `F1`, `S2`, “finding 1,” or “stage 2”—never bare `#N`. Do not introduce closing keywords for ordinary references. Preserve exact review comment IDs and validation provenance fields in their required labeled formats.

Immediately before posting, completely reread the selected target in an earlier assistant tool round: use `read_issue` for an issue or `read_pr` for a PR, continuing every range with the unchanged snapshot. Then pass the exact prepared body to `add_issue_comment` or `add_pr_comment` respectively. Record the verified comment URL and opaque ID.

If publication or exact comment verification fails after the push, preserve whether the result says the mutation may have succeeded, then return an incomplete result to the caller with the exact pushed `HEAD`, the unverified or missing comment state, and recovery instructions to perform a fresh complete parent read and reconcile the exact body and returned identity without recommitting or repushing. Never repost unless that complete read conclusively establishes the prepared body is absent; never blindly retry an ambiguous write. For an ordinary repair, preserve the exact originating review ID supplied by the caller. For a validation-origin repair, preserve the already-validated structured provenance payload verbatim. The top-level caller reports the workflow status. Never retry a commit or push as publication recovery.

## Step 5: Confirm

Report to the user in CLI output:
- What was committed (files and message).
- Where it was pushed.
- Where the progress comment was posted (with URL), or that posting was skipped.

Do not include next-step suggestions in the CLI output. The harness surfaces the caller's declared next-step.
