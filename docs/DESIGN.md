# Design direction

Established 2026-10-08 from the founding discussion. This file owns product direction and essential invariants; source and executable evidence own implemented behavior. Proposed mechanisms below remain revisable as demonstrations produce evidence.

## Agreed scope

- Build an independent Rust product under Apache-2.0, not a Rust port of the Pi package structure.
- Make Ubuntu Linux, macOS, and WSL Ubuntu first-class. Native Windows is out of initial scope.
- Improve current usage first: independent project tabs, a main agent, and efficient parallel subagents.
- Preserve valuable Scramjet outcomes: coding tools, reusable commands/agents/skills, project instructions, command composition and continuation, graduated autonomy, informed approval, model selection, and useful session continuity.
- Keep the retained alternate-screen experience: browse unfinished output, edit while browsing, expand tool/agent details, and copy selected text without disrupting live work.
- Treat normal macOS Command+C/Command+V behavior as an early native feasibility question, not late polish.
- Investigate approximately 8,000 simultaneous LLM requests from one node. Queued tasks, open requests, engine-scheduled requests, and completed-request throughput are separate measurements.
- Actively manage provider concurrency and rate limits. On-prem vLLM behind a reverse proxy is a first-class deployment scenario, not the only provider type.
- Preserve extensible boundaries, but defer a general extension system. No Python, TypeScript, Pi extension API, or Pi ecosystem compatibility is required.
- PromptSwarm is a reference for scale ambition, not a workflow model to port or a runtime dependency.

## Current direction (2026-11-03)

Make scramjet-rs a rock-solid daily driver with Scramjet's features, its retained full-screen UI included, on a small, well-organized codebase. Build beyond Scramjet (admission and bounds for many children, children that write, macOS, capacity) only after that (user decision, 2026-11-03). Order: first remove the earlier-stage demos and the runtime they exercised, so that `scramjet-rs` itself is the agent and the tests are fast (git history keeps the removed code; admission is written for the agent when its phase comes); then review the code drafted by test-only models; then Scramjet's features, usability first ([the task list](TASKS.md)).

Approvals (user decision, 2026-10-30, replacing the same day's read-only command list): tool calls run without approval questions by default, as in Scramjet. `--ask` opts in to questions (every shell command, writes and edits outside the project, `a` to allow all until exit) for when the user wants to watch. Approval questions and command allowlists are not containment: the shell can do anything, and a question shows a command, not what it does. Recoverability comes from Git and clones or worktrees; real confinement needs an OS sandbox (Landlock or bubblewrap on Linux, Seatbelt on macOS), planned with parallel subagents and unattended runs.

Scramjet (`~/repos/scramjet`) and Proxy (`~/repos/proxy`) are the working reference for protocol and tool behavior. A limit or check needs a concrete basis: a documented contract, a user decision, a real safety risk, or an observed failure. "No document guarantees X" is not a reason to reject X. When a self-imposed rule blocks the real system, compare with Scramjet and loosen or remove it. Where Scramjet's behavior comes from its history rather than its design (command files registered as Pi prompt templates, `/scramjet settings` apart from `/settings`), do better (user decision, 2026-11-03).

Familiar names (user decision, 2026-11-04). The UI should be no less familiar than Claude Code's or Codex's. Where they and Pi/Scramjet name or bind the same action differently, the name their users know is the one listed and documented, and the other names a user would type stay accepted without being listed, so that typing one never sends it to the model as a prompt. `/clear` is listed, with `/new` and `/reset` accepted (both Claude Code and Codex understand `/new`). By the user's choice, `/settings` stays listed over Claude Code's `/config`, which is accepted.

Proxy keeps authentication; Rust never reads OAuth credentials or changes Proxy state. Missing usage is recorded as unknown, not zero, and local cancellation is not proof that remote generation or billing stopped.

### Agreed product shape (2026-11-03)

User decisions from `/rs:steer` on 2026-11-03:

- **Interface.** `scramjet-rs` itself is the agent (`scramjet-rs`, `scramjet-rs -p …`, `--continue`, as Scramjet's CLI), with no `agent` subcommand or demo commands. The retained full-screen transcript becomes the main interactive UI, as Scramjet's default `retained` mode: an editor that stays visible during a turn (Enter queues a steering message, Alt+Enter a follow-up; `steeringMode` `all` and `followUpMode` `one-at-a-time` by default, as in Scramjet), tool output and summaries expanded in place with Ctrl+O, mouse-wheel scrolling and the UI's own selection and copy. The inline renderer goes once the retained UI covers it, so there is one renderer.
- **Command sets are their own concept**, not Pi prompt templates: a set brings commands with goals, a lifecycle, next-step policies and delegation, and its agents. Scramjet behavior that exists only because it registers command files as prompt templates (such as `delegate-only` commands staying typeable and in the `/` list) is not binding.
- **One source of truth for command sets.** A set is installed by reference from a location (`install`, `remove`, `list`; recorded in the settings, not copied), as Pi installs a local path. Mach 12 and `scramjet` are installed from `~/repos/scramjet/packages/scramjet` (moving Mach 12 to its own repository is outside this project). An agent named `<set>:<name>` belongs to the installed set `<set>`: a loose file claiming that namespace is skipped with a warning. `# Available subagents` lists command-set agents, as Scramjet's does.
- **Settings.** scramjet-rs has its own settings file, kept apart from Scramjet's, in Scramjet's format and with its key names where the behavior matches (`hideThinkingBlock`, `doubleEscapeAction`, `branchSummary.skipPrompt`, `defaultThinkingLevel`, `steeringMode`, `followUpMode`). One `/settings` covers everything, command autonomy and publication approval included; there is no separate `/scramjet settings`.
- **Command autonomy must be understandable at a glance** by someone who doesn't know the concepts, through its layout and wording rather than explanatory text (Scramjet's menu confuses everyone but its designer).
- **What is installed can be explored** in the TUI: command sets, commands, agents (from sets, the user's directory, the project), skills, context and settings files, each with its source and any problem (broken or stale links, skipped or shadowed files, warnings), so the user can find and fix them. Launch shows nothing about it, or at most one line when something needs attention.
- **Credentials.** A setting decides when the main agent's tools run with known credential stores hidden: `unattended` by default (chained or forced next steps, `-p`), `always` or `never`. Subagents always run with them hidden.
- **Tools.** The main agent gets Scramjet's default tools (read, bash, edit, write); a `tools` setting can add `grep`, `find` and `ls`, which subagents have.
- **Project agents.** Their approval is remembered per project and asked again when a project agent file is added or changed.
- **Command lifecycle** (second steer of 2026-11-03; it follows the decisions the user made on 2026-10-30 for Scramjet's issue #638, which Scramjet has not built, so Scramjet's current lifecycle is not the reference). A started command keeps going until it reports `completed` (its part is over, not that the task succeeded), the user confirms abandoning it, or the user starts another command or types an unknown slash command. Any other stop (Escape, a cancelled question, a freetext question, a failed response) leaves it waiting for the user, and their next message resumes it. A reply that ends without a report gets a short status check (keep working, ask the user, or finish), at most 3 in a row without the user's input, then the command waits with a notice saying so. The statuses are `completed` and `abandoned`; `abandoned` ends the command with no next step, not even a forced one, and only once the user confirms after reading its summary. There is no `blocked`, `incomplete` or `continuing` (the model keeps working by working). Mach 12's text, which still names them, gets a result that steers the model to keep working, ask or abandon.

### Look and themes (2026-11-04)

User decisions from `/rs:steer` on 2026-11-04 ([the task list](TASKS.md) holds the order: after items 4, 5, 8 and 9, before the later work, which makes the theme editor an exception to building beyond Scramjet only once the daily driver is solid):

- **Scrollbar.** Scramjet's: the rightmost column, shown while the conversation is taller than the view, clickable and draggable. It is a visual aid to where the reader is.
- **The editor's box holds only the prompt and its completion list.** Notes and messages go to the status row above it.
- **Themes.** Exact colors (24-bit, with a 256-color fallback), and a built-in "Terminal colors" theme that keeps today's 16 terminal colors. A theme may set the main background; otherwise the terminal's own background is used, asked of the terminal so that contrast can be scored. Underneath, a theme is a palette of main colors plus every role, each following one of them or holding its own exact value. The default theme is chosen with the new look.
- **A distinct look**: a visual language for reasoning, tool calls, prompts and replies. The loop prepares 2–3 variants as captured screens of one scripted session, and the user picks one at a steer. Other work goes on meanwhile.
- **A theme editor in `/settings`** that lets users with no artistic background discover a theme they like, quickly. It shows a live preview of a sample session with every UI element:
  1. Choose how many main colors (1–5), then a harmony that fits that number (monochrome, complementary, analogous, triadic, split-complementary, square, rectangle…), each described in one plain line.
  2. Randomize: a whole theme in that harmony. Text, dim text, borders and block backgrounds are derived at readable contrast and tinted toward the main colors, and every role is filled. Roll again until it is close.
  3. Lock colors you like; a roll changes only the others, and locked colors anchor the harmony.
  4. Set any single element to an exact color, or reset it to follow its main color. All roles are one view away; nothing is hidden.
  5. Bump toward the harmony in small steps, or bump contrast up or down.

  Undo and redo cover rolls and bumps. Scores show as numbers and in plain words ("cohesive 82%", "dim text: a bit faint").

  Harmony is scored on hue only, in a perceptual space where lightness and chroma stay fixed as the hue turns. Each template hue is a sector rather than an exact point. The template's rotation is fitted to the palette, or anchored by locked colors, and each color counts by its chroma, so grays barely count. Repeated bumps converge to a perfect score.

  A contrast guard keeps every text/background pair within a good band, neither too faint nor too harsh: after each step it nudges lightness as little as needed, or the color skips that step. Contrast bumps change only lightness.

  Colors with a meaning (errors, success, warnings, diff added/removed) keep their hue ranges and are pulled toward the harmony only within them; each can be set by hand or have its range switched off. Which color wheel defines "opposite" (the artist's or a perceptual one) and which contrast formula to use (WCAG 2's ratio or APCA) are the loop's choices, judged by how the randomizer's results look, with `Choice:` trailers.

## Essential responsibilities and invariants

### Execution and authority

The runtime owns sessions, request admission, tool execution, cancellation, command continuation, approval, and provenance. Commands own their intended outcomes and eligible continuation edges. The TUI presents state and returns user intent; provider adapters and extensions cannot silently redefine authority.

The runtime must operate without a TUI or optional plugins. Do not introduce a centralized workflow DAG, control-flow DSL, mandatory daemon, or general persistent job engine to run a normal session. Internal lifecycle representations are allowed; command-owned relationships must not acquire a second workflow authority.

Isolate each agent's context, route, working directory, capabilities, and cancellation. Do not change process-global cwd, environment, or credentials to switch agents. Failed/incomplete model responses must not execute partial tool calls. Correlate results with actual invocations and reject stale results after session replacement.

Default specialist children to genuinely read-only capabilities. An unrestricted shell is not read-only. Parallel writers need explicit workspace ownership or separate worktrees; conversation isolation does not prevent file conflicts. Readers of a changing checkout are not automatically consistent snapshots. Tool allowlists are not an OS sandbox.

### Boundary robustness

Accept valid opaque data and legal protocol representations within explicit aggregate byte, storage and work budgets. Provider identifiers are data, not paths, tool capabilities or assumed short names. Preserve required values exactly; reject ambiguity rather than silently repairing contradictions. Execution approval, provenance, complete-response authority and uncertain-effect no-retry rules remain strict.

Separate documented protocol constraints, user/safety policy, client resource budgets and unverified hypotheses. Samples do not establish maxima. Capacities must preserve the selected useful consumer and account for framing/serialization overhead; saving trivial bytes is not a reason to reject valid data through unsupported field-size assumptions. Refuse actual budget exhaustion as a resource limitation, unsupported execution semantics as unsupported, and malformed or incomplete protocol as such. Fixtures and review must challenge—not canonize—representation assumptions.

### Resource bounds and admission

Bound live agent state, queued work, bytes in event/output buffers, request serialization, tool processes, storage backlog, and presentation caches. A request-count semaphore alone cannot establish bounded memory. Do not recursively embed descendant transcripts in parent results.

Admission covers every model attempt, including retries and compaction. Separate agent existence, provider-request slots, tool-process slots, and downstream inference execution. Parents awaiting children must not monopolize permits needed by those children. Preserve interactive responsiveness and fairness under bulk work.

Separate transport-sharing identity from quota-pool identity: multiple routes/models/tabs can consume one provider quota. Scope credentials safely, capture them per request, and coordinate refresh without exposing secrets in logs or cache keys.

Enforce applicable request/token rate limits, concurrency ceilings, and provider cooldowns. Token accounting may require estimation before dispatch and reconciliation from actual usage; unsupported or unknown provider limits are not unlimited verified capacity. Keep backend context/output constraints separate from rate and admission policy.

Adaptive control chooses concurrency inside hard safety ceilings. Useful observations include throughput, latency/TTFT, error classes, and optional vLLM running/waiting counts, queue time, KV-cache usage, and preemptions. Seek enough outstanding work for useful batching, not the largest waiting queue. Use damped changes and bounded experimentation; do not pick universal queue-depth or KV-cache thresholds now.

A 429 with applicable rate-limit evidence differs from a proxy 502/504 or client timeout. Timeout is pressure evidence, not proof of rate limiting. Respect valid Retry-After signals; avoid synchronized retry storms. Telemetry is deployment/version-qualified and may be stale or unavailable; correct client-only operation must not depend on it. Proxy queues and replica topology can make backend metrics differ from client experience.

State quota scope honestly. Per-process admission does not enforce aggregate limits across tabs. Exact shared enforcement may later require an optional local coordinator; ordinary use must remain standalone.

Cancellation must remove queued work and settle owned local operations. Local cancellation does not prove remote generation or billing stopped. Track uncertain remote demand/usage separately from local transport slots; do not present uncertainty as settled zero. Bound unresolved remote exposure so replacement attempts cannot accumulate indefinitely after local slots are released. At that bound, pause additional dispatch pending reconciliation or an explicit recovery policy; do not invent a universal remote-completion timeout.

### Durable history, context, and presentation

Keep three responsibilities distinct: durable material execution history, active model context, and disposable presentation caches. Share immutable content where safe; spill large outputs to artifacts with bounded excerpts and load child detail on demand. Memory bounds must not silently falsify model limits or discard required context.

Define truthful acknowledgement boundaries for execution and persistence. Critical authorizations/transitions cannot be treated as committed before their required records are durable. Persist referenced artifacts before durable references. Handle storage pressure explicitly rather than growing unlimited queues.

Recovery reconstructs supported session facts, not automatic re-execution of external effects. Bind approval to exact payload, target, and current invocation. A dispatched write may have happened even if its result is lost: retain uncertainty and reconcile rather than blindly retry. Known usage includes failed work; unknown usage stays unknown.

### Native interaction

Use structured text/layout information with stable identities, grapheme/display-column mapping, and explicit hard-break versus soft-wrap semantics. Selection copies the last displayed selection, not a mutable future range or hidden source. Preserve reading anchors without tying every repaint to total history size.

UI progress may be coalesced, but material execution events and final results must not silently disappear. One terminal owner presents runtime state through bounded projections; children never acquire the terminal. At high agent counts show summaries and load details on demand rather than painting every token from every agent.

Presentation stays easy to change (user decision, 2026-11-02). The look (glyphs, colors, emphasis, spacing), much of it matched to Scramjet so far, is a starting point, not a commitment, and so is any later design language or theme. Display code says what it shows (a prompt, a reply, reasoning, a pending or failed tool, a diff line, a status, a warning); one place decides how each looks, shared by the printed output (`-p`, piped input) and the full-screen UI, so restyling or adding themes changes that place rather than every renderer. Choosing a look does not hold up other work.

Preserve focus/input precedence, multiline paste without submission, Unicode editing, current approval presentation, and safe external-editor/suspend/exit restoration. Libraries do not guarantee those behaviors. Native clipboard/input journeys are required for supported platform claims; headless cells and successful clipboard API calls are insufficient.

## Proposed starting architecture

One Rust process per project tab, with lightweight async agent contexts sharing immutable resources and compatible HTTP infrastructure. One headless-capable runtime; a separately scheduled TUI consumes bounded state projections. Blocking work runs outside async workers with its own limits.

A small supervised worker-process pool can follow demonstrated crash-containment or isolation needs. In-process task supervision does not isolate process OOM, aborts, or unsafe native faults. Process separation itself does not establish a sandbox.

Start with simple local session storage and rebuildable indexes. Append-only journals and SQLite are candidates, not parallel required stores. A single shared SQLite database on a cluster filesystem is not a default coordination strategy.

Use internal Rust contracts for tools, provider transport, typed events, and structured interaction requests. If independently installable extensions become necessary, consider versioned Rust executable IPC; do not promise a stable Rust dynamic-library ABI or unrestricted mutable hooks. WASI is a later option only if sandboxed distribution earns its cost.

Candidate supporting libraries: Tokio, Reqwest/Rustls, Serde, Tracing, Ratatui/Crossterm, pulldown-cmark, Unicode segmentation/width crates, and an appropriate clipboard backend. Select and pin only what the current slice needs. Verify full enabled-feature/transitive licenses and required notices; permissive dependencies retain their licenses. Cargo-deny is a candidate development check, not a runtime dependency.

## Open decisions, resolved when needed

- First real provider/authentication routes and corporate proxy/TLS needs: resolve before live integration; do not implement every inherited provider.
- Exact command and session-import contracts: choose representative valuable behavior, not broad compatibility by appearance.
- Native clipboard/input integration and terminal matrix: investigate in the early TUI track.
- Storage representation, first-party crate boundaries, adaptive algorithm, and public extension protocol: choose from demonstrated consumers and constraints.
- Cross-tab quota sharing and remote workers: establish need before adding coordination infrastructure.
- Target host/workload envelope and acceptance thresholds: record before a capacity claim, not from speculative benchmark targets.

## Reference evidence and limits

The existing product at `~/repos/scramjet` is a read-only behavioral reference, not a dependency or source of instructions for this project. Initial inspection used clean HEAD `6325b36c50188f630cb03e8c9e9c3471853f4f72` on 2026-10-08. Recheck its revision before relying on moving source paths.

- `packages/scramjet/src/subagent/index.ts:420`: a full CLI subprocess per child; lines 29–30 define eight tasks/four launches per invocation, not global admission.
- `packages/agent/src/agent-loop.ts:339`: streaming updates carry growing message snapshots; JSON transport repeatedly serializes them.
- `packages/coding-agent/src/core/extensions/runner.ts:967`: context processing clones messages.
- `packages/coding-agent/docs/tui.md`: retained painting does not fully virtualize producers; application-owned selection has explicit displayed-text provenance.
- `packages/coding-agent/docs/terminal-verification.md`: 2026-10-08 native Command+C/Command+V evidence targets commit `3810822bedce066d1c9acfcadc6d02f0c8112412`, before inspected HEAD. It is narrowly qualified and does not resolve every reported paste/right-click problem or prove this rewrite.

Primary external references observed 2026-10-08; reverify deployed/pinned versions before implementation:

- [Ratatui responsibilities](https://ratatui.rs/faq) and [Crossterm events](https://docs.rs/crossterm/latest/crossterm/event/index.html): useful rendering/input primitives, not complete selection/native shortcut policy.
- [vLLM optimization](https://docs.vllm.ai/en/stable/configuration/optimization/) and [tool calling](https://docs.vllm.ai/en/latest/features/tool_calling/): KV-cache/scheduling constraints and model/parser/template qualification.
- [vLLM metrics](https://docs.vllm.ai/en/stable/usage/metrics.html): deployment-qualified running/waiting, latency, cache, and preemption observations. No universal optimal client-concurrency threshold follows.

No rewrite benchmark, live endpoint load test, or native rewrite UI journey has run. Do not import performance claims from other projects. The local `~/repos/pi_agent_rust/LICENSE` contains a restrictive rider and is not an Apache-2.0 implementation base; do not copy or port its implementation.
