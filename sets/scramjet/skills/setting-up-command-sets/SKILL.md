---
name: setting-up-command-sets
description: Use when starting a repository for a Scramjet command set, or when deciding or recording how an existing one is versioned, licensed or kept loadable by Claude Code.
---

# Setting Up a Command-Set Repository

A command-set repository is read by two kinds of participants: the harnesses that load it, and the agents that develop it. The layout serves the first; the repository's `CLAUDE.md` tells the second what the author decided, so that an agent working there bumps, tags and records changes as the author wants without asking each time.

## Layout

At the repository's top (or one directory per set, each with its own `set.yaml`):

- `set.yaml`: `name` (the `<set>` of `/<set>:<command>`) and a `description`. Without it the set takes its directory's name, which a clone can change.
- `commands/<name>.md`, `agents/<name>.md`, `skills/<name>/SKILL.md`: bare names, no `<set>:` prefix and no `:` anywhere, so Windows can check the files out. The format is in the `writing-scramjet-commands` skill.
- `autonomy-defaults.yaml`, optional: the set's defaults for next-step edges and publications.
- `.claude-plugin/plugin.json`, optional: makes the same directory a Claude Code plugin (below).
- `README.md` for users: what the set does, how to install it (`scramjet install git:github.com/OWNER/REPO`), its commands. `LICENSE`, and `CHANGELOG.md` when versioned.
- `CLAUDE.md` for the agents developing the set: the decisions below and what each means for their work.

Run `scramjet lint --strict <repository>` once the layout is there and after each change.

## Decisions the author owns

Ask the author each of these; do not choose for them. Give what each option means for their users and for the agents working in the repository, and a recommendation when the author's context supports one.

### Versioning

Scramjet 2 reads no set version: a git install follows the branch, and `@REF` (`git:github.com/OWNER/REPO@v1.2.0`) pins a tag or commit. Claude Code keeps a marketplace install at `plugin.json`'s `version` until that changes, and with no `version` takes each commit. So:

- **SemVer releases:** `vX.Y.Z` tags, a `CHANGELOG.md`, and `plugin.json`'s `version` bumped with each release. Users can pin a release while a regression is fixed. Costs a release step and a judgment of what breaks.
- **Plain commits:** no tags, no changelog, no `version` in `plugin.json`. Every commit reaches users; there is nothing to pin but a commit hash.

For a command set, a major version is a change that breaks how someone uses it: a command or agent renamed or removed, arguments changed, an artifact another command or user reads changed in shape, a next-step route users rely on removed. Minor is a new command, agent, skill or behavior; patch a fix that changes nothing a user relies on.

### Claude Code compatibility

Whether the set should also load as a Claude Code plugin (`claude --plugin-dir <repository>` to try it). If so: `.claude-plugin/plugin.json` with the same `name` as `set.yaml` (or its commands run under another prefix there), `description`, `author`, `license`, and tool names in Claude Code's spelling (`Bash`, `Read`) in `allowed-tools` and `tools`. Scramjet-only features (`next`, `delegate`, the publication tools, `autonomy-defaults.yaml`) have no counterpart in Claude Code; say in the README what works there. For a marketplace, `claude plugin tag` makes `<name>--v<version>` tags.

### License

Which license, if any: a `LICENSE` file and `plugin.json`'s `license` naming it. Without one, others have no stated right to reuse it.

## Recording them

Write a `CLAUDE.md` with a section per decision, saying what to do, not only what was chosen. `scramjet lint` looks for a `## Versioning` heading in the repository's `CLAUDE.md` (or `AGENTS.md`) when `plugin.json` has a `version`, and warns when the repository has `vX.Y.Z` tags but `plugin.json` gives no version. For SemVer, for example:

```md
## Versioning

SemVer, from v0.1.0. Between releases, each change adds a line under `## Unreleased` in `CHANGELOG.md` (newest first: Added, Changed, Fixed). A release, when the author asks for one: rename that section `## X.Y.Z — YYYY-MM-DD`, set `version` in `.claude-plugin/plugin.json` to X.Y.Z, commit, and tag the commit `vX.Y.Z`. Major for a change that breaks how the set is used (a command, agent or argument renamed or removed, an artifact's shape changed), minor for something new, patch for a fix. Pushing commits and tags is the author's to approve.
```

For plain commits: `## Versioning` saying there are no versions, so that no agent adds a `version`, tags or a changelog.

Keep `CLAUDE.md` to what an agent needs to work there; the README is for users.
