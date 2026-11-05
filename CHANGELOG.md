# Changelog

What changed in each version of scramjet-rs, for the people using it. The newest comes first.

## Unreleased

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
