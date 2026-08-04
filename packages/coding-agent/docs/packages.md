> scramjet can help you create scramjet packages. Ask it to bundle your extensions, skills, prompt templates, or themes.

# Scramjet Packages

Scramjet packages bundle extensions, skills, prompt templates, and themes so you can share them through npm or git. A package can declare resources in `package.json` under the `pi` key, or use conventional directories.

## Table of Contents

- [Install and Manage](#install-and-manage)
- [Failure-Safe npm Self-Updates](#failure-safe-npm-self-updates)
- [Package Sources](#package-sources)
- [Creating a Scramjet Package](#creating-a-scramjet-package)
- [Package Structure](#package-structure)
- [Dependencies](#dependencies)
- [Package Filtering](#package-filtering)
- [Enable and Disable Resources](#enable-and-disable-resources)
- [Scope and Deduplication](#scope-and-deduplication)

## Install and Manage

> **Security:** Scramjet packages run with full system access. Extensions execute arbitrary code, and skills can instruct the model to perform any action including running executables. Review source code before installing third-party packages.

```bash
scramjet install npm:@foo/bar@1.0.0
scramjet install git:github.com/user/repo@v1
scramjet install https://github.com/user/repo  # raw URLs work too
scramjet install /absolute/path/to/package
scramjet install ./relative/path/to/package

scramjet remove npm:@foo/bar
scramjet list                     # show installed packages from settings
scramjet update                   # update the CLI and all non-pinned packages
scramjet update --extensions      # update all non-pinned packages only
scramjet update --self            # update the CLI only
scramjet update --self --force    # reinstall the CLI even if current
scramjet update npm:@foo/bar      # update one package
scramjet update --extension npm:@foo/bar
```

By default, `install` and `remove` write to global settings (`~/.scramjet/agent/settings.json`). Use `-l` to write to project settings (`.scramjet/settings.json`) instead. Project settings can be shared with your team, and scramjet installs any missing packages automatically on startup.

To try a package without installing it, use `--extension` or `-e`. This installs to a temporary directory for the current run only:

```bash
scramjet -e npm:@foo/bar
scramjet -e git:github.com/user/repo
```

## Failure-Safe npm Self-Updates

Scramjet uses a verified package-tree transaction only when it can positively qualify all of these conditions:

- npm manages the current global installation on Linux, macOS, or WSL;
- the installed and target package names are the same;
- the product is a real directory at npm's exact global scoped-package path;
- the executing coding-agent runtime is contained in that product tree;
- `package.json`, `bin.scramjet`, and the canonical `scramjet` launcher symlink have the expected identity and containment;
- the product, launcher, backup, quarantine, and temporary-launcher locations support the required same-filesystem operations.

Immediately before mutation, Scramjet revalidates those facts and requires unique transaction paths. It renames the complete product tree to a sibling backup, runs the existing npm command with inherited standard I/O, structurally validates the replacement, and starts the absolute canonical launcher with a fixed timeout. A timed-out probe receives `SIGTERM`, then `SIGKILL` after a bounded grace period if necessary; Scramjet waits for the child to close before moving or cleaning package trees.

A successful npm exit is not enough to commit. The replacement must retain the expected package name, declared `bin.scramjet`, launcher symlink text and target, product containment, and a successful fresh launcher probe. Only then is the backup eligible for cleanup and `Updated scramjet` printed.

If npm returns a failure or replacement verification fails, Scramjet quarantines partial canonical state, restores the previous package by rename, atomically restores the launcher through a temporary sibling symlink, and repeats structural and fresh-process verification. The command remains unsuccessful even when restoration succeeds. Output then distinguishes these states:

- **Verified commit:** prints `Updated scramjet` and exits successfully.
- **Verified commit with incomplete cleanup:** remains successful, confirms the updated launcher/package runtime, and reports each retained artifact and cleanup error.
- **Verified restoration:** exits unsuccessfully, identifies the failed phase, confirms only that the previous launcher/package runtime was restored and verified, and warns that postinstall-managed command data may have changed.
- **Unverified restoration:** exits unsuccessfully, reports restoration failures and exact retained paths, makes no availability claim, and advises inspection before repair rather than blindly repeating the update.

Cleanup is availability-first. If removing a backup or quarantine fails—including `EBUSY`, `EACCES`, `EPERM`, `ENOTEMPTY`, `EIO`, identity substitution, or a partially removed network-filesystem artifact—the verified canonical installation remains untouched. Inspect the reported path and resolve the underlying filesystem condition before removing it. An open file on network storage is one possible cause; Scramjet neither attributes contention to a process without evidence nor scans or terminates processes.

The recovery guarantee does not cover pnpm, Yarn, Bun, native Windows, package-name migration, linked/source installations, configured npm wrappers, or npm layouts whose fresh-launch runtime cannot be proven to fit inside the product tree. Those paths retain their existing package-manager behavior and output without rollback claims. Parent-process termination, `SIGKILL`, or host failure during evacuation is also outside scope.

The transaction restores only the canonical launcher and package runtime. Scramjet's npm postinstall manages command-set data outside the package tree, so files under `${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/` may have changed after an otherwise rolled-back update.

## Package Sources

Scramjet accepts three source types in settings and `scramjet install`.

### npm

```
npm:@scope/pkg@1.2.3
npm:pkg
```

- Versioned specs are pinned and skipped by package updates (`scramjet update`, `scramjet update --extensions`).
- Global installs use `npm install -g`.
- Project installs go under `.scramjet/npm/`.
- Set `npmCommand` in `settings.json` to pin npm package lookup and install operations to a specific wrapper command such as `mise` or `asdf`.

Example:

```json
{
  "npmCommand": ["mise", "exec", "node@20", "--", "npm"]
}
```

### git

```
git:github.com/user/repo@v1
git:git@github.com:user/repo@v1
https://github.com/user/repo@v1
ssh://git@github.com/user/repo@v1
```

- Without `git:` prefix, only protocol URLs are accepted (`https://`, `http://`, `ssh://`, `git://`).
- With `git:` prefix, shorthand formats are accepted, including `github.com/user/repo` and `git@github.com:user/repo`.
- HTTPS and SSH URLs are both supported.
- SSH URLs use your configured SSH keys automatically (respects `~/.ssh/config`).
- For non-interactive runs (for example CI), you can set `GIT_TERMINAL_PROMPT=0` to disable credential prompts and set `GIT_SSH_COMMAND` (for example `ssh -o BatchMode=yes -o ConnectTimeout=5`) to fail fast.
- Refs pin the package and skip package updates (`scramjet update`, `scramjet update --extensions`).
- Cloned to `~/.scramjet/agent/git/<host>/<path>` (global) or `.scramjet/git/<host>/<path>` (project).
- Runs `npm install` after clone or pull if `package.json` exists.

**SSH examples:**
```bash
# git@host:path shorthand (requires git: prefix)
scramjet install git:git@github.com:user/repo

# ssh:// protocol format
scramjet install ssh://git@github.com/user/repo

# With version ref
scramjet install git:git@github.com:user/repo@v1.0.0
```

### Local Paths

```
/absolute/path/to/package
./relative/path/to/package
```

Local paths point to files or directories on disk and are added to settings without copying. Relative paths are resolved against the settings file they appear in. If the path is a file, it loads as a single extension. If it is a directory, scramjet loads resources using package rules.

## Creating a Scramjet Package

Add a `pi` manifest to `package.json` or use conventional directories. Include the `pi-package` keyword for discoverability (existing ecosystem packages use this keyword, so it is retained for continuity).

```json
{
  "name": "my-package",
  "keywords": ["pi-package"],
  "pi": {
    "extensions": ["./extensions"],
    "skills": ["./skills"],
    "prompts": ["./prompts"],
    "themes": ["./themes"]
  }
}
```

Paths are relative to the package root. Arrays support glob patterns and `!exclusions`.

### Package Metadata

Add `video` or `image` fields to show a preview in package listings:

```json
{
  "name": "my-package",
  "keywords": ["pi-package"],
  "pi": {
    "extensions": ["./extensions"],
    "video": "https://example.com/demo.mp4",
    "image": "https://example.com/screenshot.png"
  }
}
```

- **video**: MP4 only. On desktop, autoplays on hover. Clicking opens a fullscreen player.
- **image**: PNG, JPEG, GIF, or WebP. Displayed as a static preview.

If both are set, video takes precedence.

## Package Structure

### Convention Directories

If no `pi` manifest is present, scramjet auto-discovers resources from these directories:

- `extensions/` loads `.ts` and `.js` files
- `skills/` recursively finds `SKILL.md` folders and loads top-level `.md` files as skills
- `prompts/` loads `.md` files
- `themes/` loads `.json` files

## Dependencies

Third party runtime dependencies belong in `dependencies` in `package.json`. Dependencies that do not register extensions, skills, prompt templates, or themes also belong in `dependencies`. When scramjet installs a package from npm or git, it runs `npm install`, so those dependencies are installed automatically.

Scramjet bundles core packages for extensions and skills. If you import any of these, list them in `peerDependencies` with a `"*"` range and do not bundle them: `@leanandmean/ai`, `@leanandmean/agent`, `@leanandmean/coding-agent`, `@leanandmean/tui`, `typebox`.

Other scramjet packages must be bundled in your tarball. Add them to `dependencies` and `bundledDependencies`, then reference their resources through `node_modules/` paths. Scramjet loads packages with separate module roots, so separate installs do not collide or share modules.

Example:

```json
{
  "dependencies": {
    "shitty-extensions": "^1.0.1"
  },
  "bundledDependencies": ["shitty-extensions"],
  "pi": {
    "extensions": ["extensions", "node_modules/shitty-extensions/extensions"],
    "skills": ["skills", "node_modules/shitty-extensions/skills"]
  }
}
```

## Package Filtering

Filter what a package loads using the object form in settings:

```json
{
  "packages": [
    "npm:simple-pkg",
    {
      "source": "npm:my-package",
      "extensions": ["extensions/*.ts", "!extensions/legacy.ts"],
      "skills": [],
      "prompts": ["prompts/review.md"],
      "themes": ["+themes/legacy.json"]
    }
  ]
}
```

`+path` and `-path` are exact paths relative to the package root.

- Omit a key to load all of that type.
- Use `[]` to load none of that type.
- `!pattern` excludes matches.
- `+path` force-includes an exact path.
- `-path` force-excludes an exact path.
- Filters layer on top of the manifest. They narrow down what is already allowed.

## Enable and Disable Resources

Use `scramjet config` to enable or disable extensions, skills, prompt templates, and themes from installed packages and local directories. Works for both global (`~/.scramjet/agent`) and project (`.scramjet/`) scopes.

## Scope and Deduplication

Packages can appear in both global and project settings. If the same package appears in both, the project entry wins. Identity is determined by:

- npm: package name
- git: repository URL without ref
- local: resolved absolute path
