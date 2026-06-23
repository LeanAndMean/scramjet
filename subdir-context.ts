import { createHash } from "node:crypto";
import { readFile, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, relative, resolve, sep } from "node:path";
import type { ExtensionAPI, ExtensionContext, SessionEntry } from "@earendil-works/pi-coding-agent";
import { isReadToolResult } from "@earendil-works/pi-coding-agent";
import type { ScramjetState, SubdirDiscovery } from "./types.ts";

export const MAX_DIRS = 20;
export const MAX_DEPTH = 10;
export const CANDIDATES = ["CLAUDE.md", "AGENTS.md"] as const;
export const SUBDIR_CONTEXT_DISCOVERY_TYPE = "scramjet:subdir-context-discovery";

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

	if (!rel) return [];

	if (rel.startsWith("..") || resolve(normalizedCwd, rel) !== fileDir) {
		return [fileDir];
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
	filename: (typeof CANDIDATES)[number];
	content: string;
}

export async function discoverContextFiles(
	dirs: string[],
	loadedPaths: Set<string>,
	cwd: string,
	logger?: ScramjetState["logger"],
	skipCwdCheck?: boolean,
): Promise<DiscoveredFile[]> {
	const results: DiscoveredFile[] = [];
	let realCwd: string | null = null;
	if (!skipCwdCheck) {
		try {
			realCwd = await realpath(cwd);
		} catch (err: unknown) {
			if (logger && isNodeError(err) && err.code !== "ENOENT") {
				logger.warn("subdir-context", `realpath(cwd) failed: ${err.code}`, { cwd });
			}
			return results;
		}
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

		if (realCwd && !realDir.startsWith(realCwd + sep) && realDir !== realCwd) continue;
		if (loadedPaths.has(realDir)) continue;
		loadedPaths.add(realDir);

		for (const candidate of CANDIDATES) {
			const filePath = resolve(dir, candidate);
			try {
				const content = await readFile(filePath, "utf-8");
				results.push({ dir, realpath: realDir, filename: candidate, content });
			} catch (err: unknown) {
				if (logger && isNodeError(err) && err.code !== "ENOENT") {
					logger.warn("subdir-context", `readFile failed: ${err.code}`, { path: filePath });
				}
			}
		}
	}

	return results;
}

export function createStableId(displayPath: string): string {
	const hash = createHash("sha256").update(displayPath).digest("hex").slice(0, 12);
	return `scrctx-${hash}`;
}

function isNodeError(err: unknown): err is NodeJS.ErrnoException {
	return err instanceof Error && "code" in err;
}

function isOutsideCwd(filePath: string, cwd: string): boolean {
	const normalizedCwd = resolve(cwd);
	let resolved: string;
	if (filePath.startsWith("~/")) {
		resolved = resolve(homedir(), filePath.slice(2));
	} else {
		resolved = resolve(normalizedCwd, filePath);
	}
	const fileDir = dirname(resolved);
	const rel = relative(normalizedCwd, fileDir);
	return rel !== "" && (rel.startsWith("..") || resolve(normalizedCwd, rel) !== fileDir);
}

interface DiscoveryJournalEntry {
	toolCallId: string;
	realpath: string;
	filename: string;
	displayPath: string;
	content: string;
	syntheticId: string;
}

export function reconstructSubdirState(entries: readonly SessionEntry[]): {
	loadedPaths: Set<string>;
	discoveries: SubdirDiscovery[];
} {
	let loadedPaths = new Set<string>();
	let discoveries: SubdirDiscovery[] = [];

	for (const entry of entries) {
		if (entry.type === "compaction") {
			loadedPaths = new Set();
			discoveries = [];
			continue;
		}
		if (entry.type !== "custom" || entry.customType !== SUBDIR_CONTEXT_DISCOVERY_TYPE) continue;

		const data = entry.data as DiscoveryJournalEntry | undefined;
		if (
			!data ||
			typeof data.toolCallId !== "string" ||
			typeof data.realpath !== "string" ||
			typeof data.filename !== "string" ||
			typeof data.displayPath !== "string" ||
			typeof data.content !== "string" ||
			typeof data.syntheticId !== "string"
		) {
			continue;
		}

		loadedPaths.add(data.realpath);
		discoveries.push({
			toolCallId: data.toolCallId,
			realpath: data.realpath,
			filename: data.filename,
			displayPath: data.displayPath,
			content: data.content,
			syntheticId: data.syntheticId,
		});
	}

	return { loadedPaths, discoveries };
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

		const outsideCwd = isOutsideCwd(path, cwd);
		const discovered = await discoverContextFiles(dirs, state.subdirLoadedPaths, cwd, state.logger, outsideCwd);
		if (discovered.length === 0) return;

		for (const file of discovered) {
			const displayPath = relative(cwd, resolve(file.dir, file.filename)) || file.filename;
			const syntheticId = createStableId(displayPath);
			const discovery: SubdirDiscovery = {
				toolCallId: event.toolCallId,
				realpath: file.realpath,
				filename: file.filename,
				displayPath,
				content: file.content,
				syntheticId,
			};
			state.subdirDiscoveries.push(discovery);

			pi.appendEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, {
				toolCallId: event.toolCallId,
				realpath: file.realpath,
				filename: file.filename,
				displayPath,
				content: file.content,
				syntheticId,
			} satisfies DiscoveryJournalEntry);
		}
	});

	pi.on("session_compact", () => {
		state.subdirLoadedPaths.clear();
		state.subdirDiscoveries = [];
	});

	const rebuild = (_event: unknown, ctx: ExtensionContext) => {
		const result = reconstructSubdirState(ctx.sessionManager.getBranch());
		state.subdirLoadedPaths = result.loadedPaths;
		state.subdirDiscoveries = result.discoveries;
	};

	pi.on("session_start", rebuild);
	pi.on("session_tree", rebuild);
}
