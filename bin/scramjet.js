#!/usr/bin/env node
import { main } from "@earendil-works/pi-coding-agent";
import scramjetExtension from "../dist/index.js";

process.title = "scramjet";
await main(process.argv.slice(2), { extensionFactories: [scramjetExtension] });
