# Changelog

What changed in each version of scramjet-rs, for the people using it. The newest comes first.

## Unreleased

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

- Replies over the Responses API failed as an "unexpected response stream" when a provider sent a reasoning summary before its item, without its part's start, or with its ending events missing their ids, as the company's GPT models do without Proxy in between. Such streams are now read, as Scramjet's provider package reads them: an item's events find it by id, by position, or as the one still open, and an item, part or call missing its start is made when its events come. A function call runs with its final copy's arguments, also when the separate end of its arguments never came, and a call repeated with other arguments or another's call id stops the reply.
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

- The local Proxy proxy's credential directories are no longer hidden by scramjet-rs itself; its private package names them.
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
- The local Proxy route comes from a private package, installed with `scramjet-rs install`, instead of being built in.
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
