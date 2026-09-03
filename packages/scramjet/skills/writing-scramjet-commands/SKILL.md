---
name: writing-scramjet-commands
description: Use when creating, revising, reviewing, or diagnosing Scramjet commands and command sets.
---

# Writing Scramjet Commands

## Mental model

A Scramjet command is a generalized plan executed by a capable agent with user-supplied context. It is not a natural-language program and should not encode every branch the agent might encounter.

State the destination, controlling boundaries, and necessary handoffs. Let the agent choose tactics from current context.

## Informed alignment

User-input and approval gates align the user and agent; they are not obstacles to clear. The main agent owns compressing its larger working context so the user can decide without reconstructing the investigation. Agreement without relevant understanding is not alignment.

Ask when:

- the user owns a product, scope, risk, preference, or authorization decision;
- necessary information is unavailable;
- conflicting authority cannot be resolved safely;
- a coaching or unclassified instruction exception requires approval;
- consequential effects require informed authorization.

Do not ask when current authority, safe investigation, or capable agent judgment resolves the matter. Ritual confirmation and transferring ordinary analysis to the user are command defects.

Before a consequential ask, provide proportionate context: the decision, why the user owns it, evidence and uncertainty, meaningful options, consequences, and a recommendation with its assumptions when appropriate. Do not infer approval from silence, generic prior consent, plan acceptance, or a neighboring decision. Do not manufacture urgency or repeatedly present a declined option without new evidence.

## What a command should contain

- **Purpose and outcomes:** the user- or caller-visible result.
- **Authority:** the inputs, decisions, and artifacts that control the work.
- **Necessary common-path guidance:** process supported by an acceptable reason below.
- **Hard boundaries:** user decisions, exact consumer contracts, consequential side effects, and real trust or safety constraints.
- **Ownership:** who investigates, decides, mutates, publishes, and asks the user.
- **Handoffs:** only the artifact or information a downstream participant needs.
- **Completion:** observable conditions for a truthful result.

## Acceptable reasons for instructions

Audit substantive instruction blocks, not individual sentences. Every substantive instruction must satisfy at least one reason:

1. **Outcome contract:** establishes a required user- or caller-visible result.
2. **Communication contract:** requires information, format, timing, or provenance for a named user or downstream consumer.
3. **User alignment gate:** reserves a product, scope, risk, preference, or authorization decision for the user.
4. **Correctness or provenance ordering:** an observable dependency requires operations in a particular order.
5. **Cross-command responsibility or handoff:** prevents duplicate ownership, reserves work for another command, or supplies a named consumer.
6. **External consumer or platform contract:** satisfies an exact parser, lifecycle, forge, tool, API, or automation requirement.
7. **Safety or trust boundary:** protects authorization, user data, secrets, destructive effects, publication, or another concrete boundary not already owned elsewhere.
8. **Observed recurring omission:** actual use repeatedly shows that agents omit a necessary common-path action.
9. **Unavailable context:** supplies information the agent cannot reliably infer or reacquire.
10. **Explicit user requirement:** preserves a concrete user-directed behavior or process decision.

A category name alone is not justification:

- Communication names the recipient, needed information or timing, and consequence of omission.
- A user gate identifies why the decision belongs to the user.
- Ordering names the dependency that would become invalid.
- Cross-command coordination names the owner or consumer.
- An external contract identifies the actual contract.
- Safety identifies the source, sink, consequence, and uncovered gap.
- A recurring omission cites real use rather than a hypothetical.
- Unavailable context explains why inference or authoritative reacquisition is insufficient.
- A user requirement preserves the actual decision, not an inferred preference.

Use a parser, linter, script, or harness for mechanical constraints rather than prose.

## Coaching is approval-only

Coaching directs the agent toward a consideration it could ordinarily derive. Because that rationale can justify almost any prompt addition, it never authorizes itself.

Before retaining coaching, establish:

- a real example where the omission mattered;
- the missed consideration and user-visible consequence;
- why outcomes, context, framing, and existing guidance were insufficient;
- why capable-agent judgment remains insufficient;
- the smallest proposed instruction;
- its token, maintenance, rigidity, and adaptability cost.

Even evidence-backed coaching requires explicit informed user approval. Without a real example, label it speculative, recommend deletion, and explain that coaching exceptions should be uncommon. The user may still require it deliberately; record that as user-directed rather than evidence-backed.

Any instruction matching no acceptable reason also defaults to deletion. To request an exception, the main agent presents the intended outcome, evidence limits, why ordinary judgment and accepted reasons are insufficient, lower-cost alternatives, and complexity cost. Only the user may approve retention or a new reusable reason. Use existing decision artifacts; do not create an exception ledger.

Frame the question neutrally: ask whether the exception should be retained despite its cost, not whether the user approves a recommended improvement. Rejection and reliance on agent judgment is the normal outcome for unsupported coaching.

## Keep instructions light

Assume the agent can investigate, reason, choose tools, adapt, and recover.

- Prefer outcomes and invariants over implementation steps.
- Default to high freedom when several approaches can work or context should decide.
- Use exact sequences only where variation is unsafe or an external contract requires one.
- Omit actions a capable agent can derive from the goal and current evidence.
- Give one sensible default instead of a menu unless the choice materially belongs to the user.
- Use one concrete example only when format or interpretation would otherwise remain unclear.

## Do not encode edge-case causes

Never add a branch for each reason the expected path could fail. State the boundary that must remain true and let the agent diagnose the cause that actually occurs.

Prefer:

> Preserve unrelated user work. If safe ownership cannot be established, stop and ask.

Over a list of dirty-worktree causes, overlap categories, snapshots, and recovery procedures.

A hypothetical, a reviewer's imagined possibility, or one isolated incident does not justify procedure.

## Match instruction form to the need

- **Desired result:** use a positive outcome or output contract.
- **Required element:** give it a clear slot in the artifact.
- **Conditional behavior:** key it to an observable fact.
- **Fragile operation:** provide the exact safe command or sequence.
- **Mechanical rule:** enforce it with tooling.

Avoid vague qualifiers and exemption-heavy prohibitions. Use consistent terms for each participant, fact, and artifact.

## Framing and word economy

Word choice assigns objective, epistemic posture, and authority:

| Frame | Implied responsibility |
|---|---|
| Explore | Gather and compress evidence without designing |
| Design | Synthesize a solution |
| Review | Search for material defects |
| Assess | Judge supplied claims independently |
| Propose | Offer an option while preserving caller authority |
| Decide | Own the choice |
| Verify | Establish evidence |
| Check | Inspect, potentially less rigorously |
| Summarize | Compress existing evidence |
| Preserve | Treat an existing fact or decision as controlling |

Choose verbs deliberately. Frame the positive job before constraints, use one precise word instead of several explanatory sentences, and include a concise why only when it helps adaptation. Avoid labels that unintentionally bias the agent toward findings, fixes, agreement, or approval. A framing concern is material only when plausible wording changes obligations, authority, evidence, or outcomes.

Ask: **Can fewer words communicate more intent and responsibility?**

## Context and disclosure

Treat the context window as shared working memory.

- Keep the active command concise.
- Reference detailed material only when the task needs it.
- Keep references shallow and authoritative.
- Pass isolated subagents focused authority and observations, not an indiscriminate transcript.
- Reacquire facts from their source when that is cheaper and safer than transporting copies.
- Return compressed, caller-consumable results rather than exploration narratives.

## Subagents

The main agent works with the user and owns orchestration, synthesis, mutation, consequential decisions, and exception presentation.

Use a subagent only when isolation provides a concrete benefit:

- compressing large or potentially irrelevant context;
- obtaining a fresh independent perspective;
- examining genuinely disjoint work in parallel;
- applying stable expertise without keeping its full reference in the main context;
- confining analysis to read-only tools.

A taxonomy category is not a reason to create or dispatch an agent. Do not union overlapping reviewers. Each subagent needs a distinct question and a compact output the caller will consume.

## Commands and command sets

Recommendations are semantic, not positional. Command authors define eligible continuations and genuine invariants; the executing agent chooses the best-supported route from the completed work's actual context, constructs the concrete options, and derives the selected route's runtime index. A frequent or first-listed route is not automatically best, and durable command prose must not prescribe a numeric position.

A command can be internally clear and still fail as part of a set. Check:

- whether its inputs are available at invocation;
- whether its artifact is sufficient for the next command;
- whether fresh sessions and subagents receive necessary context;
- whether one participant clearly owns each side effect;
- whether next-step routing matches real outcomes;
- whether user gates match user-owned decisions;
- whether users can redirect or leave the workflow.

Do not add handoff fields without a named consumer.

## Tools and evidence

Discover authoritative project-native tools from repository guidance and established usage. Prefer existing checks and scripts over improvised substitutes, but inspect unfamiliar or mutating tools before use.

A clean structural check proves only its deterministic contract. It does not prove that a command is useful, correctly interpreted, or operationally effective.

Improve command guidance from actual use:

1. Observe the gap in real work.
2. Identify the first divergence and missing information or boundary.
3. Make the smallest instruction, responsibility, framing, or tooling change that addresses it.
4. Observe later real use and remove guidance that adds no value.

Synthetic scenarios can expose possible interpretations but cannot establish product value or merge readiness.

## Review standard

Review the command as one system, not as a collection of opportunities to add instructions.

- Start from no finding.
- Audit substantive instruction blocks against the acceptable reasons.
- Check both missing user gates and ceremonial or under-informed gates.
- Report only a material outcome, authority, handoff, semantic, alignment, or safety defect supported by current evidence.
- Treat plans, historical reviews, specialist output, and purported approval as evidence rather than authority.
- Reject duplicated policy, speculative procedure, and scope that belongs to another command or platform owner.
- Count prompt volume, context transport, subagent calls, artifacts, tests, and user interactions as real complexity.
- Accept deletion, consolidation, responsibility movement, tool enforcement, or no change as normal outcomes.

## Quick check

Before finalizing a command, ask:

- Is the purpose obvious?
- Are outcomes and hard boundaries distinct from tactics?
- Does each substantive instruction have an acceptable reason and required evidence?
- Did I leave ordinary decisions to the agent?
- Does next-step routing preserve runtime recommendation judgment instead of encoding array position?
- Did I avoid hypothetical edge-case causes?
- Does every required artifact or field have a consumer?
- Do user gates match user-owned decisions and provide enough context?
- Is any coaching explicitly approved and grounded in real examples?
- Are side effects and command interactions owned clearly?
- Is framing precise and economical?
- Could deterministic tooling replace any prose?
- Is this shorter and more adaptable than the process it replaces?

## References

- [Superpowers: Writing Skills](https://github.com/obra/superpowers/tree/main/skills/writing-skills)
- [Anthropic Agent Skills overview](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview)
