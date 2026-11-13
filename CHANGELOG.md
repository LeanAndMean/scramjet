# Changelog

What changed in each version of Scramjet 2, for the people using it. The newest comes first; the versions before 2.140.0, when it was scramjet-rs, are under their own heading after them.

## Unreleased

## 2.220.0 — ACP runs ! commands

### Added

- `scramjet acp` runs a command in the session's project as `!` does, or `!!` with `exclude` (`_io.github.leanandmean.scramjet/bash`), also while a turn runs, and answers with its output, exit code and whether it was stopped; `…/abort_bash` stops it. Its output goes to the model with the next prompt, after the turn when one runs.

## 2.219.0 — ACP reloads the settings

### Added

- `scramjet acp` reads the settings again in every open session (`_io.github.leanandmean.scramjet/reload`), as `/reload` does: the command sets, agents, skills, MCP servers, providers and context files, with what is wrong in them as notices, and the commands and models listed again. The MCP servers the client gave stay. It is refused while a turn runs.

## 2.218.0 — ACP moves in the tree and labels entries

### Added

- `scramjet acp` moves a session's conversation to one of its entries (`_io.github.leanandmean.scramjet/navigate_tree`), as `/tree` does: the prompt gone back to is given back to edit, a question gone back to is asked again, and with `summarize` the branch left is summarized first, shown as a compaction. It is refused while a turn runs.
- `scramjet acp` labels an entry, or takes its label away (`…/set_label`), as `/tree`'s labels do.

## 2.217.0 — ACP forks, branches and lists the tree

### Added

- `scramjet acp` forks a session before one of its prompts (`_io.github.leanandmean.scramjet/fork`), as `/fork` does, and answers with the new session and the prompt to edit; it branches the whole conversation into a new session (`…/branch`, optionally named), as `/branch` does. Both are refused while a turn runs.
- `scramjet acp` lists a session's entries as `/tree` does (`…/tree`), with the ids `fork` takes, labels and the current entry.

## 2.216.0 — ACP exports sessions and gives the usage

### Added

- `scramjet acp` exports a session on request (`_io.github.leanandmean.scramjet/export_html`), as `/export` does: a web page, or a session file for a `.jsonl` path, relative paths within the project; the answer names the file.
- `scramjet acp` gives `/usage`'s summary as data (`_io.github.leanandmean.scramjet/usage`): each provider's quota, then the requests, tokens and cost of today, this week, the last 30 days and by model.

## 2.215.0 — ACP compacts and renames sessions

### Added

- `scramjet acp` compacts a session on request (`_io.github.leanandmean.scramjet/compact`, with optional instructions), as `/compact` does, shown as compaction updates; it is refused while a turn runs, and prompts sent during it run once it ends. `session/cancel` stops it.
- `scramjet acp` renames a session (`_io.github.leanandmean.scramjet/rename`), as `/rename` does, and tells the client its new title; `initialize` lists the extension methods offered.

## 2.214.0 — ACP deletes saved sessions

### Added

- `scramjet acp` deletes a saved session (`session/delete`), as Ctrl+D in `/resume` does: to the trash when a `trash` command is installed, else for good. A session open on the connection is refused until it is closed; one already gone or never saved counts as deleted.

## 2.213.0 — ACP loads saved sessions

### Added

- `scramjet acp` loads a saved session (`session/load`), showing its current branch again before it goes on: the prompts, replies, reasoning, each tool call with its result, and compactions, so clients such as Toad can reopen past conversations. A turn that failed is said in a message of its own.

## 2.212.0 — ACP lists, resumes and closes sessions

### Added

- `scramjet acp` lists the saved sessions (`session/list`: a folder's or every project's, newest first, 50 a page, titled by their name or first prompt), resumes one in the folder it belongs to without replaying it (`session/resume`), and closes an open one, cancelling its turn (`session/close`).

## 2.211.0 — ACP sets the model and thinking level

### Added

- ACP sessions have `model` and `thought_level` config options: the providers' models grouped by provider, as `/model` lists them, and the current model's thinking levels, as Shift+Tab cycles them. A change is remembered as `/model` and Shift+Tab remember it, and one set during a turn applies at the turn's next pause; when the agent's `switch_scramjet_model` or a next step changes them, a `config_option_update` tells the client.

### Changed

- An ACP `autopilot` setting now applies at once, as do the other options, and turning it off while a next step's countdown runs stops the countdown.

## 2.210.0 — ACP offers next steps

### Added

- In `scramjet acp`, a finished command's next steps and the agent's suggested ones are offered to the client: as the `_io.github.leanandmean.scramjet/next_step` request to a client that knows Scramjet's extensions, which may also change the step's model, level and fresh session, else as an agent message that the next prompt answers. They used to be left out.
- ACP sessions have an `autopilot` config option (a boolean where the client offers those, else `on`/`off`): with it on, the recommended step runs after the 3-second countdown unless the client answers, holds it (`_io.github.leanandmean.scramjet/hold`) or a prompt comes.

## 2.209.0 — ACP asks questions in the client's forms

### Added

- In `scramjet acp`, the agent's select and confirm questions and a status report's abandon question are `elicitation/create` forms when the client offers them; a client without forms gets them as freetext questions, their options listed, which the next prompt answers. Project agents' consent and write grants are permission requests, where they were denied.

## 2.208.0 — ACP asks the client's permission

### Added

- `scramjet acp --ask` runs, where it was refused: each call `--ask` would ask about, and each publication waiting for approval, is a `session/request_permission` to the client (Allow, Allow all calls this session, Deny; Approve publication, Cancel). A turn cancelled meanwhile withdraws the request, and an answer that was not offered denies.

## 2.207.0 — ACP takes images while a turn runs

### Added

- In `scramjet acp`, a prompt with images sent while a turn runs is queued like any other: its images are saved to temporary files whose paths go before its text, as the full screen pastes an image, where it was refused before.

## 2.206.0 — ACP follows a next step's fresh session

### Added

- In `scramjet acp`, a next step that starts a fresh session sends `_io.github.leanandmean.scramjet/session_changed` with the new session's id to a client that knows Scramjet's extensions, and its updates then carry that id. Other clients keep their thread's id and are told in it that a new session started.

## 2.205.0 — ACP shows subagents' children

### Added

- In `scramjet acp`, a `subagent` call's updates carry its children (agent, task, state, replies and tool calls, usage) in `_meta`, as they run and in its result, and a line per child as the call's content for clients that read no `_meta`.

## 2.204.0 — ACP shows compactions and what commands sent

### Added

- `scramjet acp` reports a compaction as ACP's `compaction_update` (in progress, then completed with its summary, failed or cancelled) to a client offering `session.compaction`, and as an agent message to others; before, it was only a notice.
- A prompt that runs a skill or a command set's command is followed by a `user_message_chunk` with the text it sent in its place, marked with what was typed.

## 2.203.3 — `scramjet acp` stops cleanly on SIGTERM

### Fixed

- `scramjet acp` on SIGTERM cancels its running turns, killing their commands, answers their prompts `cancelled` and exits with status 143, as when its client closes stdin; before, it died at once and left a running command behind.

## 2.203.2 — Login tokens kept out of their endpoints' errors

### Fixed

- A login endpoint's error that echoes the refresh token or device code sent in its form (renewing a ChatGPT login, GitHub's device login) shows `[redacted]` in its place, and a credential cut across the end of a long error body no longer leaves its start shown.

## 2.203.1 — Credentials kept out of servers' errors

### Fixed

- A server's error that echoes a credential the request carried (an API key, an `Authorization` or other header from `models.json` or `auth.json`, an MCP server's headers) shows `[redacted]` in its place, in the full screen, `-p` and `scramjet acp` alike: model requests, model lists, logins, downloads and MCP's HTTP transport.

## 2.203.0 — The ACP client's MCP servers

### Added

- `scramjet acp` starts the MCP servers the client gives in `session/new`, over stdio or streamable HTTP (`mcpCapabilities.http`), with their variables and headers, for that session beside those of `settings.json`; one of the same name replaces the settings' entry, and runs though `disabledMcpServers` names it. A server it cannot take (the older SSE transport, a bad URL) refuses the session with what is wrong.

### Fixed

- In `scramjet acp`, `available_commands_update` no longer arrives before `session/new`'s answer, and `--retries` is followed.

## 2.202.0 — ACP's `max_tokens` and `refusal` stop reasons

### Added

- In `scramjet acp`, a turn whose reply stopped at the model's output token limit ends with `stopReason: max_tokens`, and one the model or the provider's content filter declined (Claude's `refusal`, a `content_filter` finish) with `refusal`, instead of an error; the text it gave stays, and a warning notice says why it went no further.

## 2.201.0 — Images in ACP prompts

### Added

- `scramjet acp` takes images in prompts (`promptCapabilities.image`): each goes to the model with the prompt's text, fitted to size as an `@` image is, and shows in the session as `[Image #N]`. A prompt with an image is never a command, and one sent while a turn runs is refused for now.

## 2.200.0 — ACP tool calls with their arguments and diffs

### Added

- In `scramjet acp`, each tool call carries its arguments (`rawInput`) and, for a read, edit or write, the absolute path it works on (`locations`, with a read's first line), so a client can follow along; a finished edit adds a `diff` per replacement, and a write that made its file a `diff` from nothing, beside the output.

## 2.199.0 — `scramjet acp`, a start of RPC mode

### Added

- `scramjet acp [options]` runs Scramjet as an Agent Client Protocol (v1) agent over stdio: `initialize`, `session/new`, `session/prompt` and `session/cancel`, with replies, reasoning, tool calls, their output and usage streamed as `session/update`s, commands and skills listed for the client, and a prompt sent during a turn steering it (`followUp` in its `_meta` queues it for the turn's end). Permission requests, next-step lists, images, the client's MCP servers and the other session methods come later (README).

## 2.198.0 — Local models' context counted as for every model

### Changed

- Compaction for a local model no longer corrects its chars/4 estimate by the runtime's count of earlier requests, nor asks Ollama to count a long request first: it goes by the larger of the usage the runtime reports and the estimate, as since 2.173.0. Session files no longer store `_scramjet_calibration` (those written by 2.179.0 to 2.197.0 read as before).

## 2.197.0 — Search tools answer as Claude Code's do

### Changed

- `grep`, `find`, `read` and `ls` that find nothing answer plainly again ("No matches found", "No files found matching pattern", the missing path), as Claude Code's tools do: grep no longer searches the rest of the project or drops its glob to show matches elsewhere, find no longer greps a phrase it was given, read no longer lists files of the same name, and ls no longer lists the nearest directory above a missing one.
- grep's errors are the pattern matcher's own, without the added advice to set `literal: true` or about line breaks in the pattern.

## 2.196.0 — Edits match as Codex's apply_patch does

### Changed

- An edit whose old text matches only with each line's indentation set aside now writes its new text as given, as Codex's `apply_patch` writes a patch's lines, rather than shifting it to the file's indentation or refusing it when the indentation differs unevenly.
- An edit no longer matches with escapes such as `\n` and `\"` read as the characters they stand for (2.186.0), which could apply a change other than the one sent, and a refused edit no longer says that its new text is already in the file.

## 2.195.0 — vLLM's models are not told apart by name

### Changed

- `/login` → Local models no longer recognizes gpt-oss on vLLM by its name to offer it low, medium and high: a vLLM model's thinking levels come only from how it answers the check, so one that thinks whatever it is asked is saved without levels, as any other.

## 2.194.0 — A reply of reasoning only ends the turn again

### Changed

- A Chat Completions reply that is only reasoning, with no answer and no tool call, ends the turn as on every other protocol, rather than being asked for again once (added in 2.188.0 for a habit of gpt-oss's).

## 2.193.0 — No renaming of gpt-oss's tool calls

### Changed

- Calls to tools by names Scramjet does not offer (gpt-oss's `search`, `open`, and `find` with a `query`) are no longer rewritten into grep, read and find: as for any other model, they are refused with the names of the tools that are offered. Scramjet carries no code for one model's habits.

## 2.192.0 — MCP servers outside the sandbox

### Changed

- MCP servers no longer start inside `--sandbox` (or the `sandbox` setting): they run with your environment and credential stores, as in Codex and Claude Code, so a server whose key is in a credential store (a search server's, say) works under the sandbox. Changing the sandbox with `/sandbox` no longer restarts them, and Scramjet's separate `sandbox-cache` for their `uvx` and `npx` is no longer used (it can be deleted from `~/.local/state/scramjet`).

## 2.191.0 — Ollama counts a long request before compaction

### Changed

- On an Ollama model (0.12.6 or later), when the estimate says the next request would be compacted or cut, Ollama is first asked to count it without cutting it. Conversations that fit are no longer compacted because the estimate ran 12–52% high, and the past-the-window notice gives Ollama's count. Where the request fits, Ollama keeps it cached for the real request, so this costs almost no time.

## 2.190.1 — Ollama's cut prompts not taken as measured

### Fixed

- On an Ollama model, a request past the window, which Ollama shortens by dropping the conversation's first messages, no longer has its shortened count taken as the measure of later requests; such a count made later estimates too low, so compaction could stop while Ollama kept dropping the start.

## 2.190.0 — What is there instead

### Changed

- `ls` given a directory that does not exist shows what the nearest directory above it in the project holds (50 entries at most), as a model guessing `src` otherwise spends its next call listing the project.
- `grep` with a `glob` that leaves out every file holding the pattern shows up to 20 lines in the files it left out (gpt-oss searched a Python project with `**/*.rs`).

## 2.189.0 — Did you mean

### Changed

- `read` given a file that does not exist names the project's files of the same name (for a `mod.rs`, `index.*` or `__init__.py`, also its directory's name as a file), as a model guessing a path (`src/agent/local/mod.rs` for `local.rs`) often means one of them.

### Fixed

- A `grep`, `find` or `ls` call with an empty `path` shows `in .` rather than `in ` with nothing after it.

## 2.188.1 — Matches elsewhere

### Fixed

- `grep` given a `path` in the project where the pattern is not found shows up to 20 lines elsewhere in the project that hold it. Local models (gpt-oss) kept searching `tests/` for tests that are in `src/`.

## 2.188.0 — Local models that stop to think

### Changed

- A Chat Completions reply that is only reasoning, with no answer and no tool call, is asked for again once, saying so. Ollama's gpt-oss now and then stops after reasoning about its next search, which ended the turn with nothing done.
- `find` given a plain phrase that names no file also shows up to 20 lines holding it as text, found as `grep` with `literal: true` would. Local models give `find` the text they look for, and often ignored the hint to use `grep`.

## 2.187.0 — No idle tools in -p

### Changed

- `-p` no longer offers `suggest_scramjet_next_steps`, which no one could answer there, nor, unless a message is a slash command (or `-`) or the continued session has a command waiting, `report_scramjet_command_status`. Small local models called them anyway; each request is also about 750 tokens smaller.

## 2.186.0 — Edits that spell escapes otherwise

### Changed

- An edit whose old text matches nowhere else is looked for with string escapes read as what they stand for: a line break for `\n`, `"` for `\"`, a line continued with `\` joined. Local models such as gpt-oss write a literal's value where the file has its source. Only the part the edit changes is written, and only when it spells no such escape; otherwise the refusal quotes the file's lines exactly.
- An edit refused because its old text is not there says when its new text already is, with the lines: the edit may have been applied already.

## 2.185.0 — Edits with other indentation

### Changed

- An edit whose old text matches nowhere, even loosely, is looked for with each line's indentation set aside, as Codex's patches are. Found with the same indentation missing or added on its lines, it applies, with the new text's lines shifted alike; lines copied with their indentation right stay as they are. Found with indentation differing unevenly, the refusal quotes the file's lines exactly. Local models such as gpt-oss drop the indentation of the lines they copy.

## 2.184.2 — A line break in a grep pattern

### Fixed

- grep given a pattern with a line break says it matches within single lines, instead of suggesting `literal: true`, which does not help there.

## 2.184.1 — Hints after a missed search

### Fixed

- grep's error for a pattern that is not a valid regex says to set `literal: true` to search for it as plain text, and `find` given a pattern with no glob characters that matches nothing says it matches file names and that grep searches contents. Local models such as gpt-oss passed text with `(` to grep and phrases to find.

## 2.184.0 — Replacing every occurrence

### Added

- The edit tool takes `replaceAll` on an edit (Claude Code's `replace_all` too), replacing every occurrence of its text instead of refusing one that is not unique; that refusal now says so.

## 2.183.0 — Where a refused edit's text is

### Changed

- An edit refused because its text occurs more than once says the lines where each occurrence starts ("Found 3 occurrences … on lines 12, 40 and 88"; the first ten, then how many more), so the model can add the right context without searching the file again.

## 2.182.4 — gpt-oss's find-in-page

### Fixed

- gpt-oss's `find` calls that carry its browser's `query` (text to look for, not a file pattern) run as a literal `grep`, where they failed for a missing `pattern`; its `max_results` on `find` sets the limit.

## 2.182.3 — Paths grep and find show

### Fixed

- `grep` and `find` show the files they find from the current directory (`src/agent/calls.rs`, not `agent/calls.rs` for a search in `src`, nor `calls.rs` for a file searched), as `rg` does, so the model reads them at the first try. Outside it they show the whole path. On Ollama's gpt-oss:20b, one edit task had 8 failed reads of such paths in 48 requests; two runs since had none.

## 2.182.2 — gpt-oss's other tool names

### Fixed

- gpt-oss's `search` calls with its browser's `query` (not grep's `pattern`) now run grep too, and `open` runs `read`; its `line_start` and `line_end`, given to either, read those lines. One edit task on Ollama's gpt-oss:20b had run 49 requests, 31 of them refused `search` calls, without making the edit; it now finishes in 20 to 50.

## 2.182.1 — gpt-oss finds grep

### Fixed

- gpt-oss calls grep `search`, the name of its training's browser tool, and kept calling it however often it was told no such tool exists: one task on Ollama's gpt-oss:20b ran 274 requests, 158 of them to `search`, and never finished. Such a call, carrying grep's `pattern`, now runs grep and stays in the conversation as grep.
- A call to a tool that is not offered is answered with the names of those that are.

## 2.182.0 — gpt-oss's thinking levels on vLLM

### Changed

- `/login` → Local models offers thinking levels for gpt-oss on vLLM, told by the weights' path vLLM lists: low, medium and high reach it as `reasoning_effort`; off and minimal are not offered, as vLLM refuses them. Such models were saved with no levels before.

## 2.181.1 — `localhost` answers at once on Windows

### Fixed

- Connections try a host's IPv6 and IPv4 addresses together, the next one 250 ms after the last unless it failed sooner, as Node does. On Windows, `localhost` is `::1` first: `/login` → Local models missed a runtime listening on `127.0.0.1` (WSL's Ollama, reached through mirrored networking), and each request to a `localhost` address could wait seconds before reaching it.

## 2.181.0 — gpt-oss's thinking levels on LM Studio

### Changed

- `/login` → Local models offers thinking levels for an LM Studio model that thinks but that `none` does not stop, such as gpt-oss: low, medium and high reach it, and off is not offered. Such models were saved with no levels before. A model LM Studio lists as reasoning counts as thinking even when the check's answer showed none.

## 2.180.0 — gpt-oss's thinking levels on llama.cpp

### Changed

- `/login` → Local models offers thinking levels for a llama.cpp model whose chat template takes `reasoning_effort`, such as gpt-oss's: low, medium and high reach its template; off is offered only where `none` stops its thinking. Such models were saved with no levels before.

## 2.179.1 — A stopped local runtime says how to start it

### Fixed

- A model a local runtime serves, sent a prompt while the runtime is not running, fails at once with how to start it (`ollama serve`, `vllm serve MODEL`, `lms server start`…), rather than retrying for 14 seconds and saying only "Connection refused".

## 2.179.0 — Local models' token count kept across restarts

### Changed

- On a model a local runtime serves, the runtime's last count of a request is saved in the session, so a session continued or resumed in a new process estimates its size from it rather than from chars/4.

## 2.178.0 — Local models keep their token count past a compaction

### Changed

- On a model a local runtime serves, how far its count of a request differed from the chars/4 estimate now carries over a compaction, `/new`, `/resume` and a `/tree` move, as a ratio, instead of starting again from chars/4, which ran 12–52% above Ollama's counts and compacted early.

## 2.177.1 — An Ollama model's window known before its first request

### Fixed

- A prompt to an Ollama model that is not loaded (after Ollama restarts, or once its keep-alive ends) loads it first and reads the window it is served with, so compaction and the over-window warning use it from the first request. Before, that request went with the window read earlier, which Ollama could cut silently.

## 2.177.0 — Local models follow a runtime's window reloaded mid-session

### Changed

- A session on a local model asks its runtime for the model's window again at each prompt, so a model loaded again with another window (Ollama restarted with another `OLLAMA_CONTEXT_LENGTH`, `lms load -c`, llama.cpp or vLLM started with other flags) is followed without starting again, and the change is said. Before, the first window read was kept until the process ended.

### Fixed

- The footer and compaction no longer go back to a local model's saved window after a model switch, a session switch or `/reload`, while its runtime serves it with another.

## 2.176.0 — LM Studio in Local models

### Added

- `/login` → Local models finds LM Studio at port 1234 (or an address given) and saves its models under `lm-studio`, each with the window LM Studio loaded it with (read from its REST API once the check has loaded it; the model's own maximum is shown before), whether it sees images, and its embedders left out. A model that thinks is asked whether `reasoning_effort` none stops it, and then offers off. LM Studio refuses a prompt past its window rather than cutting it, so compaction corrects its estimate by LM Studio's token counts.

## 2.175.2 — gpt-oss on Ollama offers no off

### Fixed

- `/login` → Local models asks an Ollama model that thinks once more whether `reasoning_effort` none stops it. One that keeps thinking, as gpt-oss does, is saved without an `off` level: its levels go as `reasoning_effort`, and off is taken as the lowest. Before, gpt-oss was offered off and kept thinking at medium. Models saved before keep their entry until checked again.

## 2.175.1 — Ollama's count taken on a first turn

### Fixed

- On an Ollama model, its count of a request below half its window now corrects compaction's estimate too, when the request was too small to have been cut (clear of the half window Ollama cuts to, and under the window by characters over four). A first turn reading a file on a small window no longer compacts early on the raw estimate.

## 2.175.0 — Local models count tokens as their runtime does

### Changed

- On a model a local runtime serves, compaction and the cutting of large tool outputs correct their characters-over-four estimate by the runtime's own count of the last request: by Ollama's when it is above half the window (below it the count may be of a prompt Ollama cut), by llama.cpp's and vLLM's always. Live against Ollama, chars/4 ran 19% over its count for the system prompt and tools; corrected, the next request's estimate was within 12 tokens, so compaction no longer runs early by that much.

## 2.174.0 — Local models fit a tool output too large for their window

### Changed

- On a model a local runtime serves, when compaction cannot bring a request under the window less the reserve for the reply, the request's largest tool outputs are cut in the middle, with a note saying so to the model, and a notice tells you once per turn; the session keeps them whole. Before, Ollama cut such a prompt to half its window without an error (20,681 tokens to 6,146 here). When the system prompt and tools alone fill the window, nothing is cut and the overflow notice stays.

## 2.173.0 — Local models say when a request overflows

### Changed

- On a model a local runtime serves, each request of a turn, the first included, is measured with its system prompt and tools as well as by the usage the runtime reports, which Ollama counts after cutting a prompt to fit; compaction runs on whichever is larger. When the request still passes the window, a notice says so once per turn, as Ollama then drops the conversation's start without an error.
- When the newest reply with its tool output alone is more than compaction keeps whole (`keepRecentTokens`), compaction summarizes everything before that reply instead of nothing, on every provider: a turn reading several large files at once could not be compacted at all.

## 2.172.0 — Compaction fits small local windows

### Changed

- On a model a local runtime serves, `compaction.reserveTokens` and `keepRecentTokens` are each capped at a quarter of its window. With their defaults (16384 and 20000), a window of 16k or less never compacted, and Ollama silently dropped the start of a longer conversation; a 40k one compacted at 24k yet kept 20k whole, so it compacted again at its next check.

## 2.171.0 — Local models compact within a turn

### Changed

- On a model a local runtime serves (`/login` → Local models), automatic compaction is checked before each request of a turn, not only between turns: a runtime such as Ollama drops what does not fit its window without an error, so a turn reading large files could lose its instructions before the overflow compaction other providers trigger could help.

## 2.170.0 — Local models follow their runtime's window

### Changed

- A session on a model saved from `/login` → Local models takes the window its runtime serves it with now (Ollama's once it has loaded the model, llama.cpp's and vLLM's from their lists) in place of the one saved, and says so when they differ, so compaction runs before the conversation outgrows it. Ollama cuts a prompt longer than its window without saying so, losing the instructions; `/login`'s warning for a small window now says that instead of claiming the runtime refuses it.

## 2.169.1 — A first login picks a model

### Fixed

- Logging in with `/login` (ChatGPT, GitHub Copilot, an API key or a package's login) when the session's model is one no provider offers, as on a first start with none, switches the session to the model a new session would start with, as saving a local model already did, instead of leaving it on a model it cannot reach.

## 2.169.0 — Thinking levels for local models

### Added

- A local model that thinks gets thinking levels that work: Ollama's are sent as `reasoning_effort` (off as `none`), and on vLLM and llama.cpp a model whose thinking stops when its chat template is asked (Qwen3's) gets off and high. The check in `/login` → Local models says which, and when the thinking comes as text in the answer, which flag of the runtime sends it apart. Check a saved model again to get them.

### Fixed

- Saving a local model when the session's model is one no provider offers (as on a first start with none) switches the session to it, instead of leaving it on a model it cannot reach.

## 2.168.0 — Local models in /login

### Added

- `/login` → Local models: Ollama, llama.cpp's server and vLLM found running on this machine, or a runtime at an address you give, with each model's context window as the runtime reports it. A model is saved in your `models.json` only after it has called a harmless tool; a failure says at which step (not reachable, key needed, no models, did not answer, no tool call) and what to change. Saved runtimes can be checked again or removed from the same list.

## 2.167.0 — The sandbox in a list

### Added

- `/sandbox` on its own opens a list of the session's sandbox: its mode (read-only, workspace-write or off), its network, "Add a directory…" and each directory granted, for the session or always here; Enter changes the mode or the network, adds a directory, or makes a granted one read-only again (and forgets it if it was granted always). `/sandbox` with an argument works as before, and so does `/sandbox` in line mode.
- `/settings` has Sandbox and Sandbox network rows: the mode and network new sessions start with (`--sandbox` still overrides them). A mode is not saved when the sandbox cannot start on this machine.

## 2.166.0 — Write access kept for a project

### Added

- When the agent asks for write access, you can allow it always for this project as well as for the session. The grant is kept in `sandbox-grants.json` beside your settings, under the project's path, never in the repository; each later session with a sandbox in that project starts with it. `/sandbox` lists it under "Always here", and `/sandbox remove DIR` forgets it.

## 2.165.0 — The agent asks for write access

### Added

- With a sandbox on, the agent is told which directories it can write, and asks for more with a new `request_permissions` tool, giving the directories and why; you allow them for the rest of the session or deny them. In `-p` and steps that autopilot runs, no one is there to answer, so the request is refused with a note the agent can pass on.

## 2.164.1 — Sandbox grants that hold

### Fixed

- In `read-only`, granting a directory that holds a repository and then a parent of the project no longer lets commands write the project; nor can they move it out from under its read-only mount by renaming a directory above it.
- Granting a directory inside a `.git`, `.scramjet` or `.agents` that stays read-only is refused (grant that directory itself) instead of making all of it writable; granting `/`, or `/dev`, `/proc`, `/run`, `/tmp` or what holds them, is refused, as it would bring back sockets that lead out of the sandbox.
- `/add-dir .` in `read-only` makes the project writable to writes and edits too, not only to commands; `/sandbox remove` restarts the MCP servers so that they lose the grant too.

## 2.164.0 — Changing the sandbox while working

### Added

- `/sandbox` shows the sandbox's mode, network, writable directories and what stays read-only in them, and changes the mode (`off` included) or the network without a restart.
- `/add-dir DIR`, and `--add-dir DIR` at start, as in Claude Code, let the sandboxed agent's commands, writes and edits change another directory for the session; its `.git`, `.scramjet` and `.agents` stay read-only unless granted by name. `/sandbox remove DIR` takes a grant back.

## 2.163.0 — Reading other harnesses' sessions

### Added

- The built-in `scramjet` set has a `reading-past-sessions` skill: where Scramjet, Scramjet 1, Pi, dreb, Claude Code, Codex, OpenCode, Cline and Posit Assistant keep a project's sessions, and which fields hold the prompts, tool calls, summaries and command reports. The agent's instructions point to it when earlier work on the project may hold a missing detail.

## 2.162.0 — Setting up a command set's repository

### Added

- The built-in `scramjet` set has a `setting-up-command-sets` skill: a new set's repository layout, and the decisions its author owns (SemVer releases or plain commits, Claude Code compatibility, license), recorded in its `CLAUDE.md` with what each means for an agent working there.
- `scramjet lint` warns when `plugin.json` gives a `version` but the repository's `CLAUDE.md` (or `AGENTS.md`) has no `## Versioning` section, and when the repository has `vX.Y.Z` tags but `plugin.json` gives no version.

## 2.161.0 — The linter catches what breaks a set elsewhere

### Added

- `scramjet lint` warns about `:` in command and agent file names (Windows cannot check them out), a set without `set.yaml`, tool names Claude Code denies for their case (`bash` for `Bash`, `find` for `Glob`) or that neither harness nor a configured MCP server has (with the closest name), statuses the harness refuses (`blocked`, `incomplete`…), `next:` candidates and `/set:command` mentions that name no command, `autonomy-defaults.yaml` edges their command's `next:` block never offers, and a `.claude-plugin/plugin.json` naming the set otherwise.

### Fixed

- `scramjet lint` points `autonomy-defaults.yaml` warnings at the line of the edge they name, counts agent files in its summary, and no longer repeats a file's path inside its messages.

## 2.160.0 — No command sets from ~/.local/share/scramjet

### Changed

- Command sets in `~/.local/share/scramjet` (or `SCRAMJET_CACHE`), where an old Scramjet 1 install seeded Mach 12, are no longer read: a stale copy there no longer runs. Each one found is named in a warning at start and in `scramjet list`, with what to do instead (for Mach 12, `scramjet install git:github.com/LeanAndMean/mach12`).

## 2.159.0 — The progress indicator means the agent is working

### Changed

- The terminal's progress indicator (OSC 9;4) stops while the agent waits for you (a question, next steps, an approval) and starts again when it works, so a tab that stops spinning means it is your turn.
- It is on by default in Windows Terminal, ConEmu, Ghostty 1.2+ and iTerm2 3.6.6+, told from the environment, and off elsewhere and inside tmux; `terminal.showTerminalProgress` (Terminal progress in `/settings`) still turns it on or off, and Claude Code's `terminalProgressBarEnabled` is read too.

## 2.158.0 — Provider packages listed as providers

### Changed

- A package that brings only providers (a `models.json`, with no commands, agents or skills), such as a company's provider package, is listed under `[Providers]` when a session starts, no longer under `[Command sets]`, and is not counted as a command set.

## 2.157.1 — No Scramjet 1 warnings in the home directory

### Fixed

- Started in the home directory, Scramjet 2 no longer takes Scramjet 1's `~/.scramjet/agent` (or `SCRAMJET_CODING_AGENT_DIR`) for a project command set named `agent`, which listed it in `scramjet list`, took the providers of Scramjet 1's `models.json`, and warned of each `mach12:` and `scramjet:` agent link there.

## 2.157.0 — Packages say which Scramjet they need

### Added

- A `models.json` can name the earliest Scramjet it works with, as npm's `engines`: `"engines": {"scramjet": ">=2.157.0"}`. An older Scramjet takes nothing from it but its `credentialFiles` and says which version it needs; `scramjet install` refuses such a package.
- `scramjet install`, and the start for a listed package, say when a package is a Scramjet 1 extension (a `package.json` with a `pi` manifest) rather than that it holds no command set.

### Changed

- The installers say where another `scramjet` (Scramjet 1's) is left unchanged, to run should Scramjet 2 fail, and, when they replace an earlier Scramjet 2, how to go back to it. The README's Install section says how to fall back.

## 2.156.0 — A day's cache on Responses

### Changed

- OpenAI Responses servers are asked to keep the prompt cache for 24 hours (`prompt_cache_retention`) at the default long retention, as in Scramjet 1, not only Chat Completions servers. A model whose `models.json` entry (or its provider's) sets `"compat": {"supportsLongCacheRetention": false}` is not asked.

## 2.155.0 — --api-key

### Added

- `--api-key KEY`, as in Scramjet 1: the requests to the model's provider carry KEY, as it is, for the run, in place of the key `/login` saved or `models.json` gives; `/login` shows it as the key from `--api-key`. A key with spaces is refused without repeating it.

## 2.154.0 — Cache retention

### Added

- `--cache-retention short|long` and the variable `SCRAMJET_CACHE_RETENTION` (or `PI_CACHE_RETENTION`; also `none`), as in Scramjet 1: `short` asks providers for their default caching (5 minutes on Anthropic's) instead of an hour, `none` for no caching. Long stays the default.

## 2.153.0 — Scramjet 1's command lines

### Added

- `--provider NAME`, as in Scramjet 1: `--provider P --model M` chooses `P/M`.
- `scramjet update --extensions` updates only the packages, `--extension NAME` one of them, `--self` only the program, and `--force` takes the newest release even when it is not newer, as in Scramjet 1.

### Changed

- `--verbose`, `--mode text`, `-ne` (`--no-extensions`) and `-np` (`--no-prompt-templates`) are taken, changing nothing, so Scramjet 1's aliases and scripts with them start. `--mode json` and `rpc`, `--extension`, `--prompt-template`, `--api-key` and `--model-definition` stop with what to do instead, rather than as unknown arguments.

## 2.152.1 — The ssh-agent in the hidden-stores note

### Fixed

- With `hideCredentials` set, a failed command's note no longer says the ssh-agent is hidden: only `--sandbox` keeps it out of reach.

## 2.152.0 — Credentials in every turn

### Changed

- The agent's tools see your credential stores in every turn by default, so `gh` and `git` over SSH work in a loop or autopilot turn too. `hideCredentials` set to `unattended` or `always` hides them as before.
- A command that failed for want of a hidden store tells the agent to ask you if the task needs that access, and to go on without it if it does not.

### Fixed

- The agent's commands no longer get the terminal, as in Scramjet 1, so an `ssh` passphrase or host-key prompt, or `sudo`'s, fails at once instead of waiting where nobody sees it.

## 2.151.0 — --offline

### Added

- `--offline`, as in Scramjet 1: the same as `SCRAMJET_OFFLINE=1`, so the start neither clones packages nor looks for a newer release. Scramjet 2 refused it as an unknown argument.

## 2.150.0 — MCP servers start again when used

### Changed

- An MCP server that failed to start or ended starts again when the model next uses it, a minute or more later, as in Scramjet 1; until then its tool calls say when. `/mcp reconnect` still starts it at once.

## 2.149.0 — Requests through HTTPS_PROXY

### Added

- Requests go through the `http://` proxy that `HTTPS_PROXY` or `HTTP_PROXY` names, with its `user:password@` if given, and not to the hosts `NO_PROXY` lists, as in Scramjet 1. Servers on this machine are always reached directly.

## 2.148.1 — Saying so when no provider offers a model

### Fixed

- `scramjet -p` with no provider set up stops at once with one error saying to log in or add providers, as Scramjet 1 does, rather than starting a turn that fails, printing a usage line and leaving a session file.
- With no provider set up, a prompt in the full screen says no provider offers any model, rather than naming a default model you never chose, and the footer shows `no-model` without an effort level.

## 2.148.0 — npm's packages, ready for Scramjet 1's update

### Added

- Each release can be packed for npm, as esbuild's are: `@leanandmean/scramjet` runs the program from a package of its own for each platform, so that `npm install -g @leanandmean/scramjet`, which Scramjet 1's `scramjet update` runs, brings Scramjet 2. They are not published yet.

### Changed

- When npm installed the program, `scramjet update self` says to update it with `npm install -g @leanandmean/scramjet`, and the start's notice of a newer release names that command, rather than replacing a file npm keeps.

## 2.147.1 — Lists keep their keys on a short screen

### Fixed

- A list longer than the screen, such as `/resume`'s or `/hotkeys`', cut its keys to one line, losing those at the end (Ctrl+D delete, Esc close); it now shows them on up to three lines and one or two rows fewer, as long as three rows stay in view.

## 2.147.0 — The scramjet set comes with the program

### Added

- The `scramjet` command set comes with the program, as it does with Scramjet 1: `/scramjet:troubleshoot`, its agents for designing, reviewing and diagnosing commands, and the `writing-scramjet-commands` skill, so a release needs nothing installed to write command sets. A release writes its files under `~/.local/share/scramjet/builtin/`; a set you install under the name `scramjet` replaces it.

### Fixed

- The agent can read the README when you ask it about Scramjet: a release, which has no checkout, used to point it at the build machine's path; it now writes its own README beside the built-in set.

## 2.146.0 — Commands name tools as Claude Code does

### Changed

- A command's `allowed-tools` may name the built-in tools as Claude Code does (`Bash`, `Read`, `Grep`, `Glob`, `Edit`, `Write`), as agents' `tools` already could, so one command file serves both; Claude Code matches those names case-sensitively and asks for, or in `dontAsk` mode refuses, a tool listed as `bash`. The `scramjet` set's command lists them so.

## 2.145.0 — Import Scramjet 1's setup

### Added

- `scramjet import` copies your Scramjet 1 setup from `~/.scramjet/agent`: settings (merged into yours, your keys kept; paths made absolute, npm packages left out), `models.json` providers that have a `baseUrl` and an `api`, keybindings, ChatGPT's and GitHub Copilot's logins, your own agents, themes, skills and context files. It never replaces a file, lists what it leaves and why (API keys, providers that only changed a built-in one, command sets' agents, extensions, prompts, sessions), and `--dry-run` only lists. The first start that finds Scramjet 1's settings and none of yours says so, once.
- `install.sh` (Linux, macOS) and `install.ps1` (Windows) install the newest release in one line, checked against its `SHA256SUMS`; on Windows the program's directory goes first on your user `Path`, ahead of Scramjet 1's npm directory, and on Linux and macOS the line to add is shown when another `scramjet` comes first.

## 2.144.0 — Unattended turns say why git and gh fail

### Changed

- When a command run with the credential stores hidden fails and runs `git`, `gh`, `ssh` or another program whose login lives in them, its result says the stores are hidden and why (`hideCredentials`, or `--sandbox`), so that the agent tells you instead of taking "Host key verification failed" or "not logged in" for a fault.

## 2.143.1 — Quiet when output is cut short, and a guide from Scramjet 1

### Fixed

- `scramjet --help | head`, and any other command whose output is cut short, ends quietly instead of with "failed printing to stdout: Broken pipe".
- An `npm:` entry in `packages`, Scramjet 1's, says so instead of "not a directory".

### Added

- The README's "Coming from Scramjet 1" says which of `~/.scramjet/agent`'s files to copy and what does not carry over, and Install how to keep both programs' `scramjet` on the `PATH`.

## 2.143.0 — 300 model requests before a turn stops

### Changed

- A turn, and a subagent's, stops for you after 300 model requests rather than 200 ("stopped after 300 model requests in one turn").

## 2.142.1 — Fixes from the first runs on macOS and Windows

### Fixed

- On Windows, a package from a local repository given as `file:///C:/path/repo` is cloned under `~/.local/share/scramjet/git/localhost/C/path/repo`, and a source path's `..` is refused between `\` as between `/`.
- On macOS, `--ask`'s question for a write outside the project says "through a symlink" only when one leads there, not for every path under `/var`, which is a link to `/private/var`.
- Source checkouts on Windows keep LF line ends (`.gitattributes`), which the built-in instructions and changelog need.

## 2.142.0 — The start says when a newer release is out

### Added

- The full screen's start says when a release newer than the running one is out, and that `scramjet update self` installs it. It looks at `LeanAndMean/scramjet`'s release list at most once a day, in the background, so what it finds shows at the next start; `SCRAMJET_OFFLINE` turns it off. The last finding is kept in `~/.local/state/scramjet/version.json`.

## 2.141.0 — scramjet-rs's versions under their own heading

### Changed

- The changelog keeps the 0.x versions, from before scramjet-rs became Scramjet 2, under a `scramjet-rs` heading of their own; `/changelog` and What's new show it above the first of them.

## 2.140.0 — Scramjet 2's version, and updates from its public releases

### Changed

- The version is now 2.140.0: Scramjet 2's major, the minor and patch counting on from 0.139.0's. Its releases are GitHub prereleases until one is called stable.
- `scramjet update self` takes the newest release of the public `LeanAndMean/scramjet` over HTTPS, with no `gh` needed, prereleases included, still checked against its `SHA256SUMS` and run before it replaces the program. Until Scramjet 2's first release there, it finds only Scramjet 1's older versions and says the program is up to date.

# scramjet-rs

Scramjet 2 was scramjet-rs, "Scramjet in Rust", until 2.140.0. Its versions then were 0.x, and until 0.139.0 its program was `scramjet-rs`.

## 0.139.0 — The command is `scramjet`

### Changed

- The program is `scramjet` (`scramjet.exe` on Windows), no longer `scramjet-rs`: `--help`, error messages, the banner, the terminal's title and the instructions name it so, `/export` writes `scramjet-session-….html`, and release archives are `scramjet-VERSION-ARCH-OS.tar.gz`. Scramjet 1's npm package also installs a `scramjet`: whichever comes first on the `PATH` runs (`which -a scramjet`). `cargo install --path .` installs the new name beside an older `scramjet-rs`, which stays until removed.
- Text that compares with Scramjet 1 now says "Scramjet 1".

## 0.138.1 — Scramjet 1's agent directory under a ~

### Fixed

- A `SCRAMJET_CODING_AGENT_DIR` starting with `~` keeps that directory's `auth.json` hidden from the agent's tools, as before 0.138.0.

## 0.138.0 — Scramjet 2's directories

### Changed

- scramjet-rs keeps its files where Scramjet 2 will: settings, credentials, models and keybindings in `~/.config/scramjet/` (beside Scramjet 1's `autonomy.yaml`), sessions and other state in `~/.local/state/scramjet/`, packages in `~/.local/share/scramjet/git/`. Files in the `scramjet-rs` directories are not moved for you: move them over yourself.
- Your own agents, skills, themes, `SYSTEM.md` and global `CLAUDE.md`/`AGENTS.md` are read from `~/.config/scramjet/`; Scramjet 1's `~/.scramjet/agent` is no longer read (its `auth.json` stays hidden from the agent's tools), and bash commands no longer get its `bin` (Scramjet 1's `rg` and `fd`) on their `PATH`.

## 0.137.0 — CLAUDE.local.md

### Added

- A `CLAUDE.local.md` beside a `CLAUDE.md` or `AGENTS.md` (in the project, the directories above it or `~/.scramjet/agent`) is read after them, as Claude Code reads it: instructions of your own that stay out of version control. `--no-context-files` leaves it out too.

## 0.136.2 — Updating on Windows while another session runs

### Fixed

- On Windows, `scramjet-rs update self` failed with `Access is denied` when a session started before the previous update was still running, as the copy moved aside then could be neither removed nor replaced. Each update now moves the running program to a free `scramjet-rs.exe.old` name and removes those no session runs any more.

## 0.136.1 — MCP servers that are slow to start the first time

### Fixed

- A program-run MCP server that took more than 10 seconds to answer, as `uvx` or `npx` do the first time while they download it, failed with `conflict initialized response id: expected 1, got 0`. Such servers now get the `initialize` handshake first, as Claude Code sends it, and have the full 30 seconds to start.

## 0.136.0 — Skills honor ignore files

### Changed

- Skill discovery leaves out what the `.gitignore`, `.ignore` and `.fdignore` files in a skills directory and its subdirectories ignore, as Pi does, so drafts and build output kept beside skills are not loaded; `scramjet-rs lint` skips them too. The rules read as git reads them: a pattern without a `/` matches at any depth, and the nearest file's rule wins.

## 0.135.0 — Windows: subagents run commands, read-only

### Changed

- On Windows, subagents get their shells (bash, PowerShell, cmd) instead of none, so Mach 12's reviewers can run `git` and `gh`. Their commands run at Windows' low integrity level: they can read, but cannot change files on this computer's disks, and get a `TEMP` of their own that is removed after each command. Unlike Linux's sandbox they keep the network and can read the credential stores, and they can still write to WSL's files and network drives; a notice says so once per run. An agent listing `Bash` gets PowerShell where Git Bash is not installed.

## 0.134.1 — Windows: `-p` no longer waits on a program left running

### Fixed

- On Windows, a program a command left running in the background (`start /b`, `&`) no longer holds `-p`'s output open after scramjet-rs exits, so a script or pipe reading it ends with the run.

## 0.134.0 — A package's programs from its releases

### Added

- A git package's `models.json` can name a program in `release` (`{"repository": "OWNER/REPO", "program": "PATH"}`): `install`, `update` and a start that clones the package download it for this machine from the release of the clone's newest `vX.Y.Z` tag through `gh`, check it against the release's `SHA256SUMS` and unpack it to PATH, so the package's users need no compiler. The private provider package does so from its 0.3.0 for Linux and Windows on x86_64.

## 0.133.0 — Command sets with their own skills, and the command-writing skill

### Added

- A command set's own skills, in a `skills/<name>/SKILL.md` at its top as a Claude Code plugin's, load with the set; `install`, `list` and `/settings` count them, and `scramjet-rs lint` checks them as they load.
- The `writing-scramjet-commands` skill comes with the `scramjet` set, rewritten for scramjet-rs: the format it reads (file names without `:`, frontmatter, `next` policies, `autonomy-defaults.yaml`), the `completed` and `abandoned` statuses, and `scramjet-rs lint --strict`. After installing the set, take Scramjet's `~/repos/scramjet/packages/scramjet/skills` out of the `skills` setting, which would otherwise come first.
- The `scramjet` command set lives in this repository, `sets/scramjet`: `/scramjet:troubleshoot` and the six read-only command-authoring agents, in Mach 12's new form (no `:` in file names, Claude Code's tool names, only the `completed` and `abandoned` statuses). Install it with `scramjet-rs install ~/repos/scramjet-rs/sets/scramjet` in place of Scramjet's copy (`scramjet-rs remove scramjet` first).

## 0.132.0 — `scramjet-rs lint`

### Added

- `scramjet-rs lint [--strict] [--json] DIR...` checks a command set, or a package's sets, in place of Scramjet's Node `scramjet-command-lint`, in its output format: as errors the files the harness would skip (commands and agents), as warnings what it would ignore and commands without an early `## Goals` list or with their arguments outside one `<user-context>` (delegate-only: `<caller-context>`) block. It accepts files named without `:`, so Mach 12's own repository passes. The agent's instructions name it.

## 0.131.0 — Agent files with Claude Code's tool names

### Added

- An agent file's `tools` and `disallowedTools` may name tools as Claude Code does: `Read`, `Write`, `Edit` (and `MultiEdit`), `Bash`, `PowerShell`, `Grep`, `Glob` (scramjet-rs's `find`) and `LS`, so one agent file serves both. Mach 12's own repository names them so.

## 0.130.0 — Command set files without `:` in their names

### Added

- A command set's files may be named `commands/<name>.md` and `agents/<name>.md` (an agent's frontmatter `name:` without its set too), still run as `/<set>:<name>`: Git for Windows cannot check out a name with `:`. `<set>:<name>.md` keeps working; a name given both ways is registered once, with a warning.
- A set's name comes from its `set.yaml`'s `name`, else from its directory's, so a repository's clone can hold a set under any directory name.

## 0.129.0 — The shell of `!` commands on Windows

### Added

- On Windows, `!` and `!!` commands run in bash when Git Bash is found, else in PowerShell, or in the shell `/settings` → Shell of ! commands chooses (`defaultShell`: `bash`, `powershell` or `cmd`, Claude Code's key). Before, without Git Bash they failed.

### Changed

- On Windows without Git Bash, a `models.json` `!command` value (an `apiKey` or header) runs in PowerShell, as the agent's `powershell` tool does, rather than in cmd.

## 0.128.0 — PowerShell and cmd tools on Windows

### Added

- On Windows the agent has a `powershell` tool, on by default: PowerShell 7 (`pwsh`) where it is installed, else Windows PowerShell 5.1, its description naming the edition and what 5.1 lacks. A `cmd` tool, off by default, runs a cmd.exe command line (`tools` in `/settings`). Both print UTF-8, end whole on Escape or a timeout, and list what they left running.
- On Windows without Git Bash the `bash` tool is off, and each launch says so and how to get it; `/settings` marks a shell that is not found. `!` commands still need bash there.

## 0.127.0 — All seven tools by default; grep and find built in

### Changed

- The agent has all seven tools by default: read, bash, edit, write, grep, find and ls (`tools` in `/settings` narrows them).
- `grep`, `find` and `@` file completion are built in, with ripgrep's libraries: nothing is downloaded into `~/.scramjet/agent/bin` and neither ripgrep nor `fd` is needed. They answer as before, ignore files honored and credential stores left out.
- A subagent whose file lists no `tools:` gets the read-only ones of the agent's own tools, as Claude Code's inherit; `disallowedTools` in its file removes some. Subagents keep `grep` and `find` where the read-only sandbox is missing, as on Windows.

## 0.126.1 — Ctrl+Backspace on native Windows, selections across a wrapped row

### Fixed

- In the native Windows program, Ctrl+Backspace deletes a word also when Windows Terminal took over a console started elsewhere (its default-terminal handoff, which leaves `WT_SESSION` unset).
- Shift+Left and Shift+Right across the end of a wrapped row in the prompt no longer show stray characters on both rows.

## 0.126.0 — Windows: stopped commands end whole, and unattended turns run them

### Changed

- On Windows, turns nobody started (`-p`, autopilot, a next step that runs by itself) run bash, grep and find with the credential stores not hidden, saying so once, instead of refusing them: scramjet-rs has no sandbox there yet. Subagents' commands stay refused.
- On Windows, a bash call whose shell ended while programs it started still run lists them, with how to stop one.

### Fixed

- On Windows, Escape or a timeout now ends every program a bash call started, also those whose parent had already ended (as `sleep` under Git Bash): each call runs in a job of its own.

## 0.125.0 — The program updates itself from releases

### Added

- `scramjet-rs update self` replaces the program with the newest release of `LeanAndMean/scramjet-rs`, downloaded through `gh` while the repository is private: the archive must match the release's `SHA256SUMS` and the program in it must run and give the release's version before it replaces this one. `scramjet-rs update` now updates the packages, then the program.

## 0.124.0 — What's new, and the changelog in a view of its own

### Added

- The first start on the full screen after scramjet-rs changed version opens What's new: the changelog of the versions since the one run before, newest first, in a view that scrolls on its own and Escape closes. The very first start records the version without showing it; a resumed session or a prompt given on the command line skips it until the next plain start.
- `collapseChangelog` (Changelog in `/settings`) shows each version's title line first; `d` switches between titles and whole versions in the view.

### Changed

- `/release-notes` (`/changelog`) opens the same view on every version, rendered as markdown, instead of writing the changelog into the conversation.

## 0.123.0 — Bash calls a model can find and stop

### Changed

- A bash call without a `timeout` now stops after 30 minutes, so a command that hangs for good ends; a longer `timeout` still holds.
- Each call's shell is named `scramjet-rs-bash-PID-N` (its `$0`, also in its error messages), so `ps` tells the calls apart and `pkill -f` stops one.
- A call that ends while processes it started still run lists them in its result, with their process group and the `kill -- -GROUP` that stops them all.

## 0.122.0 — Project command sets approve their own publications again

### Changed

- A command set inside a project (`.scramjet/<set>/`) may again mark its own commands' publications `auto-approve` in its `autonomy-defaults.yaml`, as in Scramjet; 0.97.7 had ignored that with a warning. A project's `autonomy.yaml` still cannot set publications.

## 0.121.0 — Prompt rows without dots

### Changed

- The prompt's rows after its first start with two spaces, under the text after `> `, rather than `. `, as in Claude Code and Codex.

## 0.120.0 — Retries spread out

### Changed

- A failed request retried without a Retry-After from the provider now waits its doubling backoff give or take a tenth (as Codex does), so that subagents failed together are not all sent again at the same moment. A provider's Retry-After is still waited exactly.

## 0.119.0 — Other scramjet-rs processes wait out a provider's Retry-After too

### Changed

- A provider's Retry-After now also holds the user's other scramjet-rs processes, other sessions and `-p` runs, sending to the same model there: they wait until then and start a quarter of a second apart, rather than each being refused in turn. The waits are kept in `$XDG_RUNTIME_DIR/scramjet-rs-cooldown` (else a private directory in the temporary directory).

## 0.118.1 — Requests held by a Retry-After start one after another

### Fixed

- Requests held by a provider's Retry-After no longer all reach it at the same moment when the wait ends: they start a quarter of a second apart, the refused request first, so parallel subagents are not refused together again. A longer wait given meanwhile is waited out too.

## 0.118.0 — Requests wait out a provider's Retry-After together

### Changed

- When a provider answers a request with Retry-After, the other requests this process sends to the same model there, the main agent's, subagents' and compactions', wait until then before they are sent, rather than each being refused in turn. The status line says `Provider asked to wait, sending in Ns`; Escape stops the wait. A wait longer than `retry.provider.maxRetryDelayMs` is not waited for.

## 0.117.0 — At most 8 subagents at once

### Changed

- At most 8 subagents run at once in a scramjet-rs process, whatever calls and conversations started them; the others wait their turn in the order they were asked for, and Escape stops those waiting. Several `subagent` calls in one reply used to run up to 4 children each, so three of them ran 12.

## 0.116.1 — `/status`, `/hotkeys`, `/mcp` and `/usage` follow a theme change

### Fixed

- What `/status`, `/session`, `/hotkeys`, `/mcp` and `/usage` showed in the conversation kept the colors of the theme in use when they ran; it now takes a new theme with the rest of the screen.

## 0.116.0 — `!` commands that end during a turn join the conversation

### Changed

- A `!` or `!!` command run in the full screen while a turn runs now shows at the conversation's end as soon as it ends, where it scrolls and Ctrl+O expands it, instead of staying cut to a few lines above the status line until the turn ends. Only the one still running stays there. The session still adds it after the turn, in the same place.

## 0.115.1 — `bg` lets a stopped turn run on

### Fixed

- After Ctrl-Z during a turn outside the full screen (as in `-p`), `bg` now lets the turn run on in the background, printing its reply there, instead of stopping it again at once (`Stopped (tty output)`). The status line waits for `fg`, which also brings back the turn's keys; a backgrounded run now exits on SIGTERM rather than stopping for the terminal. The full screen still stops when put in the background, as vim does.

## 0.115.0 — `/resume` continues a session in its own folder

### Changed

- A session of another folder chosen in `/resume` (Tab) now continues in that folder, as in Scramjet: tools run there, and that project's `AGENTS.md`, command sets, skills, themes and MCP servers apply; the footer shows its folder. A session whose folder no longer exists is still forked into this project, saying so.

## 0.114.2 — A stopped command's note follows the stop

### Fixed

- When a command's turn fails or is cancelled, `/NAME is waiting for you. Reply to continue.` now shows after the `Error: …` or `(cancelled)` that ended it, not before.

## 0.114.1 — Exported pages label commands and skills as the screen does

### Fixed

- `/export` and `--export` show a command's or skill's expansion under `[command] NAME` or `[skill] NAME`, without its tags, and compactions and branch summaries under `[compaction]` and `[branch]`, as the screen labels them.

## 0.114.0 — An edited command runs as edited

### Changed

- A command runs as its file says at that moment, when typed, taken as a next step or delegated, rather than as it was when scramjet-rs started or last ran `/reload`. A long-running session that edits its own commands (or chains through fresh sessions) no longer sends their old text. A file that no longer reads as that command leaves the last version found.

## 0.113.0 — The conversation stays in the terminal after exit

### Added

- Scramjet's `retainTranscriptOnExit` setting (Retain transcript in `/settings`, off by default): on exit, the full screen writes the whole conversation, the prompt and the footer into the terminal's scrollback, as they were on the screen, rather than leaving nothing behind.

## 0.112.0 — The terminal's tab shows the agent working

### Added

- Scramjet's `terminal.showTerminalProgress` setting (Terminal progress in `/settings`, off by default): while a turn, a compaction or a `!` command runs, the full screen tells the terminal it is busy (OSC 9;4), which Windows Terminal shows as a ring in the tab and progress on the taskbar, and clears it when the work ends or the screen is left. tmux does not pass it on.

## 0.111.0 — Lists' keys on narrow terminals, and /resume's notes that go

### Changed

- A list's keys that do not fit beside its title go on under it, on up to two more lines where the screen has room, rather than being cut off: `/resume`'s delete and rename keys now show below about 100 columns, and `/tree`'s filter keys at 80.
- `/resume`'s notes after a key (`Session deleted`, `Failed to rename: …`) go by themselves after 2 seconds, 3 for a failure, as in Scramjet, rather than staying until the next key.

## 0.110.0 — Images sent as they are, or not at all

### Added

- Scramjet's image settings: `images.autoResize` (on: images over 2000x2000 pixels or 4.5 MB that the agent reads or a prompt attaches are scaled down; off: sent as they are) and `images.blockImages` (on: no image goes to any model, each replaced by `Image reading is disabled.`, also in the conversation already held). `/settings` has a row for each (Resize images, Block images), applied at once.

## 0.109.1 — Next-step and approval settings safe alongside Scramjet

### Fixed

- `/settings` now takes Scramjet's update lock (`autonomy.yaml.lock`) while it changes an `autonomy.yaml`, so a next-step or publication setting saved by Scramjet at the same moment is no longer lost. A lock left by a process that has ended is taken back; one held by a running process is waited for briefly, then the change is refused with `update lock is busy`.

## 0.109.0 — Wheel, prompt height and completion rows as you set them

### Added

- Scramjet's layout settings: `scrollWheelStep` (3 lines a notch of the mouse wheel), `editorMaxHeightPercent` (30: the most rows the prompt's text takes, as a share of the screen's) and `autocompleteMaxVisible` (5 entries in a completion list), each kept within Scramjet's range. `/settings` has a row for each (Wheel scrolls, Prompt height, Completion rows), applied at once.

## 0.108.2 — Models no longer claim the next-step record

### Fixed

- The record of a next step chosen (or dismissed) now tells the model that the harness added it, so models stop taking it for a call they made by mistake. The session and the screen keep Scramjet's record as before.

## 0.108.1 — No status line left after SIGTERM

### Fixed

- When `-p` is ended by SIGTERM or SIGHUP, its status line (such as `Running` or a retry's countdown) is erased, so the shell's prompt no longer follows it on the same row.

## 0.108.0 — Compaction as you set it

### Added

- The `compaction` setting, in Scramjet's format: `enabled` (false: the conversation is never compacted by itself, only by `/compact`), `reserveTokens` (16384: compaction runs once the context passes the model's window less this, and summaries are capped by it) and `keepRecentTokens` (20000, the latest part kept whole), with `branchSummary.reserveTokens` for `/tree`'s summaries. `/settings` has an Auto-compact row, and `/reload` applies them.

### Changed

- The footer says `auto` beside the context only while compaction runs by itself, as Scramjet's.

## 0.107.0 — Retries as you set them

### Added

- The `retry` setting, in Scramjet's format: `enabled`, `maxRetries` (3), `baseDelayMs` (2000, doubled at each retry) and `provider.maxRetryDelayMs` (60000; 0 for no limit) say how a request that failed transiently is sent again. `/reload` applies them, and `--retries N` still overrides the number for the run.

## 0.106.0 — The session in the terminal's title

### Added

- The full screen titles the terminal's tab or window `scramjet-rs - NAME - FOLDER` (the session's name once it has one, and the project's folder), as Scramjet does, and clears the title when it leaves the screen, at the exit and while the external editor runs.

## 0.105.0 — Your own shell and lines before each command

### Added

- The `shellPath` setting names the shell that runs the agent's and your `!` commands (subagents' and the sandbox's too), in place of bash, as in Scramjet; one that is not there fails the command saying so. `shellCommandPrefix` runs its lines before each command, for example `shopt -s expand_aliases` and your aliases.

## 0.104.1 — One `!` command at a time

### Fixed

- On the full screen, a `!` command typed while another runs with no turn going is now refused with `A bash command is already running`, as during a turn and as in Scramjet; before, both ran at once.

## 0.104.0 — Sessions in a directory of your choice

### Added

- `--session-dir DIR` and the `sessionDir` setting save and find sessions in DIR rather than in the project's directory under `~/.local/state/scramjet-rs/sessions`, as in Scramjet; a relative DIR is from the current directory. `-c`, `-r`, `/resume` and `--session ID` look there, whatever project a session there was of; `/resume`'s Tab still lists every project's, and `/usage` counts the sessions there.

## 0.103.1 — A key command's children end with it on Windows

### Fixed

- On Windows, a `models.json` key command that is cancelled or runs past 10 s now ends with the programs it started; before, only its shell was stopped.

## 0.103.0 — Keys printed by a command

### Added

- A `models.json` `apiKey` or header value starting with `!` is a command, as in Scramjet: it runs with `sh` (10 s at most) each time a request or a model-list read is sent, and what it prints is the value, so a key can come from a password manager (`"apiKey": "!pass show openrouter"`) rather than sit in a file the agent's tools can read. A command that fails stops the request with what went wrong, never with what it printed. `/login` takes such a key too.

## 0.102.0 — Packages from a directory, with their skills and themes

### Added

- `scramjet-rs install DIR` of a directory that is not itself a command set installs it as a package, recorded in the settings' `packages` as Pi records a local path: its command sets, and its `skills` and `themes` directories, are loaded, and sets added to it later come at once. `install ~/repos/scramjet/packages/scramjet` brings Mach 12, `scramjet`, its skill and its theme; sets of it installed in `commandSets` before move into the package. `remove DIR` takes it out.
- A package from git brings its `skills` and `themes` directories too.

### Changed

- A theme file with a built-in theme's name and the same colors is no longer reported as skipped.

### Fixed

- A package's top directories count as command sets only when their `commands` or `agents` hold markdown, or they have a `models.json` or `set.yaml`: Scramjet's `dist` and `src`, whose `commands` hold code, were installed as empty sets.

## 0.101.0 — Ctrl+Backspace in Windows Terminal

### Added

- In Windows Terminal, Ctrl+Backspace deletes the word before the cursor in the prompt and in lists' searches, where it deleted one character; in `/resume` with an empty search it deletes the selected session, once confirmed, as in Scramjet (`app.session.deleteNoninvasive`). Ctrl+W and Alt+Backspace now delete a word in lists' searches too.

## 0.100.0 — Regular expressions in `/resume`'s search

### Added

- A search starting `re:` in `/resume` (and in `/model` and the other searched lists) is a case-insensitive regular expression, as in Scramjet's session search: `re:v\d+\.\d+` finds the sessions naming a version. Matches starting earlier come first; an empty or invalid one matches nothing.

## 0.99.3 — Thinking levels a model's map renames are sent as named

### Fixed

- A `thinkingLevelMap` that sends a level as a text of its own (`"off": "disabled"`, `"xhigh": "max"`) now sends that text for that level, in Responses, Messages and Chat Completions, as Scramjet does; before, `off` became the next level up and a renamed level could become a lower one. Shift+Tab, the footer and `/settings` show Scramjet's level names (`xhigh`), not the text sent.

## 0.99.2 — SIGTERM no longer leaves the editor behind

### Fixed

- SIGTERM while `$VISUAL` or `$EDITOR` has the terminal (Ctrl+G, or a `/settings` row) passes on to the editor and waits up to 5 seconds for it to end before exiting, so it can save and give the terminal back itself, rather than being left to stop in the background at its next read. The prompt's temporary file is removed, as after an edit.

## 0.99.1 — A full disk no longer empties a hard-linked file

### Fixed

- `write` and `edit` on a file with other hard links, or owned by someone else, which they write in place, now find room for the new text before cutting the old, so a full disk leaves the file as it was rather than empty.
- On Windows, `read` of a directory says it is a directory (use `ls`) rather than `Access is denied`, and `scramjet-rs remove` takes a `~\…` path, as `install` writes them and Windows' shells pass `~` on as typed.

## 0.99.0 — What another user left where scramjet-rs looks is left out

### Changed

- `CLAUDE.md` and `AGENTS.md` files in the project and the directories above it, the project's `.scramjet/SYSTEM.md`, `APPEND_SYSTEM.md`, `autonomy.yaml`, command sets and agents, and `.agents/skills` directories are now left out when a user other than you or root owns them, as Git refuses such repositories. A session started in `/tmp` no longer takes a `/tmp/AGENTS.md` another user put there. Each one left out is listed under `/settings` → Installed.

## 0.98.7 — MCP calls' arguments shown in full before approval

### Fixed

- With `--ask`, an MCP call whose arguments are too long for its call line (over 200 characters) now shows them in full above the approval question, so nothing past the cut is approved unseen.

## 0.98.6 — Invisible characters shown

### Fixed

- Replies, tool output, approvals and lists now show Unicode's invisible characters as escapes (`\u{e0041}`): the tag characters, which can spell out text the model reads but a terminal shows as nothing, soft hyphens, line and paragraph separators, and the rest of the default-ignorable ones. Tool output used to show bidirectional overrides too, which could reorder what a line looked like. Emoji variation selectors (`⚠️`) still show as emoji.

## 0.98.5 — More credential stores hidden

### Fixed

- Subagents, `--sandbox` and turns with the credential stores hidden now also hide Cargo's registry tokens (`~/.cargo/credentials.toml`), Hugging Face's (`~/.cache/huggingface/token` and `stored_tokens`), the GitHub Copilot editor plugins' (`~/.config/github-copilot`), glab's, RubyGems' and Terraform's, also where `CARGO_HOME`, `HF_HOME`, `XDG_CACHE_HOME` or `HF_TOKEN_PATH` put them.

## 0.98.4 — Typing on does not answer the agent's questions

### Fixed

- On the full screen, a question of the agent's (an `--ask` approval, project agents, a publication, a next step or `get_scramjet_user_input`) that appears while you are typing takes none of those keys: a `y`, an `a` or an Enter meant for your message used to answer it at once. It takes keys once typing has paused for half a second.

## 0.98.3 — Project agents always ask

### Fixed

- The `subagent` tool no longer offers `confirmProjectAgents`, and ignores it when a model passes it, so a model cannot skip your approval before project-local agents run.

## 0.98.2 — `--ask` sees through symlinks

### Fixed

- Under `--ask`, a `write` or `edit` whose path leads out of the project through a symlink now asks first, naming where the link leads, as one given outside it does; one through a symlink that leads nowhere asks too. Before, only the path as given was compared with the project's, while the write followed the link.

## 0.98.1 — Saved keys' refusals say what helps

### Fixed

- A saved key refused for a model whose server comes from a later file's entry of the same provider no longer says that `/login` again would help: `/login` saves a key only for the servers of the first entry naming the provider (the user's own `models.json` comes first), and the refusal now says so. The servers an entry's `modelOverrides` give are among those a key is saved for.

## 0.98.0 — Credentials tied to their servers

### Changed

- A key `/login` saves is tied to the servers its provider's entry names then, and sent only to them: a package's entry of the same name, or an entry edited to another address, no longer receives it. A key saved before this version is not sent until saved again with `/login`, which says so. Keys are no longer saved for a server reached over plain `http://` other than this machine.
- A login's token (ChatGPT, GitHub Copilot, a package's login) goes only to the servers its own login names, over `https://`, or to a server on this machine such as a local proxy; a `models.json` entry that names the login's provider with another address is refused, saying why.

## 0.97.10 — Publications shown as they will be sent

### Fixed

- A publication's approval, and its call once expanded, show each field's exact text in a code block rather than rendering it as markdown: an image's address, an HTML comment GitHub would hide, and a heading in the body that looked like one of the approval's own fields now show as written.

## 0.97.9 — YAML that cannot fill memory

### Fixed

- A command, agent or skill file's frontmatter, or an `autonomy.yaml`, whose aliases (`*name`) would expand past 100,000 copied nodes is refused with a warning, as Scramjet refuses one, rather than filling memory at start: twelve lines could make scramjet-rs use all the memory it could get, so a cloned project could stop it from starting.

## 0.97.8 — Reads that end, and stop on Escape

### Fixed

- `read` no longer loads a whole file to show a part of it: a file of any size is read in pieces, keeping only the lines shown, and Escape stops a long read. A device, FIFO or socket (`/dev/zero`, a named pipe) is refused, pointing to bash with a limit, where it used to hang the turn or fill memory; `edit` refuses them too.

## 0.97.7 — Projects cannot approve their own publications

### Fixed

- A command set inside a project (`.scramjet/<set>/`) can no longer approve its own commands' publications: an `auto-approve` in its `autonomy-defaults.yaml` is ignored with a warning at start, and they ask you, as a project's `autonomy.yaml` already could not approve any. Installed and global sets keep their defaults.

## 0.97.6 — Replies cannot drive the terminal

### Fixed

- A reply or reasoning shown on a terminal (`-p`, line mode, the full screen) shows control characters as escapes such as `\u{1b}]52;…`, as tool output already did, so a model's text can no longer set the clipboard, change the title or clear the screen. Replies piped or written to a file stay exact.

## 0.97.5 — Session files private

### Fixed

- Session files are created readable by you alone (`0600`), not by everyone (`0644`); until now only their directory, when scramjet-rs made it, kept others out. Files made by earlier versions keep their mode: `chmod 600 ~/.local/state/scramjet-rs/sessions/*/*.jsonl` tightens them.

## 0.97.4 — Files kept whole when a write fails

### Fixed

- `write` and `edit` no longer cut a file before writing it: the new text goes into a file beside it, renamed over it once complete, with the file's mode and group kept and a symlink left a link. A full disk or a kill during the write now leaves the file as it was (it used to be left empty or half written). A file with other hard links, owned by someone else, or in a directory that takes no new file is still written in place.

## 0.97.3 — Instruction files that lead into a credential store

### Fixed

- A `CLAUDE.md`, `AGENTS.md`, `SYSTEM.md` or `APPEND_SYSTEM.md` that is, or links to, a file in a credential store (`~/.ssh`, `~/.netrc`, those `--list-credential-stores` lists) is no longer sent to the model with every request: it is left out, and the start and `/reload` say `instructions: PATH: hidden, as it holds credentials`. Subagents leave it out too.

## 0.97.2 — Commands that stop processes by name

### Fixed

- A bash command that stops processes by a pattern from its own text (`pkill -f "gunicorn … --bind :8080"`, `pgrep -f … | xargs kill`) no longer kills the shell running it, which ended it with `Command terminated (signal: 15 (SIGTERM))` before the rest of the command ran. The command now reaches bash through its environment, not its command line.

## 0.97.1 — A read stuck on a long line

### Fixed

- `read` at a line longer than 50 KB now says how long it is and gives a `sed … | head -c` command to see it, as Scramjet does, instead of returning nothing and pointing at the same line again; a read cut by the 50 KB limit says so.

## 0.97.0 — Calls that change files run in their order

### Changed

- Of the calls a model makes in one reply, those that change files (bash, write, edit) now wait for the calls before them, and the calls after them wait for them, as in Claude Code and Codex: a read of a screenshot a command makes, or a build after an edit, sees what the earlier call did instead of failing or building half-edited code. Calls that only look (read, grep, find, ls, subagents) still run side by side, and so do bash calls, and edits and writes of different files. A waiting call shows in its place at once; after Escape it does not run.

## 0.96.1 — Downloaded rg and fd within commands' reach

### Fixed

- Bash commands, `!` commands and subagents' commands find the `rg` and `fd` scramjet-rs downloaded into the agent directory's `bin` (`~/.scramjet/agent/bin`), which is put first on their `PATH` as in Scramjet; a command calling `rg` failed with `command not found` where neither is installed.

## 0.96.0 — Commands without a time limit

### Changed

- A bash call runs until it ends, Escape stops it or the model's own `timeout` passes, as in Scramjet; it is no longer killed after 120 seconds by default, which cut long builds and test runs short.

## 0.95.9 — Times of calls run together

### Fixed

- In printed output (`-p`, piped input), a call that ran beside a slower one before it shows how long it took, not how long until it was printed.

## 0.95.8 — Code in list items

### Fixed

- A reply's list item shows no blank line between an indented code block and the item's next line, as Scramjet shows it; a blank line showed there.

## 0.95.7 — Wrapping indented lines

### Fixed

- An indented line of a reply's paragraph wraps back to where the paragraph starts (the screen's edge, the item's text or the quote's border), as Scramjet wraps it; it wrapped under its own indent.

## 0.95.6 — Indented lines in list items

### Fixed

- A line of a reply's list item indented past the item's text keeps the extra columns, as Scramjet shows it (four fewer from four on); it showed under the item's text.

## 0.95.5 — Blocks inside quotes

### Fixed

- A quote in a reply shows its headings, lists, nested quotes, rules, code blocks and tables as such, each line behind the quote's border, as Scramjet shows them; they showed as the quote's plain text. A line without `>` goes on in the paragraph or item it continues, nested quotes included, and a quote's last empty `>` lines are no longer shown.

## 0.95.4 — Table rows without bars

### Fixed

- A line right under a reply's table that starts no other block (a list, heading, quote, rule, fence or indented code) shows as the table's next row, as Scramjet shows it, even without `|`; it showed as text after the table.

## 0.95.3 — Indented code and line breaks in replies

### Fixed

- A reply's block indented by four spaces shows as a code block between fences, as Scramjet shows it, where it showed as text; one that goes on with a paragraph's text stays text.
- A backslash ending a reply's line breaks the line there without showing, as Scramjet's; it still shows at the end of a paragraph or before another block.

## 0.95.2 — Compaction that frees the context after long thinking

### Fixed

- Compaction keeps about the latest 20k tokens of the conversation again when the model thought at length: a reply counts as many tokens as it added to the context, as the next request's usage shows, where only its thinking's summary was counted. A `/compact` after a reply of 27k tokens of Claude's thinking kept about 56k tokens; it now keeps about 32k, from the start of that reply.

## 0.95.1 — Why commands do not run unattended on Windows

### Fixed

- On Windows, a command refused in a turn nobody started (`-p`, autopilot, a next step running by itself) says why in full: there is no sandbox there yet to hide the credential stores, as the `hideCredentials` setting asks. grep and find refused there say so too, where they said only that there is no sandbox.

## 0.95.0 — Installing the sets a directory holds

### Changed

- `scramjet-rs install DIR` of a directory that is not a command set installs the sets it holds, found as in a git package's clone (`scramjet-rs install ~/repos/scramjet` installs Mach 12 and `scramjet`); run again, it adds sets that appeared since. `scramjet-rs remove DIR` removes the sets installed from inside it.
- Installed sets in the home directory are written in the settings as `~/…`, so that the settings work on another machine with another home directory.

## 0.94.0 — Selecting the conversation without the mouse

### Changed

- With the prompt empty, Shift+Up, Shift+Left, Shift+Home and Ctrl+Shift+Left start a selection in the conversation at the end of its last line shown, so that it can be selected and copied with the keyboard alone; before, the keys moved only a selection the mouse had made. Ctrl+C copies it and Escape lets it go.

## 0.93.0 — Missing packages cloned at start

### Changed

- A session's start clones the git packages in the settings' `packages` that are not cloned yet, as Scramjet's start does, so that settings copied to another machine bring their command sets; it said to run `scramjet-rs update` before. `SCRAMJET_OFFLINE` (or `PI_OFFLINE`) keeps it from cloning, and `/reload` still only warns.

## 0.92.1 — Selected line breaks and steady columns

### Fixed

- A line break selected in the prompt shows as a selected cell at the end of its row, which showed nothing before.
- Shift+Up and Shift+Down in the conversation come back to the column they started from after passing a shorter line, rather than keeping the shorter line's.

## 0.92.0 — Selecting with the keyboard

### Added

- Shift+Left/Right/Up/Down, Shift+Home/End and Ctrl+Shift+Left/Right (also Alt+Shift) select text in the prompt, shown reversed: typing or a paste replaces it, Backspace or Delete removes it, Ctrl+C copies it and keeps it, Escape or a move without Shift lets it go.
- The same keys move the end of a selection made with the mouse in the conversation, by a character, a word, a line or to a line's start or end, scrolling to keep it shown.
- `/hotkeys` lists them under Selecting (`tui.editor.selectLeft` and the rest), so they can be given other keys.

## 0.91.0 — Your own words in spelling

### Added

- Spelling knows common developer words (`repo`, `config`, `async`, `stdin`, `tmux`, `clippy`, `subagent`…), which it underlined before.
- Words of your own, one a line, go in `words.txt` beside the settings file; `/reload` reads it again.
- Spelling in `/settings` (`spellCheck`) turns the underlining off.

## 0.90.0 — Release notes in a session

### Added

- `/release-notes` (Claude Code's name; Scramjet's `/changelog` works too) shows this changelog in a box, the newest version first and the rest behind Ctrl+O.

## 0.89.4 — The sign-in page opens on Windows

### Fixed

- On Windows, `/login` opens the sign-in page in the default browser (through `rundll32`, as Claude Code opens pages there) rather than trying `xdg-open`, which Windows lacks.

## 0.89.3 — MCP servers started through .cmd launchers on Windows

### Fixed

- On Windows, an MCP server's `command` named without an extension is found with `PATHEXT`'s, so `npx` runs `npx.cmd` rather than failing with `program not found`; so are the programs a `models.json` names (`providersCommand`, `logins`).

## 0.89.2 — Unzipping under a path with a space on Windows

### Fixed

- On Windows, when its own `tar.exe` cannot unpack a downloaded ripgrep or `fd`, PowerShell's fallback now takes paths with spaces (a user name such as `C:\Users\Jane Doe`).

## 0.89.1 — Escape stops a waiting download

### Fixed

- Escape stops a `grep` or `find` call waiting for ripgrep or `fd` to download; the download goes on behind it, for the next call.

## 0.89.0 — ripgrep and fd downloaded when missing

### Added

- When ripgrep or `fd` is not installed, scramjet-rs downloads its latest release from GitHub into `~/.scramjet/agent/bin`, as Scramjet does: at the full screen's start, saying so, and at a `grep` or `find` call. `@` completion uses the downloaded `fd`. `SCRAMJET_OFFLINE=1` (or `PI_OFFLINE`) turns it off.

### Changed

- The `tools` setting keeps `grep` and `find` when their program is missing but can be downloaded; they are left out only with `SCRAMJET_OFFLINE` set.

## 0.88.5 — Sessions with a cut last line, and paths, on Windows

### Fixed

- On Windows, a session whose file ends in a cut line (another program's write, or a crash) takes new prompts again: they failed with `Error: session: Access is denied. (os error 5)` and were not sent. A write cut short by a full disk is now cut back there too.
- On Windows, paths are shown with `\` throughout: settings' `~/…` and relative entries, the skills, themes, agents and `autonomy.yaml` places, the XDG directories, and `~` alone without a `\` after it; problems in `/settings`' Installed and `install`'s warnings show the home as `~\`.

## 0.88.4 — Ctrl+G and pasted images on Windows

### Fixed

- A prompt edited with Ctrl+G in an editor that saves Windows line endings (`\r\n`) no longer shows `\r` at each line's end: line breaks become plain newlines, as in Scramjet.
- On Windows, a pasted image's path no longer reads `Temp\/scramjet-rs-clipboard-….png`.

## 0.88.3 — Commands on Windows run in Git Bash, not WSL

### Fixed

- On Windows, shell commands no longer run in WSL's Linux when Git Bash is missing: WSL's `bash.exe` is passed over, Git Bash is also found beside `git.exe` on the `PATH` (as Claude Code finds it), and without a bash the command fails saying to install Git for Windows or put an MSYS2 or Cygwin `bash.exe` on the `PATH`.

## 0.88.2 — Logins and sessions on Windows

### Fixed

- On Windows, `/login` saves its credentials (the lock beside `auth.json` was refused as `Access is denied`), and the ChatGPT login no longer fails reading `/dev/urandom`.
- On Windows, two scramjet-rs processes writing the same session take turns, as on Linux and macOS, where before their writes went unserialized (Windows cannot lock a directory, so a `.lock` file in it is locked).

## 0.88.1 — Starting on Windows

### Fixed

- On Windows, scramjet-rs starts and saves its sessions: the project's path no longer carries `\\?\` (which made its sessions directory's name invalid), and the session directory is opened as Windows allows. A native build now runs a scripted turn on Windows, in `-p` and on the full screen.

## 0.88.0 — The clipboard on Windows

### Added

- On Windows, pasting text or an image (Alt+V) and copying (`/copy`, copy-on-select) use Windows' clipboard through one PowerShell kept running, as under WSL, where before only copying through the terminal (OSC 52) worked. Not yet run on Windows itself.

## 0.87.2 — The external editor and `@` paths on Windows

### Fixed

- On Windows, Ctrl+G and `/settings`' editor run `$VISUAL` or `$EDITOR` through `cmd.exe`, as Scramjet does, so that an editor installed as a `.cmd` (`code --wait`) starts, and the file's path is quoted, as a profile's path may hold spaces.
- `@` completion takes `\` in what is typed as `/`, and shows `fd`'s Windows paths with `/`, as Scramjet does.

## 0.87.1 — Settings, sessions and state found on Windows

### Fixed

- Settings, sessions, logins and other state are found in the home directory where `HOME` is not set, as on Windows (`%USERPROFILE%`, in the same `.config`, `.local/state` and `.local/share` layout as on Linux), and `~\` starts a path in the home directory there as `~/` does. A project's sessions on Windows go in a directory named after its path (`C--Users-…`), not into the project's own directory. A relative `XDG_CONFIG_HOME`, `XDG_DATA_HOME` or `XDG_STATE_HOME` is now ignored everywhere, as the XDG specification says.

## 0.87.0 — The terminal UI's code builds for Windows

### Changed

- The full screen, `-p` and line mode now compile for native Windows, with the console's own modes, size and input in place of a Unix terminal's; the console closing exits as a hangup does, and Ctrl+Z does nothing there. Only type-checked: no Windows build has been linked or run yet.

## 0.86.0 — A clipboard image pastes on macOS

### Added

- On macOS, Ctrl+V (or Alt+V) pastes a clipboard image as the path of a PNG file it is saved to, read through `osascript` as Claude Code reads it. Not yet tried on a Mac.

### Fixed

- On macOS, the sandbox's check that a file is not in a credential store finds an open file by the file itself, so that a rename while it is checked cannot point it elsewhere.

## 0.85.1 — The Seatbelt sandbox keeps a project under /tmp read-only

### Fixed

- On macOS, a read-only sandbox (subagents, `--sandbox read-only`) no longer lets commands write a project that lies under `/tmp`, `/var/tmp` or `TMPDIR`, and no directory above the project can be renamed, which in `workspace-write` would have moved its `.git`, `.scramjet` and `.agents` out of their protection.

## 0.85.0 — A Seatbelt sandbox for macOS

### Added

- On macOS, subagents' commands, `--sandbox` and unattended turns' hidden credential stores run under Seatbelt (`sandbox-exec`), as Codex confines its commands there, where they needed bubblewrap before and so failed: every file readable, writes only to `/tmp`, `/var/tmp`, `TMPDIR` and, in `workspace-write`, the project but its `.git`, `.scramjet` and `.agents` (also before they exist), the network and Unix sockets only when it is on. Not yet tried on a Mac.

## 0.84.0 — /fork offers a next-step list again

### Changed

- `/fork` before a prompt a next-step choice sent leaves the choice out of the new session and offers the list again there, as `/tree` does; the prompt goes into the editor if the list is dismissed.
- Without the full screen, `/tree` and `/fork` back to where a next-step list was offered name its steps (`Next steps offered here: …`), as there is no list to choose from.

## 0.83.1 — The macOS code compiles

### Fixed

- The sandbox's check of stores no longer fails to compile on macOS, where a file's device number has another type. `scripts/check-macos.sh` type-checks and lints both macOS targets on Linux; no macOS binary is built or run yet.

## 0.83.0 — Chains of sessions in /resume

### Added

- `/resume` shows each session a fresh-session next step chained with its `completed` summary in place of its prompt, and a `chain 2/3` mark. Ctrl+X shows chains flat (the default), threaded (each step under the one before) or folded (one row per chain; Right or Enter opens it, Left closes it), and the form chosen is remembered (`resumeChains`).

## 0.82.1 — A question kept after one asked again still waits

### Fixed

- When a question asked again by going back is followed, in the same reply, by a question you were to answer at the prompt, the running command waits for your reply again, as it did the first time, instead of getting a status check. A delegated command kept there is still refused if delegated again in the turn.

## 0.82.0 — Going back asks a question again

### Changed

- Going back with `/tree` (or Escape twice) to the answer of a question the agent asked (choose an option, or yes/no) asks it again, and the turn goes on with your new answer, branching from there. Other tool calls the agent made in the same reply keep their results and are not run again.

## 0.81.0 — Going back reopens a next-step list

### Changed

- Going back with `/tree` (or Escape twice) to where a next-step list was offered offers it again, with its options and its recommendation, instead of leaving you after it as if dismissed: pick the report or reply that offered it, the list's record, or the prompt your pick sent. What you pick then branches from there; Escape puts that prompt in the editor, as going back to a prompt does.

## 0.80.0 — /usage at a glance

### Changed

- `/usage` opens on a short summary: each quota window as one bar with when it resets and whether it lasts at this pace, what was spent today, this week and in the last 30 days (with the change from the 30 before), the models most of it went to and the costliest session. Where nothing has a price, it counts tokens.
- The tables, charts and speeds are a key away: `d` on the full screen, or `/usage details`. `/status` shows the quota in the same short rows.

## 0.79.0 — Nested repositories writable under --sandbox

### Changed

- Under `--sandbox workspace-write` only the project's own `.git` (with the Git directory a `.git` file names), `.scramjet` and `.agents` stay read-only, as in Codex. Repositories inside the project are writable again, so `rm -rf build` works over a CMake or pip checkout, and `write` and `edit` refuse only what the sandbox keeps read-only.

## 0.78.0 — Waiting for you at every hand-off

### Changed

- Whenever a turn ends, the status row says `Waiting for you` with the time since the agent stopped: after a reply, a freetext question, a next-step list and `/compact` or a `!` command, until you type or send something. It said so before only while a question or approval held a running turn.

## 0.77.0 — The quota in /status

### Added

- `/status` (`/session`) ends with the current provider's quota as its latest response reported it, the same rows as `/usage` shows (ChatGPT's windows and credits, Copilot's premium requests), as Codex shows plan limits in its `/status`.

## 0.76.1 — MCP servers stopped gently

### Fixed

- An MCP server run as a program is stopped as MCP says: its input is closed and it is given 2 seconds to exit, then asked with SIGTERM and given 2 more, and only then killed, so that its own clean-up runs. It was killed at once before.

## 0.76.0 — The last hours' usage, and sessions by size

### Added

- `/usage` shows the last hour's and the last 5 hours' requests, tokens and cost beside today's, so the current rate of spending is one row away.
- `/usage` counts the last 30 days' sessions by their tokens (under 10k, to 100k, to 1M, to 10M, and more), with each size's cost and a bar.

## 0.75.0 — When a quota runs out at the current pace

### Added

- `/usage` says when a quota window runs out if it goes on being used at the pace it has been since it started (`runs out 03:18 at this pace`), for windows that would run out before they reset: ChatGPT's, and GitHub Copilot's weekly limit and monthly premium requests.

## 0.74.2 — Nested repositories kept read-only behind a changed permission

### Fixed

- Under `--sandbox workspace-write`, a command that made a directory unreadable (`chmod 300`) or unsearchable (`chmod 000`) no longer leaves the repositories below it writable to the next command, and a repository made at the moment a command starts is no longer missed.

## 0.74.1 — Repositories made during a sandboxed session kept read-only

### Fixed

- Under `--sandbox workspace-write`, a repository cloned or created inside the project after the session's first command now has its `.git` kept read-only from the next command on; before, it stayed writable until scramjet-rs restarted.

## 0.74.0 — A full disk's session lines kept past the exit

### Changed

- Session lines kept in memory while the disk is full are also copied to a private directory in memory (`$XDG_RUNTIME_DIR/scramjet-rs-kept`, else `/dev/shm`), so they survive leaving, a signal or a kill. The next scramjet-rs run writes them to their session once there is room, and says so; resuming that session first takes them in. Leaving while they are copied no longer asks you to free space first.

## 0.73.0 — Every folder's sessions in /resume

### Added

- Tab in `/resume` lists the sessions of every project, each with its folder, and Tab again only this one's, as in Scramjet. Choosing another project's session continues it in a new session of this project, as `--session` with its id offers; that session stays as it was.

## 0.72.0 — ChatGPT's other limits and credits in /usage

### Added

- `/usage` shows the limits ChatGPT keeps for particular models beside the plan's 5-hour and weekly ones, each row named after its model (`gpt-5.3-codex-spark 5h limit`), and the account's credits (`118 credits`, `Unlimited` or `Available`), read as Codex reads them.

## 0.71.1 — The quota of a refused response

### Fixed

- `/usage` keeps the quota a refused response reports, such as ChatGPT's or Copilot's when a limit is reached, where it kept only replies', so it showed the last one before the limit. Compaction requests' quota is kept too.

## 0.71.0 — GitHub Copilot's quota in /usage

### Added

- `/usage` shows GitHub Copilot's quota as its latest response reported it: the monthly premium requests (how many of the plan's are used, and any made past them), and the session and weekly rate limits when Copilot sends them, each with a bar and when it starts again. The headers are read as VS Code's Copilot extension reads them, on any provider whose responses carry them.

## 0.70.0 — ChatGPT's quota in /usage

### Added

- `/usage` shows how much of ChatGPT's (Codex) five-hour and weekly limits is used, with a bar each and when each starts again, as the latest response reported it (its `x-codex-*` headers, read as Codex reads them). It heads that provider's tab and the All report, and is kept across sessions in the state directory's `quota.json`.

## 0.69.0 — Weekly usage, split by provider or model

### Added

- `/usage` shows each week of the last 12 (from Mondays) with its cost, requests and a bar, under the last 14 days. Both charts' bars are split by provider on the All tab and by model on a provider's tab, in shades a legend names (the largest three, then the rest together), and measure tokens where nothing shown has a price, so a subscription's usage is drawn too.

## 0.68.0 — /usage as a view, provider by provider

### Changed

- On the full screen, `/usage` opens in the conversation's place, which it gives back on Escape, showing the current provider's usage alone; Tab (or Left and Right) moves to all providers together or to each other provider that has responses. Without a terminal it still prints every provider's.

## 0.67.0 — The costliest sessions in /usage

### Added

- `/usage` lists the 10 costliest sessions of the last 30 days, each with when it started, its requests, tokens and cost, its project and its name or first prompt (such as the command it ran), marking the current one.

## 0.66.0 — Usage by provider, and speed percentiles

### Changed

- `/usage` adds a table of the last 30 days by provider, the current provider first, and a speed table for the models with timing saved: the median first output and output rate beside the slowest tenth's (p90 and p10). Models saved without a provider (before 0.17.0) count under `unknown`.

## 0.65.0 — Turning MCP servers off and on

### Added

- `/mcp disable [SERVER|all]` stops MCP servers and `/mcp enable [SERVER|all]` starts them again, as in Claude Code; the choice is kept in the settings' `disabledMcpServers`, so a disabled server does not start next time. `/mcp` lists disabled servers, and Tab completes the server names.

## 0.64.0 — Earlier sessions without a terminal

### Added

- Resuming a session of a chain with piped input (no full screen) prints a `[session]` block for each session before it, with what ran and its summary, above its conversation, as the full screen shows them.

## 0.63.3 — The status row during a list you opened

### Fixed

- A list or dialog you open during a turn (`/model`, `/settings`, `/tree`) leaves the status row on what the turn is doing, where it said `Waiting for you` as for the agent's own questions.

## 0.63.2 — No session file before the first prompt

### Fixed

- A new session's file is written at its first prompt, with the model, effort and name chosen before it, so a run killed or ended before any prompt leaves no empty session for `/resume` and `--continue`. On a full disk the start no longer says the session was not written; that comes at the first prompt.

## 0.63.1 — Resuming a session held on a full disk

### Fixed

- `/resume` of a session you left while a full disk kept its lines in memory continues with those lines, where it opened its file without them and both then appended to it.

## 0.63.0 — MCP server names completed

### Added

- `/mcp reconnect ` lists `all` and the MCP servers the settings name as you type, and Tab puts in the one selected.

## 0.62.0 — Usage across sessions

### Added

- `/usage` (also `/cost` and `/stats`, as in Claude Code) adds up what the saved sessions of every project spent: requests, input and output tokens, the share read from the cache and the cost, for this session, today and the last 7 and 30 days; each day of the last two weeks with a bar for its cost; and each model of the last 30 days with its median time to first output and output rate. Costs a provider does not give (subscriptions) show as `$?`. `/status` no longer answers to those names.

## 0.61.0 — MCP servers reconnected

### Added

- `/mcp reconnect SERVER` starts an MCP server again, and `/mcp reconnect` (or `all`) every one, as Claude Code's `/mcp reconnect` does, then says whether each started and with how many tools. A server that ended or failed to start no longer needs `/reload`, and a running one can be restarted. During a response it is refused, as `/reload` is.

## 0.60.0 — A full disk's held session lines kept at exit

### Changed

- On leaving while the disk is full, lines of the session kept in memory are tried once more. On a terminal scramjet-rs then waits: free space and press Enter to write them, or Ctrl+D to leave without them. Without a terminal it says they are lost, where they were lost without a word before.

## 0.59.0 — Small MCP servers' tools in the tool list

### Added

- The tools of MCP servers small enough (together within 2% of the model's context window) are in the model's tool list, named `mcp__SERVER__TOOL` as in Claude Code, so a call takes one request where a search and a call took two. Others are still found with `mcp_search` and run with `mcp_call`. A direct call shows and asks (under `--ask`) as its `mcp_call` does. `/mcp` says how each server's tools are offered.

## 0.58.1 — Logins kept on a full disk

### Fixed

- On a full disk, saving `auth.json` (a token renewal, `/login`, `/logout`) could leave it cut short, losing every saved login. It is now left as it was. A token renewed then is used and kept in memory, and is written once there is room, so that requests go on; a login or logout that cannot be saved says so.
- The model lists logins save, and the output rates, no longer leave a temporary file behind when the disk is full.

## 0.58.0 — Earlier sessions of a chain stay on screen

### Added

- In the full screen, a session that a fresh-session next step started shows each session before it in the chain above its conversation: a `[session]` block with the command that ran, when and for how long, and what it reported when it completed. Ctrl+O opens each into its conversation. This holds while a chain runs, where the screen used to clear at each fresh-session step, and when one of its sessions is resumed. Earlier sessions never reach the model.

## 0.57.4 — Forks, clones and imports on a full disk

### Fixed

- `/fork`, `/clone`, `/import` and `--fork` work on a full disk: the new session is kept in memory and written once there is room, where they failed with `No space left on device`.
- A session left (by `/new`, `/resume`, a fork, a clone or an import) while a full disk kept some of its lines in memory keeps them and writes them once there is room, saying so, where they were lost.

## 0.57.3 — A quiet session written once the disk has room

### Fixed

- A session kept in memory on a full disk is written within a few seconds of space being freed while the full screen waits for a prompt, where it waited for the next prompt or reply.

## 0.57.2 — `--no-session` on a full disk

### Fixed

- `--no-session` runs (as `-p --no-session`) also start on a disk with no room for a new directory, where they stopped with `session: No space left on device`, and no longer say "Session not written" of a session they do not save.

## 0.57.1 — Starting on a disk with no room for a session file

### Fixed

- scramjet-rs also starts when the disk has no room to create the session file or its directory at all (a full ext4 disk, or one out of inodes), where it stopped with `session: No space left on device`: the session is kept in memory and its file is created once there is room.

## 0.57.0 — Sessions go on when the disk is full

### Changed

- When the disk is full, scramjet-rs starts and turns go on: the session keeps what it cannot write in memory, says so once, and writes it out as soon as there is room again (saying that too), so the agent can be asked to find what filled the disk and free it. It used to stop at start, or end the turn, with `session: No space left on device`.

## 0.56.1 — Session files survive a full disk

### Fixed

- A line that could not be saved whole, as when the disk is full, no longer leaves the session file unreadable: a failed write takes back what it wrote, and a line left cut short (by an older version, or a kill) is cut off before the next one is written.
- A session file with lines that cannot be read opens without them, saying which (at start, and on `/resume`, `/fork` and `/clone`), where it was refused whole.

## 0.56.0 — Sessions linked into chains

### Added

- A session that a fresh-session next step starts names the session it came from in its file (`parentSession`, marked `nextStep`), and the earlier session's record of the choice names the new session's file, so a chain of sessions such as a `/rs:loop` run can be followed again. `/resume` still lists them as before; how it shows a chain is not settled yet.

## 0.55.0 — Spelling in the prompt

### Added

- Misspelled words in the prompt are underlined in the theme's `spellcheckError` color (curly where the terminal draws it), checked against the American English dictionary Scramjet uses. Paths, URLs, identifiers in camelCase or snake_case, words with digits, short words in capitals such as `JSON` and `!` shell commands are left alone, and typing never waits for it.

## 0.54.3 — Sandbox protections stay inside the project

### Fixed

- In `--sandbox workspace-write`, a `.git` file inside the project naming a Git directory outside it (under `/run`, `/tmp` or `/mnt/wslg`) no longer brings that directory into the sandbox, where host sockets such as WSL interop's could be reached; what is outside the project is read-only already.
- A dangling `.git`, `.scramjet` or `.agents` symlink in the project no longer stops every sandboxed command from starting.
- A worktree's common Git directory inside the project (a bare repository with its worktrees), where its config and hooks are, now stays read-only too.

## 0.54.2 — Nested repositories kept read-only in the sandbox

### Fixed

- In `--sandbox workspace-write`, the `.git` of each repository inside the project (a submodule, a checkout in a directory holding several), and a Git directory a `.git` file points to, now stay read-only as the project's own `.git` does, and the directories leading to them cannot be renamed. A command could plant hooks or `core.fsmonitor` there, which ran outside the sandbox at the next `git` there, or at a top-level `git status` through a submodule. They are found when the first sandboxed command runs; a repository made later stays writable.

## 0.54.1 — Sandboxed MCP servers see new credential stores hidden

### Fixed

- An MCP server started again in the sandbox (by `/reload`, or after it ended) now has the credential stores hidden as they are at that start, as bash does at each command; it could read a store created since the servers first started, such as one from a login during the session.

## 0.54.0 — MCP servers inside the sandbox

### Changed

- Under `--sandbox`, MCP servers that run as programs start inside the sandbox too, as the main agent's bash does: they see the credential stores and variables hidden, write only where the mode allows, and reach the network as `sandboxNetwork` says. `uvx` and `npx` get caches of their own there (in scramjet-rs's state directory), as theirs in the home directory are read-only. A server that fails to start in it says it ran in the sandbox.

## 0.53.4 — Credential stores stay hidden when the directories above them move

### Fixed

- With the credential stores hidden, `read`, `edit` and `write` check the file they opened by walking up from it rather than by its path, so a bash command renaming a directory above a store (`~/.config` above `.config/gh`) at the same moment cannot let a file inside the store through.
- With the stores hidden, `write` and `edit` no longer make directories in a store before refusing a path a symlink swapped in leads there, and write into directories they may enter but not list.

## 0.53.3 — Credential stores stay hidden from a raced read

### Fixed

- With the credential stores hidden (`--sandbox`, unattended turns, subagents), `read` and `edit` check the file they opened rather than its path, and `write` and `edit` check the file before cutting it, so a bash command running at the same moment cannot swap in a symlink after the check and have a store's contents reach the model or a store overwritten.

## 0.53.2 — Sandboxed writes stay where they were checked

### Fixed

- In `--sandbox workspace-write`, `write` and `edit` open each directory below the project and the file itself without following a symlink, so a bash command running at the same moment cannot swap in a symlink after the check and lead the write outside the project or into its `.git`. A FIFO put where the file goes fails the write rather than hanging it.

## 0.53.1 — No write through a dangling symlink in the sandbox

### Fixed

- In `--sandbox workspace-write`, `write` and `edit` refuse a path through a symlink that points nowhere yet: the write would have created its target outside the project or in its `.git`.

## 0.53.0 — A sandbox for the main agent

### Added

- `--sandbox read-only` or `--sandbox workspace-write` (`-s`), and a `sandbox` setting, with Codex's names: the agent's shell runs under Bubblewrap with every file read-only, a private `/tmp`, its own processes and the credential stores hidden, and its writes and edits are refused; `workspace-write` lets both change the project, but not its `.git`, `.scramjet` or `.agents`. Commands reach the network unless the setting `sandboxNetwork` is `false`. It refuses to start without Bubblewrap; the footer and `/status` say when it is on. `!` commands you type stay outside, and MCP servers do not run inside it yet.

## 0.52.0 — The theme editor's scores shown by color

### Changed

- The theme editor's scores line shows how good each score is by the color behind it, from red for bad through amber to green for good, in place of words after each number: `Harmony 81% · Text contrast 11.3:1 · Footer contrast 5.7:1`. The roles kept from the theme it started from now say so, as `38 roles from scramjet-dark`, and roles changed in the editor as `2 roles picked by hand`.

## 0.51.0 — Main colors moved up or down in the theme editor

### Added

- In the theme editor, Alt+Up and Alt+Down move the selected main color a place up or down, as `/scoped-models` moves models. Its lock and the roles picked to follow it go with it; the roles of each place take the color now there, and the first one tints the background. Undo and redo go through moves. The keys are `app.theme.moveUp` and `app.theme.moveDown` in `keybindings.json`.

## 0.50.0 — The theme editor's background lighter or darker

### Added

- The theme editor has a Lightness row under Background: Left and Right make the main background up to three steps lighter or darker than the one rolled or chosen. Rolls keep the steps, undo and redo go through them, and the saved theme keeps them. Every color is lit anew to read well on it, and roles kept from a theme the editor started from keep their colors, only lighter or darker. A step from the terminal's own background makes the theme set its own.

## 0.49.0 — A theme started from another stays itself

### Changed

- In a theme the editor started from another theme's colors, a step of contrast or another background now keeps each role's color, only lighter or darker by as much as the step moves it, with its hue and chroma. It used to replace nearly every role with the editor's own color. A role still takes the editor's color once the main color it follows changes hue, as a roll does.

## 0.48.0 — Another harmony keeps the colors

### Changed

- In the theme editor, choosing another harmony with Left and Right keeps the main colors as they are: only the harmony's score changes, and H then steps the colors toward the new one. It used to roll the colors not locked.

## 0.47.0 — Each response's timing recorded

### Added

- Each response saved in a session now records when its request was sent and how many milliseconds passed until its headers, its first streamed text, reasoning or call, and its end (`_scramjet_timing`), with the attempts that failed and were retried before it. Subagents' saved results keep each of their responses' usage and timing too. This starts the history a later `/usage` view draws on.

## 0.46.0 — Fresh session shown and switched on next steps

### Added

- The next-step list marks each step that starts a fresh session `(fresh session)`, and Tab switches that for the selected step before it runs (`app.nextStep.toggleFreshSession` in `/hotkeys`). The record of the choice and the notices of steps that run by themselves carry the mark too.

## 0.45.0 — Running tools and replies shown while a list is open

### Changed

- While a list or menu opened during a turn is open (`/model`, `/settings`, `/resume` and the like), the tools still running and a reply still streaming keep showing their output in place above it, rather than freezing until it closes. A question a tool asks meanwhile waits for the list to close.

## 0.44.0 — Commands at once while a reply streams

### Changed

- Built-in commands that act on the harness, and Ctrl+P, also run as soon as they are typed while a reply streams, rather than waiting for the reply to end, which with long reasoning could take minutes. The reply goes on in its place above what the command says; a model chosen meanwhile takes the next request.

## 0.43.0 — Commands at once while tools run

### Changed

- Built-in commands that act on the harness (`/settings`, `/model`, `/autopilot`, `/status`, `/copy`, `/hotkeys`, `/mcp`, `/login` and the like), and Ctrl+P, run as soon as they are typed while the agent's tools or subagents run, rather than waiting for the turn's next request. The tools go on meanwhile, their output staying in place above what the command says; what a command changes for the agent takes effect from its next request. Typed while a reply streams, they still wait for that reply to end.
- After a list or question opened during a turn closes, the status line says again what the agent was doing (`Running 12s`), rather than `Working`.

## 0.42.2 — Labeled replies in the margin look, padded questions

### Fixed

- In the margin look, a reply's first row is labeled `reply`, so it no longer reads as part of the call shown above it.
- In the panels look, a question the agent asks (`get_scramjet_user_input`) has a padded row above and below it inside its call's panel, while it waits and once answered, rather than a row off the panel before the answer.

## 0.42.1 — Attached files in a box

### Fixed

- A prompt that sent files named with `@` shows its `… (sent as N lines)` note, and what was sent once Ctrl+O expands it, in the same box as a command's or skill's text, rather than flush at the screen's left edge.

## 0.42.0 — Padded panels, one left edge, wrapped tool output

### Changed

- In the panels look, each call's panel has a padded row above its header, under it and at its bottom, as Scramjet's tool boxes; reasoning and replies start one column in, at the same edge as calls' output and the prompt's `›`.
- Long lines of a call's output wrap rather than being cut with `…`, in every look; a command's last lines are counted in screen rows, as in Scramjet. Blank lines at the start and end of a command's output are no longer shown.

## 0.41.0 — Clearer call headers, failures and footer

### Changed

- A file tool's path shows in the accent color in its call's header (`read`, `edit`, `write`, `ls`, and after `in` for `grep` and `find`), and a read's line range in the warning color, as in Scramjet. The classic look stays dim.
- In the panels look, a failed call's output keeps the normal output color on its red panel, rather than red text on red.
- The footer and the usage line no longer show `$?` when nothing spent has a known price, nor `?` for a context neither counted nor bounded by a known window.

## 0.40.0 — Panels, the header and a theme for the terminal's background by default

### Changed

- The `panels` look and the `header` banner are the defaults; `classic`, `rails`, `margin`, `plain`, `card` and `logo` stay in `/settings`.
- Without a `theme` setting, the theme follows the terminal's background as Scramjet's does: `pi-light` on a light background, `scramjet-dark` on a dark or unknown one. The full screen switches as the terminal answers; printed output (`-p`) goes by `COLORFGBG`. Choosing a theme in `/settings` saves it, as before. `terminal-colors` is still offered.
- The full screen asks the terminal only for its background as it opens. Its text color and 16 colors, which only the theme editor uses, are asked when `/settings` → Theme opens.

## 0.39.0 — Every dialog's rule in the theme's border color

### Changed

- The rule under a list or another dialog, over the footer, is in the theme's `border` color, as Scramjet's selectors' rules are, where it was in the muted text color. In the theme editor it shows the `border` being edited.
- In the theme editor's roles view, a role the sample session cannot show shows on the editor while it is selected: a thinking level's color, or bash mode's, on the rule under it, as on the prompt's rules, and `selectedBg` behind the selected row, as in `/resume` and `/tree`.

## 0.38.0 — More of a theme in the theme editor's sample

### Changed

- The theme editor's sample session shows a code block without a language and a horizontal rule in its reply, and ends with a call still running, so `mdCodeBlock`, `mdHr` and `toolPendingBg` (behind a running call in the panels look) can be seen as they are edited.

## 0.37.0 — A new theme from the terminal's own colors

### Added

- `/settings` → Theme offers New theme from terminal-colors… in a terminal that says what its colors are, as Windows Terminal does: the theme editor starts from the default theme as this terminal shows it, its cyan, blue, yellow, magenta and green as the main colors, on the terminal's background, each role keeping the terminal's color (faint text halfway to the background) until a roll or a step changes what it follows. The full screen asks the terminal for its text color and 16 colors (OSC 10 and OSC 4) when it asks for its background. A theme file using the terminal's colors by number starts from them too.

## 0.36.0 — A new theme from another theme's colors

### Added

- `/settings` → Theme offers New theme from NAME… for the theme in use when the theme editor did not make it and it has exact colors (`scramjet-dark`, `pi-light`, a hand-written file). The editor starts from that theme as it looks: its accent, border, heading, variable and string colors become the main colors (each distinct one in turn, in the harmony they fit best), on its own background, else the terminal's. Each role keeps that theme's color until what it follows changes: a roll keeps the roles of locked colors as they were and lights the others anew, a contrast step or another background lights them all, and undo brings them back. The roles' view names the colors kept, and the theme is saved as a new one.
- A background of a theme's own is offered in the editor's Background row beside the terminal's, dark and light ("its own, #14213d"), also for a theme the editor made whose background was changed in its file, which opened on the editor's own dark or light one before.

## 0.35.0 — Summaries and commands in Scramjet's message box

### Changed

- Compaction and branch summaries and what a command or skill sent show as Scramjet's message boxes: a bold `[compaction]`, `[branch]`, `[command]` or `[skill]` label in the theme's `customMessageLabel`, the text in `customMessageText`, and on the full screen a box on `customMessageBg` with a column and a row of padding around it (scramjet-dark and pi-light have one; the default terminal-colors theme shows the label and text without a box). A collapsed command or skill names itself and how many lines it sent.
- A skill's text, once expanded, shows without its `<skill>` tags or the arguments already in the prompt.
- The theme editor's sample session shows a compaction, so `customMessageBg`, `customMessageText` and `customMessageLabel` can be seen as they are edited.

### Fixed

- After `/compact` on the full screen, the next prompt follows the summary after a blank line; it came right under it.

## 0.34.0 — The selected row in `/resume` and `/tree`

### Changed

- `/resume` and `/tree` show their selected row on the theme's `selectedBg`, across the width, as Scramjet's session and tree selectors do; scramjet-dark and pi-light have one, the default terminal-colors theme none. Other lists keep their bold row and `›`.

## 0.33.0 — The prompt's rules show the thinking level

### Changed

- The rules above and below the prompt take the theme's color for the thinking level (`thinkingOff` to `thinkingXhigh`, `max` taking `xhigh`'s), and its bash-mode color while the prompt starts with `!`, as Scramjet's editor border does; they were always muted. Shift+Tab changes them at once.

## 0.32.0 — Editing a theme again

### Added

- A theme made in the theme editor opens in it again: once it is the theme in use, `/settings` → Theme lists "Edit NAME…", which opens it as it was saved, with its main colors and locks, harmony, background, contrast and picked roles; a role changed by hand in the file since then keeps its color there. Its file keeps them under a `themeEditor` key, which Scramjet's loader passes over.
- Saved under its own name, the theme edited replaces its file; another name saves a copy.

## 0.31.0 — Themes on the terminal's own background

### Added

- The theme editor can make a theme on the terminal's own background: its Background row offers the terminal's (shown with its color) beside its own dark and light ones, every color is lit and its contrast scored against the terminal's background, and the saved theme leaves the background to the terminal, as Scramjet's themes do. The editor starts there when the theme in use has no background of its own, so it starts light on a light terminal. The full screen asks the terminal for its background (OSC 11), as Scramjet does; a terminal that does not answer, such as tmux 3.0a, is taken as `COLORFGBG` or Apple's Terminal say, else the row says the background is unknown.

## 0.30.0 — Every role in the theme editor

### Added

- The theme editor lists every role of the theme with Tab: all 52, each with its color, its contrast with the background in numbers and words, what it follows and a line on what it colors. Left and Right make a role follow another of the main colors than its own (a color with a meaning, such as the errors' red, then leaves its range of hues), `#` sets it to an exact color typed as hex (`ff8800` or `f80`, pasted too), and Backspace makes it follow its own again. Rolls and contrast steps keep these picks, undo and redo go through them, and the saved theme holds them.
- Its keys are in `/hotkeys` and `keybindings.json`: `app.theme.roles`, `app.theme.exact` and `app.theme.reset`.

## 0.29.0 — A theme editor

### Added

- `/settings` → Theme → New theme… opens a theme editor. Choose how many main colors the theme has (1 to 5) and a harmony that fits them (complementary, triadic, split-complementary, square…, each said in a line), on a dark or light background, then roll (R) until it is close. Lock (L) the colors you like and a roll changes only the others, placed in the harmony around them; Left and Right turn a color's hue, H steps the colors toward the harmony, + and − step the contrast, U undoes and Shift+U redoes. A sample session shows the theme in the conversation's place as you go, with the harmony's score and the text's contrast in numbers and words. Every role of the theme is derived from the main colors, its text and backgrounds kept readable: errors, success and warnings keep their reds, greens and ambers. Enter names the theme and saves it in Scramjet's format in `~/.scramjet/agent/themes/`, where Scramjet finds it too, and uses it.
- The editor's keys are in `/hotkeys` and `keybindings.json` (`app.theme.*`).

## 0.28.2 — Responses streams as providers send them

### Fixed

- Replies over the Responses API failed as an "unexpected response stream" when a provider sent a reasoning summary before its item, without its part's start, or with its ending events missing their ids, as the company's GPT models do without its proxy in between. Such streams are now read, as Scramjet's provider package reads them: an item's events find it by id, by position, or as the one still open, and an item, part or call missing its start is made when its events come. A function call runs with its final copy's arguments, also when the separate end of its arguments never came, and a call repeated with other arguments or another's call id stops the reply.
- Repeated events, items of other kinds (a web search), unknown message phases, non-text deltas, a namespace that is not a text and a queued response no longer fail a reply; a completed response with nothing in it is an empty reply.

## 0.28.1 — What a login program says when it fails

### Fixed

- A package's login program that failed sometimes showed only its exit status, without the reason it wrote; the reason is now always shown.
- A package's `models.json` whose `logins` is not an object now says so, where it was passed over without a word.

## 0.28.0 — Packages' logins

### Added

- A package can bring a login of its own: its `models.json` names a program in `logins`, which `/login` lists after ChatGPT and GitHub Copilot. The program has your browser open its sign-in page, which is also shown and copied, and the dialog says what it waits for; Escape stops it. Its token is saved in `auth.json` and renewed by the program once it expires, and the provider's models it lists are read after the login and kept current while the full screen is open, as Copilot's are. The format is in the README.

### Changed

- When no provider offers a model at start, the warning names `/login` beside `models.json`.

## 0.27.1 — MCP servers that redirect

### Fixed

- An MCP server over HTTP that redirects its address (a 307 or 308, as Python servers mounted at `/mcp` send to `/mcp/`) failed to start; a redirect on the same host is now followed, as Claude Code and rmcp follow it, and one elsewhere says where it points.

## 0.27.0 — MCP servers over HTTP

### Added

- MCP servers over streamable HTTP, written as Claude Code's `.mcp.json` lists them: `{"docs": {"type": "http", "url": "https://…", "headers": {"Authorization": "Bearer ${DOCS_TOKEN}"}}}`. Their sessions are started again when a server forgets them and ended when scramjet-rs stops; `/mcp` shows each server's URL without its query. A server that asks for authorization says to give a token in its `headers`, as OAuth sign-in is not supported yet.
- `${VAR}` and `${VAR:-default}` in an MCP server's entry (its command, arguments, `env`, URL and headers) are the environment's variables, as in Claude Code, so a key need not be written in the settings.

## 0.26.1 — MCP servers that keep running

### Fixed

- An MCP server that wrote a byte that is not UTF-8 to its standard error lost the pipe and usually ended at its next line there; what it writes is now read whatever its bytes.
- `/reload` starts again a server that ended, and only the servers whose entries changed, where it kept or restarted all of them. A server started and stopped at once, as by a quick `/reload`, is no longer left running.
- Escape stops a search waiting for a server to list its tools, which could take 10 seconds per server; a search's result is cut as other tools' are (2000 lines, 50 KB). `/mcp` shows control characters in what a server wrote as escapes.

## 0.26.0 — MCP servers' tools

### Added

- MCP servers run from the settings' `mcpServers`, written as Claude Code's `.mcp.json` lists them (`{"time": {"command": "uvx", "args": ["mcp-server-time"], "env": {…}}}`), each started in the background in the project directory as the session starts. The model gets two tools however many servers and tools there are: `mcp_search` finds the servers' tools by words from their names and descriptions and shows how to call each, and `mcp_call` runs one; the instructions list each server in one line. Servers speaking MCP's 2026-07-28 revision and older ones (the `initialize` handshake) are both understood; only servers run as programs (stdio) for now, not over HTTP.
- `/mcp` shows each server, the command it runs, its tools, or why it is not available with the end of what it wrote to its standard error. A server that cannot start is also said once when the first request goes out, which waits for servers still starting (30 seconds at most). Escape stops a tool call and cancels its request; `--ask` asks before each call.

## 0.25.0 — Providers that list their own models

### Added

- A `models.json` provider that lists no `models` offers those its server lists (`GET {baseUrl}/models`, or `/v1/models` under `anthropic-messages`), so a local vLLM or Ollama server, OpenRouter or Anthropic needs only its `baseUrl`, `api` and key. What the list says of each model is taken: Anthropic's window, output, effort levels and images; OpenRouter's limits, reasoning, images and prices, leaving out the models that take no tools; Mistral's, Groq's and vLLM's windows. Scramjet's `modelOverrides` add what a list does not say. The list is saved, read again in the background while the full screen is open once 15 minutes old, and asked for again when `/login` saves the provider's key, which then says how many models it offers.

## 0.24.0 — GitHub Copilot is told it is scramjet-rs

### Changed

- Requests to GitHub and GitHub Copilot (the login, token renewals, the model list and model requests) say they come from scramjet-rs (`User-Agent: scramjet-rs/0.24.0`), where they claimed to be VS Code's Copilot Chat. Only Copilot's API is still told the editor is VS Code (`Editor-Version: vscode/1.107.0`), as it refuses requests naming no editor or one it does not know.

## 0.23.0 — Start banners to try

### Added

- `/settings` has a Banner row (the `banner` setting) offering three banners to try in place of the plain start lines: `header`, as Scramjet's (the name and version, the main keys, then the context files, command sets and skills loaded); `card`, a box of the model, project, context files and what was loaded; and `logo`, Scramjet's logo in block characters with the version, model and project beside it, or under it on a narrower terminal. Ctrl+O expands any of them to every key and where each loaded thing is. The plain lines stay the default until one is chosen.

## 0.22.2 — Token renewals retried, and Escape stops them

### Fixed

- When renewing a GitHub Copilot or ChatGPT login's token fails because GitHub or OpenAI did not answer, or answered with an overload or server error, the request is tried again as other transient failures are, instead of ending the turn at once. A refused renewal (a revoked login) still ends it, saying to `/login`.
- Escape stops a request whose login's token is being renewed, at once, where it waited up to 30 seconds for GitHub or OpenAI; the renewal ends by itself and keeps what it got. "Waiting for reply" shows from the start of each attempt, the renewal included.

## 0.22.1 — /logout of an API key removes only the key

### Fixed

- `/logout` of an API key whose provider is named with `../` in a `models.json` file no longer deletes the `.json` file that name leads to.

## 0.22.0 — API keys in /login

### Added

- `/login` lists, under API keys, the providers of the `models.json` files, each with where its key comes from (saved, a variable, or the file), and saves a key you enter in `~/.config/scramjet-rs/auth.json` beside the logins, so it need not be written in `models.json`. Requests to that provider carry it in place of the entry's `apiKey`, as in Scramjet. A key with a space or a line break inside, as a broken paste leaves, is refused. `/logout` removes it.

## 0.21.0 — Models by provider

### Changed

- `/model` lists the models under a heading for each provider, the current model's provider first, so the same model from two providers is easy to tell apart. A query keeps the models it matches under their providers' headings, the best match selected, and typing a provider's name (`copilot`, `chatgpt`) shows its models. The list takes up to half the terminal's height. The scope's list (with `enabledModels`, Tab) keeps its order and names each model's provider.
- `/hotkeys` keeps the shortcuts a query matches under their headings.

## 0.20.1 — Package fixes

### Fixed

- `scramjet-rs install git://HOST/USER/REPO` works: the `git:` shorthand no longer takes in `git://` URLs and refuses them.
- A package whose clone would lie inside another's, or hold it (`…/tools.git` and `…/tools/extra.git` on one server), is refused, and `remove` keeps a clone that holds another package's clone, which it used to delete with that clone's changes.
- A directory in a clone's place that is not a clone is neither used by `install` nor pulled by `update`, so git is not run on a repository around it.

## 0.20.0 — Packages from git

### Added

- `scramjet-rs install git:github.com/USER/REPO[@REF]` (or `git:git@github.com:USER/REPO`, or an `https://`, `ssh://`, `git://` or `file://` URL) clones a repository with `git` into `~/.local/share/scramjet-rs/git/` and loads its command sets: the repository itself when it is one, else the sets at its top and, deeper, those with a `set.yaml`, so `git:github.com/LeanAndMean/scramjet` brings Mach 12 and `scramjet`. It says which programs the package runs. The settings list it under `packages`.
- `scramjet-rs update` pulls the packages not pinned to a ref, keeping changes made in their clones, and clones those missing; `scramjet-rs remove REPO` (or `uninstall`) deletes the clone. `list` shows which package each set came from.

## 0.19.0 — Model lists kept current

### Added

- GitHub Copilot's and ChatGPT's model lists are read again in the background while the full screen is open, once 15 minutes old and at start too, so a model the provider adds is offered without a restart or a new `/login`. The models it now offers or no longer offers are named; a list that cannot be read again stays as it was, and that is said once.

## 0.18.1 — ChatGPT login fixes

### Fixed

- At ChatGPT's login, a pasted query (`?code=…&state=…`) or an address without `http://` (`localhost:1455/auth/callback?…`) is taken, as in Scramjet, rather than ending the login.
- Pressing Escape at ChatGPT's login after its tokens were saved now says you are logged in but the model list was cancelled, rather than "Login cancelled".

## 0.18.0 — ChatGPT (Codex) login

### Added

- `/login` logs in to a ChatGPT Plus or Pro subscription (Codex): your browser opens OpenAI's login page, and the answer comes back to scramjet-rs by itself. The page's address is shown and copied, so you can open it in another browser or a private window, or paste the address the browser ends on.
- ChatGPT's Codex models for your account are offered as ChatGPT lists them at login, with their context windows and effort levels, including models newer than any list kept by hand. Their requests go to ChatGPT's Codex backend, and the token is renewed when it expires.
- Reaching a ChatGPT usage limit ends the turn at once, saying which plan and when to try again, rather than retrying.
- A `models.json` provider may use `"api": "openai-codex-responses"`, as in Scramjet.

## 0.17.0 — Models named by provider and id

### Changed

- A model is named by its provider and id, `provider/id`, in sessions, `enabledModels`, the model chosen last and output rates, as in Scramjet. Two providers may now offer the same id: GitHub Copilot's `gpt-6-luna`, `claude-opus-5` and others are listed beside the same ids from your `models.json` providers, and the start-up warning about them is gone. An id alone, as in sessions and settings saved before, names the first provider's model.
- The model chosen last is saved as Scramjet saves it, `defaultProvider` and `defaultModel`.
- `/model`'s completion shows each model's id with its provider and puts in `provider/id`. With models of several providers, the start line and "Model set to" name the provider, as the footer does: `(github-copilot) gpt-6-luna`.
- After `/reload`, a session whose model's provider is gone keeps that model (its requests fail, saying so) instead of moving to another provider's model of the same id.

## 0.16.1 — Credentials stay hidden while a login changes

### Fixed

- Saving or renewing a login no longer shows `~/.config/scramjet-rs/auth.json` to the agent's commands that are already running, such as a tmux server it started; before, each renewal of GitHub Copilot's short-lived token did. The file is now made, without logins, before the agent's first command, so a first `/login` stays hidden from those too.
- A practice run under `scripts/without-credentials.sh` no longer warns at start that the hidden `auth.json` is not JSON.

## 0.16.0 — GitHub Copilot login

### Added

- `/login` logs in to GitHub Copilot with GitHub's device login, for github.com or a GitHub Enterprise domain. Your account's Copilot models are offered from then on, with the context, input and output limits, effort levels and token prices Copilot lists, so the footer shows their cost. `/logout` removes the login.
- The login is saved in `~/.config/scramjet-rs/auth.json`, readable by you only and hidden from the agent's tools as other credential stores are. Its short-lived Copilot token is renewed when it expires.
- A model's `maxInputTokens`, as in Scramjet: automatic compaction also runs once the context passes it.

### Fixed

- Messages streams that end with `data: [DONE]` (GitHub Copilot's) are read, and Responses streams whose events each name new ids (GitHub Copilot's) too.

## 0.15.0 — Chat Completions providers

### Added

- A provider's `api` may be `openai-completions`, Chat Completions, which vLLM, Ollama, llama.cpp and many hosted services serve. Tool calls, images, reasoning (`reasoning_content` and the like), usage with cached tokens, and replies cut at the output limit work as with the other APIs.
- What such a server accepts is told from the provider's name and URL as Scramjet tells it (DeepSeek, Together, z.ai, OpenRouter and others), and a provider's or model's `compat` says otherwise with Scramjet's fields.

## 0.14.0 — HTTPS providers

### Added

- A provider's `baseUrl` may be `https://`. Its certificate is checked against the system's trusted certificates, or against those `SSL_CERT_FILE` and `SSL_CERT_DIR` name, so a company's inspection CA in the system's store works. An untrusted certificate fails at once, without retries, and says where trust comes from.

## 0.13.1 — Stores stay hidden across /reload

### Fixed

- A package's credential stores stay hidden after `/reload`, or after a change in the settings menu, while its `models.json` cannot be read. Before, they showed to subagents and hidden turns until the file was fixed.

## 0.13.0 — Packages name their credential stores

### Added

- A `models.json`, the user's or a package's, may list in `credentialFiles` the files and directories that hold its providers' credentials. The agent's tools do not see them wherever they hide the common stores: in subagents always, and in the agent's own turns as `hideCredentials` says. A store inside another hidden directory no longer stops commands from starting.
- `--list-credential-stores` lists the stores hidden from the agent's tools.

### Changed

- The company's local proxy's credential directories are no longer hidden by scramjet-rs itself; its private package names them.
- `scripts/without-credentials.sh` hides the stores that the scramjet-rs built beside it lists. Without such a build it hides the common ones and says so.

## 0.12.1 — Provider programs and models fixed

### Fixed

- A `providersCommand` program is held to its 2 seconds even when it leaves a process running on its output, and the processes it started end with it when it runs out of time. One that writes much to its error output is no longer cut off.
- A model whose `thinkingLevelMap` sends two levels as one (as Scramjet's maps for GPT-5 do) no longer stops Shift+Tab at that level.
- A Responses model without `reasoning: true` is no longer sent reasoning settings, as in Scramjet.
- After `/reload` or a change of model, a model removed from files read without problems is no longer sent requests at its old address; one whose program failed still is.

## 0.12.0 — Providers from models.json

### Changed

- Models come from the providers listed in `models.json`, in Scramjet's format: the one beside the settings (`~/.config/scramjet-rs/models.json`), then one in each installed command set. Each provider has a `baseUrl` (plain `http://` for now), an `api` (`openai-responses` or `anthropic-messages`), an `apiKey` (an environment variable's name, or the key itself) and its `models`, each with its limits, thinking levels, whether it takes images and its prices. `--help` and the README say how. `--endpoint` is gone.
- The company's local proxy's route comes from a private package, installed with `scramjet-rs install`, instead of being built in.
- `/model` and `/scoped-models` name every model's provider, as Scramjet's do, and list the models by provider.

### Added

- A `models.json` may name a program in `providersCommand` whose output adds providers, as a package that learns its models at run time does.
- `scramjet-rs install` takes a directory that has only a `models.json`.
- A model's `cost` may give `longContext` rates for responses whose input is over a number of tokens, and rates by the value of a response header (`header`, `byHeader`), such as the backend that served it.
- What is wrong in the `models.json` files is said at start, by `/reload` and by `--list-models`.

## 0.11.0 — Three new looks to try

### Added

- `/settings` has a Look row: Enter changes how the conversation shows prompts, reasoning, tool calls and replies, at once. `classic` is the look so far; `rails` puts a colored rail beside each block, `panels` puts prompts and calls on the theme's backgrounds (scramjet-dark has them), and `margin` labels each block in a column at the left. In the new looks, a call's line says how it went (✓, ✗, or ○ while it runs) and how long a command took. One of them is to become the default once picked; printed output (`-p`) keeps the classic look.

## 0.10.0 — A theme's own background

### Added

- A theme may color the main background, with `background` among its `colors` (a color or a name from its `vars`, beyond Scramjet's format). The full screen then shows everything on it, dialogs and the footer included, with the theme's `text` color as the color of plain text; a theme without one keeps the terminal's own background, as before.

## 0.9.0 — Themes show as you edit them

### Added

- The theme in use is read again each time its file is saved, and everything shows in it at once, as in Scramjet. A save that is no theme says why above the prompt, and the colors stay as they were until the file is mended.
- The `themes` setting and `--theme PATH` (repeatable) add theme files, or directories of them, to those `/settings` offers, as in Scramjet; `--no-themes` leaves only the built-in themes and those `--theme` names.

### Changed

- `/settings`' list of themes reads their files as it opens, so a theme file added meanwhile is offered without `/reload`.

## 0.8.1 — /hotkeys keeps your search

### Fixed

- After a change, `/hotkeys` shows again with the search you typed, its matches and the action you changed selected, rather than the whole list.
- The last texts naming default keys name yours: Installed's notes (`Enter opens …`, `Enter removes it once you confirm`, `Enter opens it or forgets the approval`), `/scoped-models`' description in the command list (`Ctrl+P cycling`) and the Escape twice setting's description.

## 0.8.0 — Change keys in /hotkeys

### Added

- `/hotkeys` (or Codex's name, `/keymap`) lists every key on the full screen, to search as you type; Enter on an action offers to press a new key for it, add another, remove one or restore its default. Before taking a key, it says what else that key already does where the action works, such as Ctrl+K deleting to the end of the line. The change is saved in `keybindings.json` at once and used at once. With piped input, `/hotkeys` prints the list as before.
- `/keybindings`, as in Claude Code, opens `keybindings.json` in your editor and reads it again when the editor exits, saying what is wrong in it.
- `/settings`' Installed lists `keybindings.json` among the settings files.

## 0.7.1 — Hints name your keys

### Fixed

- The hints name the keys `keybindings.json` gives: those of the lists, `/tree`, `/resume`, `/scoped-models` and `/settings`, `Esc to interrupt`, the queue's `Alt+Up to edit`, `Ctrl+O to expand`, the exit's `Ctrl+C again`, and the settings' descriptions. A hint leaves out an action without keys. `--help` says its keys are the defaults.

### Changed

- Hints write keys alike together, as `Ctrl+A/X` and `←→`, and the exit's hint `Ctrl+C`, not `Ctrl-C`. With piped input, `/scoped-models` says `Models for Ctrl+P: …`.

## 0.7.0 — Keys of your own

### Added

- Keys can be changed: `~/.config/scramjet-rs/keybindings.json`, in Scramjet's format and with its action ids, gives an action other keys, for example `{"app.message.followUp": "ctrl+q"}` where the terminal keeps Alt+Enter, as Windows Terminal does. `[]` leaves an action without keys. It is read at start and by `/reload`; what is wrong in it is said at start and listed in `/settings`' Installed.
- `/hotkeys` lists every action with the keys it has now, those of `/resume`, `/scoped-models`, `/tree` and the lists included, and names the file that changes them.

### Changed

- Ctrl+Up, Shift+Up and other arrows with a modifier no key uses no longer act as the plain arrow, as in Scramjet; Alt+Up and Alt+Down are no longer Up and Down in a dialog's text. Tab no longer adds a space to a list's search.
- Keys the terminal reports in the kitty or modifyOtherKeys forms are all read, not only some, as are F1–F12, Insert, and rxvt's Shift and Ctrl forms.

## 0.6.0 — The terminal's paste key

### Changed

- When the clipboard cannot be read (in a remote session, or under WSL when PowerShell cannot start), the first failed right-click paste names the key that pastes through the terminal itself: Ctrl+Shift+V in Windows Terminal and most others, Command+V on a Mac, Shift+Insert in xterm, which pastes only selected text. Later failures give only the cause.

### Added

- [docs/TERMINALS.md](docs/TERMINALS.md): what each terminal does with copying, pasting and the keys scramjet-rs uses, such as Windows Terminal keeping Alt+Enter for full screen.

## 0.5.0 — Copy on select

### Added

- Copy on select, in `/settings` (`copyOnSelect` in the settings file): text selected with the mouse is copied as soon as the button is let go, or at a double or triple click, so Command+V or Ctrl+V pastes it anywhere at once. It is on by default on macOS only, whose terminals keep Command+C to themselves; a right click or Ctrl+C still copies a selection either way.

### Fixed

- A copy that ends after a new selection was made no longer takes that new selection away.

## 0.4.0 — Copy under WSL inside tmux

### Changed

- Under WSL, a mouse selection and `/copy` go to Windows' clipboard through the PowerShell process that already serves pastes, in 30–200 ms. Copying now works inside tmux, whose default settings drop the terminal's copy sequence (OSC 52), so copying there used to fail silently. Copied text is no longer limited to OSC 52's 75 KB. When PowerShell fails, the copy still goes through the terminal.

## 0.3.0 — Copy on Linux without xclip

### Added

- On Linux with an X server (`DISPLAY`), a mouse selection and `/copy` are copied through the X server itself, so `xclip` and `xsel` are no longer needed for copying either, and copying works in terminals without OSC 52 (GNOME Terminal and other VTE terminals, such as Raspberry Pi OS's). The copy stays pasteable after scramjet-rs exits, held by a small `scramjet-rs` process that ends once another program copies, as `xclip` does. Under Wayland, `wl-copy` is still tried first when it is installed. WSL keeps copying through the terminal.

## 0.2.0 — Paste on Linux without xclip

### Added

- On Linux with an X server (`DISPLAY`), a right-click paste and Ctrl+V or Alt+V's image read the clipboard from the X server itself, in about 1 ms. `xclip`, `xsel` and `wl-paste` are no longer needed for pasting, though they are still tried after it. Under Wayland, this goes through XWayland. Copying still uses the tools or the terminal.
- A paste with no text on the clipboard, as when it holds only an image, says "No text on the clipboard".

## 0.1.1 — Quicker paste under WSL

### Fixed

- Under WSL, a right-click paste takes about 0.1 s instead of 0.7 s, and Alt+V's image about 0.15 s instead of 0.6 s. One PowerShell process, started with the full screen, now reads Windows' clipboard for every paste. It compiles no code and leaves no files in Windows' `%TEMP%`.
- A failed image paste under WSL says why, instead of "No image on the clipboard".

## 0.1.0

Everything before this file began; the Git history has the details.
