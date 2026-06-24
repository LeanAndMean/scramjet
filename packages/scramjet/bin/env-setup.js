// Side-effect module imported FIRST from bin/scramjet.js.
// Sets env vars and process metadata before the coding-agent main() runs.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const pkg = JSON.parse(readFileSync(join(__dirname, "..", "package.json"), "utf-8"));
process.env.SCRAMJET_PACKAGE_NAME = pkg.name;
process.env.SCRAMJET_VERSION = pkg.version;

// Honor legacy PI_-prefixed env vars for users with existing shell profiles.
// With piConfig.name = "scramjet" in coding-agent's package.json, the runtime
// reads SCRAMJET_CODING_AGENT_DIR / SCRAMJET_CODING_AGENT_SESSION_DIR.
if (process.env.PI_CODING_AGENT_DIR && !process.env.SCRAMJET_CODING_AGENT_DIR) {
	process.env.SCRAMJET_CODING_AGENT_DIR = process.env.PI_CODING_AGENT_DIR;
}
if (process.env.PI_CODING_AGENT_SESSION_DIR && !process.env.SCRAMJET_CODING_AGENT_SESSION_DIR) {
	process.env.SCRAMJET_CODING_AGENT_SESSION_DIR = process.env.PI_CODING_AGENT_SESSION_DIR;
}

// Point the changelog at Scramjet's own CHANGELOG.md.
process.env.SCRAMJET_CHANGELOG_PATH = join(__dirname, "..", "CHANGELOG.md");

process.env.PI_SKIP_VERSION_CHECK = "1";
process.title = "scramjet";
