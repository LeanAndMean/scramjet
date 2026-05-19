// Side-effect module imported FIRST from bin/scramjet.js. Builds a shim
// "Pi package directory" that lets Pi resolve its bundled assets (themes,
// prompt templates, examples) from Pi's own location while reading
// piConfig.name from scramjet. Sets PI_PACKAGE_DIR to the shim so Pi's
// TUI banner reads "scramjet vX.Y.Z" instead of "pi vA.B.C".
//
// Must run before the static `import { main } from
// "@earendil-works/pi-coding-agent"` resolves; ESM evaluates imports in
// source order, so listing this import first in bin/scramjet.js is enough.

import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scramjetRoot = fileURLToPath(new URL("..", import.meta.url));
const scramjetPkg = JSON.parse(readFileSync(join(scramjetRoot, "package.json"), "utf-8"));

// Pi's package.json restricts subpath access via `exports`, so we can't
// just resolve `@earendil-works/pi-coding-agent/package.json`. Resolve
// the main entry instead and walk up until we find the package's own
// package.json (recognized by its `name` field).
const piEntryUrl = import.meta.resolve("@earendil-works/pi-coding-agent");
let piRoot = dirname(fileURLToPath(piEntryUrl));
while (true) {
	const candidate = join(piRoot, "package.json");
	if (existsSync(candidate)) {
		const candidatePkg = JSON.parse(readFileSync(candidate, "utf-8"));
		if (candidatePkg.name === "@earendil-works/pi-coding-agent") {
			break;
		}
	}
	const parent = dirname(piRoot);
	if (parent === piRoot) {
		throw new Error("Could not locate @earendil-works/pi-coding-agent package root from " + piEntryUrl);
	}
	piRoot = parent;
}
const piPkg = JSON.parse(readFileSync(join(piRoot, "package.json"), "utf-8"));

// Cache the shim per (scramjet version, pi version) so upgrades to either
// side bust it automatically. The old version-keyed directories remain on
// disk as orphans; they are small (a package.json plus a handful of
// symlinks) and `rm -rf ~/.cache/scramjet` reclaims them whenever you care.
const cacheRoot = process.env.XDG_CACHE_HOME || join(homedir(), ".cache");
const shimDir = join(cacheRoot, "scramjet", `pi-shim-${scramjetPkg.version}-pi-${piPkg.version}`);

if (!existsSync(shimDir)) {
	const tmp = `${shimDir}.tmp-${process.pid}`;
	try {
		mkdirSync(tmp, { recursive: true });
		writeFileSync(
			join(tmp, "package.json"),
			`${JSON.stringify(
				{
					name: scramjetPkg.name,
					version: scramjetPkg.version,
					piConfig: { name: "scramjet" },
				},
				null,
				2,
			)}\n`,
		);
		// Pi-owned bundled assets — Pi resolves themes/prompts/examples
		// through these paths once PI_PACKAGE_DIR points at the shim.
		for (const subpath of ["dist", "examples"]) {
			const piPath = join(piRoot, subpath);
			if (existsSync(piPath)) {
				symlinkSync(piPath, join(tmp, subpath));
			}
		}
		// Scramjet-owned user-facing docs. `pi docs`, the changelog
		// notification, and the README hint all now reflect scramjet.
		for (const file of ["README.md", "CHANGELOG.md", "docs"]) {
			const src = join(scramjetRoot, file);
			if (existsSync(src)) {
				symlinkSync(src, join(tmp, file));
			}
		}
		renameSync(tmp, shimDir);
	} catch (err) {
		rmSync(tmp, { recursive: true, force: true });
		// Lost a race with a concurrent scramjet that built the shim first
		// — keep going if the directory now exists; otherwise surface the
		// original error so the user gets a real diagnostic.
		if (!existsSync(shimDir)) {
			throw err;
		}
	}
}

process.env.PI_PACKAGE_DIR = shimDir;

// With piConfig.name = "scramjet", Pi reads agent-dir / session-dir
// overrides from SCRAMJET_CODING_AGENT_DIR and
// SCRAMJET_CODING_AGENT_SESSION_DIR instead of the PI_-prefixed names.
// Honor the legacy variables so an existing PI_CODING_AGENT_DIR=... in
// a user's shell profile keeps working.
if (process.env.PI_CODING_AGENT_DIR && !process.env.SCRAMJET_CODING_AGENT_DIR) {
	process.env.SCRAMJET_CODING_AGENT_DIR = process.env.PI_CODING_AGENT_DIR;
}
if (process.env.PI_CODING_AGENT_SESSION_DIR && !process.env.SCRAMJET_CODING_AGENT_SESSION_DIR) {
	process.env.SCRAMJET_CODING_AGENT_SESSION_DIR = process.env.PI_CODING_AGENT_SESSION_DIR;
}

// Suppress Pi's "new Pi version available" startup banner. Scramjet pins
// Pi at pi.piTestedVersion in package.json; `pi update` would not update
// the embedded copy.
process.env.PI_SKIP_VERSION_CHECK = "1";
process.title = "scramjet";
