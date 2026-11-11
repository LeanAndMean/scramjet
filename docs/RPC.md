# RPC mode: proposal

**Status: proposal, not signed off.** Nothing here is built. The user signs it off, changes it or drops parts of it at a steer ([design](DESIGN.md#rpc-mode-and-off-device-front-ends-2026-11-09)). The decisions it needs are listed at the end. Written 2026-11-09 against scramjet-rs 0.125.0 and Scramjet's `packages/coding-agent/docs/rpc.md`.

`scramjet --mode rpc [options]` runs headless. A program that starts it writes commands to its stdin and reads responses, events and requests from its stdout, one JSON object per line. A client written for Scramjet's RPC mode should mostly work: names and shapes follow Scramjet's wherever scramjet-rs has the feature. Diagnostics go to stderr, never stdout.

## 1. One core: what moves out of the full screen

Today the agent library (`src/agent`) runs a turn and reports it through `AgentUi` (`src/agent/ui.rs:28`). What happens between turns is in the full screen's loop, `Interactive` (`src/agent_cli/interactive.rs`):

| Part | Where it is today | Proposed |
|---|---|---|
| A turn, its pauses for commands typed meanwhile (`AgentError::Paused`, `resume_turn`) | Core, with the loop in `turn_on_screen` (`interactive.rs:414`) | Core loop |
| Routing a completed command's next step (forced, closed, open, ask; chains; autopilot) | Core: `Agent::route`, `status::route` (`status.rs:714`), `record_next_step` | Stays |
| Offering the steps, the autopilot countdown, the model and effort chosen for a step, a fresh session, running the chosen step, a text step into the editor | Front end: `next_step`, `offer`, `take_step`, `take_route` (`interactive.rs:523-694`), the countdown in the picker | Core loop; the front end only shows the list and returns the pick |
| Suggested steps (`suggest_scramjet_next_steps`), lists reopened by going back, a question asked again | Front end: `suggested`, `reopen`, `ask_again` | Core loop |
| Steering and follow-up queue, its modes | The screen's `Queue` (`screen.rs:208`), handed out by `Link` | Core loop owns the queue; each front end adds to it |
| `!` commands while a turn runs | The screen's thread (`screen.rs:1008`), results through `add_shells` | Core loop |
| Remembering a model or effort in the settings | Front end (`remember_model`, `set_default_thinking_level`) | Core loop, through the settings it is given |
| Built-in commands (`/model`, `/resume`, `/tree`, `/compact`, …) | `slash.rs` and friends, choosing through the screen's pickers | Each splits into an action (core loop) and its picker (full screen). RPC mode calls the actions as commands |

The core loop is a `Session` driver in the binary crate (`src/agent_cli/drive.rs`). It sits beside `Interactive` rather than in the library, because it needs the user's `Settings`. Front ends implement `Front`: `AgentUi` plus what happens between turns.
- `offer(Offer) -> Option<Picked>`: next steps and suggestions. `Offer` carries the steps, the recommended one and the countdown's deadline. `Picked` carries the step, the model, the effort and the fresh-session choice.
- `edit(text)`: puts text into the editor.
- `session_changed`: a new, resumed or forked session, or a move in the tree.
- `say(text)`: a message for the user.

The full screen, line mode and RPC mode drive the same `Session`. `-p` stays as it is.

**Build order, each stage committed working:**
1. Extract the driver and move the full screen and line mode onto it, with no change of behavior. The PTY journeys (`tests/agent_editor_pty.py`) and the scripted screens (`scripts/screens/*.json`) show it.
2. RPC mode with `prompt`, `abort`, `get_state` and the turn's events.
3. Requests: approvals, questions and next steps.
4. Sessions, the tree, models and the other commands.
5. Clients that attach later: `get_events`, and the waiting request in `get_state`.

## 2. Framing and envelope

These follow Scramjet:
- **Records:** LF is the only record separator; a trailing `\r` on input is stripped. A line that is not a JSON object gets `{"type":"response","command":"parse","success":false,"error":…}`.
- **Responses:** every command may carry an `id`, which its `response` repeats. Every command gets exactly one `response`. `success: true` on `prompt` means the prompt was accepted; a failure after that comes as events.

Added:
- **Start:** `{"type":"ready","protocolVersion":1,"version":"0.126.0"}` is the first record. `get_state` repeats `protocolVersion`. An incompatible change raises it.
- **Sequence numbers:** every event and request carries `seq`, numbered from 1 in each process, so a client can tell what it missed (section 6). Responses carry none.
- **End of input:** at EOF on stdin, a running turn is cancelled, an open request is answered as cancelled, and the process exits once the session file is written. SIGTERM does the same.

## 3. Commands

**Kept from Scramjet, same names and shapes:**

| Command | Notes |
|---|---|
| `prompt` | `message`, `images` (base64 with `mimeType`) and `streamingBehavior` (`steer`, `followUp`) during a turn, as Scramjet. A `/command` of a command set or a skill expands as typed. A built-in, such as `/model`, gets `success:false` naming the RPC command that does it (Choice 3). |
| `steer`, `follow_up`, `abort` | Into the core loop's queue; `abort` as Escape. |
| `new_session` | With an optional `parentSession`, as `/new`. |
| `get_state` | Scramjet's fields, plus `protocolVersion`, `seq`, `autopilot`, `ask`, `sandbox`, `cwd`, `pendingRequest` (section 5) and `waitingForUser` (a parked freetext question, or a command waiting). |
| `get_messages` | Section 4. |
| `set_model`, `cycle_model`, `get_available_models` | `Model` as Scramjet's, without headers or keys (section 7). `cycle_model` cycles `enabledModels`, as Ctrl+P. |
| `set_thinking_level`, `cycle_thinking_level` | The model's own levels, as Shift+Tab. |
| `set_steering_mode`, `set_follow_up_mode` | As the `steeringMode` and `followUpMode` settings, for this process only. |
| `compact`, `set_auto_compaction` | As `/compact` and `compaction.enabled`, the latter for this process only. |
| `set_auto_retry`, `abort_retry` | As `retry.enabled`; `abort_retry` cancels the wait, as Escape does. |
| `bash`, `abort_bash` | As `!`: the output goes into the conversation with the next prompt. `"exclude":true` is `!!`. |
| `get_session_stats` | From `/session` and the usage line: tokens, cost, context. |
| `export_html` | As `/export`. |
| `switch_session` | As `/resume <path>`. |
| `fork`, `clone`, `get_fork_messages` | As `/fork` and `/clone`. Entries are named by `entryId` (section 4). |
| `get_last_assistant_text`, `set_session_name` | As `/copy`'s text and `/name`. |
| `get_commands` | Command sets' commands (`source: "command"`, with the set), skills, and the built-ins RPC mode runs. |

**Added, for what scramjet-rs has and Scramjet's RPC lacks:**

| Command | What it does |
|---|---|
| `extension_ui_response` | Answers a request (section 5). The name is Scramjet's. |
| `hold_request` | Stops a next-step list's autopilot countdown, as a key pressed on the full screen does. |
| `get_sessions` | The sessions `/resume` lists, with their chains (`parentSession`), in this folder or all. |
| `get_tree`, `navigate_tree`, `set_label` | `/tree`: its entries with `entryId`, labels and the current leaf. Going to an entry, with an optional branch summary, gives back the prompt to edit. A list or question that was open there comes back as a request again. |
| `set_autopilot` | `/autopilot on|off`. |
| `get_events` | `{"since": seq}`: the events and requests after `seq` that are still held (section 6). |
| `reload` | `/reload`. |
| `get_usage` | `/usage`'s summary as data. |

**Dropped, with the reason:**
- `/login`, `/logout`: a sign-in opens a browser or shows a device code, and handles tokens; it stays in the terminal.
- `/settings`, themes, `/hotkeys`, `/keybindings`, the clipboard and `/changelog`: they belong to the screen, and the client has its own.
- `install`, `update`, packages: subcommands for a terminal.
- `--model-definition` (Scramjet's invocation-scoped models): not built (item 9).
- `extension_error`: there are no extensions.

## 4. Messages and entries

`get_messages`, `agent_end` and `message_end` give Scramjet's message shape:
- `role`: `user`, `assistant`, `toolResult` or `bashExecution`.
- `content`: blocks of type `text`, `thinking` (the reasoning summary shown) and `toolCall` (`id`, `name`, `arguments`).
- An assistant message also has `usage`, `stopReason` and `model`. Converting from the session's Responses-shaped items is done in one place.
- Encrypted reasoning is never sent.

Each message carries `entryId`: its line in the session file. Lines are append-only, so the id stays valid for the session. `fork`, `get_tree` and `navigate_tree` take these ids. Compactions and branch summaries come as `compaction` and `branchSummary` entries, as `/tree` lists them.

## 5. Approvals, questions and next steps as requests

The core asks the user for an answer at six points:
- `--ask` approvals
- publication approvals
- the agent's select and confirm questions
- a status report's abandon question
- project agents' consent
- next-step and suggestion lists

In RPC mode each is an `extension_ui_request` with an `id` and `method` `select` or `confirm`, as Scramjet's. A Scramjet client shows them as plain dialogs. In Scramjet's own RPC mode they never reach the client: its next-step lists, questions and publication approval are `ctx.ui.custom()` widgets, which RPC mode answers with nothing (`rpc-mode.ts:227`), so they come out dismissed. Added fields say more:

```json
{"type":"extension_ui_request","seq":41,"id":"r7","method":"select","kind":"approval",
 "title":"Allow bash?","options":["Allow","Allow all this session","Deny"],
 "tool":{"name":"bash","arguments":{"command":"rm -rf target"}},"cwd":"/home/u/p"}
```

- `kind`: `approval`, `publication`, `question`, `abandon`, `projectAgents`, `nextStep` or `suggestion`.
- `optionDetails`: one entry per option, with its `label`, `description` and whether it is `recommended`.
- `message`: the question's or the publication's markdown, which the full screen shows before the options. For a publication this is the whole preview: repository, title and body.
- Approvals carry `tool` (name and arguments). For `mcp_call` they also carry the arguments the user has not seen.
- Next steps: each option has its `command` or `text`, `reason` and `freshSession`. With autopilot the request has `autoselectMs`.

**Answers:** `extension_ui_response` with the request's `id` and one of:
- `value`: the option's label, as Scramjet's.
- `index`: its position.
- `confirmed`: for confirm.
- `cancelled: true`: dismisses the request.

A next-step answer may add `model`, `thinkingLevel` and `freshSession`, as the full screen's list lets the user change them.
- A response to the answer says whether it was taken. The answer is refused if the request is gone or the index is out of range.
- An `extension_ui_request_closed` event with `id` and `outcome` (`answered`, `cancelled`, `autoselected`, `abort`) tells every client that the request is no longer open.

**Rules:**
- **One request at a time.** Parallel tool calls' approvals queue in order, as the full screen asks them.
- **No timeout.** As on the full screen, an approval waits until it is answered, cancelled, the turn is aborted or input ends. The only timed request is autopilot's countdown, which the core runs: the recommended step is chosen when `autoselectMs` passes without an answer or a `hold_request`.
- **Freetext questions.** They are not requests: as on the full screen, the turn ends and the next `prompt` answers. A `question` event carries the message, and `get_state.waitingForUser` says so.
- **Who answers.** The client's answers count as the user's, with the same policy as the full screen. A publication the full screen would ask about is asked as a request, not refused as Scramjet's RPC mode refuses it (Choice 1).
- **Other messages.** Notices and other fire-and-forget messages are `extension_ui_request` with method `notify` and no answer, as Scramjet's.

## 6. Events

Scramjet's events, from the `AgentUi` calls that exist:

| Event | From |
|---|---|
| `agent_start`, `agent_end` (messages of the turn) | A turn starts and ends; `stopped` adds `error` or `cancelled` to `agent_end`. |
| `turn_start`, `turn_end` | Each model request (`waiting`) and its end. |
| `message_start`, `message_update`, `message_end` | `progress`. `text_delta` and `thinking_delta` stream. `toolcall_start` comes when a call starts streaming. `toolcall_end` carries the parsed call. No `toolcall_delta`: the parser does not report arguments as they stream. |
| `tool_execution_start`, `_update`, `_end` | `tool_call` and `tool_running`, `tool_output` (the end of the output so far, which replaces the last), `tool_result`. Parallel calls are told apart by `toolCallId`. |
| `queue_update` | The core loop's queue changes. |
| `compaction_start`, `compaction_end` | `waiting(Compaction)`, `compacted`, `compaction_failed`; `reason` is `manual`, `threshold` or `overflow`. |
| `auto_retry_start`, `auto_retry_end` | `retrying` and the request's end. |

Added (unknown types are skipped by a Scramjet client):
- `subagents`: the children of a running `subagent` call, as the serialized `Children` (`ui.rs:286`).
- `cooldown`: a request held for a provider's Retry-After (`cooling`).
- `usage`: session tokens, context and output rate after each response.
- `model_changed`: from `switch_scramjet_model` or a next step's model.
- `bash_start` and `bash_end`: `!` commands, also those of `bash` while a turn runs.
- `session_changed`: new, resumed, forked or moved in the tree, with `sessionFile` and the leaf's `entryId`.
- `next_step`: Scramjet's record of the step chosen or the list dismissed.
- `question`: a freetext question's message.

**A client that attaches later** (a browser front end's server reconnecting a phone) gets the whole state from `get_state`, `get_messages` and, if a request is open, `pendingRequest`, the same object that was sent. `get_state.seq` is the last sequence number sent. Events of the running turn are held from its `agent_start` on, at most 8 MB, oldest dropped first. `get_events {since}` gives those after `since`, with `"complete":false` when some were dropped. In that case the client takes `get_messages` again after `agent_end`.

## 7. Nothing secret in the output

The rule: no credential, token or auth header in any event, response, request or error.
- **Models:** `Model` objects have `id`, `name`, `provider`, `api`, `baseUrl`, limits, `reasoning`, `input` and `cost`. They never have `headers`, `apiKey` or a provider's command.
- **Errors:** provider errors are passed on today as `HTTP {status}: {body}` (`client.rs:412`), and a body could echo a header. Before any string leaves the process, the core replaces every secret it holds with `[redacted]`: the routes' header values, the keys resolved from `auth.json`, commands and variables, and MCP servers' header values. The same applies to the full screen's error lines. The unit test is a scripted provider that echoes its `Authorization` header.
- **Tool and `!` output:** sent as it is, as the session file keeps it: it is what the user's commands printed, and hiding credential stores (`hideCredentials`) is what keeps it clean.
- **Sign-in:** stays in the terminal (section 3).

## 8. While a turn runs

A thread reads stdin. During a turn:
- The core answers queries (`get_state`, `get_messages`, `get_events`, `get_commands`, `get_available_models`, `get_session_stats`, `get_tree`, `get_sessions`).
- It applies `steer`, `follow_up`, `abort`, `abort_retry`, `abort_bash`, `extension_ui_response`, `hold_request`, `set_steering_mode` and `set_follow_up_mode`.
- `set_model`, `cycle_model`, the thinking levels and `set_autopilot` pause the turn between its requests, as the same built-ins typed on the full screen do (`Builtin::interrupts`), and it goes on after them.
- `bash` runs alongside the turn, as a `!` typed meanwhile does. Its output goes into the conversation when the turn ends.
- Those that would stop the turn get `success:false` with `"busy"`: `new_session`, `switch_session`, `fork`, `clone`, `navigate_tree`, `compact` and `reload`, which the full screen also refuses during a turn. The client aborts first if it means to.

## 9. Not in this proposal

- A listening port, HTTP, WebSocket, authentication or TLS; a daemon with the full screen as a client ([design](DESIGN.md#rpc-mode-and-off-device-front-ends-2026-11-09)).
- Attaching to a session started in the terminal: an open question of its own ([tasks](TASKS.md), Open questions). Section 6's state and catch-up would serve a local socket, if one is chosen.
- `--mode json` (item 9): the same events for one `-p` turn, without commands or requests. It would come after stage 2 with no extra design.
- Several sessions in one process: a client starts one process per session.

## 10. To verify

- Each stage: scripted clients in Python (standard library) over pipes against the release build and a local scripted provider (`serve` in `src/agent/tests.rs`). They check framing (U+2028 inside strings, `\r\n`, a bad line), every command's response, the events of a turn with parallel calls, approvals answered, cancelled and at EOF, a next-step list with the countdown and with `hold_request`, a fresh-session step, a client attaching mid-turn, and the redaction.
- Stage 1: the full screen unchanged, through the PTY journeys and the scripted screens.
- 1–5 live requests through the company's proxy for the streamed events (claude-haiku-5-5, gpt-6-luna).

## Decisions for the sign-off

1. **Approvals and publications in RPC mode** are asked of the client as requests, counting as the user's answer under the same policy as the full screen. The alternative is to refuse them as Scramjet's RPC mode does.
2. **The core loop owns the next-step countdown** under autopilot, so `autoselectMs` passes the same with or without a client watching. The alternative is to leave timing to each client, so no step is autoselected when no client is attached.
3. **Built-ins through `prompt`** (`"/model x"`) are refused and point to the RPC command. Scramjet's RPC mode does not run built-ins either. The alternative is to run the ones that have an RPC command.
4. **Scramjet's message shape** (section 4) rather than the session file's Responses-shaped items, so that Scramjet clients render them. The alternative is the raw items, which are complete but provider-shaped.
5. **Sign-in stays in the terminal**: there is no `/login` over RPC.
6. **The build order** of section 1, starting with the refactor of the full screen onto the shared core, which risks its behavior. It is checked against the PTY journeys and scripted screens before anything is added.
