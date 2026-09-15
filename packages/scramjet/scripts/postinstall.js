#!/usr/bin/env node

import { lstatSync, readdirSync, readlinkSync, rmSync, unlinkSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = fileURLToPath(new URL("..", import.meta.url));
const deprecatedExample = resolve(packageRoot, "..", "coding-agent", "examples", "extensions", "subagent");
const extension = join(homedir(), ".scramjet", "agent", "extensions", "subagent");
const deprecatedFiles = ["agents.ts", "index.ts"];

function errorCode(error) {
	return error && typeof error === "object" && "code" in error ? error.code : undefined;
}

function errorMessage(error) {
	return error instanceof Error ? error.message : String(error);
}

function symlinkTargets(path, expected) {
	if (!lstatSync(path).isSymbolicLink()) return false;
	return resolve(dirname(path), readlinkSync(path)) === expected;
}

function isDeprecatedManualDirectory(path) {
	const entries = readdirSync(path).sort();
	if (entries.length !== deprecatedFiles.length) return false;
	if (!deprecatedFiles.every((file, index) => entries[index] === file)) return false;
	return deprecatedFiles.every((file) => symlinkTargets(join(path, file), join(deprecatedExample, file)));
}

try {
	const stat = lstatSync(extension);
	const removable =
		(stat.isSymbolicLink() && symlinkTargets(extension, deprecatedExample)) ||
		(stat.isDirectory() && isDeprecatedManualDirectory(extension));
	if (removable) {
		console.warn(`[scramjet] Removing stale subagent extension at ${extension}`);
		if (stat.isSymbolicLink()) unlinkSync(extension);
		else rmSync(extension, { recursive: true, force: true });
	} else {
		console.warn(`[scramjet] Preserving ${extension}; remove it manually if it is the deprecated subagent extension.`);
	}
} catch (error) {
	if (errorCode(error) !== "ENOENT") {
		console.warn(`[scramjet] Stale extension check failed: ${errorMessage(error)}`);
	}
}
