#!/usr/bin/env node
// Seeds the bundled Mach 12 command set with manifest-based upgrade support.
// Fresh installs: atomic copy + manifest written. Upgrades: unedited files are
// replaced, edited files are preserved with a warning, removed files are cleaned.
// Legacy installs (pre-manifest): backup-and-reseed with user-added file recovery.
// Never blocks `npm install` — any failure prints a warning and exits 0.

import {
	copyFileSync,
	cpSync,
	existsSync,
	lstatSync,
	mkdirSync,
	readFileSync,
	readdirSync,
	readlinkSync,
	renameSync,
	rmSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { homedir, platform } from "node:os";
import { dirname, isAbsolute, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

if (platform() === "win32") {
	console.warn("[scramjet] Mach 12 seeding skipped on native Windows.");
	console.warn("[scramjet] Install inside WSL for full functionality.");
	process.exit(0);
}

const configDir = join(homedir(), ".scramjet");
const extSubagent = join(configDir, "agent", "extensions", "subagent");
const staleSubagentFiles = ["agents.ts", "index.ts"];

const MANIFEST_NAME = ".seed-manifest.json";

function errorCode(err) {
	return err && typeof err === "object" && "code" in err ? err.code : undefined;
}

function errorMessage(err) {
	return err instanceof Error ? err.message : String(err);
}

function sha256(filePath) {
	const content = readFileSync(filePath);
	return createHash("sha256").update(content).digest("hex");
}

function walkDir(dir) {
	const results = [];
	const entries = readdirSync(dir, { withFileTypes: true });
	for (const entry of entries) {
		const fullPath = join(dir, entry.name);
		if (entry.isDirectory()) {
			results.push(...walkDir(fullPath));
		} else if (entry.isFile()) {
			results.push(fullPath);
		}
	}
	return results;
}

function ensureParentDir(filePath) {
	const dir = dirname(filePath);
	mkdirSync(dir, { recursive: true });
}

function isOldSubagentExampleSymlink(dir, file) {
	const filePath = join(dir, file);
	const stat = lstatSync(filePath);
	if (!stat.isSymbolicLink()) return false;
	const target = readlinkSync(filePath).replaceAll("\\", "/");
	const suffix = `packages/coding-agent/examples/extensions/subagent/${file}`;
	return target === suffix || target.endsWith(`/${suffix}`);
}

function isStaleManualSubagentDirectory(dir) {
	const entries = readdirSync(dir).filter((entry) => entry !== ".DS_Store").sort();
	if (entries.length !== staleSubagentFiles.length) return false;
	if (!staleSubagentFiles.every((file, index) => entries[index] === file)) return false;
	return staleSubagentFiles.every((file) => isOldSubagentExampleSymlink(dir, file));
}

function cleanupStaleSubagentExtension() {
	try {
		const extStat = lstatSync(extSubagent);
		if (extStat.isSymbolicLink()) {
			console.warn(`[scramjet] Removing stale subagent extension at ${extSubagent}`);
			unlinkSync(extSubagent);
			return;
		}
		if (extStat.isDirectory()) {
			if (isStaleManualSubagentDirectory(extSubagent)) {
				console.warn(`[scramjet] Removing stale subagent extension at ${extSubagent}`);
				rmSync(extSubagent, { recursive: true, force: true });
				return;
			}
			console.warn(
				`[scramjet] Preserving ${extSubagent}; remove it manually if it is the deprecated subagent extension.`,
			);
		}
	} catch (err) {
		if (errorCode(err) !== "ENOENT") {
			console.warn(`[scramjet] Stale extension check failed: ${errorMessage(err)}`);
		}
	}
}

cleanupStaleSubagentExtension();

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

// Healthy symlink — dev tree setup. Exit before any manifest logic.
try {
	const destStat = lstatSync(dest);
	if (destStat.isSymbolicLink()) {
		if (existsSync(dest)) {
			// Healthy symlink (target exists) — dev setup, leave untouched.
			process.exit(0);
		}
		// Dangling symlink — remove it and proceed to fresh seed.
		console.warn(`[scramjet] Removing dangling symlink at ${dest}`);
		unlinkSync(dest);
	}
} catch (err) {
	if (errorCode(err) !== "ENOENT") {
		console.warn(`[scramjet] Cannot inspect ${dest}: ${errorMessage(err)}`);
	}
	// ENOENT means dest doesn't exist — fall through to fresh seed.
}

const pkgVersion = JSON.parse(readFileSync(join(pkgRoot, "package.json"), "utf-8")).version;
const manifestPath = join(dest, MANIFEST_NAME);

function buildManifestFromSrc() {
	const files = {};
	for (const filePath of walkDir(src)) {
		const rel = relative(src, filePath);
		files[rel] = sha256(filePath);
	}
	return { version: pkgVersion, files };
}

function freshSeed() {
	const tmp = `${dest}.tmp-${process.pid}`;
	try {
		mkdirSync(destParent, { recursive: true });
		cpSync(src, tmp, { recursive: true });
		const manifest = buildManifestFromSrc();
		writeFileSync(join(tmp, MANIFEST_NAME), JSON.stringify(manifest, null, "\t") + "\n");
		renameSync(tmp, dest);
		console.log(`[scramjet] Seeded Mach 12 command set at ${dest}`);
	} catch (err) {
		try {
			rmSync(tmp, { recursive: true, force: true });
		} catch (_) {}
		console.warn(`[scramjet] Mach 12 seed failed: ${errorMessage(err)}`);
		console.warn(`[scramjet] Continuing install; copy mach12/ manually to ${dest} if needed.`);
	}
}

function writeManifest(manifest) {
	const tmp = `${manifestPath}.tmp-${process.pid}`;
	writeFileSync(tmp, JSON.stringify(manifest, null, "\t") + "\n");
	renameSync(tmp, manifestPath);
}

// dest does not exist — fresh seed
if (!existsSync(dest)) {
	freshSeed();
	process.exit(0);
}

// dest exists — check for manifest
let manifest = null;
try {
	if (existsSync(manifestPath)) {
		manifest = JSON.parse(readFileSync(manifestPath, "utf-8"));
	}
} catch (err) {
	console.warn(`[scramjet] Could not read manifest: ${errorMessage(err)}; treating as legacy install.`);
}

if (manifest && manifest.version === pkgVersion) {
	// Same version — short-circuit, nothing to do.
	process.exit(0);
}

if (manifest) {
	// Manifest-based upgrade
	try {
		const newManifest = buildManifestFromSrc();
		const oldFiles = manifest.files || {};
		const newFiles = newManifest.files;
		const warnings = [];

		// Update/add files from bundle
		for (const [rel, newHash] of Object.entries(newFiles)) {
			const destFile = join(dest, rel);
			if (!existsSync(destFile)) {
				// Missing on disk — reseed from bundle
				ensureParentDir(destFile);
				copyFileSync(join(src, rel), destFile);
				continue;
			}
			const installedHash = sha256(destFile);
			const oldHash = oldFiles[rel];
			if (oldHash && installedHash !== oldHash) {
				warnings.push(rel);
				continue;
			}
			if (!oldHash && installedHash !== newHash) {
				// File exists on disk but wasn't in old manifest — user-created, preserve
				warnings.push(`${rel} (new in bundle but pre-existing on disk)`);
				continue;
			}
			if (installedHash === newHash) {
				continue;
			}
			ensureParentDir(destFile);
			copyFileSync(join(src, rel), destFile);
		}

		// Remove files that were in the old manifest but not in the new bundle
		for (const rel of Object.keys(oldFiles)) {
			if (rel in newFiles) continue;
			const destFile = join(dest, rel);
			if (!existsSync(destFile)) continue;
			const installedHash = sha256(destFile);
			if (installedHash === oldFiles[rel]) {
				// Unedited — safe to delete
				rmSync(destFile);
				// Clean up empty parent directories
				try {
					const parent = dirname(destFile);
					if (parent !== dest && readdirSync(parent).length === 0) {
						rmSync(parent, { recursive: true });
					}
				} catch (_) {}
			} else {
				warnings.push(`${rel} (removed from bundle but edited — preserved)`);
			}
		}

		writeManifest(newManifest);

		if (warnings.length > 0) {
			console.warn(`[scramjet] Mach 12 upgraded to ${pkgVersion}. Preserved edited files:`);
			for (const w of warnings) {
				console.warn(`[scramjet]   ${w}`);
			}
		} else {
			console.log(`[scramjet] Mach 12 upgraded to ${pkgVersion}.`);
		}
	} catch (err) {
		console.warn(`[scramjet] Mach 12 upgrade failed: ${errorMessage(err)}`);
		console.warn(`[scramjet] Continuing; existing commands preserved.`);
	}
	process.exit(0);
}

// Legacy migration — dest exists with no manifest
try {
	const backupName = `mach12.pre-upgrade-${Date.now()}`;
	const backupPath = join(destParent, backupName);
	renameSync(dest, backupPath);

	// Fresh seed with manifest
	freshSeed();

	if (!existsSync(dest)) {
		// freshSeed failed (it already warned) — restore backup
		renameSync(backupPath, dest);
		process.exit(0);
	}

	// Copy back user-added files (files in backup that are NOT in the bundle)
	const bundledFiles = new Set(walkDir(src).map((f) => relative(src, f)));
	for (const backupFile of walkDir(backupPath)) {
		const rel = relative(backupPath, backupFile);
		if (rel === MANIFEST_NAME) continue;
		if (bundledFiles.has(rel)) continue;
		// User-added file — copy back
		const destFile = join(dest, rel);
		ensureParentDir(destFile);
		copyFileSync(backupFile, destFile);
	}

	console.warn(`[scramjet] Legacy Mach 12 install migrated. Previous tree backed up at:`);
	console.warn(`[scramjet]   ${backupPath}`);
	console.warn(`[scramjet] User-added files were copied into the new tree.`);
	console.warn(`[scramjet] If you had edited bundled commands, re-apply edits from the backup.`);
} catch (err) {
	console.warn(`[scramjet] Legacy migration failed: ${errorMessage(err)}`);
	console.warn(`[scramjet] Continuing; check ${dest} manually.`);
}
process.exit(0);
