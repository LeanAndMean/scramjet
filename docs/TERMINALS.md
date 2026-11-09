# Terminals

What each terminal does with copying, pasting, the keys scramjet-rs uses and its questions about the terminal's colors. The full screen reports the mouse to scramjet-rs (SGR mouse reporting) and takes pastes as bracketed pastes, so a terminal's own paste key always works, and its own right click and selection usually stop working while scramjet-rs runs. When scramjet-rs cannot read the clipboard, the first failed right-click paste names the terminal's paste key. When a terminal keeps a key for itself, the program never sees it: give the action another key in `~/.config/scramjet-rs/keybindings.json` (`/hotkeys` names the actions' keys; the README, their ids), for example `{"app.message.followUp": "ctrl+q", "tui.editor.undo": "ctrl+z", "app.suspend": []}`.

Each entry gives the version and source it was checked against, and says what was not checked.

## Windows Terminal

Checked against version 1.24.12741: its `defaults.json` and the settings file it writes for new users (`userDefaults`, in `Microsoft.Terminal.Settings.Model.dll`), read on 2026-11-04. scramjet-rs knows it by `WT_SESSION`, which also gives 24-bit colors.

- **Paste:** Ctrl+Shift+V and Shift+Insert. Ctrl+V pastes as well when the settings file has it, as the one written for new users does; whether Ctrl+V then reaches scramjet-rs when the clipboard holds only an image was not checked. Alt+V always reaches it and pastes a clipboard image (checked 2026-11-04).
- **Copy:** Ctrl+Shift+C, Ctrl+Insert, and Enter while Windows Terminal itself has a selection. The settings file for new users adds Ctrl+C, which copies only when there is a selection and otherwise reaches the program.
- **Keys its defaults keep from scramjet-rs:** Alt+Enter (full screen; scramjet-rs: queue a follow-up), Ctrl+- (smaller font; undo), Ctrl+Shift+P (command palette; previous model), and Alt+Up, Alt+Left and Alt+Right (move to another pane; restore queued messages, and move by word, which Ctrl+Left/Right and Alt+B/F also do). With one pane, Alt+Up, Alt+Left and Alt+Right reach the program, on WSL and native (the user, 2026-11-09).
- **Keys that select:** its defaults (its installed `defaults.json`, read 2026-11-08) keep Ctrl+Shift+Up and Down (scroll a line), Ctrl+Shift+Home and End (scroll to the top or bottom) and Alt+Shift+arrows (resize a pane), and leave Shift+arrows, Shift+Home, Shift+End and Ctrl+Shift+Left and Right to the program, so the prompt and the conversation select by character, line and word with those. Selecting by word with Alt+Shift+Left and Right does not reach scramjet-rs there.
- **Ctrl+Backspace:** it sends `0x08` (Backspace sends `0x7f`), which other terminals send for Backspace, so scramjet-rs reads a lone `0x08` as Ctrl+Backspace only where `WT_SESSION` is set and not over ssh, as Scramjet does (its `keys.ts`, read 2026-11-08); pressing it deletes a word on WSL (the user, 2026-11-09), but not in the native `.exe`, which reads the console's key events (Alt+Backspace does; TASKS item 44). Inside tmux in Windows Terminal the variable is kept, and tmux 3.0a passed `0x08` and `0x7f` to the program unchanged (`send-keys -H`, checked 2026-11-08).
- **Clipboard:** under WSL, scramjet-rs reads and writes Windows' clipboard through a PowerShell process it keeps running, not through the terminal. In a remote session it copies with OSC 52.
- **Colors:** the full screen asks for its background, text color and 16 colors with OSC 11, 10 and 4 (for the theme editor). `Microsoft.Terminal.Control.dll` holds the answers' formats, `{};rgb:{:04x}/{:04x}/{:04x}` and `4;{};rgb:{:04x}/{:04x}/{:04x}` (read 2026-11-06), so it answers with its scheme's colors; the answers themselves were checked only from a scripted terminal sending those formats, not from Windows Terminal.
- **Progress:** with `terminal.showTerminalProgress` (Terminal progress in `/settings`), the full screen sends OSC 9;4 while a turn runs, which Windows Terminal 1.6 and later shows as a ring in the tab and progress on the taskbar (Microsoft's "progress bar sequences" page, read 2026-11-08). Seen in Windows Terminal on WSL and native (the user, 2026-11-09).

## tmux

Checked against tmux 3.0a and its manual page, 2026-11-04.

- Its default `set-clipboard external` ignores the OSC 52 sequence a program sends, so a copy through the terminal does not arrive. scramjet-rs copies through the desktop where it can (PowerShell under WSL, the X server, `wl-copy`), and only a remote session inside tmux relies on OSC 52: there, `set -s set-clipboard on` lets it through.
- The outer terminal's paste key still works: a bracketed paste sent to tmux 3.0a reached the program inside it unchanged (checked 2026-11-04).
- Shift+arrows, Shift+Home and End, and Ctrl+Shift+arrows, Home and End reach the program inside unchanged (xterm's `CSI 1;2` and `CSI 1;6` forms, sent to tmux 3.0a in a pty with an empty configuration; checked 2026-11-08).
- It leaves the questions about the terminal's colors unanswered, and shows nothing of them: a program in a tmux 3.0a pane got nothing back in 0.5 s for OSC 11 and in 1 s for OSC 11, 10 and 4 (checked 2026-11-06). scramjet-rs then goes by `COLORFGBG`, which tmux does not set, else takes the background as unknown, and the theme editor offers only its own dark and light ones and no new theme from terminal-colors.
- It drops OSC 9;4, so Terminal progress shows nothing in the outer terminal: the text around it reached tmux 3.0a's output, the sequence did not (a pty with an empty configuration, checked 2026-11-08).

## xterm

Checked against its manual page (xterm patch #412, 2026-10-11, at man.openbsd.org/xterm.1), read 2026-11-04. scramjet-rs knows it by `XTERM_VERSION`.

- **Paste:** Shift+Insert and the middle button paste the primary selection, the text selected last, not the clipboard: `selectToClipboard` is false by default. No key pastes the clipboard unless the translations add one, as its manual suggests with `Ctrl Shift <Key>V: insert-selection(SELECT)`. scramjet-rs reads the clipboard from the X server, so its right click pastes the clipboard.
- **Copy:** OSC 52 is off by default: `allowWindowOps` is false, and the default `disallowedWindowOps` lists `GetSelection` and `SetSelection`. scramjet-rs copies through the X server instead.
- **Mouse:** Shift with a button overrides mouse reporting, for xterm's own selection.
- **Keys:** its default translations bind Alt+Enter to `fullscreen()`, which keeps it from scramjet-rs.

## GNOME Terminal and other VTE terminals

VTE is the terminal library of GNOME Terminal and others, among them LXTerminal, Raspberry Pi OS's terminal (its repository, github.com/lxde/lxterminal). Checked against GNOME Terminal's help (help.gnome.org, Keyboard shortcuts) and VTE's issue tracker on 2026-11-04, not by use.

- **Paste and copy:** Shift+Ctrl+V and Shift+Ctrl+C in GNOME Terminal.
- **OSC 52:** VTE ignores it: its issue "Add support for OSC 52" (GNOME/vte#2495, opened 2018) is still open. scramjet-rs copies through the X server or `wl-copy` instead, so only a copy in a remote session is lost.
- **Mouse:** others report that Shift overrides mouse reporting, for the terminal's own selection (not checked).

## Apple's Terminal

Checked against Apple's Terminal User Guide (support.apple.com, Turn on Mouse Reporting), read 2026-11-04, not on a Mac. scramjet-rs knows it by `TERM_PROGRAM=Apple_Terminal`, and shows 256 colors there, as Scramjet does.

- **Paste and copy:** Command+V and Command+C belong to the terminal, and the program sees neither: Command+V works because the terminal pastes the text itself. Copy on select is on by default on macOS so that a selection is ready for Command+V.
- **Mouse:** Allow Mouse Reporting (View menu, Command+R) is on by default, so clicks go to scramjet-rs. Users report that holding Fn selects with the terminal instead, which Apple's guide does not say.
- **OSC 52:** users report that it is not supported. On a Mac scramjet-rs copies with `pbcopy`, so only a copy in a remote session is lost.

## iTerm2

Checked against its source (github.com/gnachman/iTerm2, `master`) and documentation (iterm2.com), read 2026-11-04, not on a Mac. scramjet-rs knows it by `TERM_PROGRAM=iTerm.app`, and in remote sessions by `LC_TERMINAL=iTerm2`, which iTerm2 sets for ssh and mosh to pass on.

- **Paste and copy:** Command+V and Command+C, as in Apple's Terminal.
- **Right click:** by default it opens iTerm2's menu and is not reported, so scramjet-rs's right click neither copies nor pastes. Settings > Pointer > "Right click reported to apps, does not open menu" (`ReportRightClick`, off by default) gives it to the program.
- **Mouse:** holding Option while a program uses the mouse selects with iTerm2 itself.
- **OSC 52:** off by default: Settings > General > "Applications in terminal may access clipboard" (`AllowClipboardAccess`) allows it.
