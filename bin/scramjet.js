#!/usr/bin/env node
import { main } from "@earendil-works/pi-coding-agent";
import scramjetExtension from "../dist/index.js";

// Suppress Pi's "new Pi version available" startup banner. Scramjet pins Pi
// at pi.piTestedVersion in package.json; `pi update` would not update the
// embedded copy, so the prompt would be misleading.
process.env.PI_SKIP_VERSION_CHECK = "1";
process.title = "scramjet";

await main(process.argv.slice(2), { extensionFactories: [scramjetExtension] });
