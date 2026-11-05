# Changelog

What changed in each version of scramjet-rs, for the people using it. The newest comes first.

## Unreleased

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
