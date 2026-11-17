# Sessions and keys

[Back to the README](../../README.md)

## The command line

```sh
scramjet 'Explain src/main.rs'            # a session that starts with that prompt
scramjet -p 'Explain src/main.rs'         # print the answer and exit (-p - reads stdin)
scramjet @shot.png 'What is wrong here?'  # send a file or an image with the prompt
scramjet -c                               # continue this project's latest session
scramjet -r                               # choose a session to resume
scramjet --model opus:high                # the newest Opus, at high effort
scramjet --list-models                    # the providers' models
scramjet list                             # the command sets and skills loaded here, and problems
scramjet lint --strict ~/repos/mach12     # check a command set before using it
```

Other main options: `--thinking LEVEL` (or `--effort`), `--models PATTERNS` (the models Ctrl+P cycles through), `--no-session` (save nothing), `--ask` (ask before shell commands and writes outside the project), `--tools LIST`, `--system-prompt` and `--append-system-prompt`, and `--project DIR`. `scramjet --help` lists every option, the keys and the built-in commands.

The agent has all seven coding tools by default (read, bash, edit, write, grep, find, ls), then the harness's own (command status, questions, delegation, subagents, model switch, suggestions, GitHub publications, MCP search); `tools` in `/settings` switches each on or off, saved as Codex's table of those changed (`"tools": {"switch_scramjet_model": false}`), and a list there (`["read", "grep"]`) names the tools on, all the harness's own when it names none of them; `grep`, `find` and `@` file completion are built in, with ripgrep's libraries, so neither ripgrep nor `fd` is needed. Tool calls run without asking; [the sandbox](configuration.md#the-sandbox) and `--ask` are how to limit them.

## In a session

The conversation fills the terminal above the prompt, which stays usable while the agent works: Enter then steers the agent before its next request, Alt+Enter queues a follow-up for when it would stop (one sent too late for the turn goes as the next prompt), and Escape interrupts, putting what it did not take back in the prompt. Built-in commands such as `/model`, `/settings` and `/autopilot` run at once while it works. Ctrl+O expands tool output, Ctrl+T hides or shows reasoning, Shift+Tab changes the effort, Ctrl+P the model. `!cmd` runs a shell command and gives its output to the agent; `!!cmd` keeps it out. On Windows it runs in bash when found, else PowerShell, or in the shell `/settings` chooses (`defaultShell`: `bash`, `powershell` or `cmd`).

The mouse scrolls and selects, and Shift with the arrows, Home or End (Ctrl+Shift+Left/Right by word) moves a selection's end (with the prompt empty, Shift+Up, Shift+Left, Shift+Home or Ctrl+Shift+Left starts one at the end of the conversation shown); in the prompt they select its text, which typing replaces. A right click or Ctrl+C copies the selection (with Copy on select in `/settings`, on by default on macOS, a selection made with the mouse is copied as soon as it is made), a right click without one pastes, and Ctrl+V (or Alt+V) pastes a clipboard image as the path of a file it is saved to. [TERMINALS.md](../TERMINALS.md) says what your terminal does with copying, pasting and these keys, such as the keys it keeps for itself.

Misspelled words in the prompt are underlined (American English, with common developer words such as `repo` and `config`), leaving paths, identifiers, short capitals and `!` commands alone; your own words go one a line in `words.txt` beside the settings file, and Spelling in `/settings` turns it off.

`/context` shows how much of the model's window each part of the context takes, and `/system-prompt` the instructions the last request sent, with each tool's definition. `/usage` shows what is left of each provider's quota where it says, and what the saved sessions spent.

## Sessions are trees

`/tree` (or Escape twice on an empty prompt) goes back to any earlier point and continues from there, optionally summarizing the branch left behind; `/fork` and `/branch` continue in a new session. `/resume` (another folder's session continues in that folder), `/compact`, `/export` (a self-contained web page) and `/copy` work as in Scramjet 1; in `/resume`, sessions that fresh-session next steps chained show their summaries, and Ctrl+X shows chains flat, threaded or folded.

Sessions are saved under `~/.local/state/scramjet/sessions/` (or `--session-dir`). A session's file shows what was sent and received; it holds the whole conversation and what the agent read, so look through it before attaching it to a report.

## Keys

`/hotkeys` (or Codex's `/keymap`) lists every key; Enter on an action changes its keys (you press the new one, and what else it means is shown first). `/keybindings` opens the file they are saved in, `~/.config/scramjet/keybindings.json`, which gives an action other keys in Scramjet 1's format and with its action ids (`{"app.message.followUp": "ctrl+q"}`); `[]` leaves an action without keys. `tui.viewport.pageUp` and `pageDown` are Page Up and Page Down; Scramjet 2 adds `tui.viewport.top` and `bottom` (Ctrl+Home, Ctrl+End) and `tui.select.first` and `last` (Home, End). `/hotkeys` writes the file as you change keys; after editing it yourself, `/reload` reads it again. `/` lists the commands and skills as you type.
