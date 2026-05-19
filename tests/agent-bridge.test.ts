import { mkdirSync, mkdtempSync, readlinkSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ensureAgentBridge } from "../commands/agent-bridge.ts";
import type { AgentRegistry } from "../types.ts";

function makeAgent(name: string, filePath: string) {
	const reg: AgentRegistry = new Map();
	reg.set(name, { name, filePath });
	return reg;
}

describe("ensureAgentBridge", () => {
	let sandbox: string;
	let scramjetRoot: string;
	let piAgentDir: string;
	let targetDir: string;
	let originalAgentDir: string | undefined;

	beforeEach(() => {
		sandbox = mkdtempSync(join(tmpdir(), "scramjet-bridge-"));
		scramjetRoot = join(sandbox, "scramjet-data");
		piAgentDir = join(sandbox, "pi-agent");
		targetDir = join(piAgentDir, "agents");
		mkdirSync(scramjetRoot, { recursive: true });
		originalAgentDir = process.env.PI_CODING_AGENT_DIR;
		process.env.PI_CODING_AGENT_DIR = piAgentDir;
	});

	afterEach(() => {
		if (originalAgentDir === undefined) delete process.env.PI_CODING_AGENT_DIR;
		else process.env.PI_CODING_AGENT_DIR = originalAgentDir;
		rmSync(sandbox, { recursive: true, force: true });
	});

	it("creates the symlink when target is absent", () => {
		const agentFile = join(scramjetRoot, "mach12:scout.md");
		writeFileSync(agentFile, "---\nname: mach12:scout\n---\nHi.");
		const reg = makeAgent("mach12:scout", agentFile);

		const result = ensureAgentBridge(reg, [scramjetRoot]);

		expect(result.skipped).toBe(false);
		expect(result.created).toEqual(["mach12:scout"]);
		expect(result.warnings).toEqual([]);
		expect(readlinkSync(join(targetDir, "mach12:scout.md"))).toBe(agentFile);
	});

	it("is idempotent: re-running with the same registry creates nothing new and warns nothing", () => {
		const agentFile = join(scramjetRoot, "mach12:scout.md");
		writeFileSync(agentFile, "body");
		const reg = makeAgent("mach12:scout", agentFile);

		ensureAgentBridge(reg, [scramjetRoot]);
		const second = ensureAgentBridge(reg, [scramjetRoot]);

		expect(second.created).toEqual([]);
		expect(second.pruned).toEqual([]);
		expect(second.warnings).toEqual([]);
	});

	it("creates the target dir if missing", () => {
		const agentFile = join(scramjetRoot, "mach12:scout.md");
		writeFileSync(agentFile, "body");
		const reg = makeAgent("mach12:scout", agentFile);

		// targetDir does not exist yet; the bridge must create it.
		const result = ensureAgentBridge(reg, [scramjetRoot]);

		expect(result.created).toEqual(["mach12:scout"]);
		expect(readlinkSync(join(targetDir, "mach12:scout.md"))).toBe(agentFile);
	});

	it("warns and skips when a non-symlink file occupies the target name", () => {
		const agentFile = join(scramjetRoot, "mach12:scout.md");
		writeFileSync(agentFile, "scramjet body");
		const reg = makeAgent("mach12:scout", agentFile);

		mkdirSync(targetDir, { recursive: true });
		const conflict = join(targetDir, "mach12:scout.md");
		writeFileSync(conflict, "user-authored content");

		const result = ensureAgentBridge(reg, [scramjetRoot]);

		expect(result.created).toEqual([]);
		expect(result.warnings.length).toBe(1);
		expect(result.warnings[0]).toContain("non-symlink");
		expect(result.warnings[0]).toContain(conflict);
	});

	it("warns and skips when a foreign symlink occupies the target name", () => {
		const agentFile = join(scramjetRoot, "mach12:scout.md");
		writeFileSync(agentFile, "scramjet body");
		const reg = makeAgent("mach12:scout", agentFile);

		const foreignDir = join(sandbox, "elsewhere");
		mkdirSync(foreignDir, { recursive: true });
		const foreignFile = join(foreignDir, "mach12:scout.md");
		writeFileSync(foreignFile, "user-symlinked content");
		mkdirSync(targetDir, { recursive: true });
		symlinkSync(foreignFile, join(targetDir, "mach12:scout.md"));

		const result = ensureAgentBridge(reg, [scramjetRoot]);

		expect(result.created).toEqual([]);
		expect(result.warnings.length).toBe(1);
		expect(result.warnings[0]).toContain("outside scramjet's data dirs");
		// Foreign symlink is preserved.
		expect(readlinkSync(join(targetDir, "mach12:scout.md"))).toBe(foreignFile);
	});

	it("refreshes a scramjet-owned symlink that points at a stale location", () => {
		const oldFile = join(scramjetRoot, "old-set", "mach12:scout.md");
		const newFile = join(scramjetRoot, "new-set", "mach12:scout.md");
		mkdirSync(join(scramjetRoot, "old-set"), { recursive: true });
		mkdirSync(join(scramjetRoot, "new-set"), { recursive: true });
		writeFileSync(oldFile, "old");
		writeFileSync(newFile, "new");
		mkdirSync(targetDir, { recursive: true });
		symlinkSync(oldFile, join(targetDir, "mach12:scout.md"));

		const reg = makeAgent("mach12:scout", newFile);
		const result = ensureAgentBridge(reg, [scramjetRoot]);

		expect(result.created).toEqual(["mach12:scout"]);
		expect(result.warnings).toEqual([]);
		expect(readlinkSync(join(targetDir, "mach12:scout.md"))).toBe(newFile);
	});

	it("prunes scramjet-owned symlinks whose targets no longer exist", () => {
		const liveFile = join(scramjetRoot, "mach12:live.md");
		const deadFile = join(scramjetRoot, "mach12:dead.md");
		writeFileSync(liveFile, "live");
		// Note: deadFile is intentionally NOT created — the symlink will dangle.

		mkdirSync(targetDir, { recursive: true });
		symlinkSync(liveFile, join(targetDir, "mach12:live.md"));
		symlinkSync(deadFile, join(targetDir, "mach12:dead.md"));

		const reg = makeAgent("mach12:live", liveFile);
		const result = ensureAgentBridge(reg, [scramjetRoot]);

		expect(result.pruned).toEqual(["mach12:dead"]);
		// Live symlink survives.
		expect(readlinkSync(join(targetDir, "mach12:live.md"))).toBe(liveFile);
	});

	it("leaves dangling symlinks alone when they point outside scramjet's roots", () => {
		const foreignFile = join(sandbox, "elsewhere", "ghost.md");
		// foreignFile is not created — the symlink dangles, but it's not ours.
		mkdirSync(targetDir, { recursive: true });
		symlinkSync(foreignFile, join(targetDir, "ghost.md"));

		const reg: AgentRegistry = new Map();
		const result = ensureAgentBridge(reg, [scramjetRoot]);

		expect(result.pruned).toEqual([]);
		expect(result.warnings).toEqual([]);
	});

	it("treats nested ownership roots correctly (project root inside global root is fine)", () => {
		const globalRoot = scramjetRoot;
		const projectRoot = join(scramjetRoot, ".scramjet");
		mkdirSync(projectRoot, { recursive: true });
		const projectAgent = join(projectRoot, "mach12:proj.md");
		writeFileSync(projectAgent, "body");

		const reg = makeAgent("mach12:proj", projectAgent);
		const result = ensureAgentBridge(reg, [globalRoot, projectRoot]);

		expect(result.created).toEqual(["mach12:proj"]);
		expect(readlinkSync(join(targetDir, "mach12:proj.md"))).toBe(projectAgent);
	});
});
