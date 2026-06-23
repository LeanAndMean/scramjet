import { chmod, mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import type { AgentMessage } from "@leanandmean/agent";
import type { AssistantMessage, Message, ToolResultMessage, Usage } from "@leanandmean/ai";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	buildSyntheticPair,
	CANDIDATES,
	createStableId,
	directoriesToCheck,
	discoverContextFiles,
	findAnchorIndex,
	formatContextBlocks,
	MAX_DEPTH,
	MAX_DIRS,
	reconstructSubdirState,
	registerSubdirContext,
	SUBDIR_CONTEXT_DISCOVERY_TYPE,
} from "../src/subdir-context.js";
import type { SubdirDiscovery } from "../src/types.js";
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

describe("discoverContextFiles", () => {
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
		const result = await discoverContextFiles([subDir], loaded, tmpDir);
		expect(result).toHaveLength(1);
		expect(result[0].filename).toBe("CLAUDE.md");
		expect(result[0].content).toBe("# Sub instructions");
		expect(result[0].dirRealpath).toBeDefined();
	});

	it("finds AGENTS.md in directory", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "AGENTS.md"), "agent stuff");

		const loaded = new Set<string>();
		const result = await discoverContextFiles([subDir], loaded, tmpDir);
		expect(result).toHaveLength(1);
		expect(result[0].filename).toBe("AGENTS.md");
		expect(result[0].content).toBe("agent stuff");
	});

	it("finds both CLAUDE.md and AGENTS.md in same directory", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "claude");
		await writeFile(join(subDir, "AGENTS.md"), "agents");

		const loaded = new Set<string>();
		const result = await discoverContextFiles([subDir], loaded, tmpDir);
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
		const result = await discoverContextFiles([subDir], loaded, tmpDir);
		expect(result).toHaveLength(0);
	});

	it("skips symlinks pointing outside cwd", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "outside content");

		const linkDir = join(tmpDir, "linked");
		await symlink(outsideDir, linkDir);

		const loaded = new Set<string>();
		const result = await discoverContextFiles([linkDir], loaded, tmpDir);
		expect(result).toHaveLength(0);

		await rm(outsideDir, { recursive: true, force: true });
	});

	it("treats root cwd as containing its subdirectories", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "root-contained");

		const loaded = new Set<string>();
		const result = await discoverContextFiles([subDir], loaded, "/");
		expect(result).toHaveLength(1);
		expect(result[0].content).toBe("root-contained");
	});

	it("discovers outside-cwd dirs when cwd boundary enforcement is disabled", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "outside content");

		const loaded = new Set<string>();
		const result = await discoverContextFiles([outsideDir], loaded, "/nonexistent-cwd", undefined, {
			enforceCwdBoundary: false,
		});
		expect(result).toHaveLength(1);
		expect(result[0].content).toBe("outside content");

		await rm(outsideDir, { recursive: true, force: true });
	});

	it("retries directory on next call when all candidates fail with transient errors", async () => {
		const subDir = join(tmpDir, "sub");
		// Create CLAUDE.md as a directory so readFile fails with EISDIR (a transient, non-ENOENT error)
		await mkdir(join(subDir, "CLAUDE.md"), { recursive: true });
		await mkdir(join(subDir, "AGENTS.md"), { recursive: true });

		const loaded = new Set<string>();
		const result1 = await discoverContextFiles([subDir], loaded, tmpDir);
		expect(result1).toHaveLength(0);
		// Directory should NOT be permanently claimed — loadedPaths should not contain it
		expect(loaded.size).toBe(0);

		// Fix the directory: replace dirs with files
		await rm(join(subDir, "CLAUDE.md"), { recursive: true });
		await rm(join(subDir, "AGENTS.md"), { recursive: true });
		await writeFile(join(subDir, "CLAUDE.md"), "recovered");

		const result2 = await discoverContextFiles([subDir], loaded, tmpDir);
		expect(result2).toHaveLength(1);
		expect(result2[0].content).toBe("recovered");
		expect(loaded.size).toBe(1);
	});

	it("keeps directory claimed when all candidates are ENOENT (genuinely no context files)", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		// No CLAUDE.md or AGENTS.md — both readFile calls will ENOENT

		const loaded = new Set<string>();
		const result = await discoverContextFiles([subDir], loaded, tmpDir);
		expect(result).toHaveLength(0);
		// Directory should stay claimed (ENOENT is not transient)
		expect(loaded.size).toBe(1);
	});

	it("keeps directory claimed when at least one candidate succeeds", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "found");
		// AGENTS.md will ENOENT — but CLAUDE.md succeeds, so directory stays claimed

		const loaded = new Set<string>();
		const result = await discoverContextFiles([subDir], loaded, tmpDir);
		expect(result).toHaveLength(1);
		expect(loaded.size).toBe(1);
	});

	it("logs non-ENOENT readFile errors", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(join(subDir, "CLAUDE.md"), { recursive: true });

		const warnings: Array<{ category: string; message: string; data?: Record<string, unknown> }> = [];
		const logger = {
			warn: (category: string, message: string, data?: Record<string, unknown>) => {
				warnings.push({ category, message, data });
			},
		} as any;

		const loaded = new Set<string>();
		const result = await discoverContextFiles([subDir], loaded, tmpDir, logger);
		expect(result).toHaveLength(0);
		expect(warnings).toContainEqual({
			category: "subdir-context",
			message: "readFile failed: EISDIR",
			data: { path: join(subDir, "CLAUDE.md") },
		});
	});

	it("skips unreadable files", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "secret");
		await chmod(join(subDir, "CLAUDE.md"), 0o000);

		const loaded = new Set<string>();
		const result = await discoverContextFiles([subDir], loaded, tmpDir);
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

		const result = await discoverContextFiles(dirs, loaded, tmpDir);
		expect(result.length).toBeLessThanOrEqual(MAX_DIRS * CANDIDATES.length);
		expect(loaded.size).toBeLessThanOrEqual(MAX_DIRS);
	});

	it("returns files ordered shallowest-first", async () => {
		const dirA = join(tmpDir, "a");
		const dirAB = join(tmpDir, "a", "b");
		const dirABC = join(tmpDir, "a", "b", "c");
		await mkdir(dirABC, { recursive: true });
		await writeFile(join(dirA, "CLAUDE.md"), "a");
		await writeFile(join(dirABC, "CLAUDE.md"), "abc");

		const loaded = new Set<string>();
		const result = await discoverContextFiles([dirA, dirAB, dirABC], loaded, tmpDir);
		expect(result[0].content).toBe("a");
		expect(result[1].content).toBe("abc");
	});

	it("synchronous claim prevents duplicate entries from concurrent calls", async () => {
		const subDir = join(tmpDir, "sub");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "content");

		const loaded = new Set<string>();
		const [r1, r2] = await Promise.all([
			discoverContextFiles([subDir], loaded, tmpDir),
			discoverContextFiles([subDir], loaded, tmpDir),
		]);
		const total = r1.length + r2.length;
		expect(total).toBe(1);
	});

	it("handles directory that does not exist", async () => {
		const loaded = new Set<string>();
		const result = await discoverContextFiles([join(tmpDir, "nonexistent")], loaded, tmpDir);
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
		const result = await discoverContextFiles([join(tmpDir, "sub")], loaded, join(tmpDir, "nonexistent-cwd"), logger);
		expect(result).toHaveLength(0);
		expect(debugCalls).toHaveLength(1);
		expect(debugCalls[0].message).toContain("ENOENT");
		expect(debugCalls[0].data?.cwd).toBe(join(tmpDir, "nonexistent-cwd"));
	});
});

describe("createStableId", () => {
	it("produces a stable deterministic id", () => {
		const id1 = createStableId("pkg/sub/CLAUDE.md");
		const id2 = createStableId("pkg/sub/CLAUDE.md");
		expect(id1).toBe(id2);
	});

	it("produces different ids for different paths", () => {
		const id1 = createStableId("a/CLAUDE.md");
		const id2 = createStableId("b/CLAUDE.md");
		expect(id1).not.toBe(id2);
	});

	it("starts with scrctx- prefix", () => {
		const id = createStableId("some/path/CLAUDE.md");
		expect(id).toMatch(/^scrctx-[a-f0-9]{12}$/);
	});

	it("is provider-safe (alphanumeric and hyphen)", () => {
		const id = createStableId("path/with/special chars/CLAUDE.md");
		expect(id).toMatch(/^[A-Za-z0-9_-]+$/);
	});
});

describe("reconstructSubdirState", () => {
	function makeCustomEntry(customType: string, data: unknown) {
		return { type: "custom" as const, customType, data, id: "e1", timestamp: 0 };
	}

	function makeCompactionEntry() {
		return {
			type: "compaction" as const,
			summary: "compacted",
			firstKeptEntryId: "e0",
			tokensBefore: 1000,
			id: "c1",
			timestamp: 0,
		};
	}

	it("returns empty state for no entries", () => {
		const result = reconstructSubdirState([]);
		expect(result.loadedPaths.size).toBe(0);
		expect(result.discoveries).toHaveLength(0);
	});

	it("restores discoveries from journal entries", () => {
		const entries = [
			makeCustomEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, {
				toolCallId: "tc_1",
				dirRealpath: "/project/sub",
				filename: "CLAUDE.md",
				displayPath: "sub/CLAUDE.md",
				content: "# Rules",
			}),
		];
		const result = reconstructSubdirState(entries as any);
		expect(result.loadedPaths.has("/project/sub")).toBe(true);
		expect(result.discoveries).toHaveLength(1);
		expect(result.discoveries[0].toolCallId).toBe("tc_1");
		expect(result.discoveries[0].content).toBe("# Rules");
	});

	it("resets state at compaction entries", () => {
		const entries = [
			makeCustomEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, {
				toolCallId: "tc_1",
				dirRealpath: "/project/sub",
				filename: "CLAUDE.md",
				displayPath: "sub/CLAUDE.md",
				content: "old content",
			}),
			makeCompactionEntry(),
			makeCustomEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, {
				toolCallId: "tc_2",
				dirRealpath: "/project/pkg",
				filename: "AGENTS.md",
				displayPath: "pkg/AGENTS.md",
				content: "new content",
			}),
		];
		const result = reconstructSubdirState(entries as any);
		expect(result.loadedPaths.size).toBe(1);
		expect(result.loadedPaths.has("/project/pkg")).toBe(true);
		expect(result.discoveries).toHaveLength(1);
		expect(result.discoveries[0].content).toBe("new content");
	});

	it("skips corrupt entries with missing fields", () => {
		const entries = [
			makeCustomEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, { toolCallId: "tc_1" }),
			makeCustomEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, null),
			makeCustomEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, {
				toolCallId: "tc_2",
				dirRealpath: "/project/sub",
				filename: "CLAUDE.md",
				displayPath: "sub/CLAUDE.md",
				content: "valid",
			}),
		];
		const result = reconstructSubdirState(entries as any);
		expect(result.discoveries).toHaveLength(1);
		expect(result.discoveries[0].toolCallId).toBe("tc_2");
	});

	it("logs dropped entries at debug level when logger is provided", () => {
		const debugCalls: Array<{ category: string; message: string; data?: Record<string, unknown> }> = [];
		const logger = {
			debug: (category: string, message: string, data?: Record<string, unknown>) => {
				debugCalls.push({ category, message, data });
			},
		} as any;

		const entries = [
			makeCustomEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, null),
			makeCustomEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, { toolCallId: 123 }),
		];
		reconstructSubdirState(entries as any, logger);

		expect(debugCalls).toHaveLength(2);
		expect(debugCalls[0].category).toBe("subdir-context");
		expect(debugCalls[0].data?.reason).toBe("null data");
		expect(debugCalls[1].data?.reason).toBe("invalid field types");
	});

	it("skips entries with wrong types for fields", () => {
		const entries = [
			makeCustomEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, {
				toolCallId: 123,
				dirRealpath: "/project/sub",
				filename: "CLAUDE.md",
				displayPath: "sub/CLAUDE.md",
				content: "x",
			}),
		];
		const result = reconstructSubdirState(entries as any);
		expect(result.discoveries).toHaveLength(0);
	});

	it("ignores unrelated custom entries", () => {
		const entries = [
			makeCustomEntry("scramjet:command-start", { command: "test:cmd" }),
			makeCustomEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, {
				toolCallId: "tc_1",
				dirRealpath: "/project/sub",
				filename: "CLAUDE.md",
				displayPath: "sub/CLAUDE.md",
				content: "content",
			}),
		];
		const result = reconstructSubdirState(entries as any);
		expect(result.discoveries).toHaveLength(1);
	});
});

describe("registerSubdirContext", () => {
	let tmpDir: string;

	beforeEach(async () => {
		tmpDir = await mkdtemp(join(tmpdir(), "subdir-hook-"));
	});

	afterEach(async () => {
		await rm(tmpDir, { recursive: true, force: true });
	});

	function makeReadEvent(path: string, content = "file content", isError = false, toolCallId = "tc_1") {
		return {
			type: "tool_result" as const,
			toolName: "read" as const,
			toolCallId,
			input: { path } as Record<string, unknown>,
			content: [{ type: "text" as const, text: content }],
			isError,
			details: undefined,
		};
	}

	function makeCtx(cwd: string) {
		return { cwd, hasUI: false, ui: {}, sessionManager: { getBranch: () => [] } };
	}

	it("registers tool_result, session_compact, session_start, and session_tree handlers", () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);
		expect(handlers.get("tool_result")?.length).toBe(1);
		expect(handlers.get("session_compact")?.length).toBe(1);
		expect(handlers.get("session_start")?.length).toBe(1);
		expect(handlers.get("session_tree")?.length).toBe(1);
	});

	it("tool_result returns undefined (does not modify content)", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "# Package rules");
		await writeFile(join(subDir, "index.ts"), "export {}");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const event = makeReadEvent("pkg/index.ts", "export {}");
		const handler = handlers.get("tool_result")![0];
		const result = await handler(event, makeCtx(tmpDir));

		expect(result).toBeUndefined();
	});

	it("tool_result records discoveries in state", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "# Package rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		await handler(makeReadEvent("pkg/file.ts"), makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(1);
		expect(state.subdirDiscoveries[0].filename).toBe("CLAUDE.md");
		expect(state.subdirDiscoveries[0].content).toBe("# Package rules");
		expect(state.subdirDiscoveries[0].toolCallId).toBe("tc_1");
		expect(state.subdirDiscoveries[0].displayPath).toBe("pkg/CLAUDE.md");
	});

	it("normalizes at-prefixed read paths before discovery", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "# Package rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		await handler(makeReadEvent("@pkg/file.ts"), makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(1);
		expect(state.subdirDiscoveries[0].displayPath).toBe("pkg/CLAUDE.md");
		expect(state.subdirDiscoveries[0].content).toBe("# Package rules");
	});

	it("tool_result journals discoveries", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "# Rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		await handler(makeReadEvent("pkg/file.ts"), makeCtx(tmpDir));

		expect(pi.appended).toHaveLength(1);
		expect(pi.appended[0].customType).toBe(SUBDIR_CONTEXT_DISCOVERY_TYPE);
		expect(pi.appended[0].data.toolCallId).toBe("tc_1");
		expect(pi.appended[0].data.content).toBe("# Rules");
		expect(pi.appended[0].data).not.toHaveProperty("syntheticId");
	});

	it("dedup: second read in same directory does not add more discoveries", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");
		await writeFile(join(subDir, "a.ts"), "a");
		await writeFile(join(subDir, "b.ts"), "b");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		await handler(makeReadEvent("pkg/a.ts", "a"), makeCtx(tmpDir));
		await handler(makeReadEvent("pkg/b.ts", "b"), makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(1);
		expect(pi.appended).toHaveLength(1);
	});

	it("discovers outside-cwd reads (immediate directory only)", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "outside rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		await handler(makeReadEvent(join(outsideDir, "file.ts")), makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(1);
		expect(state.subdirDiscoveries[0].content).toBe("outside rules");

		await rm(outsideDir, { recursive: true, force: true });
	});

	it("read at cwd level does not trigger discovery", async () => {
		await writeFile(join(tmpDir, "CLAUDE.md"), "root rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		await handler(makeReadEvent("file.ts"), makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(0);
	});

	it("session_compact clears both state fields", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const toolHandler = handlers.get("tool_result")![0];
		const compactHandler = handlers.get("session_compact")![0];

		await toolHandler(makeReadEvent("pkg/file.ts"), makeCtx(tmpDir));
		expect(state.subdirLoadedPaths.size).toBeGreaterThan(0);
		expect(state.subdirDiscoveries.length).toBeGreaterThan(0);

		await compactHandler({}, {});
		expect(state.subdirLoadedPaths.size).toBe(0);
		expect(state.subdirDiscoveries).toHaveLength(0);
	});

	it("session_compact allows re-discovery on next read", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const toolHandler = handlers.get("tool_result")![0];
		const compactHandler = handlers.get("session_compact")![0];

		await toolHandler(makeReadEvent("pkg/file.ts"), makeCtx(tmpDir));
		await compactHandler({}, {});
		await toolHandler(makeReadEvent("pkg/file.ts"), makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(1);
	});

	it("discovers all intermediate directories ordered shallowest-first", async () => {
		const dirA = join(tmpDir, "a");
		const dirABC = join(tmpDir, "a", "b", "c");
		await mkdir(dirABC, { recursive: true });
		await writeFile(join(dirA, "CLAUDE.md"), "level-a");
		await writeFile(join(dirABC, "CLAUDE.md"), "level-abc");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		await handler(makeReadEvent("a/b/c/file.ts"), makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(2);
		expect(state.subdirDiscoveries[0].content).toBe("level-a");
		expect(state.subdirDiscoveries[1].content).toBe("level-abc");
	});

	it("respects MAX_DIRS cap across multiple reads", async () => {
		for (let i = 0; i < MAX_DIRS + 3; i++) {
			const d = join(tmpDir, `d${i}`);
			await mkdir(d);
			await writeFile(join(d, "CLAUDE.md"), `content ${i}`);
			await writeFile(join(d, "file.ts"), "x");
		}

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		for (let i = 0; i < MAX_DIRS + 3; i++) {
			await handler(makeReadEvent(`d${i}/file.ts`), makeCtx(tmpDir));
		}
		expect(state.subdirLoadedPaths.size).toBeLessThanOrEqual(MAX_DIRS);
	});

	it("skips symlinks pointing outside cwd (lexical-inside escape)", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "escape");

		const linkDir = join(tmpDir, "linked");
		await symlink(outsideDir, linkDir);
		await writeFile(join(linkDir, "file.ts"), "x");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		await handler(makeReadEvent("linked/file.ts"), makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(0);

		await rm(outsideDir, { recursive: true, force: true });
	});

	it("keeps dot-dot-prefixed symlinks under the inside-cwd boundary check", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "escape");

		const linkDir = join(tmpDir, "..data");
		await symlink(outsideDir, linkDir);
		await writeFile(join(linkDir, "file.ts"), "x");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		await handler(makeReadEvent("..data/file.ts"), makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(0);

		await rm(outsideDir, { recursive: true, force: true });
	});

	it("skips unreadable CLAUDE.md without error", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "secret");
		await chmod(join(subDir, "CLAUDE.md"), 0o000);

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		await handler(makeReadEvent("pkg/file.ts"), makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(0);

		await chmod(join(subDir, "CLAUDE.md"), 0o644);
	});

	it("does not trigger on error reads", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		await handler(makeReadEvent("pkg/file.ts", "error text", true), makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(0);
	});

	it("does not trigger when event.input.path is not a string", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		const event = {
			type: "tool_result" as const,
			toolName: "read" as const,
			toolCallId: "tc_1",
			input: { path: 123 } as Record<string, unknown>,
			content: [{ type: "text" as const, text: "x" }],
			isError: false,
			details: undefined,
		};
		await handler(event, makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(0);
	});

	it("does not trigger for non-read tools", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		const event = {
			type: "tool_result" as const,
			toolName: "write" as const,
			toolCallId: "tc_1",
			input: { path: "pkg/file.ts" } as Record<string, unknown>,
			content: [{ type: "text" as const, text: "ok" }],
			isError: false,
			details: undefined,
		};
		await handler(event, makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(0);
	});

	it("session_start reconstructs state from journal entries", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const branchEntries = [
			{
				type: "custom",
				customType: SUBDIR_CONTEXT_DISCOVERY_TYPE,
				data: {
					toolCallId: "tc_1",
					dirRealpath: "/project/sub",
					filename: "CLAUDE.md",
					displayPath: "sub/CLAUDE.md",
					content: "# Restored",
				},
				id: "e1",
				timestamp: 0,
			},
		];

		const sessionStartHandler = handlers.get("session_start")![0];
		await sessionStartHandler({}, { sessionManager: { getBranch: () => branchEntries } });

		expect(state.subdirLoadedPaths.has("/project/sub")).toBe(true);
		expect(state.subdirDiscoveries).toHaveLength(1);
		expect(state.subdirDiscoveries[0].content).toBe("# Restored");
	});

	it("session_tree reconstructs state from journal entries", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const branchEntries = [
			{
				type: "custom",
				customType: SUBDIR_CONTEXT_DISCOVERY_TYPE,
				data: {
					toolCallId: "tc_2",
					dirRealpath: "/project/pkg",
					filename: "AGENTS.md",
					displayPath: "pkg/AGENTS.md",
					content: "agents",
				},
				id: "e2",
				timestamp: 0,
			},
		];

		const sessionTreeHandler = handlers.get("session_tree")![0];
		await sessionTreeHandler({}, { sessionManager: { getBranch: () => branchEntries } });

		expect(state.subdirLoadedPaths.has("/project/pkg")).toBe(true);
		expect(state.subdirDiscoveries).toHaveLength(1);
	});

	it("replay reconstruction resets at compaction boundary", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const branchEntries = [
			{
				type: "custom",
				customType: SUBDIR_CONTEXT_DISCOVERY_TYPE,
				data: {
					toolCallId: "tc_1",
					dirRealpath: "/project/old",
					filename: "CLAUDE.md",
					displayPath: "old/CLAUDE.md",
					content: "old",
				},
				id: "e1",
				timestamp: 0,
			},
			{
				type: "compaction",
				summary: "compacted",
				firstKeptEntryId: "e0",
				tokensBefore: 1000,
				id: "c1",
				timestamp: 1,
			},
		];

		const sessionStartHandler = handlers.get("session_start")![0];
		await sessionStartHandler({}, { sessionManager: { getBranch: () => branchEntries } });

		expect(state.subdirLoadedPaths.size).toBe(0);
		expect(state.subdirDiscoveries).toHaveLength(0);
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

function makeAssistant(toolCallId: string, toolName = "read", path = "file.ts"): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "toolCall", id: toolCallId, name: toolName, arguments: { path } }],
		api: "anthropic" as any,
		provider: "anthropic" as any,
		model: "claude-test",
		usage: ZERO_USAGE,
		stopReason: "toolUse",
		timestamp: 100,
	};
}

function makeToolResult(toolCallId: string, text = "result"): ToolResultMessage {
	return {
		role: "toolResult",
		toolCallId,
		toolName: "read",
		content: [{ type: "text", text }],
		isError: false,
		timestamp: 101,
	};
}

function makeDiscovery(overrides: Partial<SubdirDiscovery> = {}): SubdirDiscovery {
	return {
		toolCallId: "tc_real",
		dirRealpath: "/project/sub",
		filename: "CLAUDE.md",
		displayPath: "sub/CLAUDE.md",
		content: "# Sub rules",
		...overrides,
	};
}

describe("findAnchorIndex", () => {
	it("finds the assistant message containing the tool call id", () => {
		const messages: Message[] = [
			{ role: "user", content: [{ type: "text", text: "hi" }], timestamp: 0 },
			makeAssistant("tc_real"),
			makeToolResult("tc_real"),
		];
		expect(findAnchorIndex(messages, "tc_real")).toBe(1);
	});

	it("returns -1 when tool call id not found", () => {
		const messages: Message[] = [makeAssistant("tc_other"), makeToolResult("tc_other")];
		expect(findAnchorIndex(messages, "tc_missing")).toBe(-1);
	});

	it("finds the last matching anchor (reverse scan)", () => {
		const messages: Message[] = [
			makeAssistant("tc_1"),
			makeToolResult("tc_1"),
			makeAssistant("tc_1"),
			makeToolResult("tc_1"),
		];
		expect(findAnchorIndex(messages, "tc_1")).toBe(2);
	});

	it("finds anchor in multi-tool-call assistant message", () => {
		const msg: AssistantMessage = {
			...makeAssistant("tc_a"),
			content: [
				{ type: "toolCall", id: "tc_a", name: "read", arguments: { path: "a.ts" } },
				{ type: "toolCall", id: "tc_b", name: "read", arguments: { path: "b.ts" } },
			],
		};
		const messages: Message[] = [msg, makeToolResult("tc_a"), makeToolResult("tc_b")];
		expect(findAnchorIndex(messages, "tc_b")).toBe(0);
	});
});

describe("buildSyntheticPair", () => {
	it("creates valid matching assistant/toolResult messages", () => {
		const discovery = makeDiscovery();
		const anchor = makeAssistant("tc_real");
		const [synAssistant, synResult] = buildSyntheticPair(discovery, anchor);
		const syntheticId = createStableId(discovery.displayPath);

		expect(synAssistant.role).toBe("assistant");
		expect(synAssistant.content).toHaveLength(1);
		expect(synAssistant.content[0]).toEqual({
			type: "toolCall",
			id: syntheticId,
			name: "read",
			arguments: { path: discovery.displayPath },
		});
		expect(synAssistant.stopReason).toBe("toolUse");
		expect(synAssistant.timestamp).toBe(0);

		expect(synResult.role).toBe("toolResult");
		expect(synResult.toolCallId).toBe(syntheticId);
		expect(synResult.toolName).toBe("read");
		expect(synResult.isError).toBe(false);
		expect(synResult.timestamp).toBe(0);
		expect(synResult.content[0]).toEqual({
			type: "text",
			text: `# Project context: ${discovery.displayPath}\n\n${discovery.content}`,
		});
	});

	it("copies api/provider/model from anchor", () => {
		const anchor = makeAssistant("tc_real");
		const [synAssistant] = buildSyntheticPair(makeDiscovery(), anchor);

		expect(synAssistant.api).toBe(anchor.api);
		expect(synAssistant.provider).toBe(anchor.provider);
		expect(synAssistant.model).toBe(anchor.model);
	});

	it("uses zero usage", () => {
		const [synAssistant] = buildSyntheticPair(makeDiscovery(), makeAssistant("tc_real"));
		expect(synAssistant.usage.input).toBe(0);
		expect(synAssistant.usage.output).toBe(0);
		expect(synAssistant.usage.totalTokens).toBe(0);
		expect(synAssistant.usage.cost.total).toBe(0);
	});
});

describe("formatContextBlocks", () => {
	it("returns undefined when no discoveries", () => {
		const result = formatContextBlocks([], [makeAssistant("tc_1"), makeToolResult("tc_1")]);
		expect(result).toBeUndefined();
	});

	it("injects synthetic pairs before the triggering assistant message", () => {
		const messages: Message[] = [
			{ role: "user", content: [{ type: "text", text: "read file" }], timestamp: 0 },
			makeAssistant("tc_real"),
			makeToolResult("tc_real"),
		];
		const discovery = makeDiscovery({ toolCallId: "tc_real" });
		const result = formatContextBlocks([discovery], messages);
		const syntheticId = createStableId(discovery.displayPath);

		expect(result).toBeDefined();
		expect(result!).toHaveLength(5);
		expect(result![0].role).toBe("user");
		expect(result![1].role).toBe("assistant");
		expect((result![1] as AssistantMessage).content[0]).toMatchObject({
			type: "toolCall",
			id: syntheticId,
			name: "read",
		});
		expect(result![2].role).toBe("toolResult");
		expect((result![2] as ToolResultMessage).toolCallId).toBe(syntheticId);
		expect(result![3].role).toBe("assistant");
		expect(result![4].role).toBe("toolResult");
	});

	it("returns undefined when anchor is missing", () => {
		const messages: Message[] = [makeAssistant("tc_other"), makeToolResult("tc_other")];
		const discovery = makeDiscovery({ toolCallId: "tc_missing" });
		const result = formatContextBlocks([discovery], messages);
		expect(result).toBeUndefined();
	});

	it("skips discoveries whose synthetic ID is already in messages (idempotence)", () => {
		const discovery = makeDiscovery({ toolCallId: "tc_real" });
		const syntheticId = createStableId(discovery.displayPath);
		const messages: Message[] = [
			{
				role: "assistant",
				content: [{ type: "toolCall", id: syntheticId, name: "read", arguments: { path: "sub/CLAUDE.md" } }],
				api: "anthropic" as any,
				provider: "anthropic" as any,
				model: "claude-test",
				usage: ZERO_USAGE,
				stopReason: "toolUse",
				timestamp: 0,
			},
			makeToolResult(syntheticId),
			makeAssistant("tc_real"),
			makeToolResult("tc_real"),
		];
		const result = formatContextBlocks([discovery], messages);
		expect(result).toBeUndefined();
	});

	it("coalesces multiple discoveries at the same anchor", () => {
		const multiToolAssistant: AssistantMessage = {
			...makeAssistant("tc_a"),
			content: [
				{ type: "toolCall", id: "tc_a", name: "read", arguments: { path: "a/file.ts" } },
				{ type: "toolCall", id: "tc_b", name: "read", arguments: { path: "b/file.ts" } },
			],
		};
		const messages: Message[] = [multiToolAssistant, makeToolResult("tc_a"), makeToolResult("tc_b")];
		const d1 = makeDiscovery({ toolCallId: "tc_a", displayPath: "a/CLAUDE.md" });
		const d2 = makeDiscovery({ toolCallId: "tc_b", displayPath: "b/CLAUDE.md" });
		const result = formatContextBlocks([d1, d2], messages);

		expect(result).toBeDefined();
		expect(result!).toHaveLength(7);
		expect(result![0].role).toBe("assistant");
		expect((result![0] as AssistantMessage).content[0]).toMatchObject({ id: createStableId(d1.displayPath) });
		expect(result![1].role).toBe("toolResult");
		expect(result![2].role).toBe("assistant");
		expect((result![2] as AssistantMessage).content[0]).toMatchObject({ id: createStableId(d2.displayPath) });
		expect(result![3].role).toBe("toolResult");
		expect(result![4]).toBe(multiToolAssistant);
		expect(result![5].role).toBe("toolResult");
		expect(result![6].role).toBe("toolResult");
	});

	it("handles discoveries at different anchors", () => {
		const messages: Message[] = [
			makeAssistant("tc_1"),
			makeToolResult("tc_1"),
			makeAssistant("tc_2"),
			makeToolResult("tc_2"),
		];
		const d1 = makeDiscovery({ toolCallId: "tc_1", displayPath: "a/CLAUDE.md" });
		const d2 = makeDiscovery({ toolCallId: "tc_2", displayPath: "b/CLAUDE.md" });
		const result = formatContextBlocks([d1, d2], messages);

		expect(result).toBeDefined();
		expect(result!).toHaveLength(8);
		expect((result![0] as AssistantMessage).content[0]).toMatchObject({ id: createStableId(d1.displayPath) });
		expect((result![2] as AssistantMessage).content[0]).toMatchObject({ id: "tc_1" });
		expect((result![4] as AssistantMessage).content[0]).toMatchObject({ id: createStableId(d2.displayPath) });
		expect((result![6] as AssistantMessage).content[0]).toMatchObject({ id: "tc_2" });
	});

	it("preserves custom context messages while inspecting assistant anchors", () => {
		const custom = {
			role: "custom",
			customType: "notice",
			content: "internal",
			display: false,
			timestamp: 0,
		} as AgentMessage;
		const messages: AgentMessage[] = [custom, makeAssistant("tc_real"), makeToolResult("tc_real")];
		const discovery = makeDiscovery({ toolCallId: "tc_real" });
		const result = formatContextBlocks([discovery], messages);

		expect(result).toBeDefined();
		expect(result!).toHaveLength(5);
		expect(result![0]).toBe(custom);
		expect(result![3]).toBe(messages[1]);
		expect(result![4]).toBe(messages[2]);
	});

	it("preserves original messages when no anchor matches", () => {
		const messages: Message[] = [makeAssistant("tc_x"), makeToolResult("tc_x")];
		const d = makeDiscovery({ toolCallId: "tc_nonexistent" });
		const result = formatContextBlocks([d], messages);
		expect(result).toBeUndefined();
	});
});

describe("registerSubdirContext — context handler", () => {
	let tmpDir: string;

	beforeEach(async () => {
		tmpDir = await mkdtemp(join(tmpdir(), "subdir-ctx-"));
	});

	afterEach(async () => {
		await rm(tmpDir, { recursive: true, force: true });
	});

	function makeReadEvent(path: string, content = "file content", isError = false, toolCallId = "tc_1") {
		return {
			type: "tool_result" as const,
			toolName: "read" as const,
			toolCallId,
			input: { path } as Record<string, unknown>,
			content: [{ type: "text" as const, text: content }],
			isError,
			details: undefined,
		};
	}

	function makeCtx(cwd: string) {
		return { cwd, hasUI: false, ui: {}, sessionManager: { getBranch: () => [] } };
	}

	it("registers a context handler", () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);
		expect(handlers.get("context")?.length).toBe(1);
	});

	it("context handler returns undefined when no discoveries", () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const contextHandler = handlers.get("context")![0];
		const event = { type: "context", messages: [makeAssistant("tc_1"), makeToolResult("tc_1")] };
		const result = contextHandler(event, makeCtx(tmpDir));
		expect(result).toBeUndefined();
	});

	it("context handler injects synthetic pairs when discoveries exist", () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		state.subdirDiscoveries.push(makeDiscovery({ toolCallId: "tc_1" }));
		const syntheticId = createStableId("sub/CLAUDE.md");

		const contextHandler = handlers.get("context")![0];
		const messages = [makeAssistant("tc_1"), makeToolResult("tc_1")];
		const result = contextHandler({ type: "context", messages }, makeCtx(tmpDir)) as any;

		expect(result).toBeDefined();
		expect(result.messages).toHaveLength(4);
		expect(result.messages[0].role).toBe("assistant");
		expect((result.messages[0] as AssistantMessage).content[0]).toMatchObject({
			id: syntheticId,
			name: "read",
		});
		expect(result.messages[1].role).toBe("toolResult");
		expect((result.messages[1] as ToolResultMessage).toolCallId).toBe(syntheticId);
		expect(result.messages[2]).toBe(messages[0]);
		expect(result.messages[3]).toBe(messages[1]);
	});

	it("repeated context calls with unchanged state are idempotent", () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const discovery = makeDiscovery({ toolCallId: "tc_1" });
		state.subdirDiscoveries.push(discovery);

		const contextHandler = handlers.get("context")![0];

		const messages1 = [makeAssistant("tc_1"), makeToolResult("tc_1")];
		const result1 = contextHandler({ type: "context", messages: messages1 }, makeCtx(tmpDir)) as any;
		expect(result1.messages).toHaveLength(4);

		const result2 = contextHandler({ type: "context", messages: result1.messages }, makeCtx(tmpDir)) as any;
		expect(result2).toBeUndefined();
	});

	it("end-to-end: tool_result discovery followed by context injection", async () => {
		const subDir = join(tmpDir, "pkg");
		await mkdir(subDir);
		await writeFile(join(subDir, "CLAUDE.md"), "# Package rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const toolHandler = handlers.get("tool_result")![0];
		await toolHandler(makeReadEvent("pkg/file.ts", "export {}", false, "tc_read_1"), makeCtx(tmpDir));

		expect(state.subdirDiscoveries).toHaveLength(1);
		expect(state.subdirDiscoveries[0].toolCallId).toBe("tc_read_1");

		const contextHandler = handlers.get("context")![0];
		const messages = [makeAssistant("tc_read_1", "read", "pkg/file.ts"), makeToolResult("tc_read_1", "export {}")];
		const result = contextHandler({ type: "context", messages }, makeCtx(tmpDir)) as any;

		expect(result).toBeDefined();
		expect(result.messages).toHaveLength(4);

		const synAssistant = result.messages[0] as AssistantMessage;
		expect(synAssistant.content[0]).toMatchObject({ name: "read" });

		const synResult = result.messages[1] as ToolResultMessage;
		expect(synResult.content[0]).toMatchObject({ type: "text", text: expect.stringContaining("# Package rules") });
		expect(synResult.content[0]).toMatchObject({ type: "text", text: expect.stringContaining("# Project context:") });

		expect(result.messages[2]).toBe(messages[0]);
		expect(result.messages[3]).toBe(messages[1]);
	});

	it("reconstructed discoveries are re-injected by context when anchor exists", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const branchEntries = [
			{
				type: "custom",
				customType: SUBDIR_CONTEXT_DISCOVERY_TYPE,
				data: {
					toolCallId: "tc_old",
					dirRealpath: "/project/sub",
					filename: "CLAUDE.md",
					displayPath: "sub/CLAUDE.md",
					content: "# Restored rules",
					syntheticId: "scrctx-restored000",
				},
				id: "e1",
				timestamp: 0,
			},
		];

		const sessionStartHandler = handlers.get("session_start")![0];
		await sessionStartHandler({}, { sessionManager: { getBranch: () => branchEntries } });

		expect(state.subdirDiscoveries).toHaveLength(1);

		const contextHandler = handlers.get("context")![0];
		const messages = [makeAssistant("tc_old"), makeToolResult("tc_old")];
		const result = contextHandler({ type: "context", messages }, makeCtx(tmpDir)) as any;

		expect(result).toBeDefined();
		expect(result.messages).toHaveLength(4);
		const synResult = result.messages[1] as ToolResultMessage;
		expect(synResult.toolCallId).toBe(createStableId("sub/CLAUDE.md"));
		expect(synResult.toolCallId).not.toBe("scrctx-restored000");
		expect(synResult.content[0]).toMatchObject({ type: "text", text: expect.stringContaining("# Restored rules") });
	});

	it("reconstructed discoveries with missing anchors are skipped", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const branchEntries = [
			{
				type: "custom",
				customType: SUBDIR_CONTEXT_DISCOVERY_TYPE,
				data: {
					toolCallId: "tc_compacted_away",
					dirRealpath: "/project/sub",
					filename: "CLAUDE.md",
					displayPath: "sub/CLAUDE.md",
					content: "# Lost",
				},
				id: "e1",
				timestamp: 0,
			},
		];

		const sessionStartHandler = handlers.get("session_start")![0];
		await sessionStartHandler({}, { sessionManager: { getBranch: () => branchEntries } });

		const contextHandler = handlers.get("context")![0];
		const messages = [makeAssistant("tc_different"), makeToolResult("tc_different")];
		const result = contextHandler({ type: "context", messages }, makeCtx(tmpDir));

		expect(result).toBeUndefined();
	});
});
