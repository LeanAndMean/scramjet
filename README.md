# Scramjet

A high-velocity harness for agentic development.

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

## Overview

Scramjet is a product built on the [Pi](https://github.com/earendil-works/pi-mono) runtime that loads command sets — directories of user-defined slash commands — and wires them into emergent workflows through declared next-step policies and command delegation. It ships with a product-owned Scramjet operational set and Mach 12, a methodology for issue → plan → review → implement → PR → ship.

Active early development. Used daily, but the command-set format is not yet stable for third-party authoring. See the [package README](packages/scramjet/README.md) for install and usage.

## Packages

| Package | Role |
|---------|------|
| `packages/tui` | Terminal UI |
| `packages/ai` | LLM providers |
| `packages/agent` | Agent loop and state |
| `packages/coding-agent` | CLI, tools, sessions (Pi runtime entry point) |
| `packages/scramjet` | The product: commands, orchestration, distribution |

The four Pi runtime packages are vendored from the [LeanAndMean fork](https://github.com/LeanAndMean/pi-mono) and modified directly where needed. `packages/scramjet` is the primary published package (`@leanandmean/scramjet`); its `scramjet/` directory contains product operational commands, while `mach12/` contains the bundled development methodology.

## Development

Requires Node >= 20. See [`CLAUDE.md`](CLAUDE.md) for build commands, local development setup, iteration workflow, and formatting conventions.

## Documentation

- [`packages/scramjet/README.md`](packages/scramjet/README.md) — User-facing docs (install, usage, design philosophy)
- [`packages/scramjet/docs/scramjet-vision.md`](packages/scramjet/docs/scramjet-vision.md) — Design document and target architecture
- [`packages/scramjet/docs/command-authoring.md`](packages/scramjet/docs/command-authoring.md) — Command-set authoring reference
- [`CLAUDE.md`](CLAUDE.md) — Contributor workflow and repo conventions
- [`UPSTREAM_DIVERGENCE.md`](UPSTREAM_DIVERGENCE.md) — Pi fork tracking

## License

Apache-2.0 — see [`LICENSE`](LICENSE).
