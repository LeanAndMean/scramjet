> scramjet can create themes. Ask it to build one for your setup.

# Themes

Themes are JSON files that define colors for the TUI.

## Table of Contents

- [Locations](#locations)
- [Selecting a Theme](#selecting-a-theme)
- [Creating a Custom Theme](#creating-a-custom-theme)
- [Theme Format](#theme-format)
- [Color Tokens](#color-tokens)
- [Color Values](#color-values)
- [Terminal Background](#terminal-background)
- [Tips](#tips)

## Locations

Scramjet loads themes from:

- Built-in: `pi-dark`, `pi-light`, and `scramjet-dark` (the bundled default for dark-mode terminals)
- Global: `~/.scramjet/agent/themes/*.json`
- Project: `.scramjet/themes/*.json`
- Packages: `themes/` directories or `pi.themes` entries in `package.json`
- Settings: `themes` array with files or directories
- CLI: `--theme <path>` (repeatable)

Disable discovery with `--no-themes`.

## Selecting a Theme

Select a theme via `/settings` or in `settings.json`:

```json
{
  "theme": "my-theme"
}
```

An explicit `theme` setting is always authoritative — automatic detection never overrides it.

### Automatic Theme Detection

When no explicit theme is configured, scramjet classifies the terminal as light or dark on each interactive startup, then maps that classification to a theme. The classification uses the following precedence chain:

1. **`COLORFGBG` environment variable** — If set (e.g., `0;15` or `15;0;0`), the final semicolon-delimited field is interpreted as an xterm-256 color index. Its RGB approximation is classified by WCAG relative luminance (threshold 0.2) as light or dark.
2. **Apple Terminal heuristic** — If `TERM_PROGRAM=Apple_Terminal` and no valid `COLORFGBG` is present, classifies as `light`. Users with custom dark profiles can set `theme: "scramjet-dark"` explicitly.
3. **OSC 11 terminal query** — On terminals that support it (iTerm2, Kitty, WezTerm, Windows Terminal, Alacritty), scramjet queries the actual background color via the OSC 11 escape sequence. The query has a 100 ms timeout to avoid perceptible startup delay.
4. **Fallback** — If all of the above fail or time out, classifies as `dark`.

The classification then maps to a theme name:

- **dark** → `scramjet-dark`, scramjet's bundled default. When `scramjet-dark` is not registered — e.g. Pi used without scramjet — this falls back to `pi-dark`.
- **light** → `pi-light`.

Automatic detection runs on every interactive startup. The result is never persisted to settings — terminal profiles can change between launches, and detection will adapt.

> **Note:** macOS Terminal.app does not support OSC 11 queries (nor truecolor), but the Apple Terminal heuristic (step 2) correctly selects the light theme for its default configuration.

## Creating a Custom Theme

1. Create a theme file:

```bash
mkdir -p ~/.scramjet/agent/themes
vim ~/.scramjet/agent/themes/my-theme.json
```

2. Define the theme with all required colors (see [Color Tokens](#color-tokens)):

```json
{
  "$schema": "https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/src/modes/interactive/theme/theme-schema.json",
  "name": "my-theme",
  "vars": {
    "primary": "#00aaff",
    "secondary": 242
  },
  "colors": {
    "accent": "primary",
    "border": "primary",
    "borderAccent": "#00ffff",
    "borderMuted": "secondary",
    "success": "#00ff00",
    "error": "#ff0000",
    "warning": "#ffff00",
    "muted": "secondary",
    "dim": 240,
    "text": "",
    "thinkingText": "secondary",
    "selectedBg": "#2d2d30",
    "userMessageBg": "#2d2d30",
    "userMessageText": "",
    "customMessageBg": "#2d2d30",
    "customMessageText": "",
    "customMessageLabel": "primary",
    "toolPendingBg": "#1e1e2e",
    "toolSuccessBg": "#1e2e1e",
    "toolErrorBg": "#2e1e1e",
    "toolTitle": "primary",
    "toolOutput": "",
    "mdHeading": "#ffaa00",
    "mdLink": "primary",
    "mdLinkUrl": "secondary",
    "mdCode": "#00ffff",
    "mdCodeBlock": "",
    "mdCodeBlockBorder": "secondary",
    "mdQuote": "secondary",
    "mdQuoteBorder": "secondary",
    "mdHr": "secondary",
    "mdListBullet": "#00ffff",
    "toolDiffAdded": "#00ff00",
    "toolDiffRemoved": "#ff0000",
    "toolDiffContext": "secondary",
    "syntaxComment": "secondary",
    "syntaxKeyword": "primary",
    "syntaxFunction": "#00aaff",
    "syntaxVariable": "#ffaa00",
    "syntaxString": "#00ff00",
    "syntaxNumber": "#ff00ff",
    "syntaxType": "#00aaff",
    "syntaxOperator": "primary",
    "syntaxPunctuation": "secondary",
    "thinkingOff": "secondary",
    "thinkingMinimal": "primary",
    "thinkingLow": "#00aaff",
    "thinkingMedium": "#00ffff",
    "thinkingHigh": "#ff00ff",
    "thinkingXhigh": "#ff0000",
    "bashMode": "#ffaa00"
  }
}
```

3. Select the theme via `/settings`.

**Hot reload:** When you edit the currently active custom theme file, scramjet reloads it automatically for immediate visual feedback.

## Theme Format

```json
{
  "$schema": "https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/src/modes/interactive/theme/theme-schema.json",
  "name": "my-theme",
  "vars": {
    "blue": "#0066cc",
    "gray": 242
  },
  "colors": {
    "accent": "blue",
    "muted": "gray",
    "text": "",
    ...
  }
}
```

- `name` is required and must be unique.
- `vars` is optional. Define reusable colors here, then reference them in `colors`.
- `colors` must define all 51 required tokens.

The `$schema` field enables editor auto-completion and validation.

## Color Tokens

Every theme must define all 51 color tokens. There are no optional colors.

### Core UI (11 colors)

| Token | Purpose |
|-------|---------|
| `accent` | Primary accent (logo, selected items, cursor) |
| `border` | Normal borders |
| `borderAccent` | Highlighted borders |
| `borderMuted` | Subtle borders (editor) |
| `success` | Success states |
| `error` | Error states |
| `warning` | Warning states |
| `muted` | Secondary text |
| `dim` | Tertiary text |
| `text` | Default text (usually `""`) |
| `thinkingText` | Thinking block text |

### Backgrounds & Content (11 colors)

| Token | Purpose |
|-------|---------|
| `selectedBg` | Selected line background |
| `userMessageBg` | User message background |
| `userMessageText` | User message text |
| `customMessageBg` | Extension message background |
| `customMessageText` | Extension message text |
| `customMessageLabel` | Extension message label |
| `toolPendingBg` | Tool box (pending) |
| `toolSuccessBg` | Tool box (success) |
| `toolErrorBg` | Tool box (error) |
| `toolTitle` | Tool title |
| `toolOutput` | Tool output text |

### Markdown (10 colors)

| Token | Purpose |
|-------|---------|
| `mdHeading` | Headings |
| `mdLink` | Link text |
| `mdLinkUrl` | Link URL |
| `mdCode` | Inline code |
| `mdCodeBlock` | Code block content |
| `mdCodeBlockBorder` | Code block fences |
| `mdQuote` | Blockquote text |
| `mdQuoteBorder` | Blockquote border |
| `mdHr` | Horizontal rule |
| `mdListBullet` | List bullets |

### Tool Diffs (3 colors)

| Token | Purpose |
|-------|---------|
| `toolDiffAdded` | Added lines |
| `toolDiffRemoved` | Removed lines |
| `toolDiffContext` | Context lines |

### Syntax Highlighting (9 colors)

| Token | Purpose |
|-------|---------|
| `syntaxComment` | Comments |
| `syntaxKeyword` | Keywords |
| `syntaxFunction` | Function names |
| `syntaxVariable` | Variables |
| `syntaxString` | Strings |
| `syntaxNumber` | Numbers |
| `syntaxType` | Types |
| `syntaxOperator` | Operators |
| `syntaxPunctuation` | Punctuation |

### Thinking Level Borders (6 colors)

Editor border colors indicating thinking level (visual hierarchy from subtle to prominent):

| Token | Purpose |
|-------|---------|
| `thinkingOff` | Thinking off |
| `thinkingMinimal` | Minimal thinking |
| `thinkingLow` | Low thinking |
| `thinkingMedium` | Medium thinking |
| `thinkingHigh` | High thinking |
| `thinkingXhigh` | Extra high thinking |

### Bash Mode (1 color)

| Token | Purpose |
|-------|---------|
| `bashMode` | Editor border in bash mode (`!` prefix) |

### HTML Export (optional)

The `export` section controls colors for `/export` HTML output only. If omitted, colors are derived from `userMessageBg`.

```json
{
  "export": {
    "pageBg": "#18181e",
    "cardBg": "#1e1e24",
    "infoBg": "#3c3728"
  }
}
```

> **Note:** `export.pageBg` controls the HTML export background, not the terminal background. Scramjet cannot set the terminal's background color — that is always owned by the terminal emulator. See [Terminal Background](#terminal-background) below.

## Color Values

Four formats are supported:

| Format | Example | Description |
|--------|---------|-------------|
| Hex | `"#ff0000"` | 6-digit hex RGB |
| 256-color | `39` | xterm 256-color palette index (0-255) |
| Variable | `"primary"` | Reference to a `vars` entry |
| Default | `""` | Terminal's default foreground/background |

### Empty-String Terminal Defaults

When a color token is set to `""`, scramjet emits `\x1b[39m` (reset foreground) or `\x1b[49m` (reset background), deferring to the terminal's configured default colors. This means:

- The theme cannot guarantee contrast for empty-string tokens, because the terminal's actual foreground/background is unknown at theme time.
- Tokens like `text`, `userMessageText`, `customMessageText`, and `toolOutput` use `""` in the built-in themes to inherit the terminal's default foreground.
- If your terminal's default foreground creates poor contrast against themed element backgrounds, you can override these tokens with explicit colors in a custom theme.

### 256-Color Palette

- `0-15`: Basic ANSI colors (terminal-dependent)
- `16-231`: 6×6×6 RGB cube (`16 + 36×R + 6×G + B` where R,G,B are 0-5)
- `232-255`: Grayscale ramp

### Terminal Compatibility

Scramjet uses 24-bit RGB colors. Most modern terminals support this (iTerm2, Kitty, WezTerm, Windows Terminal, VS Code). For older terminals with only 256-color support, scramjet falls back to the nearest approximation.

Check truecolor support:

```bash
echo $COLORTERM  # Should output "truecolor" or "24bit"
```

## Terminal Background

Scramjet styles individual UI elements (message boxes, tool boxes, selected items) with themed backgrounds, but it does **not** control the terminal's overall background color. The space between and around themed elements shows the terminal emulator's own background.

This is by design — setting the terminal background via escape sequences (OSC 11 mutation) is invasive, not universally supported, and affects scrollback behavior. Instead:

- The built-in `pi-light` theme is designed for white or near-white terminal backgrounds.
- The `scramjet-dark` theme (and Pi's `pi-dark`) is designed for dark terminal backgrounds.
- Automatic detection selects the appropriate theme based on your terminal's actual background.
- For best results, ensure your terminal profile's background color matches the theme family (light background → light theme, dark background → dark theme).

### Light Theme Contrast

The built-in light theme guarantees WCAG AA contrast (4.5:1) for all explicit text-bearing foreground colors against their actual rendering surfaces (element backgrounds and white canvas). Element backgrounds (`userMessageBg`, `toolPendingBg`, etc.) maintain at least 1.30:1 contrast against white for visual separation. These guarantees hold in both truecolor and 256-color rendering modes.

## Tips

**Dark terminals:** Use bright, saturated colors with higher contrast.

**Light terminals:** Use darker, muted colors. The built-in light theme targets 4.5:1+ contrast against white.

**Color harmony:** Start with a base palette (Nord, Gruvbox, Tokyo Night), define it in `vars`, and reference consistently.

**Testing:** Check your theme with different message types, tool states, markdown content, and long wrapped text. Test in both truecolor and 256-color mode if your users may have older terminals.

**VS Code:** Set `terminal.integrated.minimumContrastRatio` to `1` for accurate colors.

## Examples

See the built-in themes:
- [pi-dark.json](../src/modes/interactive/theme/pi-dark.json)
- [pi-light.json](../src/modes/interactive/theme/pi-light.json)

Scramjet's bundled default dark theme lives at [`packages/scramjet/themes/scramjet-dark.json`](../../scramjet/themes/scramjet-dark.json).
