# Scramjet — Product Vision

Scramjet helps people turn refined, recurring ways of working with an agent into reusable commands and connect those commands into understandable, user-controlled workflows.

This document owns Scramjet's intended capabilities, user outcomes, durable principles, and product non-goals. It does not specify command schemas, runtime protocols, lifecycle representation, hooks, persistence formats, UI mechanics, or the current command inventory. Those details may change as the product is tested and improved.

## Product premise

Good agent-assisted work depends on due diligence, and due diligence often becomes a repeatable process. As people work with an agent, they refine recurring instructions until those instructions deserve to become a command: a named, reusable expression of how they want a task performed.

Once related commands exist, two needs emerge:

- Common process fragments should be reusable rather than copied into every command.
- Likely follow-up commands should be discoverable without turning the whole process into a rigid, centrally defined workflow.

Scramjet provides the connective tissue for those needs. It lets independently authored commands compose and form workflows while preserving room for human and agent judgment.

## Scramjet and command sets

**Scramjet** is the general-purpose harness. It discovers command sets, supports composition and continuation between commands, and helps users understand and control the resulting process.

A **command set** codifies a particular domain or team's way of working. Users can create sets for software development, research, infrastructure, security review, writing, or any other recurring process. Sets can coexist without Scramjet imposing one methodology on all of them.

**Mach 12** is one command set: it expresses a software-development methodology spanning issue definition, planning, implementation, review, validation, and delivery. It demonstrates reusable process fragments, guided continuation, independent assessment, and controlled automation. Mach 12 is an example of what Scramjet enables, not infrastructure every Scramjet user must adopt or imitate.

A Scramjet command is an actively invoked task with outcomes the agent is expected to complete. Skills and prompt templates are supporting resources: they can supply capabilities, guidance, or ordinary prompt text, but do not carry the command's execution guarantee or control an active command's outcomes.

## Durable principles

### Emergent workflows

Workflows emerge from relationships owned by individual commands, not from a central workflow definition. This keeps commands independently useful and allows a process to evolve one edge at a time without introducing a separate orchestration model.

### User-owned command sets

Users should be able to add, edit, remove, share, and organize commands as ordinary local content. Scramjet may ship useful command sets, but product-owned examples must not displace user ownership or prevent local adaptation.

### Composability

A recurring process fragment should have one maintained definition that other commands can reuse. Composition should preserve enough context for the fragment to be useful while keeping the caller responsible for the larger task.

### Goal-directed, adaptable execution

Commands should make durable outcomes clear while leaving tactics adaptable. Plans and procedures serve those outcomes; they should not displace user decisions, trust boundaries, exact consumer contracts, required artifacts, or consequential side effects. Older commands without an explicit goal section remain usable so authoring improvements do not become runtime lock-in.

### User control and easy disengagement

Scramjet should make workflows easier to follow, not harder to leave. Users retain control over meaningful choices and can readily return to ordinary agent interaction. Automation should be graduated and user-controlled rather than an all-or-nothing commitment.

### Unobtrusive when idle

When Scramjet has no useful action or suggestion, it should add no behavioral noise. Process guidance and history should remain supportive rather than competing with the work itself.

### Informed decisions without consent theater

Before asking for a consequential choice or taking a risky action, commands should provide the context, trade-offs, and likely consequences needed for an informed decision. Routine transitions should not accumulate ritual confirmations that provide no new information.

### Simplicity and resistance to structural debt

Scramjet should remain small enough that its behavior is understandable. It should favor existing platform capabilities and focused contracts over parallel abstractions, generalized workflow machinery, or configuration added for hypothetical needs. Repeated small additions are a reason to reassess structure before accidental complexity becomes permanent.

### Evidence-based self-improvement

Operational failures and friction can reveal gaps in commands or processes. Scramjet should help users diagnose what happened, distinguish isolated mistakes from recurring patterns, and turn sufficiently supported observations into specific improvements. One surprising result is evidence to investigate, not permission for uncontrolled rewriting.

## Broad capabilities

These capabilities describe user outcomes, not required implementations.

### Command-set discovery and coexistence

Users can make multiple command sets available to Scramjet and invoke their commands predictably. Product-provided, user-global, and project-relevant content can coexist while preserving clear ownership and understandable conflict handling.

### Reusable command composition

Commands can reuse other commands as process fragments. Authors can maintain shared due-diligence steps in one place, and users can understand when a larger command relied on those fragments.

### Command authoring feedback

Authors and agents can check command structure before installation or interactive use. Deterministic feedback should reuse runtime recognition and registration authority without making runtime loading stricter. It cannot establish whether a command is useful or well interpreted; those judgments come from independent review and actual use. Current checker behavior and scope are governed by the command-authoring guide rather than this vision.

### Guided continuation

A command can communicate useful next steps so related commands form a discoverable flow. Scramjet can help a user or agent continue when appropriate while validating choices against the command author's intended boundaries.

### In-command collaboration

A running command can ask for information, present choices, or seek confirmation without losing the larger task. The interaction should give users enough context to respond and should preserve their ability to pause, redirect, or stop.

### Graduated, user-controlled autonomy

Users can choose how much assistance Scramjet provides in moving between steps. Deterministic process structure, agent judgment, and human judgment are meaningfully different; increasing autonomy should not silently erase important decision or consent points.

### Understandable history and safe continuity

Users can understand which commands and reusable process fragments contributed to the current work. Scramjet may preserve narrowly supported continuity when doing so is safe and unsurprising, but completed work must not unexpectedly restart and paused process state must not become a general background job system.

### Operational diagnosis and process improvement

Scramjet can help investigate unexpected command behavior using relevant evidence, explain the difference between intent and outcome, and route validated process gaps toward command or product improvements. Diagnosis should remain transparent about missing evidence and should not publish or mutate external artifacts without appropriate review.

## Directional opportunities

The following areas are promising but unsettled. They require validation through real use and are not commitments to a particular design or delivery schedule.

- **Lower-friction command authoring:** help users recognize recurring work, create or revise commands, and understand how changes affect related commands.
- **Richer process-history presentation:** make command composition and progression easier to inspect without making Scramjet visually intrusive.
- **Stronger delegated capability enforcement:** ensure reusable process fragments cannot exceed the authority granted by their callers while avoiding surprising restrictions on legitimate work.
- **Project-local trust treatment:** communicate and enforce appropriate trust boundaries for commands supplied by a project rather than the user.
- **Broader operational integrations:** support command sets that work across additional forges, tools, and domains without embedding those integrations into Scramjet's core identity.

These opportunities describe problems worth exploring. Their interfaces, state models, UI, and enforcement mechanisms should follow evidence rather than being fixed in advance by this vision.

## Product non-goals

- **No centralized workflow DAG or required global workflow registry.** Commands own their relationships; Scramjet does not require a second authoritative definition of the process.
- **No general workflow programming language replacing process prose.** Commands remain human-readable descriptions of judgment-rich work rather than programs in a control-flow DSL.
- **No opaque automation that removes meaningful judgment.** Convenience must not hide consequential choices or make it unclear why an action occurred.
- **No generalized persistent workflow engine or resumable job queue.** Narrow continuity may support an active interaction, but Scramjet is not a background orchestration service.
- **No requirement that command sets resemble Mach 12.** Scramjet supports independently designed methodologies and domains.
- **No vision-level commitment to a specific UI, state representation, storage model, hook, dispatch protocol, command schema, or command inventory.** Those are replaceable implementation and product-design choices governed by focused current documentation, source, tests, and validated future work.

## Documentation authority

Different documents answer different questions:

- **Product direction and durable boundaries:** this vision.
- **Current user-facing behavior:** [`../README.md`](../README.md).
- **Command format and author-visible contracts:** [`command-authoring.md`](command-authoring.md).
- **Lifecycle facts, invariants, and transitions:** [`lifecycle-state-space.md`](lifecycle-state-space.md).
- **Journal evidence and diagnostics:** [`logging.md`](logging.md).
- **Exact runtime behavior:** source code and tests.

Historical implementation choices and superseded alternatives remain available through Git history and issue discussions. They are useful evidence, but they do not define the product vision.
