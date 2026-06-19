---
description: Locate and surface the project's contribution guidelines for planning context
allowed-tools:
  - bash
  - read
  - grep
  - glob
---

<scramjet-command name="mach12:find-contribution-guidelines">

# Find Contribution Guidelines

You are locating the project's contribution guidelines and surfacing planning-relevant guidance to the caller.

## Step 1: Locate the guidelines file

Check these paths in priority order and use the first one that exists:

1. `CONTRIBUTING.md` (repo root)
2. `DEVELOPMENT.md` (repo root)
3. `.github/CONTRIBUTING.md`

If none of these files exist, return: "No contribution guidelines were found." and stop.

## Step 2: Extract planning-relevant sections

Read the located file. Identify and surface:

- **Project layers**: what conceptual layers the codebase is organized into (e.g., models, migrations, API routes, services, UI, documentation). The plan-shaped lens cares about coverage across these layers.
- **Testing expectations**: test frameworks, coverage requirements, test types (unit / integration / end-to-end), and any testing-related rules.
- **Pre-merge requirements**: version bumps, changelog entries, documentation updates, lint/format gates, anything the project expects before a PR can merge.
- **Issue conventions**: issue templates, label taxonomy, required fields, any other shaping rules for new issues.
- **Other guidance**: anything else relevant to planning, implementation, or PR shaping that the caller should know about.

If a section is absent from the guidelines file, omit it from the output -- do not invent guidance.

## Step 3: Return the summary

Produce a concise summary organized under the headings above. The caller reads this summary and applies the guidance to whichever step is invoking the subroutine (planning, review, implementation, pre-merge checklist, etc.).

If the guidelines file is short and a paraphrase would lose nuance, quote it directly.

</scramjet-command>
