# RPC mode on ACP: proposal

**Status: signed off at the steer of 2026-11-12 and rewritten around the Agent Client Protocol (ACP) on 2026-11-12; stage 1 partly built, stage 2 done (`src/agent_cli/acp.rs`, 2.199.0–2.207.0), stage 3 done (2.208.0–2.210.0), stage 4 begun (2.210.0; sessions 2.212.0–2.214.0, `compact` and `rename` 2.215.0, `export_html` and `usage` 2.216.0, `fork`, `branch` and `tree` 2.217.0, `navigate_tree` and `set_label` 2.218.0, `reload` 2.219.0)** ([design](DESIGN.md#rpc-mode-and-off-device-front-ends-2026-11-09)). The user's decisions stand: approvals and questions asked of the client (1), the autopilot countdown in the core (2), built-ins sent as prompt text refused (3), ACP in place of Scramjet 1's protocol and message shape (4), sign-in over the protocol by `authenticate` and `logout` (5), and the build order with the full screen's refactor first (6). The choices this rewrite made within them are `Choice:` trailers in its commit, for the next steer. Written against scramjet-rs 2.198.0 and ACP v1's pages at agentclientprotocol.com (`/protocol/v1/*.md`, fetched 2026-11-12).

`scramjet acp [options]` runs headless as an ACP agent: the client starts it, writes JSON-RPC 2.0 to its stdin and reads its stdout, one message per line. ACP's clients (Zed, JetBrains IDEs, Neovim, Emacs's agent-shell, Toad, acpx) can run Scramjet, and what ACP lacks comes as extensions that other clients ignore. The options are the full screen's (`--model`, `--ask`, `--sandbox`, `--tools`, …); diagnostics go to stderr, never stdout.

**ACP v1.** v1 is the version clients speak. v2 is labelled draft and its migration guide tells agents to keep serving v1, so a client asking for 2 is answered 1 for now. v2's changes (the prompt's response only acknowledging it, updates as upserts) are added beside v1 once v2 is stable and a client needs it.

## 1. One core: what moves out of the full screen

Today the agent library (`src/agent`) runs a turn and reports it through `AgentUi` (`src/agent/ui.rs:28`). What happens between turns is in the full screen's loop, `Interactive` (`src/agent_cli/interactive.rs`):

| Part | Where it is today | Proposed |
|---|---|---|
| A turn, its pauses for commands typed meanwhile (`AgentError::Paused`, `resume_turn`) | Core, with the loop in `drive::run_turn` (stage 1); `Front::paused` runs what was typed | Core loop |
| Routing a completed command's next step (forced, closed, open, ask; chains; autopilot) | Core: `Agent::route`, `status::route`, `record_next_step` | Stays |
| Offering the steps, the autopilot countdown, the model and effort chosen for a step, a fresh session, running the chosen step, a text step into the editor | Front end: `next_step`, `offer`, `take_step`, `take_route` (`interactive.rs:521-690`), the countdown in the picker | Core loop; the front end only shows the list and returns the pick |
| Suggested steps (`suggest_scramjet_next_steps`), lists reopened by going back, a question asked again | Front end: `suggested`, `reopen`, `ask_again` | Core loop |
| Steering and follow-up queue, its modes | The screen's `Queue` (`screen.rs`), handed out by `Link` | Core loop owns the queue; each front end adds to it |
| `!` commands while a turn runs | The screen's thread, results through `add_shells` | Core loop |
| Remembering a model or effort in the settings | Front end (`remember_model`, `set_default_thinking_level`) | Core loop, through the settings it is given |
| Built-in commands (`/model`, `/resume`, `/tree`, `/compact`, …) | `slash.rs` and friends, choosing through the screen's pickers | Each splits into an action (core loop) and its picker (full screen). ACP mode calls the actions from its methods |

The core loop is a `Session` driver in the binary crate (`src/agent_cli/drive.rs`). It sits beside `Interactive` rather than in the library, because it needs the user's `Settings`. Front ends implement `Front`: `AgentUi` plus what happens between turns.
- `offer(Offer) -> Option<Picked>`: next steps and suggestions. `Offer` carries the steps, the recommended one and the countdown's deadline. `Picked` carries the step, the model, the effort and the fresh-session choice.
- `edit(text)`: puts text into the editor.
- `session_changed`: a new, resumed or forked session, or a move in the tree.
- `say(text)`: a message for the user.

The full screen, line mode and ACP mode drive the same `Session`. `-p` stays as it is.

**Build order, each stage committed working:**
1. Extract the driver and move the full screen and line mode onto it, with no change of behavior. The PTY journeys (`tests/agent_editor_pty.py`) and the scripted screens (`scripts/screens/*.json`) show it.
2. `scramjet acp` with `initialize`, `session/new`, `session/prompt`, `session/cancel` and the turn's updates (sections 2–4); tried in an ACP client if one runs here.
3. Requests: permissions, elicitations and next steps (section 5).
4. Sessions (`session/load`, `resume`, `list`, `close`, `delete`), config options, the extensions' methods, and sign-in (sections 3, 6–8).
5. Catch-up: sequence numbers, `state` and `events` (section 9).

## 2. Transport and initialization

- **Framing:** ACP's stdio transport: UTF-8, one JSON-RPC message per line, no embedded newlines (U+2028 and U+2029 are written escaped, which `serde_json` does not do by itself); a trailing `\r` on input is stripped. A line that is not JSON gets `-32700`, one that is not a request `-32600`, and any request before `initialize` `-32600`. Batches are refused.
- **`initialize`:** answered with `protocolVersion: 1`, `agentInfo` (`name: "scramjet"`, `title: "Scramjet"`, the version) and `authMethods` (section 6). `agentCapabilities`:
  - `loadSession: true`; `sessionCapabilities` with `list`, `resume`, `close` and `delete`.
  - `promptCapabilities`: `image` and `embeddedContext` true, `audio` false.
  - `mcpCapabilities`: `http` true, `sse` false (the older transport is refused, as in `settings.json`).
  - `auth.logout: {}`.
  - `_meta["io.github.leanandmean.scramjet"]`: `{"version": 1, "methods": [...]}`, the extension methods the client may call (section 8).
- **What the client offers** is kept per connection: `elicitation.form` and `.url` (section 5), `session.notices`, `session.compaction`, `session.configOptions.boolean`, and the `_meta["io.github.leanandmean.scramjet"]` object of a client that knows Scramjet's extensions (next steps, catch-up). An update or request the client has not offered is not sent; its fallback is said where it applies.
- **The client's file system and terminals are not used.** Scramjet's own tools run every call, so the sandbox, the low integrity level on Windows, approvals and the output the model sees stay those of the full screen. The cost is that an IDE's unsaved buffers are not seen, and its terminal panel does not show the agent's commands.
- **End:** at EOF on stdin, every running turn is cancelled, requests waiting on the client are dropped, and the process exits once the session files are written. SIGTERM does the same.

## 3. Sessions

ACP lets a connection hold several sessions, so a client may open several threads on one process. Each is a `Session` driver of its own, with its own working directory, model, queue and turn; they run independently. Settings, logins, the 8 child slots and the providers' cooldowns are shared, as they are today between a process's children.

- **`session/new`** `{cwd, mcpServers, additionalDirectories?}`: `cwd` is the project, as `--project`. The client's MCP servers join the user's for that session (stdio and HTTP); their `env` and `headers` values are never echoed, logged or written to the session file, which records only their names. `additionalDirectories` is not offered yet; `/add-dir`'s directories would be its counterpart. The result has the `sessionId`, the `configOptions` (section 7), and `_meta` with the session file's path. `available_commands_update` follows (section 4).
- **The `sessionId`** is the session file's id (`<created>-<pid>`, as in its name), so `session/list` and the files agree.
- **`session/load`** `{sessionId, cwd, mcpServers}`: replays the current branch as updates: `user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk` (the reasoning summary shown, never encrypted reasoning), each tool call as one `tool_call` with its final status and content, compactions and branch summaries as `compaction_update` if the client offered `session.compaction` (a branch summary's with `_meta["io.github.leanandmean.scramjet"].branch`), else as an agent message; a turn that failed as an agent message with its error. A write's diff is left out, as whether it made its file is not known. The response comes after the last update. A `cwd` other than the session's is refused with `-32602`.
- **`session/resume`:** the same without the replay.
- **`session/list`** `{cwd?, cursor?}`: `/resume`'s list, newest first, 50 a page, every project's when `cwd` is omitted. Each has `sessionId`, `cwd`, `title` (its name, else its first prompt), `updatedAt`, and `_meta` with `parentSession` (its chain) and the session file's path.
- **`session/close`:** cancels the session's turn and drops its driver; the file stays. **`session/delete`:** as Ctrl+D in `/resume` (to the trash where a `trash` command exists): refused for a session active on the connection.
- **Forks, branches, the tree, names:** extensions (section 8). A new session from a next step's `freshSession` or `/clear` in the core sends `_io.github.leanandmean.scramjet/session_changed` `{sessionId, newSessionId}` to a client that knows the extension, whose updates then carry the new id; either id reaches the session. Other clients keep the id they know for the thread, which goes on in the new session, said in an agent message (a notice with `session.notices`); `session/list` shows both files.

## 4. A prompt turn

- **`session/prompt`** `{sessionId, prompt}`: the blocks become one user message: `text` as typed; `image` sent with it as a command line's `@` image is (fitted to size, left out for a model without image input; with an image the text is never a command); `resource` (embedded) as text headed by its URI, and `resource_link` as its path or URI in the text. A prompt starting with `/` runs a command set's command or a skill as typed (`/rs:loop …`). A built-in (`/model`, `/compact`, …) is refused with `-32602`, naming the method that does it, e.g. `session/set_config_option` with `configId: "model"`.
- **The response** `{stopReason}` comes when the turn ends, next steps that run on their own (forced, or autopilot's) included: `end_turn`; `cancelled` after `session/cancel`; `max_tokens` when the model stopped on its output limit; `refusal` when the provider reports one. A turn that fails for good (a provider's error after its retries, a broken stream) gets a JSON-RPC error `-32603` with the redacted message (section 10) and `data: {"kind": …}`.
- **A prompt during a turn** of the same session is queued as the full screen queues a message typed meanwhile, by `steeringMode`, or as a follow-up with `_meta["io.github.leanandmean.scramjet"]: {"queue": "followUp"}`. Its images are saved to temporary files whose paths go before its text, as the full screen pastes an image, and the model reads them with `read`. Its response comes when the turn that took it ends. ACP does not say what a second prompt means.
- **`session/cancel`:** as Escape: the request and the running tools stop, a pending permission is answered as cancelled, and the response is `cancelled`, never an error.

**Updates** (`session/update`), from the `AgentUi` calls that exist:

| ACP update | From |
|---|---|
| `user_message_chunk` | A prompt the client did not send: a next step's, a follow-up's, a command's or skill's expansion (a message of its own, `_meta["io.github.leanandmean.scramjet"].expands` the prompt as typed); and replays. |
| `agent_message_chunk`, `agent_thought_chunk` | `progress`: text and reasoning deltas, with a `messageId` per model response. |
| `tool_call` | `tool_call`: `toolCallId`, `name`, `title` (the summary the full screen shows), `kind` (`read`; `edit` for edit and write; `search` for grep and find; `execute` for bash, PowerShell and cmd; `other`), `status: pending`, `rawInput` (the arguments), `locations` (the path of a read, edit or write). |
| `tool_call_update` | `tool_running` → `in_progress`; `tool_output` → `content` with the output so far, replacing the last; `tool_result` → `completed` or `failed` with the output as text, and for edits a `diff` block per edit (`path`, `oldText`, `newText`, from its arguments) and for a write that made its file one with the new text (`oldText` null); a write replacing a file gives its output only, as its old text is not read. A `subagent` call's children go in its `_meta` (`children`: `mode` and `runs`, each with its agent, task, state, items and usage), as they run and in its result; its content meanwhile is a line per child. |
| `usage_update` | `usage`: `used` and `size` are the context and the window, `cost` the session's in USD; tokens and the output rate in `_meta`. |
| `compaction_update` | `waiting(Compaction)` → `in_progress`; `compacted` → `completed` with the summary; `compaction_failed`, or the turn failing during it, → `failed` with the error; cancelled during it → `cancelled`. Without `session.compaction`, an agent message of its own says how it ended. |
| `notice` | `notice`, `retrying` (warning), `cooling` (info). Without `session.notices`, a notice that matters goes as an agent message and the rest are dropped. |
| `available_commands_update` | At session start and after `reload`: command sets' commands and skills, with `description` and `input.hint` from `argument-hint`. Built-ins are not listed. |
| `config_option_update` | The model or level changed by `switch_scramjet_model` or a next step. |
| `session_info_update` | The session renamed (`rename`) or titled by its first prompt. |

**Extension notifications** (`_io.github.leanandmean.scramjet/…`, ignored by other clients):
- `status`: what the full screen's working line says (waiting for the model, compacting, retrying in N s, held for a provider's Retry-After).
- `next_step`: Scramjet's record of the step chosen or the list dismissed.
- `question`: a freetext question was asked; the turn has ended and the next prompt answers it.
- `user_command`: a `!` command's start and result (section 8's `bash`).
- `session_changed`: section 3.

## 5. Requests to the client

The core asks the user at six points. Each becomes a request to the client, whose answer counts as the user's under the same policy as the full screen (decision 1):

| Point | Request | Without the client's support |
|---|---|---|
| `--ask` approvals | `session/request_permission` with the call (`toolCallId`, `title`, `kind`, `rawInput`, `locations`) and options `allow_once` "Allow", `allow_always` "Allow all calls this session", `reject_once` "Deny" | Every client has it. |
| Publication approvals | `session/request_permission` on the publication's call, its whole preview (repository, title, body) as text content | As above. |
| Project agents' consent | `session/request_permission` on the `subagent` call, naming the agents | As above. |
| The agent's select and confirm questions | `elicitation/create`, form mode: a select is a required string property with `oneOf` (`const` and `title` per option, the recommended one as `default`), a confirm a required boolean | Asked as a freetext question: the turn ends and the next prompt answers. |
| A status report's abandon question | `elicitation/create`, a confirm | As above. |
| Next-step and suggestion lists | `_io.github.leanandmean.scramjet/next_step` (below) | The list goes out as an agent message; forced steps and autopilot still run. |

- **`next_step`** `{sessionId, kind: "nextStep"|"suggestion", steps: [{message, reason, freshSession}], recommended, autoselectMs?, model, thinkingLevel}`. The result is `{"index": n}`, optionally with `model`, `thinkingLevel` and `freshSession` as the full screen's list lets the user change them, or `{"cancelled": true}`.
- **The countdown** runs in the core (decision 2): when `autoselectMs` passes with no answer, the core takes the recommended step and sends `$/cancel_request` for its request. `_io.github.leanandmean.scramjet/hold` `{sessionId}` stops it, as a key on the full screen does. With no client that knows the extension, the countdown runs the same, and a prompt sent during it stops it and is sent as usual.
- **One request at a time** per session: parallel calls' approvals are asked in order, as the full screen asks them. Different sessions' requests may be open together.
- **No timeout,** as on the full screen: a request waits until it is answered, the turn is cancelled, the session closed or input ends. An outcome Scramjet does not recognize is a denial.
- **The answer refused:** a `selected` with an `optionId` not offered, or an elicitation whose content does not fit its schema, is a denial and is said in a notice.

## 6. Sign-in

- **`authMethods`:** one `agent`-type method per sign-in `/login` offers: ChatGPT, GitHub Copilot, and each package's login program, with `id` (`chatgpt`, `copilot`, `<package>/<login>`), `name` and `description`. API-key providers and local models are not methods (below). No `terminal` method: scramjet-rs has no login-only invocation yet.
- **`authenticate {methodId}`:** runs that sign-in. Its URL, or its device code with the page to enter it, goes to the client as `elicitation/create` in URL mode, the code in `message`; a client without URL mode gets it in a `notice`, and one with neither an error saying to sign in from a terminal with `/login`. The response is `{}` once the sign-in ends, or an error saying why it failed. The tokens stay in `auth.json`, never in the output (section 10).
- **API keys:** `_io.github.leanandmean.scramjet/set_api_key` `{provider, key}` keeps the key in `auth.json`, as `/login`'s key entry does. ACP forbids asking for secrets in a form, so the client asks in its own field; the key is never echoed.
- **`logout {}`:** signs out of every method in `authMethods`; `_meta["io.github.leanandmean.scramjet"]: {"methodId": …}` signs out of one. Running sessions keep going until a request is refused.
- **`-32000` (authentication required):** `session/new` or a prompt whose model's provider has no login.

## 7. Models and levels as config options

`configOptions`, in every session's result, `session/set_config_option`'s answer and `config_option_update`:
- `model` (category `model`): the models `/model` lists, grouped by provider, values `provider/id`.
- `thought_level` (category `thought_level`): the current model's levels, as Shift+Tab cycles them; it changes with the model, and is left out for a model with no levels. With no level set, the current value is `default` (the model's own), listed first.
- `autopilot` (category `_autopilot`): a boolean for clients offering boolean options, else a select `on`/`off`; as `/autopilot on|off`.

`--ask` and `--sandbox` stay launch options. A change between turns applies at once; one during a turn waits for the turn's next pause (before a request, or while tool calls run), as the same built-ins typed on the full screen do (`Builtin::interrupts`), and is answered then; while a next-step list is open it applies at once, and autopilot turned off stops the countdown. The answer carries the options; `config_option_update` is sent only for changes the client did not make. A model or level chosen is remembered in the settings as the full screen remembers it. Session modes (deprecated in favor of config options) are not offered.

## 8. Extensions

Methods named `_io.github.leanandmean.scramjet/<name>`, following ACP's reverse-DNS convention for a domain the project controls (its GitHub account's Pages domain). Every one takes `sessionId` unless said:

| Method | What it does |
|---|---|
| `next_step` (agent → client), `hold` | Section 5. |
| `set_api_key` | Section 6. No `sessionId`. |
| `fork` `{entryId}` | `/fork` before the prompt at `entryId`: the result `{sessionId, prompt}` has the new session's id and the prompt to edit; a next-step list offered there is named in a notice. |
| `branch` `{name?}` | `/branch`: the whole conversation in a new session, named `name` when given; the result `{sessionId}`. |
| `tree` | `/tree`'s entries: `{entries, currentId}`, each entry with `entryId`, `parentId`, `active` (on the current path), `label` when given, and its `kind` with what it shows: `prompt`, `command`, `reply` and `branchSummary` with `text`, `toolCall` and `toolResult` with `name` and `summary`, `stopped` with `error`, `compaction` with `tokensBefore`, `model` with `model`, `thoughtLevel` with `level`, `reasoning`. During a turn it waits for the turn's next pause, as `rename` does. |
| `navigate_tree` `{entryId, summarize?}` | `/tree`'s moves: the result `{moved, prompt}`, `moved` false when the conversation already ends there, `prompt` the prompt gone back to, to edit, else null. `summarize` (`true`, or a focus as a string) summarizes the branch left first, shown as `compaction_update`s with `_meta["io.github.leanandmean.scramjet"].branch`; `session/cancel` stops it (`-32800`) and nothing moves. A question gone back to is asked again and its turn goes on: the result is then `{stopReason}`, when that turn ends, as a prompt's. A next-step list offered there is named in a notice, as `fork` does. Refused during a turn. |
| `set_label` `{entryId, label?}` | `/tree`'s labels: the result `{label}`, the label given, null when taken away (no `label`, or an empty one). During a turn it waits for the turn's next pause, as `rename` does. |
| `compact` `{instructions?}` | `/compact`: shown as `compaction_update`s; the result `{compacted}` says whether anything was old enough to compact. `session/cancel` stops it (`-32800`); prompts sent meanwhile run after it. |
| `bash` `{command, exclude?}`, `abort_bash` | `!` and `!!`: the output goes into the conversation with the next prompt. |
| `rename` `{name}` | `/rename`, then `session_info_update` with the title. During a turn it waits for the turn's next pause, as a config option does. |
| `reload` | `/reload`. No `sessionId`. |
| `usage` | `/usage`'s summary as data: `quotas` (each provider's latest, as saved), then `today`, `thisWeek`, `last30Days` and the last 30 days' `models`, each with `requests`, `inputTokens` (the cache's included), `outputTokens`, `cacheReadTokens`, `cost` in USD and `unpriced` (some responses had no price). Read from every saved session, answered at once, also during a turn. No `sessionId`. |
| `export_html` `{path?}` | `/export`: the current branch as a web page, or a session file for a path ending in `.jsonl`; a relative path is within the project, the default `scramjet-session-<id>.html`. The result `{path}` is the file written, null before any conversation. During a turn it waits for the turn's next pause, as `rename` does. |
| `state`, `events` | Section 9. |

`entryId` is an entry's line in the session file. Lines are append-only, so the id stays valid for the session; the replayed messages carry theirs in `_meta`.

**Not offered:** `/settings`, themes, `/hotkeys`, `/keybindings`, the clipboard and `/changelog` (the client has its own); `install`, `update` and packages (subcommands for a terminal).

## 9. A client that comes back

ACP over stdio has one client per process. A browser front end's server keeps the process and its connection, and reconnects a phone that slept: it needs to know what the phone missed.
- Every `session/update`, extension notification and request to the client carries `_meta["io.github.leanandmean.scramjet"].seq`, numbered from 1 per session, offered only to a client that knows the extension.
- `state` gives what the full screen shows: whether a turn runs, the open request (the same params that were sent, with its id), whether a freetext question waits, the queue, autopilot, `--ask`, the sandbox, the model and level, usage, and the last `seq`.
- Updates are held from a turn's start, at most 8 MB, oldest dropped first. `events {since}` gives those after `since`, with `complete: false` when some were dropped; the server then takes `session/load`'s replay after the turn.

## 10. Nothing secret in the output

No credential, token or auth header in any update, response, request or error:
- **Models:** config options carry names and ids only, never `headers`, `apiKey` or a provider's command.
- **Errors:** a server's failure is passed on as `HTTP {status}: {body}`, and a body could echo a header. Where an answer becomes a failure's text (model requests, model lists, logins, downloads, MCP's HTTP transport; `src/agent/redact.rs`), the values of the headers that request carried are replaced with `[redacted]`, each whole and without its scheme (`Bearer `): the routes' headers, the keys from `models.json` and `auth.json`, commands' output, MCP servers' headers, the client's included. Headers that carry nothing secret (`anthropic-version`, `user-agent`, …) and values under 8 characters are kept. The full screen's error lines are the same text. A command's own failure never shows what it printed (`configured.rs`). Tested with a scripted provider and MCP server that echo what they were given.
- **Tool and `!` output:** sent as it is, as the session file keeps it: it is what the user's commands printed, and hiding the credential stores (`hideCredentials`) is what keeps it clean.
- **Sign-in:** URLs and device codes go out; tokens and keys only come in.

## 11. While a turn runs

A thread reads stdin, so a session answers while its turn runs:
- Queries (`session/list`, `state`, `events`, `usage`) and the other sessions' methods are answered at once; `tree` at the turn's next pause, as it reads the session's own state.
- `session/cancel`, the answers to requests, `hold`, `abort_bash` and a prompt (queued, section 4) apply at once.
- `session/set_config_option` waits for the turn's next pause (section 7).
- `bash` runs alongside, as a `!` typed meanwhile does.
- Those that would replace the session's state are refused with `-32602` and "a turn is running": `session/load` and `resume` of the same session, `fork`, `branch`, `navigate_tree`, `compact` and `reload`, which the full screen also refuses during a turn. The client cancels first if it means to.

## 12. Not in this proposal

- A listening port, HTTP, WebSocket, authentication or TLS; a daemon with the full screen as a client ([design](DESIGN.md#rpc-mode-and-off-device-front-ends-2026-11-09)). ACP's remote transport is a draft RFD.
- Attaching to a session started in the terminal: an open question of its own ([tasks](TASKS.md), Open questions). Section 9 would serve a local socket, if one is chosen.
- ACP v2, the client's file system and terminals (section 2), session modes, `plan` updates (scramjet-rs has no plan tool), audio, `additionalDirectories`, `systemPrompt` on session setup.
- `--mode json` (item 9): the same updates for one `-p` turn, after stage 2.

## 13. To verify

- Each stage: scripted clients in Python (standard library) over pipes against the release build and a local scripted provider (`serve` in `src/agent/tests.rs`). They check framing (U+2028 inside strings, `\r\n`, a bad line, a request before `initialize`), every method's result and errors, a turn's updates with parallel calls, two sessions at once, permissions answered, cancelled and at EOF, elicitations and their fallbacks, a next-step list with the countdown and with `hold`, a fresh-session step, a client coming back mid-turn, and the redaction.
- ACP's Test Compatibility Kit (`agentclientprotocol/acp-tck`, experimental) once stage 2 runs, if it runs here without more than a scratch environment; and a real client (Toad installs with `uv`; Zed and JetBrains need the user's desktop).
- Stage 1: the full screen unchanged, through the PTY journeys and the scripted screens.
- 1–5 live requests through the local proxy for the streamed updates (claude-haiku-5-5, gpt-6-luna).
