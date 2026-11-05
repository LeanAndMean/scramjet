# Changelog

What changed in each version of scramjet-rs, for the people using it. The newest comes first.

## Unreleased

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
