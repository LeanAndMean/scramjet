import { createHash } from "node:crypto";
import { access, constants, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, relative, resolve, sep } from "node:path";
import type { AgentMessage } from "@leanandmean/agent";
import type { AssistantMessage, ToolCall } from "@leanandmean/ai";
import type { ExtensionAPI, ExtensionContext, SessionEntry } from "@leanandmean/coding-agent";
import type { ScramjetState } from "./types.js";

export const MAX_DIRS = 20;
export const MAX_DEPTH = 10;
export const CANDIDATES = ["CLAUDE.md", "AGENTS.md"] as const;

function normalizeReadPathInput(filePath: string): string {
	return filePath.startsWith("@") ? filePath.slice(1) : filePath;
}

function isOutsideRelativePath(rel: string): boolean {
	return rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel);
}

function isPathInsideOrEqual(parent: string, child: string): boolean {
	const rel = relative(parent, child);
	return rel === "" || !isOutsideRelativePath(rel);
}

export function directoriesToCheck(filePath: string, cwd: string): { dirs: string[]; outsideCwd: boolean } {
	const normalizedCwd = resolve(cwd);
	const normalizedInput = normalizeReadPathInput(filePath);
	let resolved: string;
	if (normalizedInput.startsWith("~/")) {
		resolved = resolve(homedir(), normalizedInput.slice(2));
	} else {
		resolved = resolve(normalizedCwd, normalizedInput);
	}

	const fileDir = dirname(resolved);
	const rel = relative(normalizedCwd, fileDir);

	if (!rel) return { dirs: [], outsideCwd: false };

	if (isOutsideRelativePath(rel)) {
		return { dirs: [fileDir], outsideCwd: true };
	}

	const parts = rel.split(sep).filter(Boolean);
	const dirs: string[] = [];
	for (let i = 0; i < Math.min(parts.length, MAX_DEPTH); i++) {
		dirs.push(resolve(normalizedCwd, ...parts.slice(0, i + 1)));
	}
	return { dirs, outsideCwd: false };
}

export interface DiscoveredPath {
	dir: string;
	dirRealpath: string;
	filename: (typeof CANDIDATES)[number];
	displayPath: string;
}

function isNodeError(err: unknown): err is NodeJS.ErrnoException {
	return err instanceof Error && "code" in err;
}

export async function discoverContextFilePaths(
	dirs: string[],
	loadedPaths: Set<string>,
	cwd: string,
	logger?: ScramjetState["logger"],
	options?: { enforceCwdBoundary?: boolean },
): Promise<DiscoveredPath[]> {
	const results: DiscoveredPath[] = [];
	const enforceCwdBoundary = options?.enforceCwdBoundary ?? true;
	let realCwd: string | null = null;
	if (enforceCwdBoundary) {
		try {
			realCwd = await realpath(cwd);
		} catch (err: unknown) {
			if (logger && isNodeError(err)) {
				if (err.code === "ENOENT") {
					logger.debug("subdir-context", "realpath(cwd) failed: ENOENT (deleted working directory?)", { cwd });
				} else {
					logger.warn("subdir-context", `realpath(cwd) failed: ${err.code}`, { cwd });
				}
			} else if (logger) {
				logger.warn("subdir-context", `realpath(cwd) failed: ${err}`, { cwd });
			}
			return results;
		}
	}

	const normalizedCwd = resolve(cwd);

	for (const dir of dirs) {
		if (loadedPaths.size >= MAX_DIRS) break;

		let realDir: string;
		try {
			realDir = await realpath(dir);
		} catch (err: unknown) {
			if (isNodeError(err)) {
				if (logger && err.code !== "ENOENT") {
					logger.warn("subdir-context", `realpath(dir) failed: ${err.code}`, { dir });
				}
			} else if (logger) {
				logger.warn("subdir-context", `realpath(dir) failed: ${err}`, { dir });
			}
			continue;
		}

		if (realCwd && !isPathInsideOrEqual(realCwd, realDir)) continue;
		if (loadedPaths.has(realDir)) continue;
		loadedPaths.add(realDir);

		let anyAccessible = false;
		let allFailuresTransient = true;
		for (const candidate of CANDIDATES) {
			const filePath = resolve(dir, candidate);
			try {
				await access(filePath, constants.R_OK);
				const rel = relative(normalizedCwd, filePath);
				const displayPath = isOutsideRelativePath(rel) ? filePath : rel;
				results.push({ dir, dirRealpath: realDir, filename: candidate, displayPath });
				anyAccessible = true;
			} catch (err: unknown) {
				if (isNodeError(err) && err.code === "ENOENT") {
					allFailuresTransient = false;
				} else if (logger && isNodeError(err)) {
					logger.warn("subdir-context", `access check failed: ${err.code}`, { path: filePath });
				} else if (logger) {
					logger.warn("subdir-context", `access check failed: ${err}`, { path: filePath });
				}
			}
		}

		if (!anyAccessible && allFailuresTransient) {
			loadedPaths.delete(realDir);
		}
	}

	return results;
}

export function createStableId(input: string): string {
	const hash = createHash("sha256").update(input).digest("hex").slice(0, 12);
	return `scrctx-${hash}`;
}

function isAssistantMessage(message: AgentMessage): message is AssistantMessage {
	return message.role === "assistant" && Array.isArray((message as AssistantMessage).content);
}

function isReadToolCall(block: ToolCall): boolean {
	return block.name === "read" && typeof block.arguments?.path === "string";
}

function isCandidateFile(path: string): boolean {
	const base = basename(path);
	return CANDIDATES.includes(base as (typeof CANDIDATES)[number]);
}

export async function reconstructSubdirState(
	entries: readonly SessionEntry[],
	cwd: string,
	logger?: ScramjetState["logger"],
): Promise<Set<string>> {
	let loadedPaths = new Set<string>();

	const toolCallPaths = new Map<string, string>();
	const successfulToolCallIds = new Set<string>();

	for (const entry of entries) {
		if (entry.type === "compaction") {
			loadedPaths = new Set();
			toolCallPaths.clear();
			successfulToolCallIds.clear();
			continue;
		}

		if (entry.type === "message") {
			const msg = (entry as any).message;
			if (!msg) continue;

			if (msg.role === "assistant" && Array.isArray(msg.content)) {
				for (const block of msg.content) {
					if (block.type === "toolCall" && block.name === "read" && typeof block.arguments?.path === "string") {
						if (isCandidateFile(block.arguments.path)) {
							toolCallPaths.set(block.id, block.arguments.path);
						}
					}
				}
			}

			if (msg.role === "toolResult" && !msg.isError && typeof msg.toolCallId === "string") {
				if (toolCallPaths.has(msg.toolCallId)) {
					successfulToolCallIds.add(msg.toolCallId);
				}
			}
		}
	}

	for (const toolCallId of successfulToolCallIds) {
		const filePath = toolCallPaths.get(toolCallId)!;
		const resolved = resolve(cwd, filePath);
		const dir = dirname(resolved);
		try {
			const realDir = await realpath(dir);
			loadedPaths.add(realDir);
		} catch (err: unknown) {
			if (isNodeError(err) && err.code === "ENOENT") {
				logger?.debug("subdir-context", `reconstruction skipped (dir removed): ${filePath}`);
			} else {
				logger?.warn("subdir-context", `reconstruction failed: ${isNodeError(err) ? err.code : err}`, {
					path: filePath,
				});
			}
		}
	}

	return loadedPaths;
}

export function registerSubdirContext(pi: ExtensionAPI, state: ScramjetState): void {
	pi.on("message_end", async (event, ctx) => {
		const message = event.message;
		if (!isAssistantMessage(message)) return;

		const cwd = ctx.cwd;
		const toolCalls = message.content.filter((b): b is ToolCall => b.type === "toolCall");
		if (toolCalls.length === 0) return;

		const readToolCalls = toolCalls.filter((tc) => {
			if (!isReadToolCall(tc)) return false;
			if (tc.id.startsWith("scrctx-")) return false;
			if (isCandidateFile(tc.arguments.path)) return false;
			return true;
		});

		if (readToolCalls.length === 0) return;

		const injections: { beforeIndex: number; newCalls: ToolCall[] }[] = [];

		for (const tc of readToolCalls) {
			const path = tc.arguments.path as string;
			const { dirs, outsideCwd } = directoriesToCheck(path, cwd);
			if (dirs.length === 0) continue;

			const enforceCwdBoundary = !outsideCwd;
			const discovered = await discoverContextFilePaths(dirs, state.subdirLoadedPaths, cwd, state.logger, {
				enforceCwdBoundary,
			});
			if (discovered.length === 0) continue;

			const newCalls: ToolCall[] = [];
			for (const file of discovered) {
				const stableId = createStableId(`${tc.id}\0${file.displayPath}`);
				newCalls.push({
					type: "toolCall",
					id: stableId,
					name: "read",
					arguments: { path: file.displayPath },
				});
			}

			const idx = message.content.indexOf(tc);
			if (idx !== -1) {
				injections.push({ beforeIndex: idx, newCalls });
			}
		}

		if (injections.length === 0) return;

		injections.sort((a, b) => b.beforeIndex - a.beforeIndex);
		const newContent = [...message.content];
		for (const { beforeIndex, newCalls } of injections) {
			newContent.splice(beforeIndex, 0, ...newCalls);
		}

		const replacement: AssistantMessage = { ...message, content: newContent };
		return { message: replacement };
	});

	pi.on("session_compact", () => {
		state.subdirLoadedPaths.clear();
	});

	const rebuild = async (_event: unknown, ctx: ExtensionContext) => {
		state.subdirLoadedPaths = await reconstructSubdirState(ctx.sessionManager.getBranch(), ctx.cwd, state.logger);
	};

	pi.on("session_start", rebuild);
	pi.on("session_tree", rebuild);
}
