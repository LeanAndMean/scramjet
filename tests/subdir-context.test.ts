import { chmod, mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { directoriesToCheck, discoverContextFiles, MAX_DEPTH, MAX_FILES } from "../subdir-context.ts";

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
