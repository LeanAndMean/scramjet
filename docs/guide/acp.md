# In an editor (ACP), early

[Back to the README](../../README.md)

`scramjet acp [options]` runs Scramjet as an [Agent Client Protocol](https://agentclientprotocol.com) (v1) agent, for editors and other clients that start an agent and speak JSON-RPC on its stdin and stdout; the options are the full screen's. It passes ACP's test kit, and has been tried in Toad, Zed and PyCharm. [RPC.md](https://github.com/LeanAndMean/scramjet/blob/main/docs/RPC.md) has the protocol's details.

## In an editor

An editor that hosts ACP agents starts `scramjet acp` as its command. In PyCharm and other JetBrains IDEs, list it in `~/.jetbrains/acp.json` under `agent_servers`:

```json
{"agent_servers": {"Scramjet": {"command": "scramjet", "args": ["acp"]}}}
```

The key is the name the IDE shows. If it does not find `scramjet`, give its full path as `command` (`command -v scramjet` prints it).

## What it does

- **Sessions and prompts:** it opens new sessions (`session/new`) and runs prompts in them, with their images (`session/prompt`, `session/cancel`). The replies, reasoning, tool calls (their arguments, files and edits' diffs) and their output, and the context's usage stream as updates. A prompt sent while a turn runs steers it, its images given as the paths of files they are saved to, as a pasted image's are.
- **What a session starts with:** the client's MCP servers are started with the session's, and command sets' commands and skills are listed for the client. A session opened with `_meta["io.github.leanandmean.scramjet"]: {"directives": false}` leaves out Scramjet's directives (as `--no-directives` does for all); a session `profile` there gives it its own instructions, tools, reach and model ([below](#a-session-profile)).
- **Permission and questions:** with `--ask`, the calls it would ask about, and publications, wait for the client's permission (`session/request_permission`). The agent's questions are the client's forms where it offers them (`elicitation/create`), and a finished command's next steps are offered to it.
- **Config options:** the model, thinking level and autopilot (`session/set_config_option`). One set during a turn applies at its next pause, as `/model` typed then does.
- **Saved sessions** are listed (`session/list`), resumed (`session/resume`), loaded with their history replayed (`session/load`), closed and deleted.
- **Sign-in** goes through `authenticate` and `logout`.
- **Scramjet's own extensions** add next steps, compaction, renaming, forks, branches and `/tree`, export, usage, `!` commands, API keys and a client's catch-up.

## A session profile

A client running many tasks over one `scramjet acp` gives each session its own instructions, tools, reach, model, thinking level and output limit in `session/new`'s `_meta["io.github.leanandmean.scramjet"].profile`, each key optional:

```json
"profile": {
  "systemPrompt": "You review one file for …",
  "appendSystemPrompt": ["…"],
  "contextFiles": false,
  "directives": false,
  "tools": ["read", "grep", "find", "bash"],
  "read": ["/data/refs"],
  "writable": true,
  "allowWrite": ["/scratch/task-42"],
  "network": false,
  "model": "local/qwen3.8:27b",
  "thinkingLevel": "low",
  "maxTokens": 4096
}
```

`systemPrompt` and `appendSystemPrompt` (a string or a list) are `--system-prompt` and `--append-system-prompt` as text, never a file's name; `contextFiles: false` is `--no-context-files`; `tools` are the session's only tools, as `--tools`. Any of `read`, `writable`, `allowWrite` or `network` runs the session's commands in the sandbox, whatever the options and settings say: `workspace-write` when `writable` is true (with `allowWrite`'s directories), else `read-only`; reads kept to the minimal reach and `read`'s absolute paths; off the network unless `network` is true. Your `denyRead` and the hidden credential stores still apply, its subagents stay read-only and off the network, and `/sandbox` cannot change it. Where the sandbox cannot start (no bubblewrap, native Windows) the session is refused. On Linux each command gets a fresh `/tmp`, so a `read` path under `/tmp` stays hidden.

`model` is the session's model, as `--model` names one (`provider/id`), `thinkingLevel` its level (`off` to `max`, as `--thinking`), and `maxTokens` the most tokens a reply may have, as `--max-output-tokens`, at most the model's own limit. A model none of the providers offers refuses the session. `session/set_config_option` still changes the model and level of a session with a profile, for that session alone: unlike other sessions', its choices are never saved in your settings or as the project's last-used model, so many sessions choosing at once leave them as they were. Likewise, when the agent asks for write access (`request_permissions`), the client can grant it for the session only, never always for the project; where the profile sets the reach, nothing widens it, so the agent is told the reach is fixed and nothing is asked.

A key not listed, or a value of the wrong type, refuses the session, naming it. The profile is saved in the session file: `session/resume` and `session/load` apply it again, and one given with them replaces it (close a session open on the connection first). The result's `_meta` has the `sandbox` the session got (`mode`, `network`, `reach`, `granted`, `summary`). A saved profile's model and level apply again at a resume, over one chosen during the session.
