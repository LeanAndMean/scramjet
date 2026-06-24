#!/usr/bin/env node
// Seeds the bundled Mach 12 command set into the user data dir on first install.
// Idempotent: skips silently if the destination already exists, so user edits
// to commands or agents are never overwritten. Atomic: copies to a temp path
// and renames into place, so a partial copy can never poison the next run.
// Never blocks `npm install` — any failure prints a warning and exits 0.

import { cpSync, existsSync, lstatSync, mkdirSync, renameSync, rmSync, unlinkSync } from "node:fs";
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

const destParent =
	process.env.SCRAMJET_CACHE ??
	join(process.env.XDG_DATA_HOME || join(homedir(), ".local", "share"), "scramjet");
if (!isAbsolute(destParent)) {
	console.warn(`[scramjet] Computed data dir (${destParent}) is not absolute; skipping Mach 12 seed.`);
	process.exit(0);
}
const dest = join(destParent, "mach12");

// Detect dangling symlinks left over from the pre-monorepo layout where
// mach12/ lived at repo root. existsSync follows symlinks and returns false
// for a dangling one, so we use lstatSync which checks the link itself.
try {
	lstatSync(dest);
	if (existsSync(dest)) {
		process.exit(0);
	}
	// lstat succeeded (link exists) but existsSync failed (target missing) — dangling symlink.
	console.warn(`[scramjet] Removing dangling symlink at ${dest}`);
	unlinkSync(dest);
} catch {
	// lstat threw — dest doesn't exist at all, which is the normal first-install path.
}

const tmp = `${dest}.tmp-${process.pid}`;
try {
	mkdirSync(destParent, { recursive: true });
	cpSync(src, tmp, { recursive: true });
	renameSync(tmp, dest);
	console.log(`[scramjet] Seeded Mach 12 command set at ${dest}`);
} catch (err) {
	// F26: nest the cleanup so a failing rmSync (e.g. EBUSY on Windows-ish
	// filesystems mounted into WSL) cannot mask the original failure that
	// brought us into the catch.
	try {
		rmSync(tmp, { recursive: true, force: true });
	} catch (cleanupErr) {
		const cleanupMessage = cleanupErr instanceof Error ? cleanupErr.message : String(cleanupErr);
		console.warn(`[scramjet] Mach 12 seed cleanup failed for ${tmp}: ${cleanupMessage}`);
	}
	const message = err instanceof Error ? err.message : String(err);
	console.warn(`[scramjet] Mach 12 seed failed: ${message}`);
	console.warn(`[scramjet] Continuing install; copy mach12/ manually to ${dest} if needed.`);
	process.exit(0);
}
