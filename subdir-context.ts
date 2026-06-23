import { createHash } from "node:crypto";
import { readFile, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { AssistantMessage, ToolResultMessage, Usage } from "@earendil-works/pi-ai";
import type { ExtensionAPI, ExtensionContext, SessionEntry } from "@earendil-works/pi-coding-agent";
import { isReadToolResult } from "@earendil-works/pi-coding-agent";
import type { ScramjetState, SubdirDiscovery } from "./types.ts";

export const MAX_DIRS = 20;
export const MAX_DEPTH = 10;
export const CANDIDATES = ["CLAUDE.md", "AGENTS.md"] as const;
export const SUBDIR_CONTEXT_DISCOVERY_TYPE = "scramjet:subdir-context-discovery";

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

export interface DiscoveredFile {
	dir: string;
	dirRealpath: string;
	filename: (typeof CANDIDATES)[number];
	content: string;
}

export async function discoverContextFiles(
	dirs: string[],
	loadedPaths: Set<string>,
	cwd: string,
	logger?: ScramjetState["logger"],
	options?: { enforceCwdBoundary?: boolean },
): Promise<DiscoveredFile[]> {
	const results: DiscoveredFile[] = [];
	const enforceCwdBoundary = options?.enforceCwdBoundary ?? true;
	let realCwd: string | null = null;
	if (enforceCwdBoundary) {
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

		if (realCwd && !isPathInsideOrEqual(realCwd, realDir)) continue;
		if (loadedPaths.has(realDir)) continue;
		loadedPaths.add(realDir);

		for (const candidate of CANDIDATES) {
			const filePath = resolve(dir, candidate);
			try {
				const content = await readFile(filePath, "utf-8");
				results.push({ dir, dirRealpath: realDir, filename: candidate, content });
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

interface DiscoveryJournalEntry {
	toolCallId: string;
	dirRealpath: string;
	filename: SubdirDiscovery["filename"];
	displayPath: string;
	content: string;
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
			typeof data.dirRealpath !== "string" ||
			(data.filename !== "CLAUDE.md" && data.filename !== "AGENTS.md") ||
			typeof data.displayPath !== "string" ||
			typeof data.content !== "string"
		) {
			continue;
		}

		loadedPaths.add(data.dirRealpath);
		discoveries.push({
			toolCallId: data.toolCallId,
			dirRealpath: data.dirRealpath,
			filename: data.filename,
			displayPath: data.displayPath,
			content: data.content,
		});
	}

	return { loadedPaths, discoveries };
}

const ZERO_USAGE: Usage = {
	input: 0,
	output: 0,
	cacheRead: 0,
	cacheWrite: 0,
	totalTokens: 0,
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
};

function isAssistantMessage(message: AgentMessage): message is AssistantMessage {
	return message.role === "assistant" && Array.isArray((message as AssistantMessage).content);
}

export function findAnchorIndex(messages: readonly AgentMessage[], toolCallId: string): number {
	for (let i = messages.length - 1; i >= 0; i--) {
		const msg = messages[i];
		if (!isAssistantMessage(msg)) continue;
		for (const block of msg.content) {
			if (block.type === "toolCall" && block.id === toolCallId) return i;
		}
	}
	return -1;
}

export function buildSyntheticPair(
	discovery: SubdirDiscovery,
	anchor: AssistantMessage,
): [AssistantMessage, ToolResultMessage] {
	const syntheticId = createStableId(discovery.displayPath);
	const assistantMsg: AssistantMessage = {
		role: "assistant",
		content: [{ type: "toolCall", id: syntheticId, name: "read", arguments: { path: discovery.displayPath } }],
		api: anchor.api,
		provider: anchor.provider,
		model: anchor.model,
		usage: ZERO_USAGE,
		stopReason: "toolUse",
		timestamp: 0,
	};
	const resultMsg: ToolResultMessage = {
		role: "toolResult",
		toolCallId: syntheticId,
		toolName: "read",
		content: [{ type: "text", text: `# Project context: ${discovery.displayPath}\n\n${discovery.content}` }],
		isError: false,
		timestamp: 0,
	};
	return [assistantMsg, resultMsg];
}

export function formatContextBlocks(
	discoveries: readonly SubdirDiscovery[],
	messages: readonly AgentMessage[],
	logger?: ScramjetState["logger"],
): AgentMessage[] | undefined {
	if (discoveries.length === 0) return undefined;

	const anchorGroups = new Map<number, SubdirDiscovery[]>();
	for (const d of discoveries) {
		const syntheticId = createStableId(d.displayPath);
		if (
			messages.some(
				(m) => isAssistantMessage(m) && m.content.some((b) => b.type === "toolCall" && b.id === syntheticId),
			)
		) {
			continue;
		}

		const idx = findAnchorIndex(messages, d.toolCallId);
		if (idx === -1) {
			logger?.debug("subdir-context", `anchor missing for ${d.displayPath} (toolCallId=${d.toolCallId})`);
			continue;
		}
		const group = anchorGroups.get(idx) ?? [];
		group.push(d);
		anchorGroups.set(idx, group);
	}

	if (anchorGroups.size === 0) return undefined;

	const sortedIndices = [...anchorGroups.keys()].sort((a, b) => a - b);
	const result: AgentMessage[] = [];
	let lastInserted = 0;

	for (const anchorIdx of sortedIndices) {
		result.push(...messages.slice(lastInserted, anchorIdx));
		const anchor = messages[anchorIdx];
		if (!isAssistantMessage(anchor)) {
			lastInserted = anchorIdx;
			continue;
		}
		const group = anchorGroups.get(anchorIdx)!;
		for (const d of group) {
			const [synAssistant, synResult] = buildSyntheticPair(d, anchor);
			result.push(synAssistant, synResult);
		}
		lastInserted = anchorIdx;
	}
	result.push(...messages.slice(lastInserted));

	return result;
}

export function registerSubdirContext(pi: ExtensionAPI, state: ScramjetState): void {
	pi.on("context", (event) => {
		const modified = formatContextBlocks(state.subdirDiscoveries, event.messages, state.logger);
		if (!modified) return;
		return { messages: modified };
	});

	pi.on("tool_result", async (event, ctx) => {
		if (!isReadToolResult(event)) return;
		if (event.isError) return;
		const path = event.input.path;
		if (typeof path !== "string") return;

		const cwd = ctx.cwd;
		const { dirs, outsideCwd } = directoriesToCheck(path, cwd);
		if (dirs.length === 0) return;

		const enforceCwdBoundary = !outsideCwd;
		const discovered = await discoverContextFiles(dirs, state.subdirLoadedPaths, cwd, state.logger, {
			enforceCwdBoundary,
		});
		if (discovered.length === 0) return;

		for (const file of discovered) {
			const displayPath = relative(cwd, resolve(file.dir, file.filename)) || file.filename;
			const discovery: SubdirDiscovery = {
				toolCallId: event.toolCallId,
				dirRealpath: file.dirRealpath,
				filename: file.filename,
				displayPath,
				content: file.content,
			};
			state.subdirDiscoveries.push(discovery);

			pi.appendEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, {
				toolCallId: event.toolCallId,
				dirRealpath: file.dirRealpath,
				filename: file.filename,
				displayPath,
				content: file.content,
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
