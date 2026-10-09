# Settings

Scramjet uses JSON settings files with project settings overriding global settings.

| Location | Scope |
|----------|-------|
| `~/.scramjet/agent/settings.json` | Global (all projects) |
| `.scramjet/settings.json` | Project (current directory) |

Edit directly or use `/settings` for common options.

## All Settings

### Model & Thinking

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `defaultProvider` | string | - | Default provider (e.g., `"anthropic"`, `"openai"`) |
| `defaultModel` | string | - | Default model ID |
| `defaultThinkingLevel` | string | - | `"off"`, `"minimal"`, `"low"`, `"medium"`, `"high"`, `"xhigh"` |
| `hideThinkingBlock` | boolean | `false` | Hide thinking blocks in output |
| `thinkingBudgets` | object | - | Custom token budgets per thinking level |

#### thinkingBudgets

```json
{
  "thinkingBudgets": {
    "minimal": 1024,
    "low": 4096,
    "medium": 10240,
    "high": 32768
  }
}
```

### UI & Display

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `theme` | string | auto-detected | Theme name (`"dark"`, `"light"`, or custom). When unset, detected from terminal background on each startup (see [themes.md](themes.md#automatic-theme-detection)) |
| `quietStartup` | boolean | `false` | Hide startup header |
| `collapseChangelog` | boolean | `false` | Show condensed changelog after updates |
| `enableInstallTelemetry` | boolean | `true` | Send an anonymous install/update version ping after first install or changelog-detected updates. This does not control update checks |
| `doubleEscapeAction` | string | `"tree"` | Action for double-escape: `"tree"`, `"fork"`, or `"none"` |
| `treeFilterMode` | string | `"default"` | Default filter for `/tree`: `"default"`, `"no-tools"`, `"user-only"`, `"labeled-only"`, `"all"` |
| `editorPaddingX` | number | `0` | Horizontal padding for input editor (0-3) |
| `autocompleteMaxVisible` | number | `5` | Max visible items in autocomplete dropdown (3-20) |
| `showHardwareCursor` | boolean | `false` | Show terminal cursor |
| `tuiMode` | string | `"retained"` | Startup-only `"retained"` or `"committed"` renderer; changes require restarting Scramjet |
| `dockEditor` | boolean | `true` | Keep input, adjacent widgets and footer docked in retained mode; live `/settings` control |
| `retainTranscriptOnExit` | boolean | `false` | Leave the full current styled transcript, widgets, rendered editor and footer in terminal scrollback on final exit from retained mode; live `/settings` control |
| `editorMaxHeightPercent` | number | `30` | Maximum wrapped input-text rows as 10–50% of terminal height; further reduced to fit the available space |
| `scrollWheelStep` | number | `3` | Retained transcript rows per wheel event, 1–20; live `/settings` control |

Exit retention preserves the current presentation, including offscreen transcript rows, colors, styling and collapsed/expanded state. It includes the rendered editor contents, not draft rows hidden by its height limit, and does not expand compacted or other-branch history. Images become labelled placeholders; selection highlighting, overlays, the scrollbar and browsing hints are excluded. This exposes rendered session and draft text to native terminal history, whose own scrollback capacity still applies. Suspension, external-editor handoffs and crash cleanup never request retention. Committed compatibility mode already writes native history and does not replay a duplicate. Session saving and `/export` are unchanged.

The editor-height percentage is a ceiling, not blank reserved space. Borders, autocomplete, widgets and the footer also require room; the input window shrinks before docking is suspended. Oversized extension content remains reachable in the undocked retained flow with a visible explanation. Undocking does not change renderer or terminal-buffer ownership. `committed` is an explicit compatibility choice with the older tail-windowed mutable-output limitation, not equivalent live browsing or a verified screen-reader mode; dock and wheel controls are unavailable there.

Numeric layout preferences are floored and clamped to their stated ranges; invalid types/non-finite values use safe defaults with settings diagnostics. Invalid explicit `tuiMode` values are rejected. Invalid explicit `retainTranscriptOnExit` values fall back to `false` with a diagnostic. These controls preserve global-write/project-override semantics: the selector identifies project overrides and shows the effective value even when editing the global preference. Failed writes show an unsaved notice and diagnostic; an in-memory value alone does not establish successful persistence. A malformed global settings file is preserved, and every blocked save attempt reports its load error even after startup diagnostics have been consumed.

### Telemetry and update checks

`enableInstallTelemetry` only controls the anonymous install/update ping to `https://pi.dev/api/report-install`. Opting out of telemetry does not disable update checks.

The upstream runtime can check `https://pi.dev/api/latest-version` for updates, but Scramjet automatically sets `PI_SKIP_VERSION_CHECK=1`, so this check never runs. Use `--offline` or `PI_OFFLINE=1` to disable all startup network operations described here, including update checks, package update checks, and install/update telemetry.

### Warnings

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `warnings.anthropicExtraUsage` | boolean | `true` | Show a warning when Anthropic subscription auth may use paid extra usage |

```json
{
  "warnings": {
    "anthropicExtraUsage": false
  }
}
```

### Compaction

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `compaction.enabled` | boolean | `true` | Enable auto-compaction |
| `compaction.reserveTokens` | number | `16384` | Tokens reserved for LLM response |
| `compaction.keepRecentTokens` | number | `20000` | Recent tokens to keep (not summarized) |

```json
{
  "compaction": {
    "enabled": true,
    "reserveTokens": 16384,
    "keepRecentTokens": 20000
  }
}
```

### Branch Summary

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `branchSummary.reserveTokens` | number | `16384` | Tokens reserved for branch summarization |
| `branchSummary.skipPrompt` | boolean | `false` | Skip "Summarize branch?" prompt on `/tree` navigation (defaults to no summary) |

### Retry

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `retry.enabled` | boolean | `true` | Enable bounded agent retries for transient and specifically recognized incomplete responses |
| `retry.maxRetries` | number | `3` | Maximum agent-level retry attempts |
| `retry.baseDelayMs` | number | `2000` | Base delay for agent-level exponential backoff (2s, 4s, 8s) |
| `retry.provider.timeoutMs` | number | SDK default | Provider/SDK request timeout in milliseconds |
| `retry.provider.maxRetries` | number | SDK default | Provider/SDK retry attempts |
| `retry.provider.maxRetryDelayMs` | number | `60000` | Max server-requested delay before failing (60s) |

Agent retries include validated missing-terminal Responses streams and accepted responses with missing bodies. Failed-turn tool calls never execute. Known quota/authentication/permission/content rejection, callback failures, and invalid evidence do not authorize retries. Provider/SDK request attempts are separate from the agent's consecutive and cumulative limits; these settings do not establish a universal HTTP-attempt or elapsed-time budget.

Codex SSE honors `retry.provider.maxRetries` (default three) and rejects server-requested waits above `retry.provider.maxRetryDelayMs`, including after the final inner attempt. That rejection suppresses outer agent retries too. Set the delay cap to `0` to disable the policy cap; long representable waits remain cancellable, and unrepresentable waits fail without retry. Other SDKs do not necessarily implement this delay cap or these request controls; forwarding an option is not proof of enforcement.

The interactive countdown describes a recovery attempt, not final task completion. Escape cancels the originating recovery even between attempt completion and retry classification. Enter/Alt+Enter in non-streaming recovery retain your draft with a wait/cancel explanation; existing streaming and compaction queues stay explicit. Final cancellation/failure leaves the editor usable and tells you to review interrupted work before submitting again. Recovered execution is not a claim that the task completed. No additional retry setting or universal timeout is introduced.

Counts must be nonnegative safe integers; millisecond settings must be integers from 0 through 2,147,483,647. Invalid values generate scoped settings diagnostics and inherit a valid lower-priority value or the default without rewriting the file. Nested `retry.provider` leaves merge individually. Zero retries disables that layer's retries; zero base delay removes intentional backoff; timeout zero retains provider/SDK semantics and does not universally disable timeouts. Codex direct API callers receive a local validation failure for invalid retry options.

```json
{
  "retry": {
    "enabled": true,
    "maxRetries": 3,
    "baseDelayMs": 2000,
    "provider": {
      "timeoutMs": 3600000,
      "maxRetries": 0,
      "maxRetryDelayMs": 60000
    }
  }
}
```

### Message Delivery

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `steeringMode` | string | `"all"` | How steering messages are sent: `"all"` or `"one-at-a-time"` |
| `followUpMode` | string | `"one-at-a-time"` | How follow-up messages are sent: `"all"` or `"one-at-a-time"` |
| `transport` | string | `"sse"` | Preferred transport for providers that support multiple transports: `"sse"`, `"websocket"`, or `"auto"` |

### Terminal & Images

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `terminal.showImages` | boolean | `true` | Show images in terminal (if supported) |
| `terminal.imageWidthCells` | number | `60` | Preferred inline image width in terminal cells |
| `images.autoResize` | boolean | `true` | Resize images to 2000x2000 max |
| `images.blockImages` | boolean | `false` | Block all images from being sent to LLM |

### Shell

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `shellPath` | string | - | Custom shell path (e.g., for Cygwin on Windows) |
| `shellCommandPrefix` | string | - | Prefix for every bash command (e.g., `"shopt -s expand_aliases"`) |
| `npmCommand` | string[] | - | Command argv used for npm package lookup/install operations (e.g., `["mise", "exec", "node@20", "--", "npm"]`) |

```json
{
  "npmCommand": ["mise", "exec", "node@20", "--", "npm"]
}
```

`npmCommand` is used for all npm package-manager operations, including installs, uninstalls, and dependency installs inside git packages. Use argv-style entries exactly as the process should be launched. When `npmCommand` is configured, git package dependency installs use plain `install` to avoid npm-specific flags in wrappers or alternate package managers.

Normally the package manager's global modules location is queried using `root -g`. As a special case, if the first element of `npmCommand` is `"bun"`, the modules location will instead be queried with `pm bin -g`.

### Sessions

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `sessionDir` | string | - | Directory where session files are stored. Accepts absolute or relative paths, plus `~`. |

```json
{ "sessionDir": ".scramjet/sessions" }
```

When multiple sources specify a session directory, precedence is `--session-dir`, `SCRAMJET_CODING_AGENT_SESSION_DIR`, then `sessionDir` in settings.json.

### Model Cycling

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `enabledModels` | string[] | - | Model patterns for Ctrl+P cycling (same format as `--models` CLI flag) |

```json
{
  "enabledModels": ["claude-*", "gpt-4o", "gemini-2*"]
}
```

### Markdown

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `markdown.codeBlockIndent` | string | `"  "` | Indentation for code blocks |

### Resources

These settings define where to load extensions, skills, prompts, and themes from.

Paths in `~/.scramjet/agent/settings.json` resolve relative to `~/.scramjet/agent`. Paths in `.scramjet/settings.json` resolve relative to `.scramjet`. Absolute paths and `~` are supported.

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `packages` | array | `[]` | npm/git packages to load resources from |
| `extensions` | string[] | `[]` | Local extension file paths or directories |
| `skills` | string[] | `[]` | Local skill file paths or directories |
| `prompts` | string[] | `[]` | Local prompt template paths or directories |
| `themes` | string[] | `[]` | Local theme file paths or directories |
| `enableSkillCommands` | boolean | `true` | Register skills as `/skill:name` commands |

Arrays support glob patterns and exclusions. Use `!pattern` to exclude. Use `+path` to force-include an exact path and `-path` to force-exclude an exact path.

#### packages

String form loads all resources from a package:

```json
{
  "packages": ["pi-skills", "@org/my-extension"]
}
```

Object form filters which resources to load:

```json
{
  "packages": [
    {
      "source": "pi-skills",
      "skills": ["brave-search", "transcribe"],
      "extensions": []
    }
  ]
}
```

See [packages.md](packages.md) for package management details.

## Example

```json
{
  "defaultProvider": "anthropic",
  "defaultModel": "claude-sonnet-4-20250514",
  "defaultThinkingLevel": "medium",
  "theme": "dark",
  "compaction": {
    "enabled": true,
    "reserveTokens": 16384,
    "keepRecentTokens": 20000
  },
  "retry": {
    "enabled": true,
    "maxRetries": 3
  },
  "enabledModels": ["claude-*", "gpt-4o"],
  "warnings": {
    "anthropicExtraUsage": true
  },
  "packages": ["pi-skills"]
}
```

## Project Overrides

Project settings (`.scramjet/settings.json`) override global settings. Nested objects are merged:

```json
// ~/.scramjet/agent/settings.json (global)
{
  "theme": "dark",
  "compaction": { "enabled": true, "reserveTokens": 16384 }
}

// .scramjet/settings.json (project)
{
  "compaction": { "reserveTokens": 8192 }
}

// Result
{
  "theme": "dark",
  "compaction": { "enabled": true, "reserveTokens": 8192 }
}
```
