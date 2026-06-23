#!/usr/bin/env node
import "./env-setup.js";
import { main } from "@scramjet/coding-agent";
import { initScramjet } from "../dist/index.js";

await main(process.argv.slice(2), { builtinInit: initScramjet });
