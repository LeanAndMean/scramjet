import { constants } from "node:fs";
import { access, readFile, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, relative, resolve, sep } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { isReadToolResult } from "@earendil-works/pi-coding-agent";
import type { ScramjetState } from "./types.ts";

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

export function formatContextBlocks(files: DiscoveredFile[], cwd: string): string {
	return files
		.map((f) => {
			const relPath = relative(cwd, resolve(f.dir, f.filename));
			return `# Project context: ${relPath}\n\n${f.content}\n\n---`;
		})
		.join("\n\n");
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

export function registerSubdirContext(pi: ExtensionAPI, state: ScramjetState): void {
	pi.on("tool_result", async (event, ctx) => {
		if (!isReadToolResult(event)) return;
		if (event.isError) return;
		const path = event.input.path;
		if (typeof path !== "string") return;

		const cwd = (ctx as { cwd?: string }).cwd ?? process.cwd();
		const dirs = directoriesToCheck(path, cwd);
		if (dirs.length === 0) return;

		const discovered = await discoverContextFiles(dirs, state.subdirLoadedPaths, cwd);
		if (discovered.length === 0) return;

		const contextText = formatContextBlocks(discovered, cwd);
		const contextContent = { type: "text" as const, text: contextText };
		return { content: [contextContent, ...event.content] };
	});

	pi.on("session_compact", () => {
		state.subdirLoadedPaths.clear();
	});
}
