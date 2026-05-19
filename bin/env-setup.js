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

// F17: raw JSON.parse on package.json crashes startup with a cryptic
// SyntaxError if the file is corrupt. npm manages these files so it's
// extraordinarily rare, but a clearer message saves an avoidable
// debugging session when it does happen.
function readPackageJson(path) {
	let content;
	try {
		content = readFileSync(path, "utf-8");
	} catch (err) {
		throw new Error(`[scramjet/env-setup] could not read ${path}: ${err.message}`);
	}
	try {
		return JSON.parse(content);
	} catch (err) {
		throw new Error(`[scramjet/env-setup] ${path} is not valid JSON: ${err.message}`);
	}
}

const scramjetRoot = fileURLToPath(new URL("..", import.meta.url));
const scramjetPkg = readPackageJson(join(scramjetRoot, "package.json"));

// Pi's package.json restricts subpath access via `exports`, so we can't
// just resolve `@earendil-works/pi-coding-agent/package.json`. Resolve
// the main entry instead and walk up until we find the package's own
// package.json (recognized by its `name` field).
const piEntryUrl = import.meta.resolve("@earendil-works/pi-coding-agent");
let piRoot = dirname(fileURLToPath(piEntryUrl));
while (true) {
	const candidate = join(piRoot, "package.json");
	if (existsSync(candidate)) {
		const candidatePkg = readPackageJson(candidate);
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
const piPkg = readPackageJson(join(piRoot, "package.json"));

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
				// S11: per-symlink try wrapper. Without this, a single bad
				// symlinkSync (e.g. EACCES on the cache dir, EEXIST against a
				// stray file) aborts the whole shim build with a stack that
				// doesn't name *which* link tripped it.
				try {
					symlinkSync(piPath, join(tmp, subpath));
				} catch (err) {
					throw new Error(
						`[scramjet/env-setup] could not symlink ${piPath} -> ${join(tmp, subpath)}: ${err.message}`,
					);
				}
			}
		}
		// Scramjet-owned user-facing docs. `pi docs`, the changelog
		// notification, and the README hint all now reflect scramjet.
		for (const file of ["README.md", "CHANGELOG.md", "docs"]) {
			const src = join(scramjetRoot, file);
			if (existsSync(src)) {
				try {
					symlinkSync(src, join(tmp, file));
				} catch (err) {
					throw new Error(
						`[scramjet/env-setup] could not symlink ${src} -> ${join(tmp, file)}: ${err.message}`,
					);
				}
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
		// F16: the race-recovery path used to be silent, which makes
		// debugging concurrent-scramjet weirdness harder than it needs to
		// be. Log once so the recovered race is visible in stderr.
		console.warn(
			`[scramjet/env-setup] shim build at ${shimDir} lost a race with a concurrent scramjet; recovered (${err.message})`,
		);
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
