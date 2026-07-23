#!/usr/bin/env node

import {
	copyFileSync,
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
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

if (platform() === "win32") {
	console.warn("[scramjet] Command-set seeding skipped on native Windows.");
	console.warn("[scramjet] Install inside WSL for full functionality.");
	process.exit(0);
}

const MANIFEST_NAME = ".seed-manifest.json";
const MANIFEST_SCHEMA_VERSION = 1;
const HASH_PATTERN = /^[0-9a-f]{64}$/;
const configDir = join(homedir(), ".scramjet");
const extSubagent = join(configDir, "agent", "extensions", "subagent");
const staleSubagentFiles = ["agents.ts", "index.ts"];

function errorCode(err) {
	return err && typeof err === "object" && "code" in err ? err.code : undefined;
}

function errorMessage(err) {
	return err instanceof Error ? err.message : String(err);
}

function lstatIfPresent(filePath) {
	try {
		return lstatSync(filePath);
	} catch (err) {
		if (errorCode(err) === "ENOENT") return null;
		throw err;
	}
}

function isPlainObject(value) {
	if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
	const prototype = Object.getPrototypeOf(value);
	return prototype === Object.prototype || prototype === null;
}

function hasExactKeys(value, expected) {
	const keys = Object.keys(value).sort();
	return keys.length === expected.length && keys.every((key, index) => key === expected[index]);
}

function isCanonicalRelativePath(value) {
	if (typeof value !== "string" || value.length === 0 || value.includes("\\") || value.includes("\0")) return false;
	if (isAbsolute(value) || value === MANIFEST_NAME) return false;
	const components = value.split("/");
	return components.every((component) => component.length > 0 && component !== "." && component !== "..");
}

function pathWithin(root, rel) {
	const target = resolve(root, rel);
	const fromRoot = relative(resolve(root), target);
	if (fromRoot === "" || fromRoot === ".." || fromRoot.startsWith(`..${sep}`) || isAbsolute(fromRoot)) {
		throw new Error("path escapes set root");
	}
	return target;
}

function sha256(filePath) {
	const stat = lstatSync(filePath);
	if (stat.isSymbolicLink() || !stat.isFile()) throw new Error("unsafe hash target");
	return createHash("sha256").update(readFileSync(filePath)).digest("hex");
}

function walkSafeTree(root) {
	const rootStat = lstatSync(root);
	if (rootStat.isSymbolicLink() || !rootStat.isDirectory()) throw new Error("unsafe tree root");
	const files = [];
	const pending = [{ absolute: root, rel: "" }];
	while (pending.length > 0) {
		const current = pending.pop();
		for (const name of readdirSync(current.absolute).sort()) {
			const absolute = join(current.absolute, name);
			const rel = current.rel ? `${current.rel}/${name}` : name;
			if (!isCanonicalRelativePath(rel)) throw new Error("unsafe tree path");
			const stat = lstatSync(absolute);
			if (stat.isSymbolicLink()) throw new Error("symlink in tree");
			if (stat.isDirectory()) {
				pending.push({ absolute, rel });
			} else if (stat.isFile()) {
				files.push(rel);
			} else {
				throw new Error("non-regular entry in tree");
			}
		}
	}
	return files.sort();
}

function validateTarget(root, rel) {
	pathWithin(root, rel);
	const rootStat = lstatSync(root);
	if (rootStat.isSymbolicLink() || !rootStat.isDirectory()) throw new Error("unsafe destination root");
	const components = rel.split("/");
	let current = root;
	for (let index = 0; index < components.length; index += 1) {
		current = join(current, components[index]);
		const stat = lstatIfPresent(current);
		if (!stat) return;
		if (stat.isSymbolicLink()) throw new Error("symlink in destination tree");
		if (index < components.length - 1) {
			if (!stat.isDirectory()) throw new Error("non-directory destination parent");
		} else if (!stat.isFile()) {
			throw new Error("non-regular destination target");
		}
	}
}

function ensureSafeParent(root, rel) {
	const components = rel.split("/").slice(0, -1);
	let current = root;
	for (const component of components) {
		current = join(current, component);
		let stat = lstatIfPresent(current);
		if (!stat) {
			mkdirSync(current);
			stat = lstatSync(current);
		}
		if (stat.isSymbolicLink() || !stat.isDirectory()) throw new Error("unsafe destination parent");
	}
}

function copyOwnedFile(sourceRoot, destinationRoot, rel) {
	const source = pathWithin(sourceRoot, rel);
	const sourceStat = lstatSync(source);
	if (sourceStat.isSymbolicLink() || !sourceStat.isFile()) throw new Error("unsafe source target");
	validateTarget(destinationRoot, rel);
	ensureSafeParent(destinationRoot, rel);
	validateTarget(destinationRoot, rel);
	copyFileSync(source, pathWithin(destinationRoot, rel));
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
	return (
		entries.length === staleSubagentFiles.length &&
		staleSubagentFiles.every((file, index) => entries[index] === file) &&
		staleSubagentFiles.every((file) => isOldSubagentExampleSymlink(dir, file))
	);
}

function cleanupStaleSubagentExtension() {
	try {
		const stat = lstatSync(extSubagent);
		if (stat.isSymbolicLink()) {
			console.warn(`[scramjet] Removing stale subagent extension at ${extSubagent}`);
			unlinkSync(extSubagent);
		} else if (stat.isDirectory() && isStaleManualSubagentDirectory(extSubagent)) {
			console.warn(`[scramjet] Removing stale subagent extension at ${extSubagent}`);
			rmSync(extSubagent, { recursive: true, force: true });
		} else if (stat.isDirectory()) {
			console.warn(`[scramjet] Preserving ${extSubagent}; remove it manually if it is the deprecated subagent extension.`);
		}
	} catch (err) {
		if (errorCode(err) !== "ENOENT") console.warn(`[scramjet] Stale extension check failed: ${errorMessage(err)}`);
	}
}

function validateFiles(files) {
	if (!isPlainObject(files)) return false;
	for (const [rel, hash] of Object.entries(files)) {
		if (!isCanonicalRelativePath(rel) || typeof hash !== "string" || !HASH_PATTERN.test(hash)) return false;
	}
	return true;
}

function parseManifest(text, descriptor) {
	let value;
	try {
		value = JSON.parse(text);
	} catch {
		return null;
	}
	if (!isPlainObject(value)) return null;
	const currentKeys = ["files", "schemaVersion", "setId", "version"];
	const isCurrent =
		hasExactKeys(value, currentKeys) &&
		value.schemaVersion === MANIFEST_SCHEMA_VERSION &&
		value.setId === descriptor.id;
	const isLegacyMach12 =
		descriptor.id === "mach12" && hasExactKeys(value, ["files", "version"]) && value.schemaVersion === undefined;
	if (!isCurrent && !isLegacyMach12) return null;
	if (
		typeof value.version !== "string" ||
		value.version.length === 0 ||
		Buffer.byteLength(value.version, "utf-8") > 256 ||
		!validateFiles(value.files)
	) {
		return null;
	}
	return { version: value.version, files: value.files, legacySchema: isLegacyMach12 };
}

function buildManifest(descriptor, pkgVersion, sourceFiles) {
	const files = {};
	for (const rel of sourceFiles) files[rel] = sha256(pathWithin(descriptor.sourceDir, rel));
	return { schemaVersion: MANIFEST_SCHEMA_VERSION, setId: descriptor.id, version: pkgVersion, files };
}

function writeManifest(destination, manifest) {
	validateTarget(destination, MANIFEST_NAME);
	const manifestPath = join(destination, MANIFEST_NAME);
	const temporary = `${manifestPath}.tmp-${process.pid}-${Date.now()}`;
	if (lstatIfPresent(temporary)) throw new Error("manifest temporary path exists");
	try {
		writeFileSync(temporary, `${JSON.stringify(manifest, null, "\t")}\n`);
		const temporaryStat = lstatSync(temporary);
		if (temporaryStat.isSymbolicLink() || !temporaryStat.isFile()) throw new Error("unsafe manifest temporary");
		validateTarget(destination, MANIFEST_NAME);
		renameSync(temporary, manifestPath);
	} catch (err) {
		const temporaryStat = lstatIfPresent(temporary);
		if (temporaryStat?.isFile() && !temporaryStat.isSymbolicLink()) rmSync(temporary);
		throw err;
	}
}

function freshSeed(descriptor, pkgVersion, sourceFiles) {
	const temporary = `${descriptor.destinationDir}.tmp-${process.pid}-${Date.now()}`;
	let createdTemporary = false;
	try {
		if (lstatIfPresent(descriptor.destinationDir)) throw new Error("destination appeared during seed");
		if (lstatIfPresent(temporary)) throw new Error("seed temporary path exists");
		mkdirSync(dirname(descriptor.destinationDir), { recursive: true });
		mkdirSync(temporary);
		createdTemporary = true;
		for (const rel of sourceFiles) copyOwnedFile(descriptor.sourceDir, temporary, rel);
		writeManifest(temporary, buildManifest(descriptor, pkgVersion, sourceFiles));
		if (lstatIfPresent(descriptor.destinationDir)) throw new Error("destination appeared during seed");
		renameSync(temporary, descriptor.destinationDir);
		createdTemporary = false;
		console.log(`[scramjet] Seeded ${descriptor.displayName} command set at ${descriptor.destinationDir}`);
		return true;
	} catch (err) {
		if (createdTemporary) {
			try {
				rmSync(temporary, { recursive: true, force: true });
			} catch {}
		}
		console.warn(`[scramjet] ${descriptor.displayName} seed failed: ${errorMessage(err)}`);
		console.warn(
			`[scramjet] Continuing install; copy ${descriptor.id}/ manually to ${descriptor.destinationDir} if needed.`,
		);
		return false;
	}
}

function upgradeOwnedSet(descriptor, pkgVersion, sourceFiles, oldManifest) {
	try {
		const newManifest = buildManifest(descriptor, pkgVersion, sourceFiles);
		const allPaths = new Set([...Object.keys(oldManifest.files), ...Object.keys(newManifest.files), MANIFEST_NAME]);
		for (const rel of allPaths) validateTarget(descriptor.destinationDir, rel);
		const warnings = [];

		for (const [rel, newHash] of Object.entries(newManifest.files)) {
			const destination = pathWithin(descriptor.destinationDir, rel);
			const stat = lstatIfPresent(destination);
			if (!stat) {
				copyOwnedFile(descriptor.sourceDir, descriptor.destinationDir, rel);
				continue;
			}
			const installedHash = sha256(destination);
			const oldHash = oldManifest.files[rel];
			if (oldHash && installedHash !== oldHash) {
				warnings.push(rel);
			} else if (!oldHash && installedHash !== newHash) {
				warnings.push(`${rel} (new in bundle but pre-existing on disk)`);
			} else if (installedHash !== newHash) {
				copyOwnedFile(descriptor.sourceDir, descriptor.destinationDir, rel);
			}
		}

		for (const [rel, oldHash] of Object.entries(oldManifest.files)) {
			if (rel in newManifest.files) continue;
			const destination = pathWithin(descriptor.destinationDir, rel);
			if (!lstatIfPresent(destination)) continue;
			validateTarget(descriptor.destinationDir, rel);
			if (sha256(destination) === oldHash) {
				validateTarget(descriptor.destinationDir, rel);
				rmSync(destination);
			} else {
				warnings.push(`${rel} (removed from bundle but edited — preserved)`);
			}
		}

		writeManifest(descriptor.destinationDir, newManifest);
		if (warnings.length > 0) {
			console.warn(`[scramjet] ${descriptor.displayName} upgraded to ${pkgVersion}. Preserved edited files:`);
			for (const warning of warnings) console.warn(`[scramjet]   ${warning}`);
		} else {
			console.log(`[scramjet] ${descriptor.displayName} upgraded to ${pkgVersion}.`);
		}
	} catch (err) {
		console.warn(`[scramjet] ${descriptor.displayName} upgrade failed due to unsafe destination tree or I/O: ${errorMessage(err)}`);
		console.warn(`[scramjet] Continuing; existing ${descriptor.displayName} commands preserved where possible.`);
	}
}

function migrateLegacyMach12(descriptor, pkgVersion, sourceFiles) {
	try {
		const installedFiles = walkSafeTree(descriptor.destinationDir);
		const bundledFiles = new Set(sourceFiles);
		const userFiles = installedFiles.filter((rel) => rel !== MANIFEST_NAME && !bundledFiles.has(rel));
		for (const rel of userFiles) pathWithin(descriptor.destinationDir, rel);
		const backupPath = join(dirname(descriptor.destinationDir), `mach12.pre-upgrade-${Date.now()}`);
		if (lstatIfPresent(backupPath)) throw new Error("legacy backup path exists");
		renameSync(descriptor.destinationDir, backupPath);
		if (!freshSeed(descriptor, pkgVersion, sourceFiles)) {
			if (!lstatIfPresent(descriptor.destinationDir)) renameSync(backupPath, descriptor.destinationDir);
			return;
		}
		for (const rel of userFiles) copyOwnedFile(backupPath, descriptor.destinationDir, rel);
		console.warn("[scramjet] Legacy Mach 12 install migrated. Previous tree backed up at:");
		console.warn(`[scramjet]   ${backupPath}`);
		console.warn("[scramjet] User-added files were copied into the new tree.");
		console.warn("[scramjet] If you had edited bundled commands, re-apply edits from the backup.");
	} catch (err) {
		console.warn(`[scramjet] Legacy migration failed: ${errorMessage(err)}`);
		console.warn(`[scramjet] Continuing; check ${descriptor.destinationDir} manually.`);
	}
}

function seedDescriptor(descriptor, pkgVersion) {
	let sourceFiles;
	try {
		if (!existsSync(descriptor.sourceDir)) {
			console.warn(`[scramjet] Bundled ${descriptor.displayName} source missing at ${descriptor.sourceDir}; skipping seed.`);
			return;
		}
		sourceFiles = walkSafeTree(descriptor.sourceDir);
	} catch (err) {
		console.warn(`[scramjet] Bundled ${descriptor.displayName} source is unsafe or unreadable: ${errorMessage(err)}`);
		return;
	}

	try {
		const destinationStat = lstatIfPresent(descriptor.destinationDir);
		if (destinationStat?.isSymbolicLink()) {
			if (existsSync(descriptor.destinationDir)) return;
			console.warn(`[scramjet] Removing dangling symlink at ${descriptor.destinationDir}`);
			unlinkSync(descriptor.destinationDir);
		}
	} catch (err) {
		console.warn(`[scramjet] Cannot inspect ${descriptor.destinationDir}: ${errorMessage(err)}`);
		return;
	}

	if (!lstatIfPresent(descriptor.destinationDir)) {
		freshSeed(descriptor, pkgVersion, sourceFiles);
		return;
	}

	const destinationStat = lstatSync(descriptor.destinationDir);
	if (!destinationStat.isDirectory()) {
		console.warn(`[scramjet] Refusing ${descriptor.displayName} seed because the destination is not a directory.`);
		return;
	}

	const manifestPath = join(descriptor.destinationDir, MANIFEST_NAME);
	const manifestStat = lstatIfPresent(manifestPath);
	if (!manifestStat) {
		if (descriptor.legacyMode === "backup-reseed") migrateLegacyMach12(descriptor, pkgVersion, sourceFiles);
		else {
			console.warn(
				`[scramjet] Refusing ${descriptor.displayName} seed because ${descriptor.destinationDir} has no owned manifest; ownership is unknown.`,
			);
		}
		return;
	}
	if (manifestStat.isSymbolicLink() || !manifestStat.isFile()) {
		console.warn(`[scramjet] Refusing ${descriptor.displayName} seed because it has an invalid manifest; existing tree preserved.`);
		return;
	}

	let manifest;
	try {
		manifest = parseManifest(readFileSync(manifestPath, "utf-8"), descriptor);
	} catch {
		manifest = null;
	}
	if (!manifest) {
		console.warn(`[scramjet] Refusing ${descriptor.displayName} seed because it has an invalid manifest; existing tree preserved.`);
		return;
	}
	if (manifest.version === pkgVersion && !manifest.legacySchema) return;
	upgradeOwnedSet(descriptor, pkgVersion, sourceFiles, manifest);
}

cleanupStaleSubagentExtension();

const pkgRoot = fileURLToPath(new URL("..", import.meta.url));
const destinationParent =
	process.env.SCRAMJET_CACHE ?? join(process.env.XDG_DATA_HOME || join(homedir(), ".local", "share"), "scramjet");
if (!isAbsolute(destinationParent)) {
	console.warn(`[scramjet] Computed data dir (${destinationParent}) is not absolute; skipping command-set seed.`);
	process.exit(0);
}

try {
	const pkgVersion = JSON.parse(readFileSync(join(pkgRoot, "package.json"), "utf-8")).version;
	const descriptors = [
		{
			id: "mach12",
			displayName: "Mach 12",
			sourceDir: join(pkgRoot, "mach12"),
			destinationDir: join(destinationParent, "mach12"),
			legacyMode: "backup-reseed",
		},
		{
			id: "scramjet",
			displayName: "Scramjet",
			sourceDir: join(pkgRoot, "scramjet"),
			destinationDir: join(destinationParent, "scramjet"),
			legacyMode: "refuse",
		},
	];
	for (const descriptor of descriptors) {
		try {
			seedDescriptor(descriptor, pkgVersion);
		} catch (err) {
			console.warn(`[scramjet] ${descriptor.displayName} seeding failed: ${errorMessage(err)}`);
		}
	}
} catch (err) {
	console.warn(`[scramjet] Command-set seeding failed: ${errorMessage(err)}`);
}
