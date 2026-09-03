#!/usr/bin/env node
import { runCommandLint } from "../dist/command-lint-cli.js";

process.exitCode = runCommandLint(process.argv.slice(2));
