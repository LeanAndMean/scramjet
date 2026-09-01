---
name: writing-scramjet-commands
description: Use when creating, revising, reviewing, or diagnosing Scramjet commands and command sets.
---

# Writing Scramjet Commands

## Mental model

A Scramjet command is a generalized plan executed by a capable agent with user-supplied context. It is not a natural-language program and should not encode every branch the agent might encounter.

State the destination, controlling boundaries, and necessary handoffs. Let the agent choose tactics from current context.

## What a command should contain

- **Purpose and outcomes:** the user- or caller-visible result.
- **Authority:** the inputs, decisions, and artifacts that control the work.
- **Necessary common-path guidance:** process that prevents a demonstrated omission or preserves a required collaboration boundary.
- **Hard boundaries:** user decisions, exact consumer contracts, consequential side effects, and real trust or safety constraints.
- **Ownership:** who investigates, decides, mutates, publishes, and asks the user.
- **Handoffs:** only the artifact or information a downstream participant needs.
- **Completion:** observable conditions for a truthful result.

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

Do not add procedure for a hypothetical, a reviewer's imagined possibility, or one isolated incident. Add guidance only for a real recurring omission, an exact external contract, a user requirement, or a concrete safety boundary not already owned elsewhere.

## Match instruction form to the need

- **Desired result:** use a positive outcome or output contract.
- **Required element:** give it a clear slot in the artifact.
- **Conditional behavior:** key it to an observable fact.
- **Fragile operation:** provide the exact safe command or sequence.
- **Mechanical rule:** enforce it with a parser, linter, script, or platform capability.

Avoid vague qualifiers and exemption-heavy prohibitions. Use consistent terms for each participant, fact, and artifact.

## Context and disclosure

Treat the context window as shared working memory.

- Keep the active command concise.
- Reference detailed material only when the task needs it.
- Keep references shallow and authoritative.
- Pass isolated subagents focused authority and observations, not an indiscriminate transcript.
- Reacquire facts from their source when that is cheaper and safer than transporting copies.
- Return compressed, caller-consumable results rather than exploration narratives.

## Subagents

The main agent works with the user and owns orchestration, synthesis, mutation, and consequential decisions.

Use a subagent only when isolation provides a concrete benefit:

- compressing large or potentially irrelevant context;
- obtaining a fresh independent perspective;
- examining genuinely disjoint work in parallel;
- applying stable expertise without keeping its full reference in the main context;
- confining analysis to read-only tools.

A taxonomy category is not a reason to create or dispatch an agent. Do not union overlapping reviewers. Each subagent needs a distinct question and a compact output the caller will consume.

## Commands and command sets

A command can be internally clear and still fail as part of a set. Check:

- whether its inputs are available at invocation;
- whether its artifact is sufficient for the next command;
- whether fresh sessions and subagents receive necessary context;
- whether one participant clearly owns each side effect;
- whether next-step routing matches real outcomes;
- whether users can redirect or leave the workflow.

Do not add handoff fields without a named consumer.

## Tools and evidence

Discover authoritative project-native tools from repository guidance and established usage. Prefer existing checks and scripts over improvised substitutes, but inspect unfamiliar or mutating tools before use.

A clean structural check proves only its deterministic contract. It does not prove that a command is useful, correctly interpreted, or operationally effective.

Improve command guidance from actual use:

1. Observe the gap in real work.
2. Identify the first divergence and missing information or boundary.
3. Make the smallest instruction, responsibility, or tooling change that addresses it.
4. Observe later real use and remove guidance that adds no value.

Synthetic scenarios can expose possible interpretations but cannot establish product value or merge readiness.

## Review standard

Review the command as one system, not as a collection of opportunities to add instructions.

- Start from no finding.
- Report only a material outcome, authority, handoff, semantic, or safety defect supported by current evidence.
- Treat plans, historical reviews, and specialist output as evidence rather than authority.
- Reject duplicated policy, speculative procedure, and scope that belongs to another command or platform owner.
- Count prompt volume, context transport, subagent calls, artifacts, tests, and recovery machinery as real complexity.
- Accept deletion, consolidation, responsibility movement, tool enforcement, or no change as normal outcomes.

## Quick check

Before finalizing a command, ask:

- Is the purpose obvious?
- Are outcomes and hard boundaries distinct from tactics?
- Did I leave ordinary decisions to the agent?
- Did I avoid hypothetical edge-case causes?
- Does every required artifact or field have a consumer?
- Are side effects and user decisions owned clearly?
- Is detailed material loaded only when needed?
- Could deterministic tooling replace any prose?
- Is this shorter and more adaptable than the process it replaces?

## References

- [Superpowers: Writing Skills](https://github.com/obra/superpowers/tree/main/skills/writing-skills)
- [Anthropic Agent Skills overview](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview)
