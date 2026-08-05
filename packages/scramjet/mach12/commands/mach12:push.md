---
description: Commit, push, and post a progress comment using session context or branch detection
argument-hint: "[context]"
delegate-only: true
allowed-tools:
  - bash
  - read
  - grep
  - glob
  - delegate
---

# Push

You are finalizing a batch of work: committing changes, pushing to remote, and documenting progress on the associated PR or issue.

<caller-context>
$ARGUMENTS
</caller-context>

This command is delegate-only. The next step belongs to the caller's `next:` declaration -- do not embed routing suggestions in the progress comment or CLI output.

## Step 1: Determine what to commit

Recognize a distinct initial validation-proof payload before applying ordinary or validation-repair staging rules. Require repository and PR identity, head branch, frozen implementation parent, exact proof paths, exhaustive path/node/finding/ownership-group mappings, classifications, consolidated-red evidence, and an intentional-red designation. This mode does not require review or assessment IDs or digests because validation has not published those artifacts yet. Reject an ambiguous payload or one mixed with validation-repair provenance before repository or remote mutation.

For initial validation-proof mode, revalidate that `HEAD` equals the frozen implementation parent, the index is empty, and the dirty path set exactly equals the declared proof paths. Require the complete dirty diff to be tests-only and reject production, temporary, unrelated, or secret-bearing content. In this mode, stage only the exact supplied proof paths—never `git add .` or `git add -A`—then require the staged diff to equal the complete declared tests-only worktree diff with no unstaged residual content. Stop before committing if any identity, mapping, path, diff, or red-evidence guard fails.

When the caller supplies a structured validation-origin provenance payload for repair or declined-proof cleanup, validate it before staging, committing, or pushing. Require review and assessment IDs/digests, frozen implementation parent `P`, original proof commit `V`, authenticated assessment head, exact pre-commit predecessor head, selected IDs, remaining staged IDs, cleanup IDs, ownership groups, unchanged proof paths, node IDs, and content identities, and the exact bounded-operation patch SHA-256; require every field to be unambiguous and internally complete. Revalidate that local `HEAD`, its upstream branch, and a fresh GitHub `headRefOid` equal the predecessor; that `V` descends directly from `P` and remains an ancestor of both the assessment head and predecessor; that the assessment-head-to-predecessor segment is merge-free; and that the index and tracked/untracked worktree were clean before the caller's bounded edits. Require the resulting complete dirty diff to match the declared operation and patch SHA-256 byte-for-byte: production paths only for repair, or exact complete ownership-group test removals only for cleanup. This is a clean authenticated committed state; never reconstruct it from comment-embedded patches. If validation fails, stop before any repository or remote mutation and report the missing or malformed fields.

Recognize a distinct assessment-cleanup payload before ordinary or validation-repair staging. Require repository and PR identity, head branch, frozen implementation parent `P`, original proof commit `V`, exact pre-cleanup head, exact rejected ownership groups and paths, surviving groups and content identities, and authenticated review provenance. Revalidate a clean index before the assessor's targeted removals, require the dirty diff to consist exactly of complete rejected-group test removals, and reject production, temporary, unrelated, secret-bearing, or survivor modifications. Stage only those exact paths, create exactly one cleanup commit, and push exactly once. Verify the commit has one parent equal to the declared pre-cleanup head, its diff is the exact tests-only rejected-group removal, local `HEAD`, upstream, and fresh GitHub `headRefOid` agree, and the worktree is clean. Return `P`, `V`, cleanup commit, removed groups/paths, surviving identities, and remote verification, then stop assessment-cleanup mode immediately. Do not enter the generic staging, commit, push, or progress-comment steps; assessment owns publication. If no group is rejected, this mode must not be invoked.

Run `git status` and `git diff --staged` to understand the current state.

For validation-origin repair or declined-proof cleanup, stage every and only path in the authenticated bounded operation. Require the staged patch SHA-256 to equal the declared complete dirty patch, with no unstaged or untracked residual. These modes bypass selective generic staging.

For ordinary mode, use these staging rules:
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

In initial validation-proof mode, create exactly one commit. Verify it has exactly one parent equal to the frozen implementation parent and that the parent-to-commit diff is tests-only and exactly matches the supplied proof paths. A focused test failure is the intentional proof state, not commit failure or merge readiness.

In validation-origin repair or declined-proof cleanup mode, create exactly one direct successor of the declared predecessor and require its predecessor-to-successor diff to equal the authenticated bounded-operation patch byte-for-byte. Do not commit if staged or residual state differs.

## Step 3: Push

```
git push
```

If no upstream is set, push with `-u` to the current branch name. In initial validation-proof mode, push exactly once, then verify local `HEAD`, the upstream branch, and a fresh GitHub `headRefOid` all equal the proof commit. Require a clean index and tracked/untracked worktree, then return the frozen implementation parent, proof commit, committed paths, and remote verification to validation. If commit, push, or verification fails, return the exact local and remote identities without retrying, force-pushing, or adapting to a changed head.

In validation-origin repair or declined-proof cleanup mode, push exactly once. Before progress publication, require the successor to have the declared predecessor as its sole parent, require its exact diff to equal the bounded-operation patch, and require local `HEAD`, upstream, and a fresh GitHub `headRefOid` to equal that successor with a clean index and tracked/untracked worktree. On any mismatch, return the exact identities and residual state without publishing, retrying, recommitting, or repushing.

## Step 4: Post progress comment

Initial validation-proof mode returns to validation after verified push and does not post a progress comment; validation owns the initial review artifact. Every other mode continues below.

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

When the caller supplies a structured validation-origin repair or cleanup provenance payload, preserve every field and value verbatim in a dedicated `Validation repair provenance` section and append the exact pushed `HEAD` as the successor head. Require the payload to include review and assessment IDs/digests, `P`, original `V`, authenticated assessment head, exact pre-commit predecessor head, selected IDs, remaining staged IDs, cleanup IDs, ownership groups, unchanged proof paths, node IDs, and content identities, and the exact bounded-operation patch SHA-256. Do not duplicate executable proof bodies. Do not summarize, reorder, omit, or rewrite these fields; if the payload is incomplete, stop before posting the progress comment and report the push workflow incomplete.

Do not include next-step suggestions in the comment body. The caller's `next:` block surfaces follow-ups -- a duplicate suggestion here would compete with the harness.

Format intentional GitHub relationships in the progress comment so they remain discoverable: same-repository issue or pull-request references use `#N`; cross-repository references use `owner/repo#N` or a canonical URL already obtained from verified GitHub evidence. Artifact-local identifiers use stable labels or plain words—such as `F1`, `S2`, “finding 1,” or “stage 2”—never bare `#N`. Do not introduce closing keywords for ordinary references. Preserve exact review comment IDs and validation provenance fields in their required labeled formats.

Then delegate to the appropriate posting subroutine:

- **Issue target:**

  ```
  /mach12:gh-comment issue <issue-number>
  ```

- **PR target:**

  ```
  /mach12:gh-comment pr <pr-number>
  ```

The subroutine handles the post and URL capture; the body content you prepared above is what gets posted.

If publication or exact comment verification fails after the push, return an incomplete result to the caller with the exact pushed `HEAD`, the unverified or missing comment state, and recovery instructions to publish and verify one progress comment without recommitting or repushing. For an ordinary repair, preserve the exact originating review ID supplied by the caller. For a validation-origin repair, preserve the already-validated structured provenance payload verbatim. The top-level caller reports the workflow status. Never retry a commit or push as publication recovery.

## Step 5: Confirm

Report to the user in CLI output:
- What was committed (files and message).
- Where it was pushed.
- Where the progress comment was posted (with URL), or that posting was skipped.

Do not include next-step suggestions in the CLI output. The harness surfaces the caller's declared next-step.
