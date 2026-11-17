# Command sets

[Back to the README](../../README.md)

## What a command set is

A command set is a directory of commands (`commands/<name>.md`), agents (`agents/<name>.md`) and skills (`skills/<name>/SKILL.md`, as a Claude Code plugin's), named `<set>:<name>` after its `set.yaml`'s `name` (else the directory's); Scramjet 1's `<set>:<name>.md` files work too. `scramjet list` shows the sets and skills loaded here, with any problems, and `scramjet lint --strict DIR` checks a set before you use it.

One comes with the program, as Scramjet 1's `scramjet` set does: `scramjet`, with `/scramjet:troubleshoot`, `/scramjet:bug` (typed as `/bug`, `/feedback` or `/report-issue`: it drafts an anonymized issue about Scramjet for `LeanAndMean/scramjet` and shows it whole before it is posted), agents for writing and reviewing commands, the skills `writing-scramjet-commands`, `setting-up-command-sets` (a new set's repository: its layout, and its versioning, Claude Code compatibility and license recorded in its `CLAUDE.md`) and `reading-past-sessions` (where Scramjet, Scramjet 1, Pi, dreb, Claude Code, Codex, OpenCode, Cline and Posit Assistant keep a project's sessions, and the fields that matter) (a release writes its files, and the README the agent reads about Scramjet, under `~/.local/share/scramjet/builtin/`; a set you install under the name `scramjet` replaces it).

## Installing them

```sh
scramjet install git:github.com/LeanAndMean/mach12           # Mach 12's command sets, from git
scramjet install git:github.com/USER/REPO                     # the sets in a git repository
scramjet install ~/work/my-set                               # a command set, by reference
scramjet install ~/repos/scramjet/packages/scramjet          # a package: its sets, skills and themes
scramjet update                                              # pull the packages not pinned to a ref, then update the program
scramjet remove git:github.com/USER/REPO                     # delete a package's clone
```

Install one by reference with `scramjet install DIR` (when DIR is not a set, the sets it holds, found as in a git package below; written with `~` for your home directory, so the settings serve another machine), or those of a git repository with `scramjet install git:github.com/USER/REPO[@REF]` (or `git:git@github.com:USER/REPO`, or an `https://`, `ssh://`, `git://` or `file://` URL): it is cloned by `git`, with your SSH configuration and credential helpers, into `~/.local/share/scramjet/git/`, and its sets are its top directory when that is a set, else the sets at its top and, deeper, those with a `set.yaml`. A session's start clones a package not cloned yet (not with `--offline` or `SCRAMJET_OFFLINE` set). A ref pins it; `scramjet update` pulls the others, and `scramjet remove REPO` deletes the clone.

A package builds nothing: `install` names the programs it runs (`providersCommand`), and takes a program its `models.json` names in `release` (`{"repository": "OWNER/REPO", "program": "bin/tool"}`) from that GitHub repository's release of the clone's newest `vX.Y.Z` tag, through `gh`: the archive `tool-X.Y.Z-ARCH-OS.tar.gz` (as `update self`'s, e.g. `x86_64-linux`, holding `tool`, or `tool.exe` on Windows), checked against the release's `SHA256SUMS`, unpacked to `bin/tool`; `update` takes a newer tag's.

## Running commands

`/<set>:<command> args` runs a command. Each run, and each `delegate` of it, reads the command's file as it is then, so an edit takes effect without `/reload`; a new command needs `/reload`. A command runs until the agent reports it `completed` or you agree to abandon it.

When it finishes, it may recommend next steps: you choose one (Left/Right picks the model it runs with, Shift+Tab the effort, Tab whether it starts a fresh session, marked `(fresh session)`), or an edge set in `/settings` (Next steps) continues automatically, and `/autopilot on` takes the recommended one after 3 seconds. The publication tools (issues, pull requests and comments on GitHub, through `gh`) show the exact payload for your approval unless the command's policy allows them.
