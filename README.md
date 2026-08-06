<p align="center">
  <img src="packages/scramjet/assets/scramjet-logo.png" alt="Scramjet logo" width="600">
</p>

# Scramjet

Turn recurring ways of working with coding agents into reusable commands, then connect those commands into guided workflows without giving up control.

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

> [!IMPORTANT]
> Scramjet is in active early development and is dogfooded daily. The third-party command-set authoring format is not yet stable, breaking changes may land between minor releases, and the bundled command sets continue to evolve. See the [current status](packages/scramjet/README.md#status) for details.

## From repeated prompts to durable workflows

Coding-agent work often accumulates coordination overhead: restating refined instructions, copying shared process steps between prompts, deciding what comes next, clearing overloaded context, and reconstructing decisions in a later session.

Scramjet reduces that friction:

- **Reusable commands** capture recurring instructions once instead of refining the same prompt repeatedly.
- **Composable subroutines** keep shared process fragments in one maintained definition instead of copies that drift.
- **Declared, validated next steps** turn likely follow-ups into guided transitions rather than manual command handoffs.
- **Fresh sessions** can give substantial steps a bounded context window.
- **Durable GitHub artifacts** carry plans, decisions, reviews, assessments, and implementation evidence across agents and sessions.

Scramjet does not impose a central workflow graph or background job system. Each command owns its possible next steps. When a transition offers choices, users can select another route or dismiss it; deterministic forced transitions follow the command's declared contract.

## A representative issue-to-ship path

```text
issue → plan → review → implement → PR → review / validation → fixes → pre-merge → ship
```

This is a representative path from the bundled Mach 12 command set, not a mandatory linear pipeline. Steps can continue in fresh sessions while GitHub issues, pull requests, and top-level discussion artifacts provide durable handoffs. The user retains control over offered choices and can dismiss or redirect choice-bearing transitions; “ship” means completing the selected delivery path, not that every merge necessarily creates a release.

See the [detailed workflow example](packages/scramjet/README.md#why) and [current command inventory](packages/scramjet/README.md#mach-12).

## Pi, Scramjet, and Mach 12

| Layer | Role |
| --- | --- |
| **Pi** | The vendored coding-agent runtime: models, tools, sessions, agent loop, terminal UI, and CLI foundations. |
| **Scramjet** | The product harness: discovers command sets and supports reusable composition and guided continuation. |
| **Mach 12** | An optional bundled issue-to-ship methodology and concrete demonstration of what Scramjet enables. |

Mach 12 is not required infrastructure. User-authored command sets can coexist with the bundled Mach 12 and Scramjet operational sets.

## Dogfooding evidence

The repository itself is Scramjet's primary dogfooding environment. A historical snapshot taken **2026-08-04** recorded one contributor, 413 default-branch commits, 157 merged pull requests, 229 closed issues, and 146 releases since the repository was created on 2026-05-12.

Representative merged work includes:

| Pull request | Scope | Scale |
| --- | --- | ---: |
| [#216 — Replace lifecycle phase machine with event-reactive fact-based design](https://github.com/LeanAndMean/scramjet/pull/216) | Lifecycle redesign with invariant, replay, documentation, and test changes | 4,470 changed lines across 30 files |
| [#246 — Rebuild model switching as a tool-driven workflow](https://github.com/LeanAndMean/scramjet/pull/246) | Runtime-to-product model switching, harness tool invocation, provider hardening, and tests | 3,457 changed lines across 30 files |
| [#396 — Prevent TUI transcript jumps on response completion](https://github.com/LeanAndMean/scramjet/pull/396) | Terminal rendering, transcript finality, image handling, compatibility, and regression coverage | 2,662 changed lines across 29 files |
| [#428 — Add executable PR validation to Mach 12](https://github.com/LeanAndMean/scramjet/pull/428) | Executable validation commands, safe handoffs, documentation, and contract tests | 1,533 changed lines across 17 files |

“Changed lines” means GitHub additions plus deletions. Each linked PR also preserves automated Mach 12 review, independent-assessment, fix, and verification records in its top-level discussion; these are inspectable workflow artifacts, not formal GitHub reviews or independent human approvals.

This snapshot demonstrates sustained same-repository dogfooding and makes the work inspectable. It is not a controlled productivity benchmark, does not isolate Scramjet's causal contribution, does not measure quality by activity or diff size, and does not demonstrate external adoption.

## Install

Requires Node >= 20.

```sh
npm install -g @leanandmean/scramjet
scramjet
```

`scramjet` is a standalone CLI that uses Pi as its runtime. See the [package README](packages/scramjet/README.md) for current usage, configuration, platform support, command inventory, and detailed behavior.

## Repository packages

| Package | Role |
| --- | --- |
| `packages/tui` | Terminal UI |
| `packages/ai` | LLM providers |
| `packages/agent` | Agent loop and state |
| `packages/coding-agent` | CLI, tools, sessions (Pi runtime entry point) |
| `packages/scramjet` | The product: commands, orchestration, distribution |

The four Pi runtime packages are vendored from a LeanAndMean fork of [upstream Pi](https://github.com/earendil-works/pi) and modified directly where needed. `packages/scramjet` is the primary published package (`@leanandmean/scramjet`); its `scramjet/` directory contains product operational commands, while `mach12/` contains the bundled development methodology.

## Development

Requires Node >= 20. See [`CLAUDE.md`](CLAUDE.md) for build commands, local development setup, iteration workflow, and formatting conventions.

## Documentation

- [`packages/scramjet/README.md`](packages/scramjet/README.md) — Current user-facing behavior, installation, and usage
- [`packages/scramjet/docs/scramjet-vision.md`](packages/scramjet/docs/scramjet-vision.md) — Product vision, capabilities, principles, and non-goals
- [`packages/scramjet/docs/command-authoring.md`](packages/scramjet/docs/command-authoring.md) — Command-set authoring reference
- [`CLAUDE.md`](CLAUDE.md) — Contributor workflow and repository conventions
- [`UPSTREAM_DIVERGENCE.md`](UPSTREAM_DIVERGENCE.md) — Pi fork tracking

## License

Apache-2.0 — see [`LICENSE`](LICENSE).
