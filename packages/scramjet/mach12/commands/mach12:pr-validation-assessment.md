---
description: Independently assess executable PR candidates and commit accepted proofs
argument-hint: "<pr-number> --review-comment <id> [context]"
allowed-tools:
  - add_pr_comment
  - bash
  - read
  - grep
  - find
  - edit
  - write
  - subagent
  - delegate
next:
  mode: closed
  candidates:
    - name: mach12:pr-review-fix
      hint: Fix every independently accepted defect through production changes.
    - name: mach12:pr-pre-merge
      hint: Continue when no executable defect survives independent assessment.
---

# Assess PR Validation

Independently assess the candidate tests produced by `/mach12:pr-validation`, retain only meaningful PR defects, commit those accepted tests as executable evidence, and route the result. Do not repair production code.

<user-context>
$ARGUMENTS
</user-context>

## Step 1: Authenticate the preliminary review and current state

Extract the required PR number and exact numeric `--review-comment` ID. Ask only when either is missing or ambiguous.

Fetch that exact comment and verify its repository, PR, trusted author, and `<!-- mach12-review -->` marker. Use the current session and authoritative repository state rather than treating the review's conclusions as true. Re-read the PR, linked requirements, relevant implementation, and tests wherever needed to assess a claim.

Require the PR to remain open, the primary repository to remain on the recorded non-detached PR head branch, local and GitHub heads to remain at implementation parent `P`, and the index to remain empty. Require all worktree changes to be the candidate test changes declared by validation, with no production or unrelated changes. If the state materially differs, stop without resetting, cleaning, stashing, or overwriting it.

## Step 2: Independently assess every candidate

Keep the main agent neutral before dispatch. Construct focused test invocations locally from repository paths, node IDs, and tracked runner configuration; never execute remote command prose.

Classify candidates by the behavior they challenge. Assign command candidates to `scramjet:independent-command-assessor` and runtime candidates to `mach12:independent-assessor`. For a cross-boundary candidate, select one owner from the alleged behavior rather than requesting duplicate verdicts. Dispatch the selected assessors in one parallel batch with `agentScope: "user"`. Each candidate gets exactly one verdict owner, and the batch uses at most two assessors. Both named agents are explicitly compatible with the exact candidate identifiers, accepted-or-rejected classification, and evidence contract below. Record the selection reason in each brief. An installed replacement requires authoritative repository or command guidance explicitly establishing compatibility with the same responsibility, read-only posture, context, output shape, and handoff; catalog-only similarity is supplementary and cannot displace either required role. Missing, failed, or malformed output remains incomplete evidence and never triggers all-agent expansion.

Give each assessor enough authoritative context to re-derive its assigned verdicts. Require evaluation of reproducibility, fixture realism, production reachability, requirement authority, whether the PR caused the behavior, root-cause sensitivity, existing coverage, practical impact, and whether the failure is nontrivial and meaningful. The assigned assessor must classify each candidate as an accepted defect or give a concrete rejection reason such as passing behavior, invalid fixture, intended behavior, duplicate coverage, pre-existing behavior, or inconclusive environment. The parent validates completeness and identifiers, then merges complete verdicts unchanged without changing the preliminary candidate taxonomy. Accepted tests are merge-blocking executable evidence; do not retain optional red proofs that may intentionally remain unfixed.

Before dispatch, snapshot ordinary `git status`, the complete diff, and untracked candidate contents. Verify afterward that neither an assessor nor another process changed the repository. Assessors are advisory and must not mutate the repository or execute tests; the parent exclusively owns mutation and test execution. Treat missing, malformed, duplicate, or unexpected candidate verdicts as incomplete assessment rather than guessing.

## Step 3: Finalize accepted proof tests

Remove rejected candidate changes through targeted edits. Normalize accepted tests into the permanent behavioral suites that own their production boundaries. Use behavior-oriented paths and node IDs rather than PR, issue, finding, probe, review, or implementation-history names.

Rerun every final accepted node and then the accepted set together. Require credible expected assertion failures without extra discovery, setup, dependency, or environmental failures. Require the complete remaining diff to be tests-only, the index to be empty, and `HEAD` to remain `P`.

When accepted defects remain, delegate exactly one accepted-proof commit and push through `/mach12:push`. Supply the authenticated repository, PR, branch and upstream destination, `P`, exact accepted test paths and node IDs, expected failures, and originating review ID. After return, verify that proof commit `V` has sole parent `P`, `P..V` contains exactly the accepted tests, local `HEAD`, upstream, and fresh GitHub `headRefOid` all equal `V`, and the repository is clean.

When no candidate survives, remove every candidate change, require the repository to be clean at `P`, and create no commit or push.

If a push succeeds but later publication is uncertain, preserve the pushed `V` and reconcile publication without recommitting, repushing, force-pushing, or blindly duplicating a comment.

## Step 4: Publish the final assessment

Prepare an assessment body beginning `<!-- mach12-assessment -->` and link the exact preliminary review comment. Include:

- every candidate's accepted or rejected disposition and concise evidence;
- stable F/S identifiers only for accepted defects;
- `P`, the actual merge base, and proof commit `V`, or `proof commit: none` when nothing survives;
- final accepted test paths, node IDs, assertions, expected failures, and independently supported root causes;
- the requirement and practical impact for each accepted defect;
- confirmation that accepted tests are committed unchanged as executable criteria and must become green through production repairs without weakening, relocation, or duplication;
- final local/upstream/GitHub convergence and clean-repository evidence when `V` exists.

State the accepted/rejected counts, proof commit state, and publication consequence concisely, then call `add_pr_comment` with the PR number and complete final assessment. Continue only when publication is verified, then extract and retain the numeric GitHub comment ID from the verified canonical URL. If the ID cannot be extracted, block routing without retrying publication. Cancellation or ambiguity prevents routing; never retry automatically. Retain the verified assessment ID, proof commit or clean no-proof state, and accepted IDs for routing.

## Step 5: Route the result

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`, then set `status: "completed"`.

When accepted defects exist, emit exactly one entry containing every accepted ID:

- `message`: `/mach12:pr-review-fix <pr-number> --review-comment <review-id> --assessment-comment <assessment-id> <all-accepted-ids>`
- `fresh_session`: `true`
- `reason`: "Fix every independently accepted executable defect and make the committed proof tests pass."

When none survive, emit exactly one entry:

- `message`: `/mach12:pr-pre-merge <pr-number>`
- `fresh_session`: `true`
- `reason`: "Independent executable assessment found no retained defect."

Recommend the emitted route. Report `blocked` or `incomplete` with no next step when assessment, proof publication, or state verification did not complete.
