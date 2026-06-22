import { constants } from "node:fs";
import { access, readFile, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, relative, resolve, sep } from "node:path";

export const MAX_FILES = 20;
export const MAX_DEPTH = 10;
export const CANDIDATES = ["CLAUDE.md", "AGENTS.md"] as const;

export function directoriesToCheck(filePath: string, cwd: string): string[] {
	const normalizedCwd = resolve(cwd);
	let resolved: string;
	if (filePath.startsWith("~/")) {
		resolved = resolve(homedir(), filePath.slice(2));
	} else {
		resolved = resolve(normalizedCwd, filePath);
	}

	const fileDir = dirname(resolved);
	const rel = relative(normalizedCwd, fileDir);
	if (!rel || rel.startsWith("..") || resolve(normalizedCwd, rel) !== fileDir) {
		return [];
	}

	const parts = rel.split(sep).filter(Boolean);
	const dirs: string[] = [];
	for (let i = 0; i < Math.min(parts.length, MAX_DEPTH); i++) {
		dirs.push(resolve(normalizedCwd, ...parts.slice(0, i + 1)));
	}
	return dirs;
}

export interface DiscoveredFile {
	dir: string;
	realpath: string;
	filename: string;
	content: string;
}

export async function discoverContextFiles(
	dirs: string[],
	loadedPaths: Set<string>,
	cwd: string,
): Promise<DiscoveredFile[]> {
	const results: DiscoveredFile[] = [];
	let realCwd: string;
	try {
		realCwd = await realpath(cwd);
	} catch {
		return results;
	}

	for (const dir of dirs) {
		if (loadedPaths.size >= MAX_FILES) break;

		let realDir: string;
		try {
			realDir = await realpath(dir);
		} catch {
			continue;
		}

		if (!realDir.startsWith(realCwd + sep) && realDir !== realCwd) continue;
		if (loadedPaths.has(realDir)) continue;
		loadedPaths.add(realDir);

		for (const candidate of CANDIDATES) {
			if (loadedPaths.size > MAX_FILES + dirs.length) break;
			const filePath = resolve(dir, candidate);
			try {
				await access(filePath, constants.R_OK);
				const content = await readFile(filePath, "utf-8");
				results.push({ dir, realpath: realDir, filename: candidate, content });
			} catch {}
		}
	}

	return results;
}
