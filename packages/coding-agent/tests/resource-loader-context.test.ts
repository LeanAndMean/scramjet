import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { loadProjectContextFiles } from "../src/core/resource-loader.js";

const tempDirs: string[] = [];

function tempDir(): string {
	const dir = mkdtempSync(join(tmpdir(), "resource-loader-context-"));
	tempDirs.push(dir);
	return dir;
}

function writeContext(dir: string, filename: string, content: string): string {
	mkdirSync(dir, { recursive: true });
	const path = join(dir, filename);
	writeFileSync(path, content);
	return path;
}

afterEach(() => {
	vi.restoreAllMocks();
	for (const dir of tempDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("loadProjectContextFiles", () => {
	it("retains only CLAUDE when same-directory contents are exactly equal", () => {
		const root = tempDir();
		const cwd = join(root, "project");
		const agentDir = join(root, "agent");
		const claudePath = writeContext(cwd, "CLAUDE.md", "same\n");
		writeContext(cwd, "AGENTS.md", "same\n");

		expect(loadProjectContextFiles({ cwd, agentDir })).toEqual([{ path: claudePath, content: "same\n" }]);
	});

	it("loads differing same-directory contents in CLAUDE then AGENTS order", () => {
		const root = tempDir();
		const cwd = join(root, "project");
		const agentDir = join(root, "agent");
		const claudePath = writeContext(cwd, "CLAUDE.md", "same");
		const agentsPath = writeContext(cwd, "AGENTS.md", "same\n");

		expect(loadProjectContextFiles({ cwd, agentDir })).toEqual([
			{ path: claudePath, content: "same" },
			{ path: agentsPath, content: "same\n" },
		]);
	});

	it("preserves global and broad-to-specific ancestor ordering", () => {
		const root = tempDir();
		const agentDir = join(root, "agent");
		const parent = join(root, "project");
		const cwd = join(parent, "child");
		const globalClaude = writeContext(agentDir, "CLAUDE.md", "global claude");
		const globalAgents = writeContext(agentDir, "AGENTS.md", "global agents");
		const parentClaude = writeContext(parent, "CLAUDE.md", "parent claude");
		const parentAgents = writeContext(parent, "AGENTS.md", "parent agents");
		const childClaude = writeContext(cwd, "CLAUDE.md", "child claude");
		const childAgents = writeContext(cwd, "AGENTS.md", "child agents");

		expect(loadProjectContextFiles({ cwd, agentDir }).map((file) => file.path)).toEqual([
			globalClaude,
			globalAgents,
			parentClaude,
			parentAgents,
			childClaude,
			childAgents,
		]);
	});

	it("retains equal contents from different directories", () => {
		const root = tempDir();
		const agentDir = join(root, "agent");
		const parent = join(root, "project");
		const cwd = join(parent, "child");
		const globalPath = writeContext(agentDir, "CLAUDE.md", "same");
		const parentPath = writeContext(parent, "CLAUDE.md", "same");
		const childPath = writeContext(cwd, "CLAUDE.md", "same");

		expect(loadProjectContextFiles({ cwd, agentDir }).map((file) => file.path)).toEqual([
			globalPath,
			parentPath,
			childPath,
		]);
	});

	it("applies the same policy to uppercase-extension aliases", () => {
		const root = tempDir();
		const cwd = join(root, "project");
		const agentDir = join(root, "agent");
		const claudePath = writeContext(cwd, "CLAUDE.MD", "claude");
		const agentsPath = writeContext(cwd, "AGENTS.MD", "agents");

		expect(loadProjectContextFiles({ cwd, agentDir })).toEqual([
			{ path: claudePath, content: "claude" },
			{ path: agentsPath, content: "agents" },
		]);
	});

	it("warns and falls back to a readable uppercase-extension alias", () => {
		const root = tempDir();
		const cwd = join(root, "project");
		const agentDir = join(root, "agent");
		mkdirSync(join(cwd, "CLAUDE.md"), { recursive: true });
		const aliasPath = writeContext(cwd, "CLAUDE.MD", "claude");
		const errorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined);

		expect(loadProjectContextFiles({ cwd, agentDir })).toEqual([{ path: aliasPath, content: "claude" }]);
		expect(errorSpy).toHaveBeenCalledWith(
			expect.stringContaining(`Warning: Could not read ${join(cwd, "CLAUDE.md")}`),
		);
	});

	it("warns for an unreadable candidate without suppressing its readable sibling", () => {
		const root = tempDir();
		const cwd = join(root, "project");
		const agentDir = join(root, "agent");
		mkdirSync(join(cwd, "CLAUDE.md"), { recursive: true });
		const agentsPath = writeContext(cwd, "AGENTS.md", "agents");
		const errorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined);

		expect(loadProjectContextFiles({ cwd, agentDir })).toEqual([{ path: agentsPath, content: "agents" }]);
		expect(errorSpy).toHaveBeenCalledWith(
			expect.stringContaining(`Warning: Could not read ${join(cwd, "CLAUDE.md")}`),
		);
	});
});
