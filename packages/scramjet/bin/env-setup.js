// Side-effect module imported FIRST from bin/scramjet.js.
// Sets env vars and process metadata before the coding-agent main() runs.

// Honor legacy PI_-prefixed env vars for users with existing shell profiles.
// With piConfig.name = "scramjet" in coding-agent's package.json, the runtime
// reads SCRAMJET_CODING_AGENT_DIR / SCRAMJET_CODING_AGENT_SESSION_DIR.
if (process.env.PI_CODING_AGENT_DIR && !process.env.SCRAMJET_CODING_AGENT_DIR) {
	process.env.SCRAMJET_CODING_AGENT_DIR = process.env.PI_CODING_AGENT_DIR;
}
if (process.env.PI_CODING_AGENT_SESSION_DIR && !process.env.SCRAMJET_CODING_AGENT_SESSION_DIR) {
	process.env.SCRAMJET_CODING_AGENT_SESSION_DIR = process.env.PI_CODING_AGENT_SESSION_DIR;
}

process.env.PI_SKIP_VERSION_CHECK = "1";
process.title = "scramjet";
