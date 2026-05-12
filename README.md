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

The install script symlinks the repo into a Pi agent's `extensions/`
directory. Pi auto-discovers `index.ts` at next startup.

```sh
git clone https://github.com/<user>/scramjet.git
cd scramjet
./install.sh
```

Default target: `$HOME/.pi/agent/extensions/scramjet`.

Flags:

| Flag             | Effect                                                                      |
| ---------------- | --------------------------------------------------------------------------- |
| `--target <dir>` | Install into `<dir>/scramjet`. `<dir>` is a Pi agent directory.             |
| `--local`        | Install into `<cwd>/.pi/extensions/scramjet` (handy for in-tree dev).       |
| `--force`        | Overwrite an existing `scramjet` entry at the target.                       |
| `-h`, `--help`   | Show usage.                                                                 |

Target resolution precedence (highest first):

1. `--target <path>`
2. `--local`
3. `$PI_CODING_AGENT_DIR/extensions/scramjet` (if the env var is set)
4. `$HOME/.pi/agent/extensions/scramjet` (default)

Re-running the script against the same target is a no-op; it will not
clobber an unrelated entry without `--force`.

If you already have the `pi` CLI, `pi install git:github.com/<user>/scramjet`
is an alternative path that does roughly the same thing.

## Uninstall

```sh
./uninstall.sh           # default target
./uninstall.sh --local   # or --target <dir>
```

`uninstall.sh` only removes its own symlink. It refuses to touch any real
file or directory at the target path.

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
