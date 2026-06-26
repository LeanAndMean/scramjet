import { chmod, mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import type { AssistantMessage, ToolCall, Usage } from "@leanandmean/ai";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	CANDIDATES,
	createStableId,
	directoriesToCheck,
	discoverContextFilePaths,
	MAX_DEPTH,
	MAX_DIRS,
	reconstructSubdirState,
	registerSubdirContext,
} from "../src/subdir-context.js";
import { freshState, recordingPi } from "./helpers.js";

describe("directoriesToCheck", () => {
	it("returns intermediate dirs for inside-cwd paths", () => {
		const result = directoriesToCheck("a/b/c/file.ts", "/project");
		expect(result).toEqual({ dirs: ["/project/a", "/project/a/b", "/project/a/b/c"], outsideCwd: false });
	});

	it("returns empty at cwd level", () => {
		const result = directoriesToCheck("file.ts", "/project");
		expect(result).toEqual({ dirs: [], outsideCwd: false });
	});

	it("returns [fileDir] for file outside cwd", () => {
		const result = directoriesToCheck("/other/path/file.ts", "/project");
		expect(result).toEqual({ dirs: ["/other/path"], outsideCwd: true });
	});

	it("caps at MAX_DEPTH", () => {
		const deep = `${Array.from({ length: 15 }, (_, i) => `d${i}`).join("/")}/file.ts`;
		const result = directoriesToCheck(deep, "/project");
		expect(result.dirs).toHaveLength(MAX_DEPTH);
		expect(result.outsideCwd).toBe(false);
	});

	it("handles absolute paths inside cwd", () => {
		const result = directoriesToCheck("/project/sub/dir/file.ts", "/project");
		expect(result).toEqual({ dirs: ["/project/sub", "/project/sub/dir"], outsideCwd: false });
	});

	it("treats inside-cwd dot-dot-prefixed directory names as inside cwd", () => {
		const result = directoriesToCheck("..data/file.ts", "/project");
		expect(result).toEqual({ dirs: ["/project/..data"], outsideCwd: false });
	});

	it("normalizes .. segments", () => {
		const result = directoriesToCheck("a/b/../c/file.ts", "/project");
		expect(result).toEqual({ dirs: ["/project/a", "/project/a/c"], outsideCwd: false });
	});

	it("handles ~/ paths by expanding to homedir", () => {
		const home = homedir();
		const result = directoriesToCheck("~/sub/dir/file.ts", home);
		expect(result).toEqual({ dirs: [join(home, "sub"), join(home, "sub/dir")], outsideCwd: false });
	});

	it("returns [fileDir] for ~/ paths when cwd is not homedir", () => {
		const home = homedir();
		const result = directoriesToCheck("~/sub/file.ts", "/project");
		expect(result).toEqual({ dirs: [join(home, "sub")], outsideCwd: true });
	});

	it("returns single dir for one-level deep file", () => {
		const result = directoriesToCheck("sub/file.ts", "/project");
		expect(result).toEqual({ dirs: ["/project/sub"], outsideCwd: false });
	});

	it("handles trailing slashes in cwd", () => {
		const result = directoriesToCheck("a/b/file.ts", "/project/");
		expect(result).toEqual({ dirs: ["/project/a", "/project/a/b"], outsideCwd: false });
	});

	it("normalizes one leading at-prefix like Pi's read tool", () => {
		const result = directoriesToCheck("@pkg/file.ts", "/project");
		expect(result).toEqual({ dirs: ["/project/pkg"], outsideCwd: false });
	});
});

describe("discoverContextFilePaths", () => {
	let tmpDir: string;

	beforeEach(async () => {
		tmpDir = await mkdtemp(join(tmpdir(), "subdir-ctx-"));
	});

	afterEach(async () => {
		await rm(tmpDir, { recursive: true, force: true });
	});

	it("finds CLAUDE.md in directory", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "# Sub instructions");

		const loaded = new Set<string>();
		const result = await discoverContextFilePaths([subDir], loaded, tmpDir);
		expect(result).toHaveLength(1);
		expect(result[0].filename).toBe("CLAUDE.md");
		expect(result[0].displayPath).toContain("CLAUDE.md");
		expect(result[0].dirRealpath).toBeDefined();
	});

	it("finds AGENTS.md in directory", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "AGENTS.md"), "agent stuff");

		const loaded = new Set<string>();
		const result = await discoverContextFilePaths([subDir], loaded, tmpDir);
		expect(result).toHaveLength(1);
		expect(result[0].filename).toBe("AGENTS.md");
	});

	it("finds both CLAUDE.md and AGENTS.md in same directory", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "claude");
		await writeFile(join(subDir, "AGENTS.md"), "agents");

		const loaded = new Set<string>();
		const result = await discoverContextFilePaths([subDir], loaded, tmpDir);
		expect(result).toHaveLength(2);
		expect(result[0].filename).toBe("CLAUDE.md");
		expect(result[1].filename).toBe("AGENTS.md");
	});

	it("skips already-loaded directories", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "content");

		const { realpath: rp } = await import("node:fs/promises");
		const realSubDir = await rp(subDir);
		const loaded = new Set<string>([realSubDir]);
		const result = await discoverContextFilePaths([subDir], loaded, tmpDir);
		expect(result).toHaveLength(0);
	});

	it("skips symlinks pointing outside cwd", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "outside content");

		const linkDir = join(tmpDir, "linked");
		await symlink(outsideDir, linkDir);

		const loaded = new Set<string>();
		const result = await discoverContextFilePaths([linkDir], loaded, tmpDir);
		expect(result).toHaveLength(0);

		await rm(outsideDir, { recursive: true, force: true });
	});

	it("discovers outside-cwd dirs when cwd boundary enforcement is disabled", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "outside content");

		const loaded = new Set<string>();
		const result = await discoverContextFilePaths([outsideDir], loaded, "/nonexistent-cwd", undefined, {
			enforceCwdBoundary: false,
		});
		expect(result).toHaveLength(1);
		expect(result[0].filename).toBe("CLAUDE.md");

		await rm(outsideDir, { recursive: true, force: true });
	});

	it("retries directory on next call when all candidates fail with transient errors", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "content");
		await writeFile(join(subDir, "AGENTS.md"), "content");
		await chmod(join(subDir, "CLAUDE.md"), 0o000);
		await chmod(join(subDir, "AGENTS.md"), 0o000);

		const loaded = new Set<string>();
		const result1 = await discoverContextFilePaths([subDir], loaded, tmpDir);
		expect(result1).toHaveLength(0);
		expect(loaded.size).toBe(0);

		await chmod(join(subDir, "CLAUDE.md"), 0o644);
		await chmod(join(subDir, "AGENTS.md"), 0o644);

		const result2 = await discoverContextFilePaths([subDir], loaded, tmpDir);
		expect(result2).toHaveLength(2);
		expect(loaded.size).toBe(1);
	});

	it("keeps directory claimed when all candidates are ENOENT", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);

		const loaded = new Set<string>();
		const result = await discoverContextFilePaths([subDir], loaded, tmpDir);
		expect(result).toHaveLength(0);
		expect(loaded.size).toBe(1);
	});

	it("keeps directory claimed when at least one candidate succeeds", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "found");

		const loaded = new Set<string>();
		const result = await discoverContextFilePaths([subDir], loaded, tmpDir);
		expect(result).toHaveLength(1);
		expect(loaded.size).toBe(1);
	});

	it("logs non-ENOENT access errors", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "secret");
		await chmod(join(subDir, "CLAUDE.md"), 0o000);

		const warnings: Array<{ category: string; message: string; data?: Record<string, unknown> }> = [];
		const logger = {
			warn: (category: string, message: string, data?: Record<string, unknown>) => {
				warnings.push({ category, message, data });
			},
		} as any;

		const loaded = new Set<string>();
		await discoverContextFilePaths([subDir], loaded, tmpDir, logger);
		expect(warnings).toContainEqual({
			category: "subdir-context",
			message: expect.stringContaining("access check failed"),
			data: { path: join(subDir, "CLAUDE.md") },
		});

		await chmod(join(subDir, "CLAUDE.md"), 0o644);
	});

	it("skips unreadable files", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "secret");
		await chmod(join(subDir, "CLAUDE.md"), 0o000);

		const loaded = new Set<string>();
		const result = await discoverContextFilePaths([subDir], loaded, tmpDir);
		expect(result).toHaveLength(0);

		await chmod(join(subDir, "CLAUDE.md"), 0o644);
	});

	it("respects MAX_DIRS cap", async () => {
		const loaded = new Set<string>();
		const dirs: string[] = [];
		for (let i = 0; i < MAX_DIRS + 5; i++) {
			const subDir = join(tmpDir, `dir${i}`);
			await mkdir(subDir);
			await writeFile(join(subDir, "CLAUDE.md"), `content ${i}`);
			dirs.push(subDir);
		}

		const result = await discoverContextFilePaths(dirs, loaded, tmpDir);
		expect(result.length).toBeLessThanOrEqual(MAX_DIRS * CANDIDATES.length);
		expect(loaded.size).toBeLessThanOrEqual(MAX_DIRS);
	});

	it("returns files ordered shallowest-first", async () => {
		const dirA = join(tmpDir, "a");
		const dirABC = join(tmpDir, "a", "b", "c");
		await mkdir(dirABC, { recursive: true });
		await writeFile(join(dirA, "CLAUDE.md"), "a");
		await writeFile(join(dirABC, "CLAUDE.md"), "abc");

		const loaded = new Set<string>();
		const result = await discoverContextFilePaths([dirA, join(tmpDir, "a", "b"), dirABC], loaded, tmpDir);
		expect(result[0].displayPath).toContain("a");
		expect(result[1].displayPath).toContain(join("a", "b", "c"));
	});

	it("synchronous claim prevents duplicate entries from concurrent calls", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "content");

		const loaded = new Set<string>();
		const [r1, r2] = await Promise.all([
			discoverContextFilePaths([subDir], loaded, tmpDir),
			discoverContextFilePaths([subDir], loaded, tmpDir),
		]);
		const total = r1.length + r2.length;
		expect(total).toBe(1);
	});

	it("handles directory that does not exist", async () => {
		const loaded = new Set<string>();
		const result = await discoverContextFilePaths([join(tmpDir, "nonexistent")], loaded, tmpDir);
		expect(result).toHaveLength(0);
	});

	it("logs cwd ENOENT at debug level", async () => {
		const debugCalls: Array<{ category: string; message: string; data?: Record<string, unknown> }> = [];
		const logger = {
			warn: () => {},
			debug: (category: string, message: string, data?: Record<string, unknown>) => {
				debugCalls.push({ category, message, data });
			},
		} as any;

		const loaded = new Set<string>();
		await discoverContextFilePaths([join(tmpDir, "sub")], loaded, join(tmpDir, "nonexistent-cwd"), logger);
		expect(debugCalls).toHaveLength(1);
		expect(debugCalls[0].message).toContain("ENOENT");
	});

	it("produces relative displayPath for inside-cwd files", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const loaded = new Set<string>();
		const result = await discoverContextFilePaths([subDir], loaded, tmpDir);
		expect(result[0].displayPath).toBe(join("pkg", "CLAUDE.md"));
	});

	it("produces absolute displayPath for outside-cwd files", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "outside");

		const loaded = new Set<string>();
		const result = await discoverContextFilePaths([outsideDir], loaded, tmpDir, undefined, {
			enforceCwdBoundary: false,
		});
		expect(result[0].displayPath).toBe(join(outsideDir, "CLAUDE.md"));

		await rm(outsideDir, { recursive: true, force: true });
	});
});

describe("createStableId", () => {
	it("produces a stable deterministic id", () => {
		const id1 = createStableId("tc_1\0pkg/sub/CLAUDE.md");
		const id2 = createStableId("tc_1\0pkg/sub/CLAUDE.md");
		expect(id1).toBe(id2);
	});

	it("produces different ids for different paths", () => {
		const id1 = createStableId("tc_1\0a/CLAUDE.md");
		const id2 = createStableId("tc_1\0b/CLAUDE.md");
		expect(id1).not.toBe(id2);
	});

	it("produces different ids for same path with different source tool calls", () => {
		const id1 = createStableId("tc_1\0pkg/CLAUDE.md");
		const id2 = createStableId("tc_2\0pkg/CLAUDE.md");
		expect(id1).not.toBe(id2);
	});

	it("starts with scrctx- prefix", () => {
		const id = createStableId("tc_1\0some/path/CLAUDE.md");
		expect(id).toMatch(/^scrctx-[a-f0-9]{12}$/);
	});

	it("is provider-safe (alphanumeric and hyphen)", () => {
		const id = createStableId("tc_1\0path/with/special chars/CLAUDE.md");
		expect(id).toMatch(/^[A-Za-z0-9_-]+$/);
	});
});

describe("reconstructSubdirState", () => {
	let tmpDir: string;

	beforeEach(async () => {
		tmpDir = await mkdtemp(join(tmpdir(), "subdir-recon-"));
	});

	afterEach(async () => {
		await rm(tmpDir, { recursive: true, force: true });
	});

	function makeMessageEntry(message: any) {
		return { type: "message" as const, message, id: "e1", parentId: null, timestamp: "0" };
	}

	function makeCompactionEntry() {
		return {
			type: "compaction" as const,
			summary: "compacted",
			firstKeptEntryId: "e0",
			tokensBefore: 1000,
			id: "c1",
			parentId: null,
			timestamp: "0",
		};
	}

	it("returns empty state for no entries", async () => {
		const result = await reconstructSubdirState([], tmpDir);
		expect(result.size).toBe(0);
	});

	it("restores loaded paths from successful read call/result pairs", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const entries = [
			makeMessageEntry({
				role: "assistant",
				content: [{ type: "toolCall", id: "tc_1", name: "read", arguments: { path: "sub/CLAUDE.md" } }],
			}),
			makeMessageEntry({
				role: "toolResult",
				toolCallId: "tc_1",
				toolName: "read",
				isError: false,
				content: [{ type: "text", text: "rules" }],
			}),
		];
		const result = await reconstructSubdirState(entries as any, tmpDir);
		expect(result.size).toBe(1);
		expect(result.has(subDir)).toBe(true);
	});

	it("ignores failed read results", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const entries = [
			makeMessageEntry({
				role: "assistant",
				content: [{ type: "toolCall", id: "tc_1", name: "read", arguments: { path: "sub/CLAUDE.md" } }],
			}),
			makeMessageEntry({
				role: "toolResult",
				toolCallId: "tc_1",
				toolName: "read",
				isError: true,
				content: [{ type: "text", text: "error" }],
			}),
		];
		const result = await reconstructSubdirState(entries as any, tmpDir);
		expect(result.size).toBe(0);
	});

	it("ignores read calls for non-candidate files", async () => {
		const entries = [
			makeMessageEntry({
				role: "assistant",
				content: [{ type: "toolCall", id: "tc_1", name: "read", arguments: { path: "sub/file.ts" } }],
			}),
			makeMessageEntry({
				role: "toolResult",
				toolCallId: "tc_1",
				toolName: "read",
				isError: false,
				content: [{ type: "text", text: "code" }],
			}),
		];
		const result = await reconstructSubdirState(entries as any, tmpDir);
		expect(result.size).toBe(0);
	});

	it("resets state at compaction entries", async () => {
		const subDir = join(tmpDir, "sub");
		const pkgDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await mkdir(pkgDir);
		await writeFile(join(subDir, "CLAUDE.md"), "old");
		await writeFile(join(pkgDir, "AGENTS.md"), "new");

		const entries = [
			makeMessageEntry({
				role: "assistant",
				content: [{ type: "toolCall", id: "tc_1", name: "read", arguments: { path: "sub/CLAUDE.md" } }],
			}),
			makeMessageEntry({
				role: "toolResult",
				toolCallId: "tc_1",
				toolName: "read",
				isError: false,
				content: [{ type: "text", text: "old" }],
			}),
			makeCompactionEntry(),
			makeMessageEntry({
				role: "assistant",
				content: [{ type: "toolCall", id: "tc_2", name: "read", arguments: { path: "pkg/AGENTS.md" } }],
			}),
			makeMessageEntry({
				role: "toolResult",
				toolCallId: "tc_2",
				toolName: "read",
				isError: false,
				content: [{ type: "text", text: "new" }],
			}),
		];
		const result = await reconstructSubdirState(entries as any, tmpDir);
		expect(result.size).toBe(1);
		expect(result.has(pkgDir)).toBe(true);
		expect(result.has(subDir)).toBe(false);
	});

	it("handles unmatched tool call (no result)", async () => {
		const entries = [
			makeMessageEntry({
				role: "assistant",
				content: [{ type: "toolCall", id: "tc_1", name: "read", arguments: { path: "sub/CLAUDE.md" } }],
			}),
		];
		const result = await reconstructSubdirState(entries as any, tmpDir);
		expect(result.size).toBe(0);
	});

	it("handles nonexistent directory gracefully", async () => {
		const entries = [
			makeMessageEntry({
				role: "assistant",
				content: [{ type: "toolCall", id: "tc_1", name: "read", arguments: { path: "nonexistent/CLAUDE.md" } }],
			}),
			makeMessageEntry({
				role: "toolResult",
				toolCallId: "tc_1",
				toolName: "read",
				isError: false,
				content: [{ type: "text", text: "data" }],
			}),
		];
		const result = await reconstructSubdirState(entries as any, tmpDir);
		expect(result.size).toBe(0);
	});
});

const ZERO_USAGE: Usage = {
	input: 0,
	output: 0,
	cacheRead: 0,
	cacheWrite: 0,
	totalTokens: 0,
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
};

function makeAssistant(toolCalls: ToolCall[]): AssistantMessage {
	return {
		role: "assistant",
		content: toolCalls,
		api: "anthropic" as any,
		provider: "anthropic" as any,
		model: "claude-test",
		usage: ZERO_USAGE,
		stopReason: "toolUse",
		timestamp: 100,
	};
}

function makeReadToolCall(id: string, path: string): ToolCall {
	return { type: "toolCall", id, name: "read", arguments: { path } };
}

describe("registerSubdirContext — message_end handler", () => {
	let tmpDir: string;

	beforeEach(async () => {
		tmpDir = await mkdtemp(join(tmpdir(), "subdir-hook-"));
	});

	afterEach(async () => {
		await rm(tmpDir, { recursive: true, force: true });
	});

	function makeCtx(cwd: string) {
		return { cwd, hasUI: false, ui: {}, sessionManager: { getBranch: () => [] } };
	}

	it("registers message_end, session_compact, session_start, and session_tree handlers", () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);
		expect(handlers.get("message_end")?.length).toBe(1);
		expect(handlers.get("session_compact")?.length).toBe(1);
		expect(handlers.get("session_start")?.length).toBe(1);
		expect(handlers.get("session_tree")?.length).toBe(1);
	});

	it("injects read tool call before triggering read when CLAUDE.md exists", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "# Package rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const message = makeAssistant([makeReadToolCall("tc_1", "pkg/file.ts")]);
		const handler = handlers.get("message_end")![0];
		const result = (await handler({ type: "message_end", message }, makeCtx(tmpDir))) as any;

		expect(result).toBeDefined();
		expect(result.message.content).toHaveLength(2);
		const injected = result.message.content[0] as ToolCall;
		expect(injected.type).toBe("toolCall");
		expect(injected.name).toBe("read");
		expect(injected.arguments.path).toBe(join("pkg", "CLAUDE.md"));
		expect(injected.id).toMatch(/^scrctx-/);
		expect(result.message.content[1]).toEqual(makeReadToolCall("tc_1", "pkg/file.ts"));
	});

	it("injects both CLAUDE.md and AGENTS.md when both exist", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "claude");
		await writeFile(join(subDir, "AGENTS.md"), "agents");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const message = makeAssistant([makeReadToolCall("tc_1", "pkg/file.ts")]);
		const handler = handlers.get("message_end")![0];
		const result = (await handler({ type: "message_end", message }, makeCtx(tmpDir))) as any;

		expect(result.message.content).toHaveLength(3);
		expect(result.message.content[0].arguments.path).toBe(join("pkg", "CLAUDE.md"));
		expect(result.message.content[1].arguments.path).toBe(join("pkg", "AGENTS.md"));
		expect(result.message.content[2]).toEqual(makeReadToolCall("tc_1", "pkg/file.ts"));
	});

	it("returns undefined when no context files exist", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const message = makeAssistant([makeReadToolCall("tc_1", "pkg/file.ts")]);
		const handler = handlers.get("message_end")![0];
		const result = await handler({ type: "message_end", message }, makeCtx(tmpDir));

		expect(result).toBeUndefined();
	});

	it("skips non-assistant messages", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("message_end")![0];
		const result = await handler(
			{ type: "message_end", message: { role: "user", content: [{ type: "text", text: "hi" }], timestamp: 0 } },
			makeCtx(tmpDir),
		);
		expect(result).toBeUndefined();
	});

	it("skips already-injected scrctx- tool calls", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const injectedId = createStableId(`tc_1\0${join("pkg", "CLAUDE.md")}`);
		const message = makeAssistant([
			{ type: "toolCall", id: injectedId, name: "read", arguments: { path: join("pkg", "CLAUDE.md") } },
			makeReadToolCall("tc_1", "pkg/file.ts"),
		]);
		const handler = handlers.get("message_end")![0];
		const result = (await handler({ type: "message_end", message }, makeCtx(tmpDir))) as any;

		// Should still inject for tc_1 since the directory was not yet loaded
		expect(result).toBeDefined();
		// The injected call is before tc_1, but the scrctx- call is not scanned as a trigger
		expect(result.message.content.filter((c: any) => c.id.startsWith("scrctx-")).length).toBe(2);
	});

	it("skips direct reads of CLAUDE.md (no recursion)", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const message = makeAssistant([makeReadToolCall("tc_1", "pkg/CLAUDE.md")]);
		const handler = handlers.get("message_end")![0];
		const result = await handler({ type: "message_end", message }, makeCtx(tmpDir));

		expect(result).toBeUndefined();
	});

	it("skips direct reads of AGENTS.md (no recursion)", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "AGENTS.md"), "agents");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const message = makeAssistant([makeReadToolCall("tc_1", "pkg/AGENTS.md")]);
		const handler = handlers.get("message_end")![0];
		const result = await handler({ type: "message_end", message }, makeCtx(tmpDir));

		expect(result).toBeUndefined();
	});

	it("dedup: second message for same directory does not inject again", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("message_end")![0];

		const msg1 = makeAssistant([makeReadToolCall("tc_1", "pkg/a.ts")]);
		const result1 = (await handler({ type: "message_end", message: msg1 }, makeCtx(tmpDir))) as any;
		expect(result1).toBeDefined();

		const msg2 = makeAssistant([makeReadToolCall("tc_2", "pkg/b.ts")]);
		const result2 = await handler({ type: "message_end", message: msg2 }, makeCtx(tmpDir));
		expect(result2).toBeUndefined();
	});

	it("handles multiple triggering reads in one message", async () => {
		const dirA = join(tmpDir, "a");
		const dirB = join(tmpDir, "b");
		await mkdir(dirA);
		await mkdir(dirB);
		await writeFile(join(dirA, "CLAUDE.md"), "a-rules");
		await writeFile(join(dirB, "CLAUDE.md"), "b-rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const message = makeAssistant([makeReadToolCall("tc_1", "a/file.ts"), makeReadToolCall("tc_2", "b/file.ts")]);
		const handler = handlers.get("message_end")![0];
		const result = (await handler({ type: "message_end", message }, makeCtx(tmpDir))) as any;

		expect(result).toBeDefined();
		expect(result.message.content).toHaveLength(4);
		// Order: injected-for-a, original-a, injected-for-b, original-b
		expect(result.message.content[0].arguments.path).toBe(join("a", "CLAUDE.md"));
		expect(result.message.content[1]).toEqual(makeReadToolCall("tc_1", "a/file.ts"));
		expect(result.message.content[2].arguments.path).toBe(join("b", "CLAUDE.md"));
		expect(result.message.content[3]).toEqual(makeReadToolCall("tc_2", "b/file.ts"));
	});

	it("discovers all intermediate directories for deep reads", async () => {
		const dirA = join(tmpDir, "a");
		const dirABC = join(tmpDir, "a", "b", "c");
		await mkdir(dirABC, { recursive: true });
		await writeFile(join(dirA, "CLAUDE.md"), "level-a");
		await writeFile(join(dirABC, "CLAUDE.md"), "level-abc");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const message = makeAssistant([makeReadToolCall("tc_1", "a/b/c/file.ts")]);
		const handler = handlers.get("message_end")![0];
		const result = (await handler({ type: "message_end", message }, makeCtx(tmpDir))) as any;

		expect(result).toBeDefined();
		// Should have 2 injected + 1 original = 3
		expect(result.message.content).toHaveLength(3);
		expect(result.message.content[0].arguments.path).toBe(join("a", "CLAUDE.md"));
		expect(result.message.content[1].arguments.path).toBe(join("a", "b", "c", "CLAUDE.md"));
		expect(result.message.content[2]).toEqual(makeReadToolCall("tc_1", "a/b/c/file.ts"));
	});

	it("at-prefixed read paths are normalized before discovery", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const message = makeAssistant([makeReadToolCall("tc_1", "@pkg/file.ts")]);
		const handler = handlers.get("message_end")![0];
		const result = (await handler({ type: "message_end", message }, makeCtx(tmpDir))) as any;

		expect(result).toBeDefined();
		expect(result.message.content[0].arguments.path).toBe(join("pkg", "CLAUDE.md"));
	});

	it("cwd-level read does not trigger discovery", async () => {
		await writeFile(join(tmpDir, "CLAUDE.md"), "root rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const message = makeAssistant([makeReadToolCall("tc_1", "file.ts")]);
		const handler = handlers.get("message_end")![0];
		const result = await handler({ type: "message_end", message }, makeCtx(tmpDir));
		expect(result).toBeUndefined();
	});

	it("outside-cwd reads discover immediate directory only", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "outside rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const message = makeAssistant([makeReadToolCall("tc_1", join(outsideDir, "file.ts"))]);
		const handler = handlers.get("message_end")![0];
		const result = (await handler({ type: "message_end", message }, makeCtx(tmpDir))) as any;

		expect(result).toBeDefined();
		expect(result.message.content[0].arguments.path).toBe(join(outsideDir, "CLAUDE.md"));

		await rm(outsideDir, { recursive: true, force: true });
	});

	it("skips symlinks pointing outside cwd", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "escape");

		const linkDir = join(tmpDir, "linked");
		await symlink(outsideDir, linkDir);

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const message = makeAssistant([makeReadToolCall("tc_1", "linked/file.ts")]);
		const handler = handlers.get("message_end")![0];
		const result = await handler({ type: "message_end", message }, makeCtx(tmpDir));

		expect(result).toBeUndefined();

		await rm(outsideDir, { recursive: true, force: true });
	});

	it("session_compact clears subdirLoadedPaths", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const messageEndHandler = handlers.get("message_end")![0];
		const compactHandler = handlers.get("session_compact")![0];

		const msg = makeAssistant([makeReadToolCall("tc_1", "pkg/file.ts")]);
		await messageEndHandler({ type: "message_end", message: msg }, makeCtx(tmpDir));
		expect(state.subdirLoadedPaths.size).toBeGreaterThan(0);

		await compactHandler({}, {});
		expect(state.subdirLoadedPaths.size).toBe(0);
	});

	it("session_compact allows re-discovery on next message", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const messageEndHandler = handlers.get("message_end")![0];
		const compactHandler = handlers.get("session_compact")![0];

		const msg1 = makeAssistant([makeReadToolCall("tc_1", "pkg/file.ts")]);
		await messageEndHandler({ type: "message_end", message: msg1 }, makeCtx(tmpDir));

		await compactHandler({}, {});

		const msg2 = makeAssistant([makeReadToolCall("tc_2", "pkg/file.ts")]);
		const result = (await messageEndHandler({ type: "message_end", message: msg2 }, makeCtx(tmpDir))) as any;
		expect(result).toBeDefined();
	});

	it("does not trigger on messages with only non-read tool calls", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const message = makeAssistant([
			{ type: "toolCall", id: "tc_1", name: "write", arguments: { path: "pkg/file.ts", content: "x" } },
		]);
		const handler = handlers.get("message_end")![0];
		const result = await handler({ type: "message_end", message }, makeCtx(tmpDir));
		expect(result).toBeUndefined();
	});

	it("stable IDs incorporate source tool-call ID", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		// Clear loaded paths between calls to test ID generation
		const handler = handlers.get("message_end")![0];

		const msg1 = makeAssistant([makeReadToolCall("tc_1", "pkg/file.ts")]);
		const result1 = (await handler({ type: "message_end", message: msg1 }, makeCtx(tmpDir))) as any;

		state.subdirLoadedPaths.clear();

		const msg2 = makeAssistant([makeReadToolCall("tc_2", "pkg/file.ts")]);
		const result2 = (await handler({ type: "message_end", message: msg2 }, makeCtx(tmpDir))) as any;

		const id1 = result1.message.content[0].id;
		const id2 = result2.message.content[0].id;
		expect(id1).not.toBe(id2);
	});

	it("respects MAX_DIRS cap across multiple messages", async () => {
		for (let i = 0; i < MAX_DIRS + 3; i++) {
			const d = join(tmpDir, `d${i}`);
			await mkdir(d);
			await writeFile(join(d, "CLAUDE.md"), `content ${i}`);
		}

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("message_end")![0];
		for (let i = 0; i < MAX_DIRS + 3; i++) {
			const msg = makeAssistant([makeReadToolCall(`tc_${i}`, `d${i}/file.ts`)]);
			await handler({ type: "message_end", message: msg }, makeCtx(tmpDir));
		}
		expect(state.subdirLoadedPaths.size).toBeLessThanOrEqual(MAX_DIRS);
	});

	it("session_start reconstructs state from read call/result pairs", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const branchEntries = [
			{
				type: "message",
				message: {
					role: "assistant",
					content: [{ type: "toolCall", id: "tc_1", name: "read", arguments: { path: "sub/CLAUDE.md" } }],
				},
				id: "e1",
				parentId: null,
				timestamp: "0",
			},
			{
				type: "message",
				message: {
					role: "toolResult",
					toolCallId: "tc_1",
					toolName: "read",
					isError: false,
					content: [{ type: "text", text: "rules" }],
				},
				id: "e2",
				parentId: null,
				timestamp: "0",
			},
		];

		const sessionStartHandler = handlers.get("session_start")![0];
		await sessionStartHandler({}, { cwd: tmpDir, sessionManager: { getBranch: () => branchEntries } });

		expect(state.subdirLoadedPaths.has(subDir)).toBe(true);
	});

	it("session_tree reconstructs state from read call/result pairs", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "AGENTS.md"), "agents");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const branchEntries = [
			{
				type: "message",
				message: {
					role: "assistant",
					content: [{ type: "toolCall", id: "tc_1", name: "read", arguments: { path: "pkg/AGENTS.md" } }],
				},
				id: "e1",
				parentId: null,
				timestamp: "0",
			},
			{
				type: "message",
				message: {
					role: "toolResult",
					toolCallId: "tc_1",
					toolName: "read",
					isError: false,
					content: [{ type: "text", text: "agents" }],
				},
				id: "e2",
				parentId: null,
				timestamp: "0",
			},
		];

		const sessionTreeHandler = handlers.get("session_tree")![0];
		await sessionTreeHandler({}, { cwd: tmpDir, sessionManager: { getBranch: () => branchEntries } });

		expect(state.subdirLoadedPaths.has(subDir)).toBe(true);
	});

	it("replay reconstruction resets at compaction boundary", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const branchEntries = [
			{
				type: "message",
				message: {
					role: "assistant",
					content: [{ type: "toolCall", id: "tc_1", name: "read", arguments: { path: "sub/CLAUDE.md" } }],
				},
				id: "e1",
				parentId: null,
				timestamp: "0",
			},
			{
				type: "message",
				message: {
					role: "toolResult",
					toolCallId: "tc_1",
					toolName: "read",
					isError: false,
					content: [{ type: "text", text: "rules" }],
				},
				id: "e2",
				parentId: null,
				timestamp: "0",
			},
			{
				type: "compaction",
				summary: "compacted",
				firstKeptEntryId: "e0",
				tokensBefore: 1000,
				id: "c1",
				parentId: null,
				timestamp: "1",
			},
		];

		const sessionStartHandler = handlers.get("session_start")![0];
		await sessionStartHandler({}, { cwd: tmpDir, sessionManager: { getBranch: () => branchEntries } });

		expect(state.subdirLoadedPaths.size).toBe(0);
	});

	it("does not write scramjet:subdir-context-discovery entries", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("message_end")![0];
		const msg = makeAssistant([makeReadToolCall("tc_1", "pkg/file.ts")]);
		await handler({ type: "message_end", message: msg }, makeCtx(tmpDir));

		const discoveryEntries = pi.appended.filter((e: any) => e.customType === "scramjet:subdir-context-discovery");
		expect(discoveryEntries).toHaveLength(0);
	});
});
