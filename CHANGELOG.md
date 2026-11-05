# Changelog

What changed in each version of scramjet-rs, for the people using it. The newest comes first.

## Unreleased

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
