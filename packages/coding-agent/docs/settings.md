# Settings

Scramjet uses JSON settings files with project settings overriding global settings.

| Location | Scope |
|----------|-------|
| `~/.scramjet/agent/settings.json` | Global (all projects) |
| `.scramjet/settings.json` | Project (current directory) |

Edit directly or use `/settings` for common options.

## Directory model state

`SettingsManager.create(cwd, agentDir)` also reads `<agentDir>/last-used-models.json` (normally `~/.scramjet/agent/last-used-models.json`). This is automatically managed state, not a setting or project override. It maps exact lexical `path.resolve(cwd)` paths to `{ "provider": "…", "modelId": "…" }`; symlinks, parent/child directories and Git roots are not merged. Reads do not create files, and recall never creates files or `.scramjet` directories in working directories.

Successful deliberate model selection (`/model`, cycling, RPC or SDK `setModel`) updates this memory separately from the existing global default. Startup, restoration, automatic fallback and thinking-only changes do not teach it; `persistModelPreferences: false` disables both model-memory and global model/thinking saves. Two-argument calls remain global-only. Directory lookup is suppressed when either model-default field is defined in project settings or programmatic overrides; existing default getters retain their global/project field-wise merge. Programmatic model-default overrides survive saves and reloads. Thinking preferences remain global.

**Startup model** in `/settings` selects `Per-directory` (the default) or `Global`, stored as `startupModel: "per-directory"` or `"global"` in `settings.json`. Per-directory fresh launches prefer eligible directory memory over the global default; unseen directories use the existing global fallback. Global skips directory lookup and its unavailable-model warnings, restoring effective saved-default/fallback startup selection. Both modes continue recording successful deliberate selections in the global default and directory memory, subject to `persistModelPreferences`; Global is not a recording/privacy opt-out. Switching modes does not change the current live model, teach a choice or delete records, so returning to Per-directory recalls the latest deliberate directory choices. The UI saves globally, identifies project overrides and shows the effective value. Invalid explicit `startupModel` values produce scoped diagnostics and inherit a valid lower-priority value or the default without rewriting disk.

In either mode, explicit CLI/SDK models and authenticated session restoration take priority. Recall requires an exact registry identity and configured authentication, without running credential commands or probing a provider. Missing identity or authentication warns and falls back without erasing the remembered record; this check does not establish endpoint availability or entitlement. `/new` inherits the current live model rather than rereading another directory's latest global choice.

For fresh CLI scoped startup (`--models` or `enabledModels`), valid recall in scope wins. Valid recall outside scope selects the first scoped model, even when the global default is another member. Unavailable recall warns, then uses the existing global-in-scope/first-scoped fallback. Project/programmatic model defaults and `startupModel: "global"` suppress recall entirely, including unavailable-record warnings. Global mode uses the existing global-in-scope/first-scoped fallback.

Global settings and recall writes commit independently, in that order per selection. Either may succeed while the other fails; there is no rollback or two-file atomic transaction. Malformed recall documents are preserved and block recall writes, not global settings writes; invalid individual entries are ignored for lookup and preserved unless their own directory is deliberately updated. A failed reload retains the last valid recall state. Errors distinguish failed updates from recall replacements committed before unlock failure; do not blindly retry uncertain writes.

Settings and recall diagnostics add file/operation context through native `Error` causes, retaining original error type, message, code, stack, nested causes and arbitrary thrown values; `AggregateError` retains the original errors when multiple operations fail. Ordinary CLI/mode warnings and stderr include causal evidence, not only debug output, with VT sequences and control characters stripped for display. No document is appended to a diagnostic, but an original parser message may itself contain document excerpts; terminal sanitization is not secret redaction. Buffered persistence errors are not routed through tool results or the session journal. A custom backend failure does not establish that durable storage is unchanged.

The state file contains visited directory paths and model identities, not credentials. Its metadata grows without eviction; separating it from settings does not hide it from local filesystem access or backups. Custom/in-memory settings factories use private memory recall unless explicitly given a backend; see [SDK settings management](sdk.md#settings-management).

`flush()` settles queued attempts, not guaranteed successful persistence: inspect `drainErrors()`. Built-in modes report settings failures through active interactive warnings or terminal-safe stderr at orderly exit; print/JSON/RPC never put these diagnostics on protocol stdout. Runtime replacement settles settings before candidate construction and again after shutdown handlers, before invalidation. Crashes, terminal-loss emergency exits, repeated forced exits and SIGKILL cannot guarantee asynchronous settlement.

## All Settings

### Model & Thinking

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `defaultProvider` | string | - | Default provider (e.g., `"anthropic"`, `"openai"`) |
| `defaultModel` | string | - | Default model ID |
| `startupModel` | string | `"per-directory"` | Fresh-launch preference: `"per-directory"` or `"global"`; both modes continue recording deliberate directory choices |
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
| `markdown.toolDisplay` | string | `"rendered"` | Interactive Markdown reads: `"rendered"` documents or `"source"` sanitized syntax |

**Markdown reads** in `/settings` changes current and future read cards, independently of Ctrl+O expansion. Changes save to the global `settings.json` in the effective agent/profile directory (`~/.scramjet/agent` by default, configurable with `SCRAMJET_CODING_AGENT_DIR`). A project `markdown.toolDisplay` overrides the global value; the selector identifies that exact leaf and continues showing the effective value after global edits. Setting only project `markdown.codeBlockIndent` does not override the display mode. Failed saves show an unsaved notice and warning; malformed files are preserved. Invalid explicit display values or Markdown containers produce scoped diagnostics and inherit a valid lower-priority value or the default without rewriting disk.

Rendered mode uses headings, emphasis, lists and fenced code for proven textual `.md`/`.markdown` reads (case-insensitive). Collapsed cards select ten source lines before parsing; expansion shows all returned content, not unread file content. Compact resource/skill/documentation reads remain hidden until expanded. Offset/limited fragments render as supplied; generated continuation guidance stays outside the document. Historical or custom results without a provable document boundary fall back to labelled Source, so notice-looking file prose is never heuristically removed. Source exposes sanitized syntax and comments, not a byte-exact terminal dump; rendered HTML comments may be hidden. Copy captures displayed content, not original Markdown bytes or omitted content.

Arguments still arriving retain the existing read header; no document is rendered until textual result content arrives. Writes retain their existing source presentation, including streaming previews. This preference does not change execution, model-visible text, print/RPC output or HTML export, and fully custom renderers may ignore it.

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
