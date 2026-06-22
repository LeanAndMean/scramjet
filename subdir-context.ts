import { readFile, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, relative, resolve, sep } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { isReadToolResult } from "@earendil-works/pi-coding-agent";
import type { ScramjetState } from "./types.ts";

export const MAX_DIRS = 20;
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
	filename: (typeof CANDIDATES)[number];
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
	logger?: ScramjetState["logger"],
): Promise<DiscoveredFile[]> {
	const results: DiscoveredFile[] = [];
	let realCwd: string;
	try {
		realCwd = await realpath(cwd);
	} catch (err: unknown) {
		if (logger && isNodeError(err) && err.code !== "ENOENT") {
			logger.warn("subdir-context", `realpath(cwd) failed: ${err.code}`, { cwd });
		}
		return results;
	}

	for (const dir of dirs) {
		if (loadedPaths.size >= MAX_DIRS) break;

		let realDir: string;
		try {
			realDir = await realpath(dir);
		} catch (err: unknown) {
			if (logger && isNodeError(err) && err.code !== "ENOENT") {
				logger.warn("subdir-context", `realpath(dir) failed: ${err.code}`, { dir });
			}
			continue;
		}

		if (!realDir.startsWith(realCwd + sep) && realDir !== realCwd) continue;
		if (loadedPaths.has(realDir)) continue;
		loadedPaths.add(realDir);

		for (const candidate of CANDIDATES) {
			const filePath = resolve(dir, candidate);
			try {
				const content = await readFile(filePath, "utf-8");
				results.push({ dir, filename: candidate, content });
			} catch (err: unknown) {
				if (logger && isNodeError(err) && err.code !== "ENOENT") {
					logger.warn("subdir-context", `readFile failed: ${err.code}`, { path: filePath });
				}
			}
		}
	}

	return results;
}

function isNodeError(err: unknown): err is NodeJS.ErrnoException {
	return err instanceof Error && "code" in err;
}

export function registerSubdirContext(pi: ExtensionAPI, state: ScramjetState): void {
	pi.on("tool_result", async (event, ctx) => {
		if (!isReadToolResult(event)) return;
		if (event.isError) return;
		const path = event.input.path;
		if (typeof path !== "string") return;

		const cwd = ctx.cwd;
		const dirs = directoriesToCheck(path, cwd);
		if (dirs.length === 0) return;

		const discovered = await discoverContextFiles(dirs, state.subdirLoadedPaths, cwd, state.logger);
		if (discovered.length === 0) return;

		const contextText = formatContextBlocks(discovered, cwd);
		const contextContent = { type: "text" as const, text: contextText };
		return { content: [contextContent, ...event.content] };
	});

	pi.on("session_compact", () => {
		state.subdirLoadedPaths.clear();
	});
}
