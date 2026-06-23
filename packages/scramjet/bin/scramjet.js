#!/usr/bin/env node
import "./env-setup.js";
import { main } from "@scramjet/coding-agent";
import scramjetExtension from "../dist/index.js";

await main(process.argv.slice(2), { extensionFactories: [scramjetExtension] });
