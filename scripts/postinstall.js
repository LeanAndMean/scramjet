#!/usr/bin/env node
// Seeds the bundled Mach 12 command set into the user data dir on first install.
// Idempotent: skips silently if the destination already exists, so user edits
// to commands or agents are never overwritten. Atomic: copies to a temp path
// and renames into place, so a partial copy can never poison the next run.
// Never blocks `npm install` — any failure prints a warning and exits 0.

import { cpSync, existsSync, mkdirSync, renameSync, rmSync } from "node:fs";
import { homedir, platform } from "node:os";
import { isAbsolute, join } from "node:path";
import { fileURLToPath } from "node:url";

if (platform() === "win32") {
	console.warn("[scramjet] Mach 12 seeding skipped on native Windows.");
	console.warn("[scramjet] Install inside WSL for full functionality.");
	process.exit(0);
}

const pkgRoot = fileURLToPath(new URL("..", import.meta.url));
const src = join(pkgRoot, "mach12");

if (!existsSync(src)) {
	console.warn(`[scramjet] Bundled Mach 12 source missing at ${src}; skipping seed.`);
	process.exit(0);
}

const dataHome = process.env.XDG_DATA_HOME || join(homedir(), ".local", "share");
if (!isAbsolute(dataHome)) {
	// XDG spec requires absolute paths; a relative value would silently write
	// mach12 next to whatever cwd npm install was invoked from.
	console.warn(`[scramjet] XDG_DATA_HOME (${dataHome}) is not absolute; skipping Mach 12 seed.`);
	process.exit(0);
}
const destParent = join(dataHome, "scramjet");
const dest = join(destParent, "mach12");

if (existsSync(dest)) {
	process.exit(0);
}

const tmp = `${dest}.tmp-${process.pid}`;
try {
	mkdirSync(destParent, { recursive: true });
	cpSync(src, tmp, { recursive: true });
	renameSync(tmp, dest);
	console.log(`[scramjet] Seeded Mach 12 command set at ${dest}`);
} catch (err) {
	rmSync(tmp, { recursive: true, force: true });
	const message = err instanceof Error ? err.message : String(err);
	console.warn(`[scramjet] Mach 12 seed failed: ${message}`);
	console.warn(`[scramjet] Continuing install; copy mach12/ manually to ${dest} if needed.`);
	process.exit(0);
}
