# Task queue

This file owns priority order, selection and selection rationale. Each linked task record owns its current requirements, authorization, dependencies, status, owner, plan, candidate, review and next action. Do not mirror those facts here. [CLAUDE.md](../CLAUDE.md) defines read/write ownership; [PROGRESS.md](PROGRESS.md) is the activity log.

## Selection

None. Main-session document maintenance is not a selected product task. After resource reload, triage compares the relevant records and independent proposals before selecting work; no product implementation or live dispatch is selected by this migration.

## Ordered work

1. [S4.14a — real-provider transport and accountable completion](tasks/S4.14a.md). Establish a trustworthy provider boundary before building its tool consumer; compare useful offline investigation directions rather than replaying completed capture corrections.
2. [S4.14b — headless Markdown/read/Python/Bash journey](tasks/S4.14b.md). Demonstrate useful coding effects and actual execution through that boundary.
3. [S4.14c — retained terminal application and final QA](tasks/S4.14c.md). Consume the same headless journey in the terminal instead of building another execution loop.

Milestone context: [S4.14 — first real-provider coding journey](tasks/S4.14.md). Its record owns shared authorization and parent acceptance, not another queue.

## Housekeeping

Triage alone creates, reorders or removes priorities. Other roles may advance the selection to an already-eligible queued task after acceptance without changing order. Check eligibility in the task and dependency records, not from queue membership. A deliberate explicit task request may bypass order only under the existing authority rules.

Remove obsolete or completed work from this queue, retaining its disposition/reason in its task record or historical evidence. Still-relevant blocked work remains discoverable; removal never settles an effect, erases an obligation or grants acceptance. Completed pre-migration tasks and retired cursors are preserved in [the archive](PROGRESS-ARCHIVE.md), not copied into this queue.
