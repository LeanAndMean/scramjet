---
description: Locate and surface the project's contribution and release guidance
delegate-only: true
allowed-tools:
  - bash
  - read
  - grep
  - glob
---

# Find Contribution Guidelines

## Goals

- Give the caller a concise, source-grounded summary of all applicable contribution and release guidance.
- Preserve documented requirements, fallback evidence, conflicts, and meaningful omissions without inventing policy or making decisions for the caller.

## Step 1: Locate authoritative guidance

Inspect all applicable conventional contribution guidance and repository-local release instructions, rather than stopping after the first file found. Include release material directly referenced by those files.

Keep discovery bounded to conventional guidance locations and their direct references. Do not recursively crawl documentation, infer policy from release history, or search for a repository-specific filename inventory.

Treat contribution and release guidance as primary authority. Only when that authority is absent or leaves a material detail unspecified, inspect nearby project scripts, commands, manifests, and tracked metadata for evidence. Evidence can reveal a missing detail but must not silently override guidance or invent policy.

If neither guidance nor fallback evidence exists, return that no applicable guidance was found.

## Step 2: Extract relevant requirements

Read every located authority and identify:

- **Project layers**: conceptual layers relevant to planning and implementation coverage.
- **Testing expectations**: frameworks, required test types, coverage rules, and verification commands.
- **Pre-merge and release requirements**: version declarations and mirrors, generation or synchronization commands, consistency checks, changelog and documentation updates, and lint or format gates.
- **Development tooling**: documented build, lint, format, schema or artifact validation, generation, compiler/typecheck, and repository-specific verification commands. When the evidence establishes it, report each tool's authority, required or advisory relevance, and mutating or non-mutating effects.
- **Issue conventions**: templates, label taxonomy, required fields, and shaping rules.
- **Other guidance**: anything else material to the caller's planning, implementation, review, or release work.

Record each material requirement with its source path. Surface conflicting instructions and details that remain missing after bounded fallback investigation; do not select one source silently or guess. Omit categories for which no requirement was found.

## Step 3: Return the summary

Return a concise summary organized under the headings above. Distinguish authoritative requirements from fallback evidence, list the inspected source paths, and identify conflicts or missing details that the caller must resolve.

Keep the delegate read-only and non-interactive. Do not choose versions, mutate files, run release steps, or ask the user questions; the caller owns those decisions and actions.

If a source is short and paraphrasing would lose nuance, quote it directly.
