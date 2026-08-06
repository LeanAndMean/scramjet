---
description: Commit, push, and post a progress comment using session context or branch detection
argument-hint: "[context]"
delegate-only: true
allowed-tools:
  - bash
  - read
  - grep
  - delegate
---

# Push

Finalize one bounded batch of work by committing intended files, pushing once, and documenting progress on the associated PR or issue whenever one can be identified.

<caller-context>
$ARGUMENTS
</caller-context>

This command is delegate-only. Routing belongs to the caller.

## Step 1: Establish the bounded change

Run `git status`, inspect staged and unstaged diffs, and use the caller context to identify exactly which files belong to this batch. Never use `git add .` or `git add -A`, and never stage likely secrets.

When the caller supplies an **accepted validation-proof** payload, treat it as a distinct mode. Before repository or remote mutation, require:

- an authenticated open PR, repository, head branch, upstream destination, and matching upstream repository;
- local `HEAD`, upstream, and fresh GitHub `headRefOid` all equal the supplied implementation parent `P`;
- an empty index;
- worktree changes consisting exactly of the supplied accepted test paths;
- a tests-only diff with no production, temporary, unrelated, or secret-bearing content;
- every supplied node discoverable with the expected red assertion result.

Stage only the accepted test paths and require the staged diff to equal the complete worktree diff with no residual changes. If any boundary is unclear or false, stop before committing or pushing and report the observed state. Do not infer missing accepted paths or adapt the proof set.

For ordinary work, stage only the files known from the current session. If ownership is unclear, ask the user rather than guessing.

## Step 2: Commit

Review recent commit messages for repository style and create one concise commit describing why the bounded change exists. Do not add model or tool co-author footers unless established repository history requires them.

In accepted validation-proof mode, create exactly one direct successor of `P`. Before pushing, verify that it has sole parent `P` and that `P..HEAD` is the exact accepted tests-only diff. An expected focused test failure is proof evidence, not commit failure or merge readiness.

For ordinary work, verify the commit contains only the intended batch and leaves no unintended staged changes.

## Step 3: Push and verify

Push once. If ordinary mode has no upstream, set one for the current branch. In accepted validation-proof mode, push the already authenticated upstream destination explicitly; never infer or create another destination after committing.

After push, verify local `HEAD`, upstream, and a fresh GitHub PR head agree and that the index and tracked/untracked worktree are clean. If push or verification is failed or ambiguous, report exact local and remote identities without force-pushing, recommitting, or blindly retrying.

Accepted validation-proof mode returns `P`, proof commit `V`, accepted paths and nodes, and convergence evidence to assessment, then stops. It does not post a progress comment because assessment owns the final artifact.

## Step 4: Post ordinary progress

For ordinary mode, determine the active PR or issue from the caller context, preferring an open PR on the current branch. Exhaust current session context and branch-based detection before concluding no target exists. Skip publication only when neither source supports an associated PR or issue, and say why.

Prepare a concise body beginning `<!-- mach12-progress -->` with the completed change, commit, meaningful decisions, and verification. Preserve an exact originating review ID supplied by a static or executable review-fix caller. Do not include next-step suggestions.

Format intentional GitHub relationships consistently: same-repository issue or pull-request references use `#N`; cross-repository relationships use `owner/repo#N` or an already verified canonical URL. Artifact-local identifiers use stable labels or plain words and never bare `#N`.

Delegate publication to `/mach12:gh-comment pr <number>` or `/mach12:gh-comment issue <number>` as appropriate. If publication fails after a successful push, preserve the pushed commit and return an incomplete result with enough context to reconcile the comment without another commit or push.

## Step 5: Return

Report the committed files and message, pushed destination and verified head, and progress comment URL or the reason publication was skipped. Return control to the caller without proposing workflow routing.
