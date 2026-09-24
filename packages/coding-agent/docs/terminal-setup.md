# Terminal Setup

Scramjet uses the [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/) for reliable modifier key detection. Most modern terminals support this protocol, but some require configuration.

## Transcript browsing and copying

**Retained mode is the interactive default.** Scramjet owns the live transcript, scrollbar and selection instead of using terminal scrollback. Your terminal's Find feature cannot search retained history that exists only in Scramjet's memory; searching the currently painted screen, where supported, is not a full-transcript search. Use `/export` for a durable HTML view.

To use the older native-history renderer, set `"tuiMode": "committed"` in your [settings file](settings.md#ui--display) and restart Scramjet. This is a compatibility choice, not equivalent live browsing: mutable output taller than the live canvas is tail-windowed until finalized. Changing docking in `/settings` does not change renderer. The interactions below describe retained mode.

During an interactive session, Scramjet owns the alternate screen, mouse reporting, the **rightmost transcript scrollbar**, and drag selection. Wheel/trackpad input and scrollbar dragging browse retained output, including running cards. The terminal's native scrollbar/history is not the live transcript. Returning to the document bottom resumes following; passive updates do not pull you away from what you are reading. The input area is docked by default; typing there preserves the transcript anchor. `/settings` can change docking, the input-text height ceiling (30%, range 10–50%), and wheel step (3, range 1–20). Input shrinks around adjacent widgets before an oversized band falls back to explained, undocked retained browsing. Selection remains within the transcript or dock where it began; holding a selection freezes both presentations.

Select by ordinary dragging, including across a screen edge. After starting a drag, keep the left button held and use the wheel to extend the transcript selection, either with a stationary pointer or while adjusting its horizontal and vertical position; reversing the wheel adjusts the endpoint. Release before browsing if you want to keep the selection unchanged. A press without an initial drag does not start wheel selection, and wheel scrolling does not extend dock selections. Right-click a nonempty selection or use `tui.input.copy` (Ctrl+C by default) to copy displayed text without ANSI controls or scrollbar cells. Built-in text and Markdown copy omit known component padding and rejoin soft wraps, retaining genuine indentation, hard line breaks and structural boundaries. Tables, repeated quote prefixes and unannotated custom renderers retain their physical layout rather than guessing at their source. While selected, the presentation is held without inserting a status row or resizing the transcript. Successful copy or Escape releases it; resizing cancels selection. Copy failures retain the selection and temporarily display a diagnostic on the last screen row without changing geometry; clicking that diagnostic dismisses it without selecting the covered text. Without selection, Ctrl+C retains its normal application behavior; right-click never causes Scramjet to paste or submit.

Terminal-owned Copy/context menus cannot see application selection. A terminal or multiplexer that intercepts pointer input must be configured to forward it. In tmux, enable `set -g mouse on` for the application's wheel/drag path; tmux's own copy mode is a separate interaction. See [tmux.md](tmux.md) for modified-key setup.

Home/End retain editor line-start/end behavior at every scroll position. Ctrl+Home navigates to the transcript beginning; Ctrl+End navigates to the bottom and resumes following. A new user message appearing in the transcript resumes following, while passive output preserves browsing. Alt+PageUp/Alt+PageDown provide keyboard-only entry into transcript browsing; when detached, PageUp/PageDown browse and Escape returns to the tail. Docked editing and presentation toggles keep the reading anchor, while ordinary undocked editing reveals its cursor. At the tail, normal editor/list bindings apply, and focused overlays retain keyboard precedence. See [keybindings.md](keybindings.md#transcript-browsing-precedence).

Clipboard delivery uses the existing platform backend (for example `pbcopy`, `wl-copy`, `xclip`/`xsel`) or OSC 52 where appropriate. Wayland copying awaits stdin completion and a successful `wl-copy` parent exit, with a five-second subprocess timeout; observable failure tries the existing X11/OSC 52 fallbacks before releasing selection. Terminals can reject OSC 52 without acknowledgement: a successful request alone is not proof that a desktop clipboard changed. Remote sessions, clipboard security policies and other profiles require their own verification.

Orderly exit restores the shell's normal buffer and appends one readable plain-text transcript, excluding editor/widgets/temporary approval controls; images receive text labels. Suspension and external-editor handoffs restore normal terminal modes without dumping transcript copies. Crash cleanup prioritizes mode restoration; terminal loss cannot guarantee a flush. For a durable rich view, use `/export`—HTML rendering is unchanged. A retained terminal smaller than 12 columns or three rows shows a resize notice and blocks ordinary input until a usable frame is restored; configured interrupt and empty-draft exit remain available.

Built-in Kitty/iTerm2 images fit the viewport without changing retained source data. Partially visible placements and images behind overlays/selection show placeholders rather than painting through other regions; scroll to reveal the full placement or clear selection. Images remain disabled inside tmux. Custom graphics wrappers must forward the optional image-height bound described in [tui.md](tui.md); arbitrary graphics envelopes are not proven compatible.

After an expired raw Escape/CSI prefix, active mouse reporting recognizes one immediately following mouse-shaped suffix even after a long idle gap. Matching suffixes are discarded rather than inserted or acted on; other typing is replayed, with incomplete candidates released after 10 ms. Consequently, identical mouse-shaped literal text in that position is consumed, and mouse suffixes themselves fragmented beyond 10 ms are not guaranteed recovery. Bracketed paste is kept separate. This bounded ambiguity policy avoids indefinitely withholding ordinary typing; it is not universal lossless framing over arbitrarily delayed connections.

### Native compatibility evidence

Native checks cover browsing, copying, live updates, resizing, approval controls, terminal handoffs and restoration; separate pixel checks cover built-in Kitty/iTerm2 images. These configurations describe earlier retained-renderer journeys, not acceptance of every subsequent change. See [Terminal verification](terminal-verification.md) for revision-specific evidence, driver details and historical limitations.

Recorded native configurations (2026-09-21):

| Path | Tested version and configuration |
|------|----------------------------------|
| Windows Terminal / WSL | Terminal 1.24.11911.0, Windows 11 build 22631, WSL2 5.15.153.1, Ubuntu 20.04.6; 120×30; existing profile unchanged |
| Apple Terminal | 2.14 / 455.1, macOS 15.7.9 / 24G830 ARM64; 80×24 |
| iTerm2 | 3.6.11 on that macOS; 80×25; `ReportRightClick=true`; inline-image consent for graphics checks |
| Linux VTE | Xfce Terminal 1.1.3, Ubuntu 24.04.5, X11/Xvfb/Openbox; DejaVu Sans Mono 12, 80×24, native scrollbar hidden |
| tmux | 3.4 on the preceding VTE path; isolated config with mouse on and status off |
| xterm | XTerm 390 on Ubuntu 24.04.5/X11; 80×24; `selectToClipboard=true` with its native Shift+Insert paste binding |
| Kitty | 0.32.2 on Ubuntu 24.04.5/X11; 80×24 interaction and 80×30 graphics profiles |

xterm's native paste binding/source differ from VTE's Ctrl+Shift+V; the recorded paste configuration is not a new Scramjet shortcut. Native xterm evidence does not verify VS Code's integrated terminal, nor does Kitty verify every Kitty-family emulator.

These checks do not establish every emulator/profile, physical trackpad's gesture characteristics, remote desktop, multiplexer, custom extension or graphics renderer. OS-generated wheel events are not physical trackpad testing; headless tests and ordinary CI do not substitute for native desktop/clipboard evidence.

### Native selection differences — deferred

Terminal-owned selection is separate from Scramjet selection. Windows Terminal reserves Shift-mouse for its own screen-cell selection; [iTerm2 uses Option to bypass mouse reporting](https://iterm2.com/faq.html). These modifiers are terminal/profile-specific, not a universal Scramjet shortcut. They do not expose unpainted retained history or extend an application selection, and native copying can include visible padding or scrollbar cells. Scramjet's copy cleanup applies only to its ordinary drag/Ctrl+C/right-click path.

Native-selection parity remains deferred. Retained edge scrolling uses a fixed speed, not the native distance-dependent acceleration demonstrated on Windows Terminal. Changing modifiers during an active edge drag can withhold its release from the application; clear the application selection to stop a stranded gesture. See the [historical comparison](terminal-verification.md#historical-windows-selection-comparison) for the observations and their limits.

## Apple Terminal

In the tested Apple Terminal 2.14 profile, Option+PageUp arrived as unmodified PageUp. Scramjet cannot distinguish those identical bytes without stealing ordinary editor paging. Use an explicit application keybinding profile instead:

```json
{
  "tui.viewport.pageUp": "f8",
  "tui.viewport.pageDown": "f9"
}
```

Set this in `~/.scramjet/agent/keybindings.json`; keyboards with media-key defaults may require Fn+F8/Fn+F9. The Apple native journey declares and exercises this temporary profile. Other tested terminal paths exercise the default Alt+PageUp/Alt+PageDown bindings. This is not an automatic runtime terminal allowlist or a claim about every keyboard layout.

## Kitty

Keyboard reporting works out of the box.

## iTerm2

Keyboard reporting works out of the box. **Right-click copying requires iTerm2's `ReportRightClick` preference** so mouse-reporting applications receive right-clicks instead of iTerm2's native context menu. The default-off profile was tested and does not support Scramjet's right-click-copy path. This is an explicit setup requirement, not a runtime allowlist or automatic fallback.

Enable the right-click-reporting option in iTerm2's Pointer settings, or quit iTerm2 and set its existing preference from another terminal:

```sh
defaults write com.googlecode.iterm2 ReportRightClick -bool true
```

Restart iTerm2 afterward. This changes right-click behavior for mouse-reporting applications, not just Scramjet; Scramjet never changes the preference for you. Ctrl+C remains the application-selection copy alternative. Native iTerm2 menus still cannot access application-owned selection.

## Ghostty

Add to your Ghostty config (`~/Library/Application Support/com.mitchellh.ghostty/config` on macOS, `~/.config/ghostty/config` on Linux):

```
keybind = alt+backspace=text:\x1b\x7f
```

Older Claude Code versions may have added this Ghostty mapping:

```
keybind = shift+enter=text:\n
```

That mapping sends a raw linefeed byte. Inside Scramjet, that is indistinguishable from `Ctrl+J`, so tmux and Scramjet no longer see a real `shift+enter` key event.

If Claude Code 2.x or newer is the only reason you added that mapping, you can remove it, unless you want to use Claude Code in tmux, where it still requires that Ghostty mapping.

If you want `Shift+Enter` to keep working in tmux via that remap, add `ctrl+j` to your Scramjet `newLine` keybinding in `~/.scramjet/agent/keybindings.json`:

```json
{
  "newLine": ["shift+enter", "ctrl+j"]
}
```

## WezTerm

Create `~/.wezterm.lua`:

```lua
local wezterm = require 'wezterm'
local config = wezterm.config_builder()
config.enable_kitty_keyboard = true
return config
```

## VS Code (Integrated Terminal)

`keybindings.json` locations:
- macOS: `~/Library/Application Support/Code/User/keybindings.json`
- Linux: `~/.config/Code/User/keybindings.json`
- Windows: `%APPDATA%\\Code\\User\\keybindings.json`

Add to `keybindings.json` to enable `Shift+Enter` for multi-line input:

```json
{
  "key": "shift+enter",
  "command": "workbench.action.terminal.sendSequence",
  "args": { "text": "\u001b[13;2u" },
  "when": "terminalFocus"
}
```

## Windows Terminal

Add to `settings.json` (Ctrl+Shift+, or Settings → Open JSON file) to forward the modified Enter keys Scramjet uses:

```json
{
  "actions": [
    {
      "command": { "action": "sendInput", "input": "\u001b[13;2u" },
      "keys": "shift+enter"
    },
    {
      "command": { "action": "sendInput", "input": "\u001b[13;3u" },
      "keys": "alt+enter"
    }
  ]
}
```

- `Shift+Enter` inserts a new line.
- Windows Terminal binds `Alt+Enter` to fullscreen by default. That prevents Scramjet from receiving `Alt+Enter` for follow-up queueing.
- Remapping `Alt+Enter` to `sendInput` forwards the real key chord to Scramjet instead.

If you already have an `actions` array, add the objects to it. If the old fullscreen behavior persists, fully close and reopen Windows Terminal.

## xfce4-terminal, terminator

These terminals have limited escape sequence support. Modified Enter keys like `Ctrl+Enter` and `Shift+Enter` cannot be distinguished from plain `Enter`, preventing custom keybindings such as `submit: ["ctrl+enter"]` from working.

For the best experience, use a terminal that supports the Kitty keyboard protocol:
- [Kitty](https://sw.kovidgoyal.net/kitty/)
- [Ghostty](https://ghostty.org/)
- [WezTerm](https://wezfurlong.org/wezterm/)
- [iTerm2](https://iterm2.com/)
- [Alacritty](https://github.com/alacritty/alacritty) (requires compilation with Kitty protocol support)

## IntelliJ IDEA (Integrated Terminal)

The built-in terminal has limited escape sequence support. Shift+Enter cannot be distinguished from Enter in IntelliJ's terminal.

If you want the hardware cursor visible, set `PI_HARDWARE_CURSOR=1` before running Scramjet (disabled by default for compatibility).

Consider using a dedicated terminal emulator for the best experience.
