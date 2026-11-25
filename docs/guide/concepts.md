# Concepts, and a first command set

[Back to the README](../../README.md)

What you keep asking the agent for can become a command, and commands can lead into one another as a workflow. This page says what each part is, then builds a small set of your own: a command that reviews your uncommitted changes with a second agent and fixes what holds, offering a commit next. [Command sets](command-sets.md) has installing and the reference detail; the built-in `writing-scramjet-commands` skill has the full format, for the agent writing commands with you.

## Commands

A command is a Markdown file of instructions, run as `/<set>:<name> arguments`. Its frontmatter gives a `description` and an `argument-hint`, shown in the `/` list; the body is sent to the agent with the arguments put in place of `$ARGUMENTS` (or `$1`, `$2`…). The agent receives it as an active task, the arguments marked as yours (`<user-context>`), so state what the command must achieve under `## Goals` and let the agent work out how from the project in front of it.

A command runs until the agent reports it `completed`, with a summary, or `abandoned`, which you confirm. A reply that stops without either gets a status check asking it to go on, ask or finish; what only you can decide it asks with a question that waits for your answer. Each run reads the file as it is then, so an edit takes effect at the next run.

## Command sets

A command set is a directory named by its `set.yaml`:

```text
ship/
  set.yaml               name: ship, and a description
  commands/review.md     /ship:review
  commands/commit.md     /ship:commit
  agents/reviewer.md     the agent ship:reviewer
  skills/NAME/SKILL.md   skills it brings
  autonomy-defaults.yaml next steps and publications it suggests
```

Scramjet finds sets in three places: the project's `.scramjet/` directory (`.scramjet/ship/`, shared with everyone who clones the repository), the ones you install (`scramjet install`, from a directory or a git repository), and the `scramjet` set that comes with the program. The layout is a Claude Code plugin's, so one repository can serve both. `scramjet list` shows what is loaded and from where; `scramjet lint --strict DIR` checks a set.

## Agents

An agent is a file in a set's `agents/` (or your own `~/.config/scramjet/agents/`) whose body is the instructions for a subagent: a separate conversation the main agent hands one question to, getting back only the answer. Use one for a fresh look the main conversation would bias, for reading a lot to report a little, or for work in parallel. Its frontmatter gives its `name`, a `description` the main agent picks it by, its `tools` in Claude Code's names (`Read, Grep, Glob, Bash`), and optionally a `model` to run on, such as a cheaper one. Subagents are read-only: their commands run in a sandbox with every file read-only and no network, unless `sandbox.subagents` lets them write ([subagents](configuration.md#subagents)). Agents from the project's own directory, a set in `.scramjet/` included, run only after you approve them once for that project, asked again when one of their files changes.

## Skills

A skill is a directory with a `SKILL.md` whose frontmatter has a `name` and a `description`. The agent sees the descriptions and reads a skill when its task matches; `/skill:NAME` loads one now. Skills come from sets, the project's `.scramjet/skills` and `.agents/skills`, and your `~/.config/scramjet/skills` and `~/.agents/skills`. A skill is reference knowledge; a command is a task to carry out.

## Next steps

A command's `next` frontmatter says what may follow it. When the command completes, the agent offers the concrete steps that fit what it found, with a reason each, and recommends one:

| `mode` | What may follow |
|---|---|
| `forced` | Its `target` always runs next. |
| `closed` | Only its `candidates`. |
| `open` | Any command, its `candidates` as suggestions, never those in its `blacklist`; or a message, which is put in the prompt for you. |
| `ask` | Nothing is offered: the chain waits for you. |

A command with no `next` ends the chain. The steps show as a list, the recommended one first: Enter runs one, Left and Right pick the model it runs on, Shift+Tab the effort, Tab whether it starts in a fresh session (a clean context, which suits a large step), and Escape takes none.

Each step from one command to another can be set to `chain` (run at once, without the list) or `pause` (always offer the list), in `/settings` → Next steps or in `autonomy.yaml`: the project's `.scramjet/autonomy.yaml`, then your `~/.config/scramjet/autonomy.yaml`. A set's `autonomy-defaults.yaml` suggests settings, which hold once you apply them there. With neither, `/autopilot on` runs the recommended step after 3 seconds (off by default, kept per session). In `-p`, where nobody chooses, only a `chain` step or autopilot goes on.

## Delegation

A command can run another as a subroutine with the `delegate` tool: the other command's instructions are loaded into the same conversation, and the caller carries them out and stays responsible for its own report. A command used only that way says `delegate-only: true` and is left out of the `/` list and of next steps. Delegating the same command with the same arguments twice in a turn is refused, so two commands cannot loop on each other.

## Publications

The tools that post to GitHub (`create_issue`, `create_pr`, `add_issue_comment`, `add_pr_comment`, through `gh`) show you the exact text before posting: Approve publication posts it, Cancel does not. A command that lists one of them in its `allowed-tools` can be let to post with it without asking, in `/settings` or the `publications` of your `~/.config/scramjet/autonomy.yaml`; its set's `autonomy-defaults.yaml` may suggest that.

## A first command set

This builds `ship`, a set in a project with two commands and one agent. Work in a Git repository; the agent's changes are undone with Git.

### 1. The set

```sh
mkdir -p .scramjet/ship/commands .scramjet/ship/agents
```

`.scramjet/ship/set.yaml`:

```yaml
name: ship
description: Review the uncommitted changes, then commit them.
```

### 2. The reviewer agent

`.scramjet/ship/agents/reviewer.md`:

```markdown
---
name: reviewer
description: Reviews a change it did not write, with fresh eyes, and reports only real defects.
tools: Read, Grep, Glob, Bash
---
You review a change someone else made. Run `git diff HEAD` and read what it touches.
Report each defect that would make the code wrong (a bug, a missed case, a broken
test), with its file and line and why. Leave out style and taste. If you find none,
say so.
```

A reviewer that did not write the change sees it as a reader will. It is read-only, so it reports and the main agent fixes.

### 3. The review command

`.scramjet/ship/commands/review.md`:

```markdown
---
description: Review the uncommitted changes and fix what matters
argument-hint: "[what to look at most]"
next:
  mode: closed
  candidates:
    - name: ship:commit
      hint: the changes are ready
    - name: ship:review
      hint: the fixes need another look
---
# Review the changes

## Goals

- The uncommitted changes hold no defect the user would want fixed before committing.

## Focus

<user-context>
$ARGUMENTS
</user-context>

## Review

Ask the `ship:reviewer` agent to review the changes, with any focus given above.
Check each finding against the code yourself: fix those that hold, and tell the
user which you set aside and why.
```

The goal says when the work is done; the review section says only what the agent could not know: that this set has a reviewer, and that its findings are claims to check. `closed` lets it offer only a commit or another review.

### 4. The commit command

`.scramjet/ship/commands/commit.md`:

```markdown
---
description: Commit the changes with a message in the project's style
argument-hint: "[what the change is for]"
---
# Commit the changes

## Goals

- The changes are committed, with a message in the style of the project's recent commits.

## What it is for

<user-context>
$ARGUMENTS
</user-context>

## Commit

If the changes were not reviewed earlier in this conversation, delegate `ship:review`
first. Show the user the message and the files it covers, and commit once they agree.
```

It has no `next`, so the chain ends here. Run by itself, or in a fresh session, it delegates the review first.

### 5. Check it

```sh
scramjet lint --strict .scramjet/ship
scramjet list
```

The lint should find 0 errors and 0 warnings, and `scramjet list` shows `ship  project  …/.scramjet/ship  2 commands, 1 agent`. In a session that was already running, `/reload` loads the new set.

### 6. Run it

With some uncommitted changes, type:

```text
/ship:review the empty case
```

The agent hands the review to `ship:reviewer`. The first time, Scramjet asks **Run project-local agents?**, since files in a repository chose them: Yes is remembered for this project until one of the agent files changes. The reviewer's commands and answer show in its box (Ctrl+O shows more); the agent checks each finding, fixes those that hold and reports. Then the list offers `/ship:commit` with the arguments it chose, recommended, perhaps marked `(fresh session)`, and `/ship:review`. Enter runs the commit: it shows the message and asks before committing.

### 7. Let it go on by itself

To have the commit follow a review without the list, set that step to `chain`, in `/settings` → Next steps or in `.scramjet/autonomy.yaml`:

```yaml
edges:
  ship:review:
    ship:commit: chain
```

The commit still asks before committing: its instructions say so, whatever starts it.

### 8. Take it further

Ask the agent to change a command with you: the `writing-scramjet-commands` skill tells it how commands are best written (outcomes, not procedures) and to lint what it writes. To use the set in every project, move it to a repository of its own and `scramjet install` it; the `setting-up-command-sets` skill sets up such a repository, with its versioning and Claude Code compatibility. [Mach 12](https://github.com/LeanAndMean/mach12) is a larger set to read: from an issue to a merged pull request, with plans, reviews and their assessments.
