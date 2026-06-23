import { chmod, mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	CANDIDATES,
	createStableId,
	directoriesToCheck,
	discoverContextFiles,
	MAX_DEPTH,
	MAX_DIRS,
	reconstructSubdirState,
	registerSubdirContext,
	SUBDIR_CONTEXT_DISCOVERY_TYPE,
} from "../subdir-context.ts";
import { freshState, recordingPi } from "./helpers.ts";

describe("directoriesToCheck", () => {
	it("returns intermediate dirs for inside-cwd paths", () => {
		const result = directoriesToCheck("a/b/c/file.ts", "/project");
		expect(result).toEqual(["/project/a", "/project/a/b", "/project/a/b/c"]);
	});

	it("returns empty at cwd level", () => {
		const result = directoriesToCheck("file.ts", "/project");
		expect(result).toEqual([]);
	});

	it("returns [fileDir] for file outside cwd", () => {
		const result = directoriesToCheck("/other/path/file.ts", "/project");
		expect(result).toEqual(["/other/path"]);
	});

	it("caps at MAX_DEPTH", () => {
		const deep = `${Array.from({ length: 15 }, (_, i) => `d${i}`).join("/")}/file.ts`;
		const result = directoriesToCheck(deep, "/project");
		expect(result).toHaveLength(MAX_DEPTH);
	});

	it("handles absolute paths inside cwd", () => {
		const result = directoriesToCheck("/project/sub/dir/file.ts", "/project");
		expect(result).toEqual(["/project/sub", "/project/sub/dir"]);
	});

	it("normalizes .. segments", () => {
		const result = directoriesToCheck("a/b/../c/file.ts", "/project");
		expect(result).toEqual(["/project/a", "/project/a/c"]);
	});

	it("handles ~/ paths by expanding to homedir", () => {
		const home = homedir();
		const result = directoriesToCheck("~/sub/dir/file.ts", home);
		expect(result).toEqual([join(home, "sub"), join(home, "sub/dir")]);
	});

	it("returns [fileDir] for ~/ paths when cwd is not homedir", () => {
		const home = homedir();
		const result = directoriesToCheck("~/sub/file.ts", "/project");
		expect(result).toEqual([join(home, "sub")]);
	});

	it("returns single dir for one-level deep file", () => {
		const result = directoriesToCheck("sub/file.ts", "/project");
		expect(result).toEqual(["/project/sub"]);
	});

	it("handles trailing slashes in cwd", () => {
		const result = directoriesToCheck("a/b/file.ts", "/project/");
		expect(result).toEqual(["/project/a", "/project/a/b"]);
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
		expect(result[0].realpath).toBeDefined();
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

	it("discovers outside-cwd dirs when skipCwdCheck is true", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "outside content");

		const loaded = new Set<string>();
		const result = await discoverContextFiles([outsideDir], loaded, "/nonexistent-cwd", undefined, true);
		expect(result).toHaveLength(1);
		expect(result[0].content).toBe("outside content");

		await rm(outsideDir, { recursive: true, force: true });
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
				realpath: "/project/sub",
				filename: "CLAUDE.md",
				displayPath: "sub/CLAUDE.md",
				content: "# Rules",
				syntheticId: "scrctx-abc123456789",
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
				realpath: "/project/sub",
				filename: "CLAUDE.md",
				displayPath: "sub/CLAUDE.md",
				content: "old content",
				syntheticId: "scrctx-old000000000",
			}),
			makeCompactionEntry(),
			makeCustomEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, {
				toolCallId: "tc_2",
				realpath: "/project/pkg",
				filename: "AGENTS.md",
				displayPath: "pkg/AGENTS.md",
				content: "new content",
				syntheticId: "scrctx-new000000000",
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
				realpath: "/project/sub",
				filename: "CLAUDE.md",
				displayPath: "sub/CLAUDE.md",
				content: "valid",
				syntheticId: "scrctx-val000000000",
			}),
		];
		const result = reconstructSubdirState(entries as any);
		expect(result.discoveries).toHaveLength(1);
		expect(result.discoveries[0].toolCallId).toBe("tc_2");
	});

	it("skips entries with wrong types for fields", () => {
		const entries = [
			makeCustomEntry(SUBDIR_CONTEXT_DISCOVERY_TYPE, {
				toolCallId: 123,
				realpath: "/project/sub",
				filename: "CLAUDE.md",
				displayPath: "sub/CLAUDE.md",
				content: "x",
				syntheticId: "scrctx-aaa",
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
				realpath: "/project/sub",
				filename: "CLAUDE.md",
				displayPath: "sub/CLAUDE.md",
				content: "content",
				syntheticId: "scrctx-abc123456789",
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
		expect(state.subdirDiscoveries[0].syntheticId).toMatch(/^scrctx-/);
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
					realpath: "/project/sub",
					filename: "CLAUDE.md",
					displayPath: "sub/CLAUDE.md",
					content: "# Restored",
					syntheticId: "scrctx-abc123456789",
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
					realpath: "/project/pkg",
					filename: "AGENTS.md",
					displayPath: "pkg/AGENTS.md",
					content: "agents",
					syntheticId: "scrctx-def000000000",
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
					realpath: "/project/old",
					filename: "CLAUDE.md",
					displayPath: "old/CLAUDE.md",
					content: "old",
					syntheticId: "scrctx-old000000000",
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
