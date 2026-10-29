# Task queue

This file owns priority order, selection and selection rationale. Each linked task record owns its current requirements, authorization, dependencies, status, owner, plan, candidate, review and next action. Do not mirror those facts here. [CLAUDE.md](../CLAUDE.md) defines read/write ownership; [PROGRESS.md](PROGRESS.md) is the activity log.

## Selection

[S4.14c — retained terminal application and final QA](tasks/S4.14c.md), to resolve the consuming application's offline integration and settlement design against independently accepted headless contracts. Both independent advisers identified this as the largest remaining locally demonstrable part of the user-selected journey. Direct source inspection confirms immutable headless facts/exact controls and an explicit unjoined-run/finish seam, but also a synchronous drop join: terminal restoration and blocked-worker ownership need planning before implementation. Triage explicitly separates offline readiness from deployed acceptance, as previously done for b; this does not unlock live calls, whole-child acceptance or final user QA.

Accepted offline b implementation and its R1 correction are exhausted, not another planning/work/review priority. The deployed a → b → c acceptance sequence and the parent's unresolved live exposure remain controlling. No speculative historical reconciliation service or further provider correction is selected without concrete evidence.

## Ordered work

1. [S4.14c — retained terminal application and final QA](tasks/S4.14c.md). Prepare the complete fixture-backed retained application using the accepted headless owner, not another execution loop.
2. [S4.14a — real-provider transport and accountable completion](tasks/S4.14a.md). Preserve the required deployed compatibility outcome; accepted offline corrections need no further work absent a concrete finding.
3. [S4.14b — headless Markdown/read/Python/Bash journey](tasks/S4.14b.md). Preserve the required deployed journey and whole-task acceptance; do not repeat accepted offline work.

Milestone context: [S4.14 — first real-provider coding journey](tasks/S4.14.md). Its record owns shared authorization and parent acceptance, not another queue.

## Housekeeping

Triage alone creates, reorders or removes priorities. Other roles may advance the selection to an already-eligible queued task after acceptance without changing order. Check eligibility in the task and dependency records, not from queue membership. A deliberate explicit task request may bypass order only under the existing authority rules.

Remove obsolete or completed work from this queue, retaining its disposition/reason in its task record or historical evidence. Still-relevant blocked work remains discoverable; removal never settles an effect, erases an obligation or grants acceptance. Completed pre-migration tasks and retired cursors are preserved in [the archive](PROGRESS-ARCHIVE.md), not copied into this queue.
