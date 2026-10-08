# Progress and next-session handoff

This is the compact development cursor, not a runtime session database or duplicate issue tracker. [Design](DESIGN.md) owns direction; [roadmap](ROADMAP.md) owns milestones. Reconcile this snapshot against current files/Git before executing; it does not authorize replaying completed work.

## Current snapshot

- Updated: 2026-10-08.
- Stage: S0 — runnable foundation, S0.1 active.
- Workspace: `/home/user/repos/scramjet-rs`; orientation Markdown and four project-local development commands only.
- Git/remote: local Git initialized on unborn `main` with the user's authorization; no commits or remote. The nine orientation/command files are untracked. No issue/PR identifiers are available.
- Rust: installed `rustc 1.99.0` / `cargo 1.99.0` observed; no toolchain changes or dependency installation performed.
- Implementation evidence: no Rust build, tests, benchmarks, live endpoint calls, or native rewrite UI journeys yet.
- Active task/owner: S0.1 — `rs:work`, session `01a11d61-1068-78ae-a356-03b1f7884df1`; sole writer for the Cargo/source/test bootstrap, license/ignore files, README and this ledger.
- Next action: finish S0.1 implementation and offline verification, establish local history, then return to triage if accepted. No dependency or bootstrap authorization blocker was found.

## Priority queue

- Selected queue: **S0.1** only — offline CLI/library bootstrap and repeatable checks. Implementation is active; no candidate or pending product review exists yet.
- S0.1 has no dependencies. Its Cargo layout is a bounded implementation choice, not material uncertainty requiring `rs:plan`; preserve the headless/TUI responsibility boundary without speculative contracts or crate hierarchy.
- Return to triage after S0 acceptance before selecting between transport and native TUI tracks. **S1.1/S1.2** remain pending future candidates outside the actionable queue; their S0.1 dependency is not yet accepted. Their task-specific acceptance must be refined when selected.
- Establish S0.1 review need from the actual implementation and record its rationale. No required independent review has been waived; a material shared contract would require independent acceptance.

## Near-term tasks

| ID | Status | Outcome | Dependency |
|---|---|---|---|
| S0.1 | active | Minimal offline Rust CLI/library, Apache-2.0 license, local Git, and repeatable smoke/test loop | None |
| S1.1 | pending | First bounded streaming transport fixture and fixed-admission demonstration | S0.1 |
| S1.2 | pending | Small retained transcript/editor probe with an explicit native test plan and initial available-host evidence | S0.1; native acceptance depends on actual platform access |

These are bounded entry points, not authority to finish the entire roadmap. Refine the selected task before implementation. Define subsequent task IDs only as needed; do not renumber completed IDs. Missing native access should be recorded for S1.2 without blocking unrelated headless tasks.

## Next task: S0.1

**Outcome:** a new session can build, test, and run a minimal Rust executable offline, understand the actual starting layout, and use local Git history without relying on the old Scramjet runtime.

**In scope:** choose the smallest Cargo layout consistent with a headless library and eventual separate TUI; add a CLI smoke and small behavioral test; establish the standard Apache-2.0 license/ignore rules and local Git if still absent; document actual verification commands. No need for a fixed multi-crate architecture before consumers exist.

**Out of scope:** live provider calls, adaptive algorithms, full agent/tools, native integration, plugin SDK, remote repository creation, GitHub publication, CI/release infrastructure, or version/changelog preparation.

**Review/candidate:** no independent review required for the selected std-only scaffold: a fixed smoke message and thin CLI, with no material runtime/shared protocol, concurrency, persistence or native interaction contract. This does not exempt future material contracts from review. No implemented candidate exists yet.

**Implementation direction:** one Cargo package with a headless library and binary consumer; no workspace hierarchy, external dependencies, TUI or optional plugin coupling. Add a fixed offline `smoke` command, small library/CLI behavior tests, the standard Apache-2.0 text and `/target/` ignore rule. Current structural mapping confirmed there are no product modules, exported contracts or consumers to preserve; required toolchain components will be checked by execution.

**Acceptance:** the minimal CLI/library builds, tests, and runs offline on the available Linux host; local Git history is established. Run `cargo fmt --check`, `cargo check --offline`, `cargo test --offline`, `cargo clippy --offline --all-targets -- -D warnings`, and the task-specific CLI smoke; record exact commands/results and toolchain. Source and orientation docs must agree; no performance/platform claims are invented. New dependencies must be necessary and have evidenced compatible licenses.

**Unresolved/blockers:** none blocking S0.1. Exact first Cargo layout and proportional review need are implementation decisions. Remote destination/publication and first real endpoint/authentication remain user-owned decisions outside S0.1. macOS/WSL test access is unestablished and does not block available-host bootstrap acceptance.

**Suggested fresh-session invocation:** `/rs:work S0.1`, after the live harness has loaded this project's `rs` command set. Without that command, ask: “Read CLAUDE.md and docs/PROGRESS.md, then complete only task S0.1, verify its outcome, and record the next handoff.”

## Last completed work

**2026-10-08 — orientation bootstrap:** preserved the founding scope and admission-control clarification; separated agreed invariants from proposed mechanisms; established staged vertical slices, early native TUI evidence, and a single task/progress handoff path. Added a project-local work command for the existing Scramjet harness, not for the Rust product.

Reference exploration used existing Scramjet clean HEAD `6325b36c50188f630cb03e8c9e9c3471853f4f72`. Its native acceptance record targets earlier commit `3810822bedce066d1c9acfcadc6d02f0c8112412`; none of that certifies the rewrite. No source copied from the restricted-license Rust Pi project.

**Documentation checks:** local Markdown link check passed (10 file links, none missing). Independent command review found no material findings; independent staging review led to explicit bounded uncertain-remote exposure and an S2 interrupted-write/reconstruction demonstration. Initial strict lint found a Goals/context heading warning, corrected by adding a task heading; final `scramjet-command-lint --strict .scramjet/rs` passed (1 file, 0 errors, 0 warnings). Markdown fence/file-link checks also passed. No Cargo checks are applicable yet. A fresh offline RPC process successfully discovered `rs:work` via `get_commands` from this checkout (no model request; owned process exited 0). The existing interactive session started before the command file was created; project discovery runs at startup/reload, not on file creation. The user subsequently ran `/reload` and confirmed `rs:work` appeared in the interactive command list. Command execution/fresh-session chaining and native rewrite UI behavior remain untested.

**Working discipline:** the user explicitly requested efficient use of time and small concrete tests instead of prolonged theorycrafting; `CLAUDE.md` records that instruction. Prefer shipping usable slices over attempting a flawless one-shot harness.

**2026-10-08 — four-command development set:** the user approved `rs:triage`, `rs:plan`, `rs:work`, and `rs:review`. Triage owns overall priorities; the other roles own bounded task outcomes. Candidate acceptance and harness command completion are separate; proportional planning/review and fresh-session recommendations replace the singleton's forced-manual continuation policy. No forced edges, autonomy override file, Rust implementation, remote effects, or paid calls were added. Verification: `scramjet-command-lint --strict .scramjet/rs` passed (4 files, 0 errors/warnings); local Markdown file-link/fence checks passed (9 links). A fresh offline RPC `get_commands` returned exactly all four `rs` commands from this checkout; no model request, owned process exited 0. The installed runtime parser/validator accepted 12 declared edges and rejected 8 disallowed routes (including off-list publication routes). Independent holistic command review reported no material findings. These checks establish structure, discovery, and validator eligibility—not actual model execution or fresh-session/autopilot lifecycle acceptance. Reload the existing interactive harness to expose the three new commands and changed work definition. S0.1 remains the first implementation task.

**Git initialization:** `git init -b main` was rejected by installed Git 2.25.1. Compatible `git init` followed by `git symbolic-ref HEAD refs/heads/main` succeeded; no files staged, commits created, or remotes configured.

**2026-10-08 — S0.1 triage:** read progress/design/roadmap, README, and project instructions; reconciled actual file inventory and Git state. `git status --short --untracked-files=all` showed exactly the nine existing orientation/command files; `git ls-files` and `git remote` were empty. `git log -6 --oneline` confirmed no commits exist (unborn `main`), so no baseline revision is available. `rustc --version` / `cargo --version` reconfirmed 1.99.0; `uname -s` reported Linux. No Cargo checks apply before bootstrap. Selected ready S0.1 for work; left S1.1/S1.2 pending and preserved publication/native-access boundaries. The live command catalog includes all four `rs` commands and this triage invocation establishes discovery/execution of triage, not fresh-session/autopilot lifecycle acceptance. Only `docs/PROGRESS.md` was edited; existing untracked files were preserved, with no source, command-definition, Git-history, remote, or endpoint changes. Progress Markdown fence/local-link checks passed. Next bounded action: `/rs:work S0.1` in a fresh session.

## Updating this file

Keep the current snapshot, task table, and next task accurate. Record one active writer/session and its scope while work is underway; parallel subagents return evidence to that owner rather than independently editing this ledger. Preserve unrelated work and reconcile ownership before overlapping changes.

At a task boundary, update the completed outcome and next task with:

- Task ID, status (`needs-plan`, `ready`, `active`, `awaiting-review`, `corrections`, `blocked`, `done`, or `pending`), date, and active scope owner where relevant. Dependency acceptance/authorization determine eligibility separately from readiness.
- Outcome, scope/non-goals, acceptance, review need/rationale, candidate identity, and review disposition. Base/head identify committed-only candidates; dirty/unborn candidates need explicit path membership/content identity including additions/deletions/untracked files, separate from progress bookkeeping. Link larger authoritative plans/reviews rather than duplicating them. Record uncertain ownership honestly.
- Actual check commands, pass/fail/not-run results, and platform/workload qualifications. Link durable test/evidence files or issue/PR artifacts rather than embedding raw output.
- Material decisions or changes to proposed mechanisms; update design/roadmap only where direction or milestone meaning changed.
- Concrete blockers, authorization still needed, and next role/task with acceptance and out-of-scope boundaries. Triage owns short-queue selection/order; task commands update facts and may continue ready queued work but return changed priorities to triage.

Leave interrupted tasks resumable from evidence: distinguish implemented-but-unverified from not started, and never mark the whole stage complete because one task passed. Keep recent completion notes short; use Git and linked issues/PRs for older history once available. Do not put secrets, raw journals, private endpoint details, or benchmark traces here.
