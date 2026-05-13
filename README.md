# Scramjet

Smart auto-continuation for [Pi](https://github.com/earendil-works/pi-mono).

When a command finishes and suggests a next step, Scramjet shows a short
countdown and runs it. Press `Esc` (or type anything) to cancel. Scramjet
is invisible whenever there is nothing to suggest.

Also bundled:

- `draw_diagram` tool: render Mermaid, Graphviz, or PlantUML source as an
  inline image via Pi's terminal image support.
- `/scramjet on|off` command: toggle auto-continuation without unloading
  the extension.

## Install

The install script does two things: it symlinks the repo into a Pi
agent's `extensions/` directory (Pi auto-discovers `index.ts` at next
startup), and it installs a `scramjet` launcher shim on your `PATH` that
execs `pi`.

```sh
git clone https://github.com/<user>/scramjet.git
cd scramjet
./install.sh
```

Default targets:

- Extension: `$HOME/.pi/agent/extensions/scramjet`
- Shim: `$HOME/.local/bin/scramjet`

If the shim's bin directory is not on `$PATH`, the script prints a one-
liner to add to your shell profile and continues; the extension still
works whether or not you use the shim.

Flags:

| Flag              | Effect                                                                       |
| ----------------- | ---------------------------------------------------------------------------- |
| `--target <dir>`  | Install extension into `<dir>/scramjet`. `<dir>` is a Pi agent directory.    |
| `--local`         | Install extension into `<cwd>/.pi/extensions/scramjet` and shim into `<cwd>/.pi/bin/scramjet` (handy for in-tree dev). |
| `--bin-dir <dir>` | Install the launcher shim into `<dir>/scramjet`.                             |
| `--force`         | Overwrite an existing `scramjet` entry at either target.                     |
| `-h`, `--help`    | Show usage.                                                                  |

Extension target resolution precedence (highest first):

1. `--target <path>`
2. `--local`
3. `$PI_CODING_AGENT_DIR/extensions/scramjet` (if the env var is set)
4. `$HOME/.pi/agent/extensions/scramjet` (default)

Shim target resolution precedence (highest first):

1. `--bin-dir <path>`
2. `--local` (→ `<cwd>/.pi/bin/scramjet`)
3. `$HOME/.local/bin/scramjet` (default)

Re-running the script against the same targets is a no-op; it will not
clobber an unrelated entry without `--force`.

If you already have the `pi` CLI, `pi install git:github.com/<user>/scramjet`
is an alternative path for the extension (it does not install the
launcher shim).

## Usage

After installing, launch a session with:

```sh
scramjet
```

The shim is a thin wrapper that execs `pi`, forwarding all arguments and
the current environment. `scramjet --help`, `scramjet new`, and any
other invocation behave identically to the corresponding `pi` command.
Launching with `pi` directly continues to work unchanged; the shim only
exists so users who installed "scramjet" have a matching command name.

If `pi` is not on `$PATH`, the shim exits with a message pointing at
[the Pi install instructions](https://github.com/earendil-works/pi-mono).

## Uninstall

```sh
./uninstall.sh                       # default targets
./uninstall.sh --local               # mirrors --local install
./uninstall.sh --target <dir>        # mirrors --target install
./uninstall.sh --bin-dir <dir>       # mirrors --bin-dir install
```

`uninstall.sh` removes both the extension symlink and the launcher shim,
each independently. It only removes symlinks; if either target is a real
file or directory, the script refuses to touch it.

## Compatibility

Scramjet imports `@earendil-works/pi-coding-agent` and
`@earendil-works/pi-tui`, which are resolved at runtime via the harness's
virtual module system. It works with:

- The reference `pi` binary.
- Thin custom harnesses built from `pi-mono` that preserve the
  `@earendil-works/*` virtual module namespace.

It does **not** work with hard forks that rename that namespace (e.g.
`dreb` rebranded to `@dreb/*`). The imports fail at load time.

### Multiple harnesses

The install script targets one extensions directory at a time. If you
want Scramjet in two compatible harnesses (`~/.pi/`, `~/.mypi/`, ...),
run the script once per target with `--target` or `$PI_CODING_AGENT_DIR`.

### Platform support

| Platform              | Supported |
| --------------------- | --------- |
| Linux                 | yes       |
| macOS                 | yes       |
| Windows (WSL)         | yes       |
| Windows (native)      | no        |

Native Windows is detected by `uname -s`; the install script exits with a
message pointing to WSL. There is no copy-mode fallback.

## Provider bridge

Pi pins `baseUrl: "https://api.anthropic.com"` on every Anthropic model
entry, so the SDK's normal `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN`
fallback never engages. If you point Claude Code at a proxy like
[tux](https://github.com/merckgroup/tux) or Foundry by sourcing an env
file, Pi would otherwise ignore that env and call `api.anthropic.com`
directly.

Scramjet bridges the gap: at extension load, if `ANTHROPIC_BASE_URL` is
set to anything other than `api.anthropic.com`, Scramjet calls
`pi.registerProvider("anthropic", { baseUrl, apiKey? })` so Pi's
Anthropic traffic flows through the same proxy as Claude Code's. No
per-extension config required.

| Env var                     | Effect                                                                                   |
| --------------------------- | ---------------------------------------------------------------------------------------- |
| `ANTHROPIC_BASE_URL`        | When set and not pointing at `api.anthropic.com`, activates the bridge with this URL.    |
| `ANTHROPIC_AUTH_TOKEN`      | Used as Pi's `apiKey` for the Anthropic provider. Preferred over `ANTHROPIC_API_KEY`.    |
| `ANTHROPIC_API_KEY`         | Used as Pi's `apiKey` for the Anthropic provider if `ANTHROPIC_AUTH_TOKEN` is unset.     |
| `SCRAMJET_PROVIDER_BRIDGE`  | Set to literal `0` to disable the bridge even with the above env vars set.               |

Example: sourcing the tux env file then running Pi is all that's needed:

```sh
source ~/.claudecode_tux
scramjet           # or `pi` — bridge is registered either way
```

To opt out in the same shell:

```sh
SCRAMJET_PROVIDER_BRIDGE=0 scramjet
```

Only the literal string `0` disables the bridge; other values (`false`,
`off`, the empty string) leave it active. A malformed
`ANTHROPIC_BASE_URL` raises at startup rather than silently disabling
the bridge — the misconfiguration is visible immediately instead of
manifesting as confusing direct-to-Anthropic traffic later.

When the bridge is active, Scramjet also strips per-tool
`eager_input_streaming: true` from outgoing Anthropic request payloads.
Pi sends this field by default, and Foundry's Anthropic gateway rejects
it with `INVALID_ARGUMENT: unrecognizedProperty=eager_input_streaming`.
Pi's equivalent per-model knob
(`compat.supportsEagerToolInputStreaming: false`) is not reachable from
the runtime `pi.registerProvider` API, so the bridge removes the field
in-flight via a `before_provider_request` hook. Stock Anthropic accepts
the field, so this fallback is a no-op when the bridge is inactive.

## Versions

Tested against **Pi `0.74.0`** (see `pi.piTestedVersion` in `package.json`).

The `devDependencies` for `@earendil-works/pi-coding-agent` and
`@earendil-works/pi-tui` are pinned to the same version so contributors
can type-check against an exact target. CI fails if `pi.piTestedVersion`
and the pinned dep version drift apart.

If a newer Pi release breaks Scramjet, the tested version is your
diagnostic data point: roll back to it, or open an issue against the
extension. Scramjet is not currently version-gated at runtime.

## Develop

```sh
npm install          # one-time
npm run typecheck    # tsc --noEmit
npm test             # vitest --run
npm run lint         # biome check .
```

The diagram tool detects and uses whatever renderers are installed; none
are bundled. To enable the relevant formats, install one or more of:

- Mermaid: `npm install -g @mermaid-js/mermaid-cli` (provides `mmdc`)
- Graphviz: `apt install graphviz` (or `brew install graphviz`)
- PlantUML: `apt install plantuml` (or `brew install plantuml`)
