---
description: Apply the canonical contract for a durable implementation-plan comment
argument-hint: "<initial|revision>"
delegate-only: true
---

# Plan Comment Contract

You are applying the canonical artifact contract for an implementation plan that another Mach 12 command owns.
Delegation loads this policy into the caller's current model context; it does not execute an independent formatter or
return a separately generated plan.

<caller-context>
$ARGUMENTS
</caller-context>

## Step 1: Select the mode

Accept exactly one mode:

- `initial` — shape the first durable implementation plan from the caller's planning evidence and user decisions.
- `revision` — shape a complete replacement from the prior plan, classified review deltas, architect evidence,
  pitfalls, and decisions.

If the mode is absent or invalid, state that the contract cannot be applied and stop. Do not infer a mode.

## Step 2: Build one standalone artifact

Resume the caller-owned drafting or revision work under this contract. The result must be the exact post-ready Markdown
body, with `<!-- mach12-plan -->` as its first line. Only a complete body that has passed Step 6 may carry that marker.

Make the artifact independently usable by an implementation agent in a fresh session. It must state the goal and scope,
the selected design and concise rationale, a test strategy proportionate to the change, session-sized implementation
stages, actionable pitfalls, and a decision log when meaningful decisions exist.

Give each substantive requirement one authoritative home, normally the stage that implements it. Each stage must identify
its outcome, files to create or modify, implementation-critical contracts, dependencies, and verification. Overview,
test, pitfall, and decision sections may summarize or cross-reference stage content but must not repeat complete stage
instructions or duplicate test matrices.

Format intentional GitHub relationships so they remain discoverable: same-repository issue or pull-request references use
`#N`; cross-repository references use `owner/repo#N` or a canonical URL already obtained from verified GitHub evidence.
Artifact-local identifiers use stable labels or plain words—such as `F1`, `S2`, `N3`, “finding 1,” or “stage 2”—never
bare `#N`. Closing keywords remain limited to explicitly authorized closure behavior. Preserve exact comment URLs and
numeric provenance fields when their stronger format is required.

## Step 3: Preserve implementation-critical evidence

When planning or review established any of the following, retain the concrete contract rather than reducing it to a broad
architecture summary or file list:

- invariants and trust-boundary validation;
- interfaces and data shapes;
- ownership and correlation rules;
- event and mutation ordering;
- persistence and failure semantics;
- rollback, atomicity, and retry boundaries;
- expensive call-site inventories whose rediscovery would materially burden implementation;
- production-realistic test seams rather than shortcuts that bypass production routing;
- cross-package build ordering and executable or generated-artifact provenance;
- generated-output drift recovery steps;
- stage dependencies and ordering constraints;
- manual checks, with an explicit statement of whether each blocks stage completion.

Retain concise rationale for the selected design and rejected alternatives only where it prevents re-litigation or guards
against a plausible wrong implementation. Preserve exact commands, paths, or inventories when their specificity is part
of the implementation contract.

## Step 4: Compress without losing contracts

Remove material that does not help a fresh implementation session execute the selected design:

- raw exploration, journal, or probe transcripts;
- complete rejected blueprints;
- repeated solution-selection or ladder analysis already captured by the selected design and rationale;
- generic repository guidance already supplied by project instructions;
- duplicated requirements or test matrices;
- speculative LOC estimates and non-contractual helper details;
- version bumps, changelog entries, or other release-preparation work owned by the pre-merge workflow.

Use qualitative compression, not a numeric byte limit or truncation. Merge stages that are only documentation or
verification ceremony unless they form a genuinely independent, session-sized implementation unit.

## Step 5: Apply mode-specific rules

For `initial` mode, synthesize the selected architect evidence, test strategy, project constraints, and actual user
decisions. Do not preserve discarded architect proposals merely because they were detailed.

For `revision` mode, produce a standalone replacement, never a patch or delta. Preserve every still-valid contract,
pitfall, and decision from the prior plan; remove one only when the revision supersedes it or makes it irrelevant. Apply
classified findings and new architect evidence without allowing them to erase unaffected detail.

In the Decision Log, use `[user-decided]` only when the user genuinely directed or constrained the decision. A user merely
confirming the agent's default remains `[agent-proposed]`. Tag both selected and rejected decisions, and preserve correct
attribution through revisions.

## Step 6: Run the final self-check

Before the caller passes the artifact to the publication tool, check it against the issue requirements, selected design, relevant architect
evidence, test strategy, user decisions, project constraints, and, in revision mode, the prior plan plus classified
deltas.

Check specifically for:

- missing implementation-critical contracts from Step 3;
- contradictions between stages or evidence;
- duplicated substantive requirements or test matrices;
- incorrect or lost decision attribution;
- stages that depend on unstated prior work;
- a body that is not complete, standalone, or post-ready;
- intentional GitHub relationships that are not linkable, or artifact-local identifiers written as bare `#N`.

Revise every defect that the available evidence can resolve before publication. If evidence is genuinely missing or
contradictory, do not emit a marker-bearing candidate; instead surface only a concise list of the unresolved items so the
caller can gather evidence or ask the user.

## Step 7: Respect caller ownership

Do not call tools, dispatch subagents, ask the user questions, post to GitHub, report top-level command status, choose an
architecture, classify review findings, approve the plan, or route the workflow. The calling command owns those actions
and must provide concise decision context, then pass the complete final candidate only as arguments to the applicable
forge publication tool. When effective policy requires approval, the approval card presents the exact payload. Regardless
of policy, guarded publication and exact verification apply.

Do not pass candidate Markdown through this command's arguments: argument substitution tokenizes and rejoins content and
cannot preserve exact formatting. Invoke this contract at most once per turn because delegated frames are latched and a
repeated same-command delegation is rejected as a cycle.
