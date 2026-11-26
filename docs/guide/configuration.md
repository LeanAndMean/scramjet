# Configuration

[Back to the README](../../README.md)

## Settings and where files are

[The settings reference](settings.md) lists every key of `settings.json`, its values and its default. `/settings` shows every setting under headings (Model, Display, Input, Conversation, Tools and safety, Commands and sets), each with a line saying what it does; typing shows only the settings whose name, value, heading or description has each word typed; also each command's next steps, publication approval, and what the session loaded from where, with any problems (Enter on a file opens it in `$EDITOR`).

Scramjet's settings (`~/.config/scramjet/settings.json`) and sessions (`~/.local/state/scramjet/sessions/`, or `--session-dir`) are apart from Scramjet 1's, with the same setting names. Your own agents (`~/.config/scramjet/agents`), skills (`~/.config/scramjet/skills`), themes, `SYSTEM.md` and global `CLAUDE.md`/`AGENTS.md` are in `~/.config/scramjet/` too. It shares Scramjet 1's `autonomy.yaml` files (the next steps that continue by themselves), `~/.config/scramjet/autonomy.yaml` included. [Where Scramjet keeps its files](install.md#where-scramjet-keeps-its-files) lists the rest.

## Context files and skills

Both `CLAUDE.md` and `AGENTS.md` are read, from `~/.config/scramjet`, the project and the directories above it, and after them each directory's `CLAUDE.local.md`: your own notes, kept out of version control, as in Claude Code. Claude Code's other instruction files are read too: your `~/.claude/CLAUDE.md` and `~/.claude/rules` before the project's files, and in the project and each directory above it `.claude/CLAUDE.md` after its `CLAUDE.md`, then its `.claude/rules` last. A rule is each `.md` file under `rules`, its frontmatter left out. As in Claude Code, one whose frontmatter gives `paths` (gitignore-style globs, a list or separated by commas, `{a,b}` expanded) waits until the agent reads, writes or edits a file they match, relative to the directory holding that `.claude` (your `~/.claude/rules`: relative to the project); then it is added to the conversation, and with it, for each directory between the project and that file, its `CLAUDE.md`, `.claude/CLAUDE.md`, `CLAUDE.local.md` and rules. Each comes once in a conversation (again after a compaction), and a notice says `Loaded` and which; a subagent's reads, writes and edits bring them into its own conversation the same way, from the directory it works in. As in Claude Code, the `CLAUDE.md` and `CLAUDE.local.md` files and the rules are sent without their block-level HTML comments (`<!-- … -->` on lines of their own, outside code), and each `@path` in them outside code (relative to the file, absolute or `~/`; `\ ` for a space in the path) brings in that file right after it, and its own imports, up to four hops; a file is read once, however many import it. `--bare` leaves out `~/.claude`. `/init` has the agent write an `AGENTS.md` for the project from what its files show (an existing `AGENTS.md` or `CLAUDE.md` it proposes changes to, made once you approve them), left uncommitted for you to review. `/memory` lists the context files this session read, those there but not read, and where yours (`~/.config/scramjet/AGENTS.md`), the project's (`AGENTS.md`) and your notes for it (`CLAUDE.local.md`) would go; on the full screen Enter opens one in your editor, creating it when it is not there, and the instructions are read again once it is saved. Skills are directories with a `SKILL.md`, found in `.scramjet/skills`, `.agents/skills` and Claude Code's `.claude/skills` (in the project and the directories above it up to its Git root), the command sets, `~/.config/scramjet/skills`, `~/.agents/skills` and `~/.claude/skills`; the first of a name wins; `/<name>` runs one, as in Claude Code, and `/skill:<name>` too (the only way when a built-in or a command has the name; a skill goes before a `.claude/commands` command of its name). A skill of Claude Code's `.claude/skills` runs as in Claude Code: its arguments substituted (`$ARGUMENTS`, `$0`, the names its `arguments` lists), `${CLAUDE_SKILL_DIR}` and the session's variables replaced, and its `` !`command` `` lines run first, as a `.claude/commands` command's. `/skills` lists them, each with where it comes from and whether the model is offered it (a skill whose `disable-model-invocation` is true runs only when you type it), after any problem in skill files; on the full screen Enter opens one's file in your editor.

Claude Code's commands, the `.md` files of `.claude/commands` in the project and the directories above it up to its Git root, then `~/.claude/commands`, run as they do in Claude Code: `/NAME args`, a file in a subdirectory as `/DIR:NAME`, the first of a name winning. The arguments fill `$ARGUMENTS`, `$ARGUMENTS[N]` and `$N` (counted from 0) and the names the frontmatter's `arguments` gives, or follow the text as `ARGUMENTS: …` when it has none of them. Then each `` !`command` `` (at the start of a line or after a space) and each ```` ```! ```` block runs, in the order they come, and is replaced by what it printed; they run as the agent's commands do, in the sandbox when it is on and after asking under `--ask`, and an argument you type never runs as one. If one fails or you deny it, the prompt is not sent and a notice says why. The result is sent as your prompt, starting no command set's command. The banner lists them under `[Commands]` and the `/` list with the others, but for those whose name a built-in command or a command set's takes, which run instead.

Claude Code's installed plugins come too, those its settings turn on: each listed in its `plugins/installed_plugins.json` (for you, or for this project) whose `enabledPlugins` entry, in `~/.claude/settings.json` or the project's `.claude/settings.json` or `.claude/settings.local.json` (the later winning), is `true`. As in Claude Code, a plugin's commands run as `/PLUGIN:NAME`, as `.claude/commands` commands do, with `${CLAUDE_PLUGIN_ROOT}` its directory; its skills are `PLUGIN:NAME`, run as `.claude/skills` skills; its agents are `PLUGIN:NAME`, as a command set's. A plugin is listed with the command sets (`scramjet list`, `/settings`), after those you installed; one whose name an installed set has is left out. Turn one off in Claude Code (`/plugin`), or start with `--bare` for none. Their hooks and MCP servers are not read. A Claude Code file whose frontmatter does not parse as YAML is read again with its values quoted, as Claude Code does, so that a description holding `: ` or `#` still loads.

Where Claude Code's `CLAUDE_CONFIG_DIR` is set, its directory takes the place of `~/.claude` for the skills, commands, `CLAUDE.md` and rules read from there, as in Claude Code, and its `.credentials.json` is hidden from the agent with the other credential stores.

## Credentials

The agent's tools see what you see, `gh`'s login and the ssh-agent included, in every turn. With the `hideCredentials` setting `unattended` (turns nobody started: a next step that runs by itself, autopilot, `-p`) or `always`, they cannot see common credential stores (`~/.ssh`, `~/.netrc`, …), those `models.json` files name, or credential variables; `--list-credential-stores` lists the stores. On WSL a Windows program a command runs (`cmd.exe`, `powershell.exe`) still reads them through `\\wsl.localhost\`, so that Windows programs keep working in those turns. No instruction file (`CLAUDE.md`, `AGENTS.md`, `SYSTEM.md`) is read from one, also through a link, and a command that failed for want of one is told why, so that the agent asks you or goes on without it. tmux servers the agent starts then get their own socket directory: attach with `TMUX_TMPDIR=~/.local/state/scramjet/tmux tmux attach`. Commands never get the terminal, so a passphrase or password prompt fails at once rather than wait.

## The sandbox

Tool calls run without asking; there is no permission mode, and by default the main agent's shell is not sandboxed (beyond hiding credential stores when `hideCredentials` asks, [above](#credentials)). The sandbox limits what the agent's commands can change ([the modes](#the-modes)), what they can reach on [the network](#the-network) and what they can read ([hidden paths](#writable-hidden-and-readable-paths) and [the reach](#the-reach)); [`/sandbox`](#the-sandbox-screen) shows and changes all of it. [The settings reference](settings.md#the-sandbox) lists its keys. Whatever it allows, work in Git, a clone or a worktree, which is how you recover.

### The modes

`--sandbox read-only` or `--sandbox workspace-write` (`-s`, or the `sandbox` setting), as Codex names them, runs the main agent's shell under Bubblewrap on Linux and Seatbelt on macOS, as subagents' is. On Windows `--sandbox` is refused.

- `read-only`: commands can change nothing.
- `workspace-write`: the project is writable but for its own `.git`, `.scramjet` and `.agents` (repositories nested in it stay writable, as in Codex).

The `write` and `edit` tools refuse what the mode does not allow.

On Linux the sandbox's `/tmp` is the agent's own, apart from yours and from the sockets in it: a file a command writes there is still there for its next command, until the session's process ends (each subagent has one of its own, removed as it ends). The file tools share it: `read` finds there what a command wrote to `/tmp`, and `write` puts a file under `/tmp` there, whatever the sandbox's mode, for the next command to find; a path in your `/tmp` that it does not hold, or in a directory the sandbox shows from your `/tmp` (a project or a granted directory there), they read from yours. A command that fails naming a file your `/tmp` has and its own does not is told so after its output, with what it can do: the main agent can ask for the file's directory with `request_permissions`, a subagent says so in its result. On macOS the temporary directories are shared with your programs.

### The network

Commands reach the network unless `sandbox.network` is `false`, which keeps them off it with no sandbox too, said at every start:

- On Linux they then run under Bubblewrap with the files as they are but the network unshared (the internet, the local network and this machine's own servers) and `/run`'s sockets hidden (D-Bus, Docker's, WSL's way to Windows' programs). Unix sockets elsewhere, as in `/tmp`, stay open.
- On macOS Seatbelt closes every socket (untried).
- On Windows nothing can, so the Network row says it stays on.

The agent's `web_fetch` tool, which Scramjet runs itself rather than a command, is refused then too (not on Windows, where the network stays on). A `!` command, which you run, keeps the network.

### Writable, hidden and readable paths

The `sandbox` setting is an object of Claude Code's shape with Codex's modes (a mode's name alone, `"sandbox": "read-only"`, and `sandboxNetwork` are still read):

```json
{
  "sandbox": {
    "mode": "workspace-write",
    "network": false,
    "filesystem": {
      "allowWrite": ["~/scratch"],
      "denyRead": ["~/notes"],
      "allowRead": ["~/notes/shared"]
    }
  }
}
```

- `allowWrite` (Codex's `writable_roots` is read too) makes directories writable in the sandbox in every project, such as a tool's cache in the home directory (uv's `~/.cache/uv`, which `uv run` writes). A directory that does not exist is not writable; `/reload` applies the list again, so one made or added since the start becomes writable, and one taken out is read-only again.
- `denyRead` hides files and directories as the credential stores are hidden: from commands, from `read`, `grep` and `find`, and from subagents, in the turns that hide the stores (as `hideCredentials` says, and every turn in the sandbox).
- `allowRead` makes what is in a `denyRead` directory readable again (writable where it would be), but never a credential store, which only `hideCredentials` shows; what `denyRead` names deeper in it stays hidden. The project and the directories the sandbox makes writable are re-opened so too.

`~` and relative paths are taken from the settings file's directory.

### The reach

`"blockReadsOutsideWorkingDirectories": true` (Claude Code's key, read under `permissions` too) keeps the agent to the project. It hides the home directory, the other users' homes and the mounted drives (`/mnt`, WSL's Windows drives among them, and `/media`), but for:

- the project and the directories the sandbox makes writable;
- the command sets', skills' and agents' directories, and this project's sessions;
- where the project is a linked worktree, its repository's Git directories;
- each directory on `PATH` in the home directory, with its install prefix and where its programs' symlinks lead (`~/.cargo`, `~/.nvm/versions/node/…`, `~/.local/share/claude/versions`), `~/.rustup` and uv's `~/.local/share/uv`;
- git's `~/.gitconfig` and `~/.config/git`.

`allowRead` re-opens more, and system directories stay readable.

`"blockReadsOutsideWorkingDirectories": "minimal"` (after Codex's `:minimal`) hides more: everything at the top of `/` but the system's own directories (`/usr`, `/etc`, `/opt`, `/bin`, `/sbin`, the `/lib` directories, and `/dev`, `/proc`, `/sys`, `/tmp` and `/run`), so `/var`, `/srv` and a site's `/scratch` or `/nix` too; and in the home directory only the project, the writable directories and what Scramjet gives the agent, not `PATH`'s programs, the toolchains or git's configuration. `allowRead` re-opens what a task needs. On macOS it keeps `/System`, `/Library`, `/Applications` and `/private` too (untried); on Windows it is the same as `true`.

What keeps to the reach:

- `read`, `grep` and `find`, always.
- Commands, on Linux and macOS (untried there), without a sandbox as well. Without one, everything else stays writable to them; what the reach hides stays hidden and unchanged, but for `allowWrite`'s directories and those `--add-dir`, `/add-dir` and the screen's Also writable add, which stay readable and writable; tmux servers they start get their own socket directory, as with `hideCredentials`; and the credential variables and stores outside the reach stay as `hideCredentials` says.
- Subagents, always, with the main agent's writable directories (`/add-dir`'s too) kept in their reach; `sandbox.subagents.blockReadsOutsideWorkingDirectories` sets theirs apart.

On Windows the reach hides the home directory and the other users' profiles (`C:\Users`) and keeps the temporary directory; since nothing there can hide files from commands, the first command says that only `read`, `grep` and `find` keep to it.

### The sandbox screen

`/sandbox` (or Codex's `/permissions`, and `/settings`' Sandbox row) opens one screen for all of it, in plain rows:

- **Can change**: nothing, this project, or anything with no sandbox (the modes).
- **Can read**: everything you can, this project and your tools, or this project and the system only (the reaches); on Windows "read, grep and find only", as nothing there keeps commands to it.
- **Network**.
- **Also writable**: the directories granted, `allowWrite`'s marked "every project" and this project's "set here". With no sandbox it shows while Can read hides something, as what it then re-opens.
- **Hidden** (`denyRead`) and **Also readable** (`allowRead`).
- **Subagents**, opening the same rows for `sandbox.subagents` ("same as the main agent" where they take its reach or lists).

Each list has Add…, and Enter on a path takes it out.

A change applies at once, the reach too, and is remembered for this project: the next session here, a resumed one too, starts with it, and `/reload` keeps it. A project keeps only the rows changed in it, marked "(set here)", and a default's path taken out there shows as "(taken out here)", to put back; the other rows follow the default, so a default changed later reaches them. What each project changed is kept by the project's path in `sandbox-projects.json` beside your settings, never in the repository, so a cloned repository cannot loosen its own sandbox; `--sandbox` at start still wins over it for that session.

- Ctrl+R resets the project to the defaults, once confirmed, forgetting its rows, the directories made writable here included.
- Ctrl+S saves the sandbox as it is then as every project's default in your settings, and the project then follows it (the directories granted only for the session stay out of `allowWrite`; those made writable here go in).

Without the screen, `/sandbox MODE`, `/sandbox network on|off` and `/sandbox remove DIR` change it at once, remembered as the screen's changes are, and `/sandbox add DIR` for the session only. `/add-dir DIR` (or `--add-dir DIR` at start, as in Claude Code) makes another directory writable too, its own `.git`, `.scramjet` and `.agents` kept read-only unless granted by name; with no sandbox it only re-opens a directory Can read hides.

The agent is told what is writable, and asks for more with `request_permissions`, naming the directories and why; when a command fails on a write the sandbox refused ("Read-only file system", on macOS "Operation not permitted"; also in a command that still ends well, as through a pipe, when an error message gives it), it is reminded that it can ask. You allow it for the session, always for this project (added to its Also writable row, as the screen's changes are, and forgotten by `/sandbox remove DIR`), or deny it. Those kept in `sandbox-grants.json` before 2.344.0 move into each project's row at its next start there. In `-p` and in the steps autopilot runs no one answers, so it is refused.

### What it leaves out

- On Linux, `sandbox.launcher` names a program started with Bubblewrap's arguments in `bwrap`'s place, such as a wrapper for machines where `bwrap` alone cannot start (a name alone is found on `PATH`, a path is taken as `allowWrite`'s are). It serves the stores hidden by `hideCredentials` and subagents' commands too; macOS and Windows ignore it.
- MCP servers run outside the sandbox, with your environment and credential stores, as in Codex and Claude Code: they are programs you configured, not the agent's, and changing the sandbox does not restart them.
- Hooks (the settings' `hooks`) and the status line's command run outside it too, as Claude Code's do: they are your commands, given the agent's calls; a `PreToolUse` hook can block a call or change its input before the sandbox runs it.
- `--ask` asks before every shell command and every write or edit outside the project, as a way to watch, not containment.

## Subagents

Two agents are there without any command set: `scramjet:explore`, which finds where something is in the code, traces how it flows or maps what depends on what, and answers with cited evidence and what it could not establish (the breadth asked: quick, medium or very thorough); and `scramjet:general-purpose`, for multi-step investigations that need judgment. The agent's instructions list these, your own agents (`~/.config/scramjet/agents`, and Claude Code's `~/.claude/agents`, which a file of the same name in Scramjet's replaces) and the command sets', as Claude Code lists its agents; a project's `.scramjet/agents`, and Claude Code's `.claude/agents` of the project and the directories above it up to its Git root (the nearest winning, Scramjet's over them), are reached with the call's `agentScope` and your approval, and are not listed. `/agents` lists every agent a call can name, project ones included, each with where it comes from, the tools and model its file asks for, and what is wrong in agent files; on the full screen Enter opens one's file in your editor.

Subagents run inside this process, read-only and off the network whatever the main agent's sandbox: their shell runs under Bubblewrap with every file read-only, a private `/tmp` of its own, kept between its commands (where a tmux it starts runs apart from yours and ends with its command), no network and credential stores hidden. On macOS it runs under Seatbelt, with `/tmp` and `TMPDIR` writable and shared, as in Codex; on Windows at the low integrity level, with a `TEMP` of its own, the network, and nothing hidden.

`sandbox.subagents` sets theirs apart:

- `{"network": true}` lets their commands reach the network.
- `"filesystem": {"denyRead": [...], "allowRead": [...]}`: each list given replaces the main agent's for them; without one they take the main agent's.
- `{"mode": "workspace-write"}` lets a subagent whose agent file lists `write` or `edit` write in its working directory (the project, or the task's `cwd`), its `.git`, `.scramjet` and `.agents` kept read-only; its commands then run in a sandbox where only that directory is writable. An agent file that lists no tools keeps the read-only ones.
- `"filesystem": {"allowWrite": [...]}` adds directories writable to such subagents (with `"mode": "read-only"`, only those).

With the main agent's own sandbox on, a subagent writes only where the main agent may as well, so `--sandbox read-only` keeps every subagent read-only too. On Windows subagents cannot write.

Subagents running at once share the directory: give parallel writers separate `cwd`s, or worktrees. `isolation: worktree` in an agent's file, or `"isolation": "worktree"` in a task, as in Claude Code, runs each of its tasks in a git worktree of its own (a chain's steps that ask for one share it, so each sees what the earlier changed):

- It is on a new branch `worktree-agent-…` from the repository's `HEAD` (uncommitted changes are not in it), made under the state directory's `worktrees/` (`~/.local/state/scramjet/worktrees/<project>/agent-…`).
- It is writable to the subagent even when the main agent's sandbox is not, its git files read-only, so the subagent leaves its changes uncommitted.
- A worktree left unchanged is removed with its branch; files git ignores in it, such as build output, an ignored notes directory or a `.env`, go with it, as in Claude Code, and the result names them.
- Otherwise the result names its path and branch, and so does the subagent's box on the screen; you or the main agent remove it once its changes are taken or dropped.
- If Scramjet ends before the subagent does (its terminal closed, the process killed), the next start in that project finishes the worktree as the subagent's end would have, and says what it did: removed if unchanged, else kept, with its path and branch.

## MCP servers

MCP servers come from the settings' `mcpServers`, as Claude Code's `.mcp.json` lists them: programs (`{"time": {"command": "uvx", "args": ["mcp-server-time"]}}`) and servers over streamable HTTP (`{"docs": {"type": "http", "url": "https://…", "headers": {"Authorization": "Bearer ${DOCS_TOKEN}"}}}`), with `${VAR}` and `${VAR:-default}` taken from the environment; no OAuth sign-in yet. The model finds their tools with `mcp_search` and runs one with `mcp_call`; the tools of servers that fit in 2% of the model's context window are also in its tool list, as `mcp__SERVER__TOOL` (Claude Code's names). Which are is decided once every server has started, and again when a model with another window is chosen, so the tool list stays the same while they run; `/mcp` shows how each server is and how its tools are offered, a server that failed or ended starts again when the model uses it a minute or more later, `/mcp reconnect [SERVER]` starts one again at once, or all, and `/mcp enable|disable [SERVER]` starts or stops one, or all, as Claude Code's, saved in the settings' `disabledMcpServers`. No plugins.

## Themes, looks, banners and bottoms

The themes, chosen in `/settings` or with `/theme` (`theme`; `/theme NAME` uses one by name):

- `scramjet-dark`, its light companion `scramjet-light`, and Pi's `pi-light`, each painting its own background and text color, so they look alike in every terminal. The default follows the terminal's background: `scramjet-light` on a light one, as the terminal answers or `COLORFGBG` says.
- `terminal-colors`, the terminal's own 16 colors.
- Theme files in Scramjet 1's format, from `~/.config/scramjet/themes/`, the project's `.scramjet/themes/`, the files and directories of the `themes` setting and `--theme`. Beyond Scramjet 1's format, a theme's `colors` may hold a main `background`, which the full screen paints under everything, with the theme's `text` color over it.

The theme in use shows each save of its file at once; `/reload` reads them all again.

`/settings` → Theme → New theme… makes one, while a sample session shows it:

- Choose 1–5 main colors and a harmony, and a dark or light background of its own or the terminal's (asked of the terminal, else as `COLORFGBG` says).
- Roll (R) until it is close, lock (L) the colors you like and roll again.
- Step toward the harmony (H) or the contrast (+/−), and make the background lighter or darker (Lightness).
- Move a main color up or down (Alt+Up, Alt+Down; the first tints the background).
- Undo (U) and redo (Shift+U).
- Tab lists every role (all 52 colors, each with its contrast and what it colors), where Left and Right make one follow another main color and `#` sets it to an exact color.

Enter names the theme and saves it in `~/.config/scramjet/themes/`; once it is in use, Edit NAME… in the same list opens it again, and saving it under its name replaces it. Another theme in use with exact colors (`scramjet-dark`, `scramjet-light`, `pi-light`, a file of your own) offers New theme from NAME…, which starts from its colors: each role keeps that theme's color until a roll or a step changes what it follows. So does `terminal-colors` in a terminal that says what its 16 colors are (Windows Terminal does; tmux 3.0a does not), starting from the colors it shows.

The look (the `look` setting, `/settings`' Look row) is how prompts, reasoning, tool calls and replies are drawn:

- `tinted`, the default: each prompt and call filled with the theme's background for a prompt and for how the call went, without sides, a rule above carrying the call and one below, inset a column, in the fill's state color (as `cards` in a theme without backgrounds).
- `panels`: each prompt and call on the theme's backgrounds (the default before 2.290.0).
- `classic`: a `●` before each call in the color of how it went, little else.
- `rails`: a rail beside each block, a call's in the color of how it went.
- `margin`: a column of labels down the left: who speaks, a call's tool in the color of how it went, the kind of notice.
- `cards`: a rounded card around each prompt, and around each call in the color of how it went, its tool, glyph and time on the top edge.
- `ribbons`: as `tinted`, its rules the fill's full width.
- `outline`: a log of actions, as Codex shows them: a `•` in the color of how each call went and a bold verb, its output under a `└`, and a rule saying how long a turn worked.
- `timeline`: one spine down the conversation, as `git log --graph` draws it, with a node on it for each prompt, reply and call, a call's `○`, `●` or `✗` in the color of how it went.
- `mono`: no color but a failure's red: a call's `○`, `✓` or `✗` and the rail beside its output, dashed while it runs and heavy once it failed, say how it went, so it reads the same in any theme.

The banner is `header` by default; `/settings`' Banner row also offers `plain` start lines, `card` and `logo`, Scramjet's logo (the `banner` setting). Ctrl+O expands them to every key and what was loaded.

The bottom of the screen (the status row, the prompt and the footer; the `bottom` setting, `/settings`' Bottom row):

- `bar`, the default: `pi`'s prompt over a status bar, where the session is, then the output rate, the context's fill with a gauge, the cost and the model after a mark in the thinking level's color, with no background behind them.
- `pi`: the original bottom, with the session's token counts on its second footer row (the default before 2.290.0).
- `boxed`: the prompt in a rounded box in the thinking level's color and each dialog in a box of its own, with a footer that goes onto more rows rather than cutting anything on a narrow screen: where the session is and its modes, then the output rate, the context's fill and the cost, and the model.
- `composer`: as Codex's, the prompt or a dialog on the prompt's background with an edge in the thinking level's color, the output rate, the context's fill, the cost and the model on the row under it, where the session is under them.

All but `pi` leave out the session's token counts and `auto`, which `/usage` and `/status` show. Every bottom shows the branch's open pull request after the branch, as `PR #N`, when the project's `origin` is on GitHub and `gh pr list` finds exactly one open from the branch; nothing when `gh` is missing or signed out. It is looked up again at the start, when the branch changes and after each turn or command.
