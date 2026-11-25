# Compatibility

[Back to the README](../../README.md)

An update keeps reading the files you have. When a version changes a setting, a key binding's id, a file's format or a command set's format, it goes on reading the old form, or converts it, rather than refusing it. Features and designs keep changing from release to release; your files are what stays usable.

This holds from the first stable release on. The alphas before it kept most old forms too, but not by promise.

## What it covers

- **Settings** (`settings.json`): a key that is renamed keeps its old name, and a value it no longer takes keeps its meaning. Scramjet already reads several older forms: `retry.maxDelayMs` for `retry.provider.maxRetryDelayMs`, `sandbox` as a mode's name with `sandboxNetwork` beside it (the form before `sandbox` became an object), and Claude Code's `terminalProgressBarEnabled`.
- **Providers and logins:** `models.json`, yours and packages', and `auth.json`.
- **Key bindings** (`keybindings.json`): an action that is renamed keeps its old id.
- **Themes:** a theme that was complete stays complete. A color added later takes a value of its own in your theme until you set one.
- **Next steps and approvals:** `autonomy.yaml`, global and in a project.
- **Sessions:** every session file a version wrote, with its chain, can be resumed, forked and exported by later ones.
- **Command sets:** `set.yaml`, commands, agents, skills and `autonomy-defaults.yaml`, and packages' layout.

What a version cannot use in a file is warned of and passed over, and the rest of the file applies; the warning names the file and the key. Two files are used whole or not at all when one holds a mistake: `autonomy.yaml` (as Scramjet 1 reads it: both read the same file) and a theme lacking a color. A later version never turns a file that was right into one of those.

## What it does not cover

- **Going back** to an older version: a file a newer one wrote may hold what the older one warns of and passes over.
- **What you see:** the screen's looks, bottoms and layout, the messages, `--help`, and the output of `-p`, `--list-models` and the other commands. A setting you never set follows the new version's default.
- **Scramjet's own records** in the state folder (`~/.local/state/scramjet`): model lists, catalogs' copies, usage records, the release check, the model each project used last, unsent drafts. A version may start one afresh. Sessions there are covered (above).
- **Options and built-in commands:** their names and arguments are not part of the promise.
- **ACP:** what `scramjet acp` speaks is the [Agent Client Protocol](https://agentclientprotocol.com), versioned by ACP itself; Scramjet's extensions to it are not covered.
- **Scramjet 1's files:** `scramjet import` copies them once ([migration](migration.md)); Scramjet 1's formats are not followed after that, but for `autonomy.yaml`, which both read.

## How it is kept

[`tests/compatibility`](https://github.com/LeanAndMean/scramjet/tree/main/tests/compatibility) holds a version's files as that version wrote them or a user had them: settings, `models.json`, `auth.json`, key bindings, a theme, both `autonomy.yaml` files, a session with commands, tool calls, a name and a compaction, and a command set. Every build's tests read them all: with no warning, each setting still one the program takes, the session's conversation sent on, the key, prices and limits applied, the set linting clean. A version that changes one of those formats adds its own files beside the old ones, which stay. The other files listed above are kept by the same rule, without files of their own yet.

An update that stops reading a file you had is a bug: report it with `/bug`, or at [the issues](https://github.com/LeanAndMean/scramjet/issues), with the warning or what changed.
