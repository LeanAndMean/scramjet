# scramjet-rs

A terminal coding agent written in Rust: an independent rewrite of Scramjet that talks to the providers its `models.json` lists and runs Scramjet's command sets (Mach 12 among them).

**Status:** an early but working interactive agent, verified live through a local proxy on Linux and WSL2 Ubuntu. macOS is untested. Parallel agents at scale (the ~8,000 concurrent requests goal) are not built yet; [docs/TASKS.md](docs/TASKS.md) lists the pending work.

## Build and run

You need stable Rust (checked with 1.99), a C compiler (for the regular expressions syntax highlighting uses), and a provider (below). Subagents' tools and unattended turns run under Bubblewrap (`bwrap`), and fail without it; `@` file completion uses `fd`.

```sh
cd /path/to/scramjet-rs
cargo install --path . --locked --offline     # `cargo fetch` first if the crates are not cached
cd /path/to/your/project
scramjet-rs
```

`cargo install` puts the release build in `~/.cargo/bin` (on the `PATH` rustup sets up). That copy changes only when you run the command again, which replaces it with the checkout's current code, uncommitted changes included. To try the current code without replacing the copy you work in, run the build in the checkout, which changes with every build and goes with `cargo clean`:

```sh
cargo build --release --offline --locked
/path/to/scramjet-rs/target/release/scramjet-rs
```

```sh
scramjet-rs 'Explain src/main.rs'            # a session that starts with that prompt
scramjet-rs -p 'Explain src/main.rs'         # print the answer and exit (-p - reads stdin)
scramjet-rs @shot.png 'What is wrong here?'  # send a file or an image with the prompt
scramjet-rs -c                               # continue this project's latest session
scramjet-rs -r                               # choose a session to resume
scramjet-rs --model opus:high                # the newest Opus, at high effort
scramjet-rs --list-models                    # the providers' models
scramjet-rs install ~/repos/scramjet/packages/scramjet/mach12   # a command set, by reference
scramjet-rs install git:github.com/LeanAndMean/scramjet          # the sets in a git repository
scramjet-rs update                           # pull the packages not pinned to a ref
scramjet-rs list                             # the command sets and skills loaded here, and problems
```

Other main options: `--thinking LEVEL` (or `--effort`), `--models PATTERNS` (the models Ctrl+P cycles through), `--no-session` (save nothing), `--ask` (ask before shell commands and writes outside the project), `--tools LIST`, `--system-prompt` and `--append-system-prompt`, and `--project DIR`.

## Providers

Models come from the providers in `~/.config/scramjet-rs/models.json` and in the `models.json` of each installed command set or package, in Scramjet's format. Each provider has a `baseUrl` (`http://`, or `https://`, trusting the system's certificates or those `SSL_CERT_FILE` and `SSL_CERT_DIR` name, as OpenSSL does), an `api` (`openai-responses`; `anthropic-messages`, under which requests go to `/v1/messages`; or `openai-completions`, Chat Completions at `/chat/completions`, which vLLM, Ollama and llama.cpp serve), an `apiKey` (the variable of that name in the environment, else the text itself) and the `models` it offers:

```json
{
  "providers": {
    "local": {
      "baseUrl": "http://localhost:8000/v1",
      "api": "openai-responses",
      "apiKey": "LOCAL_API_KEY",
      "models": [
        { "id": "qwen3-coder", "name": "Qwen3 Coder", "reasoning": true, "input": ["text", "image"],
          "contextWindow": 262144, "maxTokens": 32768,
          "cost": { "input": 0.5, "output": 2, "cacheRead": 0.05, "cacheWrite": 0.5 } }
      ]
    }
  }
}
```

A provider that lists no `models` offers those its server lists (`GET {baseUrl}/models`, or `{baseUrl}/v1/models` under `anthropic-messages`), asked with the key its requests carry. Its list is saved in `~/.local/state/scramjet-rs/models/` and, like the logins' lists below, read again while the full screen is open once 15 minutes old. Each model takes what the list says of it: Anthropic's give the window, output, effort levels and images; OpenRouter's also the prices and leave out the models that take no tools; Mistral's, Groq's and vLLM's give the window. What the list does not say (OpenAI's and Ollama's say only the id) stays unknown, without reasoning, and the provider's `modelOverrides`, as in Scramjet, can say it: `"modelOverrides": {"gpt-6": {"reasoning": true, "contextWindow": 400000}}`.

A model may also give its own `api`, `baseUrl` and `headers`, and a `thinkingLevelMap` as in Scramjet. What a Chat Completions server accepts is told from its provider's name and URL as Scramjet tells it, and a provider's or model's `compat` (Scramjet's fields, such as `maxTokensField`, `thinkingFormat` or `requiresToolResultName`) says otherwise. A file may name a program in `providersCommand` (`["bin/models", "arg"]`, a path from the file's directory) whose output, `{"version": 1, "providers": {...}}`, adds the providers it learns at run time; a package installed with `scramjet-rs install DIR` can bring one, as the private package for the author's local proxy does. A file's `credentialFiles` (`["~/.config/acme", "token.json"]`) names the files and directories holding its providers' credentials, which the agent's tools do not see, as the common stores below. What is wrong in these files is said at start and by `--list-models`.

`/login` logs in to a ChatGPT Plus or Pro subscription (Codex) or to GitHub Copilot (github.com or a GitHub Enterprise domain). ChatGPT's login opens your browser and comes back by itself; the page's address is shown and copied, so you can open it in another browser or a private window instead, or paste the address the browser ends on. For Copilot you enter the code it shows on GitHub's page. Each provider's models for your account are offered after those of the files, as the provider lists them, with their limits and effort levels (and Copilot's prices). While the full screen is open, the lists are read again in the background once 15 minutes old, at start too, so a model the provider adds is offered without a restart; what changed is said, and a list that cannot be read again stays as it was. The login is saved in `~/.config/scramjet-rs/auth.json`, readable by you only and hidden from the agent's tools as the credential stores below are; `/logout` removes it. Under API keys, `/login` also lists the providers of the `models.json` files, each with where its key comes from now, and saves a key you enter in that file, out of `models.json`; requests to that provider then carry it in place of the entry's `apiKey`, as in Scramjet, and a provider whose server lists its models is asked for them again with it. A model is named by provider and id (`github-copilot/gpt-6-luna`), so Copilot's are offered beside another provider's of the same id; an id alone names the first provider's.

A package can bring a login of its own: its `models.json` names the program in `logins` (`{"acme": {"name": "Acme Cloud", "command": ["bin/login"]}}`, a path from the file's directory), and `/login` lists it after ChatGPT and Copilot. The program is given one line on its input, `{"version": 1, "action": "login", "provider": "acme"}`, and answers in lines of JSON: `{"open": URL}` opens a page in your browser (shown and copied as ChatGPT's is), `{"say": TEXT}` tells you what it waits for, and `{"credential": {"access": TOKEN, "expires": MILLISECONDS, ...}}` ends the login, saved in `auth.json`; Escape stops it. Given that `credential`, the action `refresh` answers the renewed one once it expired, and `models` answers `{"provider": {...}}`, the provider's entry in this format with its models, read after the login and kept current as the logins' lists are. Requests to those models carry the token (`Authorization: Bearer`). Any action may answer `{"error": TEXT, "transient": true}` instead; `src/agent/login/program.rs` describes the format.

## Where the rest is

- `scramjet-rs --help`: every option, the keys and the built-in commands.
- `/hotkeys` (or Codex's `/keymap`): every key; Enter on an action changes its keys (you press the new one, and what else it means is shown first). `/keybindings` opens the file they are saved in. `/` lists the commands and skills as you type.
- `/settings`: every setting, each with a line saying what it does; also each command's next steps, publication approval, and what the session loaded from where, with any problems (Enter on a file opens it in `$EDITOR`).
- [docs/TERMINALS.md](docs/TERMINALS.md): what your terminal does with copying, pasting and scramjet-rs's keys, such as the keys it keeps for itself.
- The commit messages: how each behavior was decided and verified.

## In a session

The conversation fills the terminal above the prompt, which stays usable while the agent works: Enter then steers the agent before its next request, Alt+Enter queues a follow-up for when it would stop, and Escape interrupts. Ctrl+O expands tool output, Ctrl+T hides or shows reasoning, Shift+Tab changes the effort, Ctrl+P the model. `!cmd` runs a shell command and gives its output to the agent; `!!cmd` keeps it out. The mouse scrolls and selects; a right click or Ctrl+C copies the selection (with Copy on select in `/settings`, on by default on macOS, a selection is copied as soon as it is made), a right click without one pastes, and Ctrl+V (or Alt+V) pastes a clipboard image as the path of a file it is saved to.

Sessions are trees: `/tree` (or Escape twice on an empty prompt) goes back to any earlier point and continues from there, optionally summarizing the branch left behind; `/fork` and `/branch` continue in a new session. `/resume`, `/compact`, `/export` (a self-contained web page) and `/copy` work as in Scramjet.

## Command sets

A command set is a directory of commands (`commands/<set>:<name>.md`) and agents. Install one by reference with `scramjet-rs install DIR`, or those of a git repository with `scramjet-rs install git:github.com/USER/REPO[@REF]` (or `git:git@github.com:USER/REPO`, or an `https://`, `ssh://`, `git://` or `file://` URL): it is cloned by `git`, with your SSH configuration and credential helpers, into `~/.local/share/scramjet-rs/git/`, and its sets are its top directory when that is a set, else the sets at its top and, deeper, those with a `set.yaml`. A ref pins it; `scramjet-rs update` pulls the others, and `scramjet-rs remove REPO` deletes the clone. A package builds nothing: `install` names the programs it runs (`providersCommand`). `/<set>:<command> args` runs a command. A command runs until the agent reports it `completed` or you agree to abandon it. When it finishes, it may recommend next steps: you choose one, or an edge set in `/settings` (Next steps) continues automatically, and `/autopilot on` takes the recommended one after 3 seconds. The publication tools (issues, pull requests and comments on GitHub, through `gh`) show the exact payload for your approval unless the command's policy allows them.

## Coming from Scramjet

- Its providers are those of its own `models.json` files, which have no built-in providers to fall back on: each provider entry lists its models or has its server list them; ChatGPT's (Codex) and GitHub Copilot's come from your account once you `/login`, as the provider lists them. They speak HTTP or HTTPS, with the Responses, Messages or Chat Completions API, not through a proxy (`HTTPS_PROXY`). `/login` logs in to ChatGPT and GitHub Copilot only (not to Anthropic's subscription), and takes API keys only for the providers of those files, and there is no Pi extension API.
- Its settings (`~/.config/scramjet-rs/settings.json`) and sessions (`~/.local/state/scramjet-rs/sessions/`) are its own, with Scramjet's setting names. It shares Scramjet's `autonomy.yaml` files, agents (`~/.scramjet/agent/agents`), skills, `SYSTEM.md`, and `CLAUDE.md`/`AGENTS.md` context files. Scramjet's session files cannot be imported.
- Built-in commands carry Claude Code's and Codex's names (`/clear`, `/exit`, `/rename`, `/status`); Scramjet's (`/new`, `/quit`, `/name`, `/session`) still work. One `/settings` holds everything; there is no `/scramjet settings`.
- A command's status is `completed` or `abandoned` (only after you confirm); there is no `blocked`, `incomplete` or `continuing`. A reply that ends without a report gets a short status check, at most 3 in a row.
- Subagents run inside this process, read-only: their shell runs under Bubblewrap with every file read-only, a private `/tmp`, no network and credential stores hidden.
- In turns nobody started (a next step that runs by itself, autopilot, `-p`), the agent's tools cannot see common credential stores (`~/.ssh`, `~/.netrc`, …), those `models.json` files name, or credential variables; `--list-credential-stores` lists the stores. The `hideCredentials` setting makes that `always` or `never`. tmux servers the agent starts then get their own socket directory: attach with `TMUX_TMPDIR=~/.local/state/scramjet-rs/tmux tmux attach`.
- `edit` changes only the text it matched, also when it matched with trailing whitespace or typographic quotes aside (Scramjet then rewrites the whole file normalized).
- Themes: `terminal-colors` (the terminal's own 16 colors, the default), Scramjet's `scramjet-dark` and Pi's `pi-light`, and theme files in Scramjet's format from `~/.scramjet/agent/themes/`, the project's `.scramjet/themes/`, the files and directories of the `themes` setting and `--theme`, chosen in `/settings` (`theme`). The theme in use shows each save of its file at once; `/reload` reads them all again. Beyond Scramjet's format, a theme's `colors` may hold a main `background`, which the full screen paints under everything, with the theme's `text` color over it. `/settings` → Theme → New theme… makes one: choose 1–5 main colors and a harmony, a dark or light background of its own or the terminal's (asked of the terminal, else as `COLORFGBG` says), roll (R) until it is close, lock (L) the colors you like and roll again, step toward the harmony (H) or the contrast (+/−), undo (U) and redo (Shift+U), while a sample session shows it. Tab lists every role (all 52 colors, each with its contrast and what it colors), where Left and Right make one follow another main color and `#` sets it to an exact color. Enter names the theme and saves it in `~/.scramjet/agent/themes/`.
- Looks: besides the classic one, `/settings`' Look row offers three to try (`rails`, `panels`, `margin`; the `look` setting), each showing prompts, reasoning, tool calls and replies its own way.
- Banners: besides the plain start lines, `/settings`' Banner row offers three to try (`header`, as Scramjet's; `card`; `logo`, Scramjet's logo; the `banner` setting). Ctrl+O expands them to every key and what was loaded.
- Keys: `~/.config/scramjet-rs/keybindings.json` gives an action other keys, in Scramjet's format and with its action ids (`{"app.message.followUp": "ctrl+q"}`); `[]` leaves an action without keys. `tui.viewport.pageUp` and `pageDown` are Page Up and Page Down; scramjet-rs adds `tui.viewport.top` and `bottom` (Ctrl+Home, Ctrl+End) and `tui.select.first` and `last` (Home, End). `/hotkeys` writes it as you change keys; after editing it yourself, `/reload` reads it again.
- Not yet: images shown in the terminal, GitLab publications.

## Coming from Claude Code or Codex

- Any model its providers offer (GPT, Claude, DeepSeek… through a proxy), switched mid-session with `/model` or Ctrl+P; each model's reasoning goes back only to it.
- Tool calls run without asking, as in Scramjet; there is no permission mode, and the main agent's shell is not sandboxed (beyond hiding credential stores in unattended turns, above). `--ask` asks before every shell command and every write or edit outside the project, as a way to watch, not containment. Use Git, a clone or a worktree to recover.
- `-c` continues and `-p` prints, as in Claude Code (Codex: `codex resume --last`, `codex exec`).
- Shift+Tab changes the reasoning effort (in Claude Code it changes the permission mode).
- Both `CLAUDE.md` and `AGENTS.md` are read, from `~/.scramjet/agent`, the project and the directories above it. Skills are directories with a `SKILL.md`, found in `.scramjet/skills`, `.agents/skills`, `~/.scramjet/agent/skills` and `~/.agents/skills`; `/skill:<name>` runs one.
- Commands come as command sets with next steps and delegation, rather than single prompt files.
- MCP servers come from the settings' `mcpServers`, as Claude Code's `.mcp.json` lists them: programs (`{"time": {"command": "uvx", "args": ["mcp-server-time"]}}`) and servers over streamable HTTP (`{"docs": {"type": "http", "url": "https://…", "headers": {"Authorization": "Bearer ${DOCS_TOKEN}"}}}`), with `${VAR}` and `${VAR:-default}` taken from the environment; no OAuth sign-in yet. The model finds their tools with `mcp_search` and runs one with `mcp_call`, so its tool list stays the same whatever the servers offer; `/mcp` shows how each server is. No hooks or plugins.

## Develop

```sh
cargo fmt --check
cargo clippy --offline --locked --all-targets -- -D warnings
cargo test --offline --locked --no-fail-fast   # about a minute
```

The tests need Python 3's standard library, `timeout`, Bubblewrap, ripgrep and `fd`; they use local scripted servers, never a real provider. To look at a display change, `uv run scripts/screens.py scripts/screens/sample.json /tmp/shots --set look=rails` (or `scripts/screens/start.json --set banner=logo` for a start) plays a scripted session on the release build and writes its screens as PNG and ANSI text (`cat` it); `uv` fetches the script's two Python packages on first use. Development runs inside Scramjet as a loop: `/rs:loop` builds and commits one increment per session, `/rs:review` checks risky changes, `/rs:steer` reviews direction with you, including the `Choice:` trailers in commits.

[CLAUDE.md](CLAUDE.md) holds the development rules, [docs/DESIGN.md](docs/DESIGN.md) the direction and invariants, [docs/ROADMAP.md](docs/ROADMAP.md) the stages, and [docs/TASKS.md](docs/TASKS.md) the pending work. [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES) inventories the pinned dependencies' licenses. Licensed under [Apache-2.0](LICENSE).
