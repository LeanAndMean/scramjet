---
description: Derive or verify a GitHub PR's canonical Mach 12 delivery-unit linkage
argument-hint: "<issue-number> | --pr <pr-number>"
delegate-only: true
allowed-tools:
  - bash
---

# Derive or Verify Delivery-Unit Linkage

You are deriving a delivery unit's exact PR linkage from fresh GitHub state or verifying that an existing PR exactly matches that derivation. This is a read-only, fail-closed safety boundary: never mutate issues, relationships, comments, or PR bodies.

<caller-context>
$ARGUMENTS
</caller-context>

This subroutine is `gh`-specific. GitHub native issue relationships and GraphQL PR connections are authoritative. Never delegate to an advisory relationship helper or replace a failed native read with body parsing.

## Step 1: Parse exactly one mode

Accept exactly one of these forms:

- **Creation mode:** one canonical positive decimal issue number, the candidate delivery unit `D`.
- **Verification mode:** `--pr` followed by one canonical positive decimal PR number.

The canonical positive-decimal grammar is `[1-9][0-9]*`. Apply it consistently to arguments, `Delivery-unit: #<D>`, and every issue-number field in this contract; zero, signs, and leading zeroes are malformed.

Reject missing, zero, negative, mixed, duplicate, reordered, malformed, or trailing arguments. Return `verdict: hold`, `mode: unknown`, `delivery-unit: unknown`, and reason `invalid-arguments`; do not guess a mode or number.

Resolve the repository once with `gh repo view --json nameWithOwner --jq .nameWithOwner`, split it into owner and name, and use that same repository for every REST and GraphQL read.

## Step 2: Apply fail-closed read rules

Normalize CRLF to LF before parsing text. Retain raw bodies as well as parsed fields. Every required REST page and GraphQL page must succeed and contain the requested shape. Authentication, authorization, rate-limit, network, 5xx, malformed JSON, missing fields, truncation, or pagination failure returns `verdict: hold` with reason `relationship-read-failed` or `pr-read-failed` and the failed read identified.

Use complete paginated reads, including:

```text
repos/<owner>/<repo>/issues/<issue>/comments?per_page=100
repos/<owner>/<repo>/issues/<issue>/sub_issues?per_page=100
repos/<owner>/<repo>/issues/<issue>/parent
repos/<owner>/<repo>/issues/<issue>/dependencies/blocked_by?per_page=100
```

- Follow every REST `Link` page; an HTTP 200 empty child or blocker result is authoritative emptiness.
- For the parent endpoint only, HTTP 404 is authoritative root status only when the parsed response is the normal `No parent issue found` response. Every other 404 or parent error holds.
- Never use task lists, `Part of` prose, or body references as relationship evidence on GitHub.
- Keep each comment's numeric ID, `created_at`, `updated_at`, and body. Marker-bearing edited comments whose `updated_at` differs from `created_at` violate append-only evidence.
- Read issue number, title, body, state, `state_reason`, `created_at`, and `updated_at` for every issue used in a decision.

Query every issue's claiming PRs with cursor-paginated GraphQL `closedByPullRequestsReferences(first: 100, after: $cursor, includeClosedPrs: true)`. Retain each PR's number, state, draft status, merged timestamp, URL, and body, plus:

```text
pageInfo { hasNextPage endCursor }
```

Continue until `hasNextPage` is false. A missing or non-advancing `endCursor` holds. Draft, open, closed-unmerged, and merged PRs all count as claimants.

In verification mode, fetch the PR's number, state, draft status, merged timestamp, URL, and body. Query its actual closing issues through a separate cursor-paginated GraphQL `closingIssuesReferences(first: 100, after: $cursor)` connection with the same `pageInfo { hasNextPage endCursor }` checks. Do not rely on a truncated `gh pr view` display.

## Step 3: Parse verification identity before deriving anything

In verification mode, parse the normalized PR body before reading or guessing a delivery unit. Require exactly one exact `<!-- mach12-pr -->` provenance marker and exactly one identity block immediately after it.

The only linked identity is:

```text
<!-- mach12-pr -->
<!-- mach12-delivery-unit-v1 -->
Delivery-unit: #<D>
```

The only unlinked identity is:

```text
<!-- mach12-pr -->
<!-- mach12-delivery-unit-v1 -->
Delivery-unit: none
```

Require the three lines to be consecutive and exact. Any HTML marker beginning `mach12-delivery-unit` that is duplicated, misplaced, malformed, unversioned, or unsupported holds with reason `malformed-delivery-identity`. Identity without exact provenance also holds.

Absent identity always returns `verdict: hold` with reason `missing-delivery-identity`. This is independent of provenance, title, branch, commits, current closing references, or apparent relation to Mach 12. Never infer `D` from closing references or any mutable clue. Deleting provenance, identity, and closers still holds; deleting identity alone still holds.

For a missing identity on a legacy or external PR, return informed migration guidance:

- Explain that explicit delivery identity is now mandatory and show both exact identity forms above.
- Require the user to inspect the intended delivery scope before repairing or redrafting the body.
- Use `Delivery-unit: #D` only after confirming the intended unit and its expected exact linkage.
- Use `Delivery-unit: none` only after confirming that the PR intentionally closes no issues and has no `Part of` relationship.
- Require the exact provenance marker if absent.
- Never infer identity from existing closers, auto-edit the body, or silently migrate the PR.

If the identity is `Delivery-unit: none`, require all of the following:

- zero actual `closingIssuesReferences`;
- zero standalone closing-keyword lines, including `Fixes`, `Fix`, `Fixed`, `Closes`, `Close`, `Closed`, `Resolves`, `Resolve`, or `Resolved` followed by an issue reference;
- zero standalone `Part of #<number>` lines;
- no malformed, duplicate, or additional identity/provenance marker.

On success, skip issue derivation and return `verdict: ok`, `mode: verification`, `delivery-unit: none`, `classification: unlinked`, `close-set: []`, `part-of: none`, and the empty claimant facts. There is no unlinked representation other than exact `Delivery-unit: none`.

For linked identity, require `<D>` to match the canonical positive-decimal grammar, obtain `D` only from `Delivery-unit: #<D>`, and continue. In creation mode, `D` is the validated argument.

Recursive blocker derivation uses a blocker-only **historical context**. This is internal validation context, not an accepted argument mode. Historical context retains every normal topology, classification, membership-history, plan, identity, exact close-set, `Part of`, disposition, recursive dependency, cycle, and fail-closed read check. It changes only the incompatible live-candidate requirements stated below: the unit and retained members must be closed as completed, and their sole canonical claimant must be merged. Never apply historical context to the top-level creation or verification target.

## Step 4: Read the complete linked-unit graph

Read fresh state for `D`: issue fields, all comments, direct children, parent, direct blockers, and all claiming PRs.

When a parent exists, read its issue fields, all comments needed for classification, direct children, native parent, and blockers. Confirm the parent's direct children include `D` and `D` reports that same parent.

For every direct batch member, read its issue fields, full comment stream, native parent, direct children, direct blockers, and all claiming PRs. Confirm both native directions agree: the batch lists the member and the member reports the batch.

For every direct blocker of `D` or a retained member, read its issue fields, body, comments, parent, children, blockers, dispositions, all claiming PRs, and each claimant's actual closing references. Follow blockers recursively while tracking visited issue IDs. A dependency cycle, unreadable edge, or ambiguous graph holds.

If a membership decision or disposition names a destination or successor needed to validate current state, read that issue's fields, comments, parent, direct children, blockers, and claimant facts too. Do not accept a declared destination or successor without native materialization.

## Step 5: Classify every issue exactly

Inspect all single-line HTML comments beginning `mach12-initiative` or `mach12-batch` and the first nonblank body line.

- **Initiative:** the first nonblank line is exactly `<!-- mach12-initiative-v1 -->`, appearing exactly once, with no other initiative/batch marker variant.
- **Batch:** the first nonblank line is exactly `<!-- mach12-batch-v1 -->`, appearing exactly once, with no other initiative/batch marker variant.
- **Ordinary:** no initiative/batch coordination marker is present and the issue has no direct children. `<!-- mach12-issue -->` is provenance only.

Hold duplicate special markers, misplaced markers, unknown or unversioned variants, malformed marker-prefix comments, ordinary issues with children, and every marker/topology contradiction.

Apply the role matrix:

- A root ordinary issue is a valid single-source delivery-unit candidate.
- A root batch with a non-empty direct set of ordinary sources is a valid multi-source delivery-unit candidate.
- An ordinary or batch directly under an initiative is valid and yields that direct initiative as `part-of`.
- An initiative is never a delivery unit and always holds.
- An ordinary issue directly under a batch is source-only; hold with reason `source-owned-by-batch` and redirect to that batch number.
- A nested initiative, nested batch, initiative under any parent, batch under a non-initiative parent, non-ordinary batch child, or any other combination holds.

Outside historical context, require `D` to be open and not terminally dispositioned. A closed unit, an issue closed as not planned, or any active disposition on `D` cannot receive a PR.

In historical context, require `D` to be closed as completed and to have no active terminal disposition. An open unit, a unit closed for any other reason, or any active disposition is not a delivered blocker.

## Step 6: Validate membership-decision history

Scan every fetched comment for every HTML marker beginning `mach12-membership-decision`. A marker variant that is not the exact v1 marker holds. The exact block is:

```text
<!-- mach12-membership-decision-v1 -->
Action: add|remove|move
Parent: #<number>
Members: #<number>[, #<number> ...]
Before: #<all direct members>|none
After: #<all direct members>|none
Destination: #<number>|none
Destination-before: #<all destination members>|none
Destination-after: #<all destination members>|none
Dependencies-before: #<downstream> blocked-by #<upstream>[, ...]|none
Dependencies-after: #<downstream> blocked-by #<upstream>[, ...]|none
Plan-impact: initial-plan-required|replan-required
Approval: user-confirmed
Reason: <non-empty text>
Supersedes: issuecomment-<id>|none
```

Require exactly those fields in that order with no extras. Require the record to be an unedited comment on its `Parent` issue. Normalize every issue list as ascending, comma-space separated, and duplicate-free; normalize dependency edges by downstream then upstream and reject duplicates.

Validate action semantics:

- `add`: every member is absent from `Before`, present in `After`, and exactly explains the difference.
- `remove`: every member is present in `Before`, absent from `After`, and exactly explains the difference.
- `move`: the same removal rule holds for the parent; `Destination` is one issue; every member is added to the destination and exactly explains its snapshot difference.
- Only `move` may use destination snapshots. Other actions require all three destination fields to be `none`.
- `Plan-impact` is exactly one supported value, `Approval` is exactly `user-confirmed`, and `Reason` is non-empty.

Treat the numeric GitHub comment ID as record identity. `Supersedes` may name only an earlier valid record with the same marker and subject. Reject missing or forward targets, duplicate IDs, cycles, forks where multiple records supersede one target, and multiple conflicting replacements.

Apply unsuperseded records chronologically by `created_at`, using comment ID as the deterministic tie-breaker. The first active record establishes the membership and dependency baselines, including pre-contract state. Every later active `Before`, `Destination-before`, and `Dependencies-before` must equal the prior active `After`, `Destination-after`, and `Dependencies-after` for the same affected state. Final active snapshots for every affected parent and destination must exactly equal current native direct membership and dependency state.

An active batch with current members but no valid history establishing that membership holds with reason `unreconciled-membership`. Records are evidence only: never create, edit, supersede, or repair them here.

## Step 7: Validate terminal dispositions

Scan every fetched comment for every HTML marker beginning `mach12-disposition`. A marker variant that is not exact v1 holds. The exact block is:

```text
<!-- mach12-disposition-v1 -->
Disposition: split|replaced|abandoned
Unit: #<number>
Successors: #<number>[, #<number> ...]|none
Members-before: #<all direct members>|none
Members-after: none
Member-moves: #<source>->#<successor>[, ...]|none
Dependencies-before: #<downstream> blocked-by #<upstream>[, ...]|none
Dependencies-after: #<downstream> blocked-by #<upstream>[, ...]|none
Approval: user-confirmed
Reason: <non-empty text>
Supersedes: issuecomment-<id>|none
```

Require exact field order, no extras, normalized issue/move/dependency lists, append-only timestamps, subject identity (`Unit` equals the issue carrying the comment), user-confirmed approval, non-empty reason, and the same strict supersession rules as membership decisions. Only one valid unsuperseded terminal disposition may exist per unit.

Require `Members-after: none` and no remaining native members. `split` requires at least two successors, `replaced` exactly one, and `abandoned` none. Every successor must exist. When members existed, every source must move exactly once, each move must target a declared successor, every declared successor must receive at least one member, and native successor memberships must match. The final dependency snapshot must equal native dependency state.

A malformed, conflicting, partially materialized, or multiple active disposition holds. Any active disposition on `D` holds with reason `delivery-unit-dispositioned`; a disposition on a retained member makes that member unreconciled rather than delivered.

## Step 8: Validate the canonical batch plan and retained members

For a batch, inspect all comments for every marker beginning `mach12-plan`. Hold malformed marker variants. An exact plan record is one comment containing exactly one exact single-line `<!-- mach12-plan -->` marker. Duplicate exact markers in one comment or any otherwise ambiguous plan record hold with reason `stale-or-incomplete-plan`; never choose among duplicate markers. Select the latest exact plan record by `created_at`, with numeric comment ID as deterministic tie-breaker. Require a plan and require it to postdate every active membership decision affecting the batch or retained membership.

Semantically compare the plan with every retained member's current body and complete comments:

- The exact retained native member set must be represented; removed or non-member issues must not remain described as retained deliverables.
- Each source's scope, material requirements, constraints, acceptance criteria, implementation coverage, and verification mapping must be covered.
- A material body or comment requirement added after the plan requires a revised plan.
- If a member body has a later `updated_at` and the edit cannot be established as non-material, hold conservatively.

On failure, use reason `stale-or-incomplete-plan` and direct the caller to revise membership or planning through the batch workflow. Never author the revision.

Require every retained member to exist, classify as ordinary, have no children, report `D` as parent, have no active disposition, and be coherent with the other retained sources. Outside historical context, each retained member must be open; in historical context, each must be closed as completed. A stale, conflicting, incomplete, independently delivered, mis-parented, missing, or separately claimed retained member holds. Partial completion never silently narrows the close set: move incomplete work to explicitly linked successor units through an approved membership revision and publish a newer exact plan first.

## Step 9: Validate blockers as canonically delivered

Every direct blocker of `D` or a retained source must be delivered or the operation holds with reason `undelivered-blocker`. Manual closure is insufficient.

A blocker is delivered only when all of these are true:

- it is closed as completed and has no active terminal disposition masquerading as delivery;
- it has exactly one claiming PR in any state;
- that sole claimant is merged and carries exact Mach 12 provenance and `Delivery-unit: #<blocker>` identity;
- recursively deriving the blocker as an ordinary or batch unit in historical context succeeds;
- the merged claimant's complete actual `closingIssuesReferences` exactly equal the blocker's canonical ordinary/batch close set;
- its standalone `Part of` line exactly matches its direct initiative parent, if any, and is absent otherwise.

A closed blocker with no canonical merged delivery PR, multiple claimants, a closed-unmerged claimant, inconsistent close set, malformed identity, unresolved blocker, or dependency cycle remains undelivered.

## Step 10: Enforce claimant cardinality

Use complete claimant results for `D` and every retained source.

Apply exactly one claimant rule:

- **Top-level creation target:** `D` and every retained source must have zero claiming PRs in every state.
- **Top-level verification target:** `D` and every retained source must have claimant set exactly `{current PR}`. Any additional draft, open, closed-unmerged, or merged claimant holds.
- **Recursive historical blocker:** replace the top-level mode's claimant rule with this rule: the union of claimant results for `D` and every retained source must be exactly one PR; that sole claimant must be merged, carry exact Mach 12 provenance and `Delivery-unit: #D` identity, have complete actual `closingIssuesReferences` exactly equal to the recursively derived close set, and have the exact standalone `Part of` line required by `D`'s direct initiative parent or none otherwise.

A merged other claimant means the unit or source is already delivered. A closed-unmerged claimant must be reopened/reused or the unit retired/replaced. Never ignore an older or non-open claimant.

## Step 11: Derive and, when applicable, compare exact linkage

Derive only after every prior validation succeeds:

- **Ordinary:** close set is exactly `{D}`.
- **Batch:** close set is exactly `{D} ∪ {exact current direct retained source members}`.
- Order canonical body lines as `D` first, then batch members in ascending numeric order.
- If `D` has a direct initiative parent, `part-of` is exactly that initiative; otherwise it is none.

The canonical linked body grammar contains exactly one standalone `Fixes #N` line per derived close-set member. Reject duplicate, missing, extra, combined, or alternative closing-keyword lines. It contains exactly one standalone `Part of #<direct-initiative>` line when `part-of` exists and zero standalone `Part of` lines otherwise.

Never close an initiative, sibling, removed source, successor, transitive descendant, or dependency-only issue.

In verification mode, compare as exact sets in both directions:

- The identity's `D` must equal the derived delivery unit.
- Complete actual `closingIssuesReferences` must equal the derived close set; report sorted `missing` and `extra` issue numbers.
- Canonical standalone `Fixes` lines must exactly represent the same set and order.
- Standalone `Part of` cardinality and number must exactly match the direct initiative parent.
- The claimant set must still equal only the current PR.

Deleting closers while identity remains re-derives `D` and reports missing closers. Adding unrelated, sibling, or initiative closers reports them as extra. Verification never edits the PR body.

## Step 12: Return structured prose

Return only one of these verdicts: `ok` or `hold`. Invalid arguments use this parse-failure variant:

```text
verdict: hold
mode: unknown
delivery-unit: unknown
reason: invalid-arguments
```

A successfully parsed verification request that holds before identity resolution uses:

```text
verdict: hold
mode: verification
delivery-unit: unknown
reason: pr-read-failed|missing-delivery-identity|malformed-delivery-identity
```

Use the precise applicable reason, not the literal alternatives above. After creation arguments parse or verification identity resolves, always include:

```text
verdict: ok|hold
mode: creation|verification
delivery-unit: #<D>|none
```

Reserve `delivery-unit: none` for a verification request whose PR has the exact explicit-unlinked identity. Never use it when identity is unresolved.

For `ok`, also include:

```text
classification: ordinary|batch|unlinked
close-set: [#<D>, ...]
part-of: #<initiative>|none
claimants: <complete current claimant facts>
```

For `hold`, also include a stable lowercase hyphenated reason slug, the precise observed-versus-expected difference, and one actionable reconciliation, redrafting, or migration instruction. Never ask the user a question and never mutate GitHub state.
