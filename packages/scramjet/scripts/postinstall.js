#!/usr/bin/env node

import { createHash } from "node:crypto";
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
import { homedir, platform } from "node:os";
import { dirname, isAbsolute, join, normalize, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

if (platform() === "win32") {
	console.warn("[scramjet] Command-set seeding skipped on native Windows.");
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
	return createHash("sha256").update(readFileSync(filePath)).digest("hex");
}

function walkDir(dir) {
	return readdirSync(dir, { recursive: true, withFileTypes: true })
		.filter((entry) => entry.isFile())
		.map((entry) => join(entry.parentPath, entry.name));
}

function ensureParentDir(filePath) {
	mkdirSync(dirname(filePath), { recursive: true });
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
			console.warn(`[scramjet] Preserving ${extSubagent}; remove it manually if it is the deprecated subagent extension.`);
		}
	} catch (err) {
		if (errorCode(err) !== "ENOENT") {
			console.warn(`[scramjet] Stale extension check failed: ${errorMessage(err)}`);
		}
	}
}

cleanupStaleSubagentExtension();

const pkgRoot = fileURLToPath(new URL("..", import.meta.url));
const pkgVersion = JSON.parse(readFileSync(join(pkgRoot, "package.json"), "utf-8")).version;
const destParent =
	process.env.SCRAMJET_CACHE ?? join(process.env.XDG_DATA_HOME || join(homedir(), ".local", "share"), "scramjet");

if (!isAbsolute(destParent)) {
	console.warn(`[scramjet] Computed data dir (${destParent}) is not absolute; skipping command-set seed.`);
	process.exit(0);
}

const sets = [
	{ name: "mach12", label: "Mach 12", migrateLegacy: true },
	{ name: "scramjet", label: "Scramjet", migrateLegacy: false },
];

function isPlainObject(value) {
	return value !== null && typeof value === "object" && !Array.isArray(value) && Object.getPrototypeOf(value) === Object.prototype;
}

function isSafeRelativePath(rel, dest) {
	if (typeof rel !== "string" || rel.length === 0 || rel.includes("\0") || rel.includes("\\") || isAbsolute(rel)) {
		return false;
	}
	if (normalize(rel) !== rel || rel === "." || rel.split("/").some((part) => part === "" || part === "." || part === "..")) {
		return false;
	}
	const resolved = resolve(dest, rel);
	const fromDest = relative(resolve(dest), resolved);
	return fromDest !== "" && !fromDest.startsWith("..") && !isAbsolute(fromDest);
}

function validateManifest(value, dest) {
	if (!isPlainObject(value) || typeof value.version !== "string" || !isPlainObject(value.files)) return null;
	for (const [rel, hash] of Object.entries(value.files)) {
		if (!isSafeRelativePath(rel, dest) || typeof hash !== "string" || !/^[a-f0-9]{64}$/i.test(hash)) return null;
	}
	return value;
}

function rejectSymlinkComponents(dest, rel) {
	let current = dest;
	for (const part of rel.split("/")) {
		current = join(current, part);
		try {
			if (lstatSync(current).isSymbolicLink()) throw new Error(`managed path contains a symlink: ${rel}`);
		} catch (err) {
			if (errorCode(err) === "ENOENT") return;
			throw err;
		}
	}
}

function buildManifest(src) {
	const files = {};
	for (const filePath of walkDir(src)) {
		files[relative(src, filePath)] = sha256(filePath);
	}
	return { version: pkgVersion, files };
}

function writeManifest(manifestPath, manifest) {
	const tmp = `${manifestPath}.tmp-${process.pid}`;
	writeFileSync(tmp, `${JSON.stringify(manifest, null, "\t")}\n`);
	renameSync(tmp, manifestPath);
}

function freshSeed(set, src, dest) {
	const tmp = `${dest}.tmp-${process.pid}`;
	try {
		mkdirSync(destParent, { recursive: true });
		rmSync(tmp, { recursive: true, force: true });
		cpSync(src, tmp, { recursive: true });
		writeFileSync(join(tmp, MANIFEST_NAME), `${JSON.stringify(buildManifest(src), null, "\t")}\n`);
		renameSync(tmp, dest);
		console.log(`[scramjet] Seeded ${set.label} command set at ${dest}`);
		return true;
	} catch (err) {
		try {
			rmSync(tmp, { recursive: true, force: true });
		} catch {}
		console.warn(`[scramjet] ${set.label} seed failed: ${errorMessage(err)}`);
		console.warn(`[scramjet] Continuing install; copy ${set.name}/ manually to ${dest} if needed.`);
		return false;
	}
}

function warnUnowned(set, dest) {
	console.warn(`[scramjet] Preserving unowned ${set.label} command set at ${dest}.`);
	console.warn(`[scramjet] To install the bundled set, move the existing path and copy ${set.name}/ manually to ${dest}.`);
}

function upgrade(set, src, dest, manifestPath, manifest) {
	try {
		const newManifest = buildManifest(src);
		const warnings = [];

		for (const [rel, newHash] of Object.entries(newManifest.files)) {
			rejectSymlinkComponents(dest, rel);
			const destFile = join(dest, rel);
			if (!existsSync(destFile)) {
				ensureParentDir(destFile);
				copyFileSync(join(src, rel), destFile);
				continue;
			}
			const installedHash = sha256(destFile);
			const oldHash = manifest.files[rel];
			if (oldHash && installedHash !== oldHash) {
				warnings.push(rel);
				continue;
			}
			if (!oldHash && installedHash !== newHash) {
				warnings.push(`${rel} (new in bundle but pre-existing on disk)`);
				continue;
			}
			if (installedHash !== newHash) copyFileSync(join(src, rel), destFile);
		}

		for (const rel of Object.keys(manifest.files)) {
			if (rel in newManifest.files) continue;
			rejectSymlinkComponents(dest, rel);
			const destFile = join(dest, rel);
			if (!existsSync(destFile)) continue;
			if (sha256(destFile) === manifest.files[rel]) {
				rmSync(destFile);
				const parent = dirname(destFile);
				if (parent !== dest && readdirSync(parent).length === 0) rmSync(parent, { recursive: true });
			} else {
				warnings.push(`${rel} (removed from bundle but edited — preserved)`);
			}
		}

		writeManifest(manifestPath, newManifest);
		if (warnings.length > 0) {
			console.warn(`[scramjet] ${set.label} upgraded to ${pkgVersion}. Preserved edited files:`);
			for (const warning of warnings) console.warn(`[scramjet]   ${warning}`);
		} else {
			console.log(`[scramjet] ${set.label} upgraded to ${pkgVersion}.`);
		}
	} catch (err) {
		console.warn(`[scramjet] ${set.label} upgrade failed: ${errorMessage(err)}`);
		console.warn(`[scramjet] Continuing; existing commands preserved.`);
	}
}

function migrateLegacy(set, src, dest) {
	const backupPath = join(destParent, `${set.name}.pre-upgrade-${Date.now()}`);
	try {
		renameSync(dest, backupPath);
		if (!freshSeed(set, src, dest)) {
			renameSync(backupPath, dest);
			return;
		}
		const bundledFiles = new Set(walkDir(src).map((file) => relative(src, file)));
		for (const backupFile of walkDir(backupPath)) {
			const rel = relative(backupPath, backupFile);
			if (rel === MANIFEST_NAME || bundledFiles.has(rel)) continue;
			const destFile = join(dest, rel);
			ensureParentDir(destFile);
			copyFileSync(backupFile, destFile);
		}
		console.warn(`[scramjet] Legacy ${set.label} install migrated. Previous tree backed up at:`);
		console.warn(`[scramjet]   ${backupPath}`);
		console.warn(`[scramjet] User-added files were copied into the new tree.`);
		console.warn(`[scramjet] If you had edited bundled commands, re-apply edits from the backup.`);
	} catch (err) {
		console.warn(`[scramjet] Legacy migration failed: ${errorMessage(err)}`);
		console.warn(`[scramjet] Continuing; check ${dest} manually.`);
	}
}

function seedSet(set) {
	const src = join(pkgRoot, set.name);
	const dest = join(destParent, set.name);
	if (!existsSync(src)) {
		console.warn(`[scramjet] Bundled ${set.label} source missing at ${src}; skipping seed.`);
		return;
	}

	try {
		const stat = lstatSync(dest);
		if (stat.isSymbolicLink()) {
			if (set.migrateLegacy && !existsSync(dest)) {
				console.warn(`[scramjet] Removing dangling symlink at ${dest}`);
				unlinkSync(dest);
			} else {
				if (!set.migrateLegacy) console.warn(`[scramjet] Preserving ${set.label} symlink at ${dest}.`);
				return;
			}
		}
	} catch (err) {
		if (errorCode(err) !== "ENOENT") {
			console.warn(`[scramjet] Cannot inspect ${dest}: ${errorMessage(err)}`);
			return;
		}
	}

	if (!existsSync(dest)) {
		freshSeed(set, src, dest);
		return;
	}

	const manifestPath = join(dest, MANIFEST_NAME);
	let parsed = null;
	let manifestPresent = false;
	try {
		manifestPresent = existsSync(manifestPath);
		if (manifestPresent) parsed = JSON.parse(readFileSync(manifestPath, "utf-8"));
	} catch (err) {
		console.warn(`[scramjet] Could not read ${set.label} manifest: ${errorMessage(err)}.`);
	}
	const manifest = validateManifest(parsed, dest);
	if (!manifest) {
		if (set.migrateLegacy) migrateLegacy(set, src, dest);
		else warnUnowned(set, dest);
		return;
	}
	if (manifest.version === pkgVersion) return;
	upgrade(set, src, dest, manifestPath, manifest);
}

for (const set of sets) seedSet(set);
