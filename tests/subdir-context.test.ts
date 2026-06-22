import { chmod, mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	directoriesToCheck,
	discoverContextFiles,
	formatContextBlocks,
	MAX_DEPTH,
	MAX_FILES,
	registerSubdirContext,
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

	it("returns empty for file outside cwd", () => {
		const result = directoriesToCheck("/other/path/file.ts", "/project");
		expect(result).toEqual([]);
	});

	it("caps at MAX_DEPTH", () => {
		const deep = `${Array.from({ length: 15 }, (_, i) => `d${i}`).join("/")}/file.ts`;
		const result = directoriesToCheck(deep, "/project");
		expect(result.length).toBe(MAX_DEPTH);
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
		const home = require("node:os").homedir();
		const result = directoriesToCheck("~/sub/dir/file.ts", home);
		expect(result).toEqual([join(home, "sub"), join(home, "sub/dir")]);
	});

	it("returns empty for ~/ paths when cwd is not homedir", () => {
		const result = directoriesToCheck("~/sub/file.ts", "/project");
		expect(result).toEqual([]);
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

	it("respects MAX_FILES cap", async () => {
		const loaded = new Set<string>();
		const dirs: string[] = [];
		for (let i = 0; i < MAX_FILES + 5; i++) {
			const subDir = join(tmpDir, `dir${i}`);
			await mkdir(subDir);
			await writeFile(join(subDir, "CLAUDE.md"), `content ${i}`);
			dirs.push(subDir);
		}

		const result = await discoverContextFiles(dirs, loaded, tmpDir);
		expect(result.length).toBeLessThanOrEqual(MAX_FILES);
		expect(loaded.size).toBeLessThanOrEqual(MAX_FILES);
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

describe("formatContextBlocks", () => {
	it("returns empty string for empty array", () => {
		expect(formatContextBlocks([], "/project")).toBe("");
	});

	it("formats a single file with header, content, and separator", () => {
		const result = formatContextBlocks(
			[{ dir: "/project/sub", realpath: "/project/sub", filename: "CLAUDE.md", content: "hello world" }],
			"/project",
		);
		expect(result).toBe("# Project context: sub/CLAUDE.md\n\nhello world\n\n---");
	});

	it("formats multiple files separated by double newline", () => {
		const result = formatContextBlocks(
			[
				{ dir: "/project/a", realpath: "/project/a", filename: "CLAUDE.md", content: "aaa" },
				{ dir: "/project/a/b", realpath: "/project/a/b", filename: "AGENTS.md", content: "bbb" },
			],
			"/project",
		);
		expect(result).toContain("# Project context: a/CLAUDE.md");
		expect(result).toContain("# Project context: a/b/AGENTS.md");
		expect(result.indexOf("a/CLAUDE.md")).toBeLessThan(result.indexOf("a/b/AGENTS.md"));
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

	function makeReadEvent(path: string, content = "file content", isError = false) {
		return {
			type: "tool_result" as const,
			toolName: "read" as const,
			toolCallId: "tc_1",
			input: { path } as Record<string, unknown>,
			content: [{ type: "text" as const, text: content }],
			isError,
			details: undefined,
		};
	}

	function makeCtx(cwd: string) {
		return { cwd, hasUI: false, ui: {}, sessionManager: {} };
	}

	it("registers exactly one tool_result and one session_compact handler", () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);
		expect(handlers.get("tool_result")?.length).toBe(1);
		expect(handlers.get("session_compact")?.length).toBe(1);
	});

	it("discovers CLAUDE.md and prepends to content", async () => {
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

		expect(result).toBeDefined();
		const content = (result as any).content;
		expect(content).toHaveLength(2);
		expect(content[0].type).toBe("text");
		expect(content[0].text).toContain("# Project context: pkg/CLAUDE.md");
		expect(content[0].text).toContain("# Package rules");
		expect(content[1].text).toBe("export {}");
	});

	it("second read in same directory does NOT prepend again", async () => {
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
		const result2 = await handler(makeReadEvent("pkg/b.ts", "b"), makeCtx(tmpDir));
		expect(result2).toBeUndefined();
	});

	it("read outside cwd does not trigger discovery", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "outside");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		const result = await handler(makeReadEvent(join(outsideDir, "file.ts")), makeCtx(tmpDir));
		expect(result).toBeUndefined();

		await rm(outsideDir, { recursive: true, force: true });
	});

	it("read at cwd level does not trigger discovery", async () => {
		await writeFile(join(tmpDir, "CLAUDE.md"), "root rules");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		const result = await handler(makeReadEvent("file.ts"), makeCtx(tmpDir));
		expect(result).toBeUndefined();
	});

	it("session_compact clears state so subsequent read re-discovers", async () => {
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

		await compactHandler({}, {});
		expect(state.subdirLoadedPaths.size).toBe(0);

		const result = await toolHandler(makeReadEvent("pkg/file.ts"), makeCtx(tmpDir));
		expect(result).toBeDefined();
		expect((result as any).content[0].text).toContain("# Project context: pkg/CLAUDE.md");
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
		const result = await handler(makeReadEvent("a/b/c/file.ts"), makeCtx(tmpDir));

		expect(result).toBeDefined();
		const text = (result as any).content[0].text;
		expect(text).toContain("level-a");
		expect(text).toContain("level-abc");
		expect(text.indexOf("level-a")).toBeLessThan(text.indexOf("level-abc"));
	});

	it("respects MAX_FILES cap across multiple reads", async () => {
		for (let i = 0; i < MAX_FILES + 3; i++) {
			const d = join(tmpDir, `d${i}`);
			await mkdir(d);
			await writeFile(join(d, "CLAUDE.md"), `content ${i}`);
			await writeFile(join(d, "file.ts"), "x");
		}

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		for (let i = 0; i < MAX_FILES + 3; i++) {
			await handler(makeReadEvent(`d${i}/file.ts`), makeCtx(tmpDir));
		}
		expect(state.subdirLoadedPaths.size).toBeLessThanOrEqual(MAX_FILES);
	});

	it("skips symlinks pointing outside cwd", async () => {
		const outsideDir = await mkdtemp(join(tmpdir(), "outside-"));
		await writeFile(join(outsideDir, "CLAUDE.md"), "escape");

		const linkDir = join(tmpDir, "linked");
		await symlink(outsideDir, linkDir);
		await writeFile(join(linkDir, "file.ts"), "x");

		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerSubdirContext(pi, state);

		const handler = handlers.get("tool_result")![0];
		const result = await handler(makeReadEvent("linked/file.ts"), makeCtx(tmpDir));
		expect(result).toBeUndefined();

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
		const result = await handler(makeReadEvent("pkg/file.ts"), makeCtx(tmpDir));
		expect(result).toBeUndefined();

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
		const result = await handler(makeReadEvent("pkg/file.ts", "error text", true), makeCtx(tmpDir));
		expect(result).toBeUndefined();
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
		const result = await handler(event, makeCtx(tmpDir));
		expect(result).toBeUndefined();
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
		const result = await handler(event, makeCtx(tmpDir));
		expect(result).toBeUndefined();
	});
});
