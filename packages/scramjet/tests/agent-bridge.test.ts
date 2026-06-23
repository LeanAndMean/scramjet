import { chmodSync, mkdirSync, mkdtempSync, readlinkSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ensureAgentBridge } from "../src/commands/agent-bridge.js";
import type { AgentRegistry } from "../src/types.js";

function makeAgent(name: string, filePath: string) {
	const reg = new Map<string, { name: string; filePath: string }>();
	reg.set(name, { name, filePath });
	return reg as AgentRegistry;
}

describe("ensureAgentBridge", () => {
	let sandbox: string;
	let scramjetRoot: string;
	let piAgentDir: string;
	let targetDir: string;
	let originalAgentDir: string | undefined;
	let originalScramjetAgentDir: string | undefined;

	beforeEach(() => {
		sandbox = mkdtempSync(join(tmpdir(), "scramjet-bridge-"));
		scramjetRoot = join(sandbox, "scramjet-data");
		piAgentDir = join(sandbox, "pi-agent");
		targetDir = join(piAgentDir, "agents");
		mkdirSync(scramjetRoot, { recursive: true });
		originalAgentDir = process.env.PI_CODING_AGENT_DIR;
		originalScramjetAgentDir = process.env.SCRAMJET_CODING_AGENT_DIR;
		// pi-coding-agent's getAgentDir() reads either PI_CODING_AGENT_DIR or
		// SCRAMJET_CODING_AGENT_DIR depending on the APP_NAME computed at
		// module init (driven by PI_PACKAGE_DIR pointing at scramjet's shim).
		// Setting both covers dev shells where the shim is preloaded and CI
		// shells where it isn't.
		process.env.PI_CODING_AGENT_DIR = piAgentDir;
		process.env.SCRAMJET_CODING_AGENT_DIR = piAgentDir;
	});

	afterEach(() => {
		if (originalAgentDir === undefined) delete process.env.PI_CODING_AGENT_DIR;
		else process.env.PI_CODING_AGENT_DIR = originalAgentDir;
		if (originalScramjetAgentDir === undefined) delete process.env.SCRAMJET_CODING_AGENT_DIR;
		else process.env.SCRAMJET_CODING_AGENT_DIR = originalScramjetAgentDir;
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

	it("warns and bails when the agent dir cannot be created (mkdirSync fails) (F34)", () => {
		// Force PI_CODING_AGENT_DIR to a path under an existing regular file.
		// `mkdirSync(file/agents, {recursive: true})` returns ENOTDIR, which
		// the bridge must surface as a warning without throwing or pretending
		// the bridge succeeded.
		const blocker = join(sandbox, "blocker-file");
		writeFileSync(blocker, "this is a regular file");
		process.env.PI_CODING_AGENT_DIR = blocker;
		process.env.SCRAMJET_CODING_AGENT_DIR = blocker;

		const agentFile = join(scramjetRoot, "mach12:scout.md");
		writeFileSync(agentFile, "body");
		const reg = makeAgent("mach12:scout", agentFile);

		const result = ensureAgentBridge(reg, [scramjetRoot]);

		expect(result.skipped).toBe(false);
		expect(result.created).toEqual([]);
		expect(result.warnings).toHaveLength(1);
		expect(result.warnings[0]).toMatch(/cannot create/);
		expect(result.warnings[0]).toMatch(/agents/);
	});

	it("warns and skips prune when statSync(target) returns EACCES, leaving the symlink in place (F33)", () => {
		// EACCES on the target (parent dir chmod 000) means we can't tell
		// whether the file is live or gone. Pruning would destroy a possibly-
		// live link, so the bridge must skip + warn rather than guess. The
		// existing test for outside-roots dangling symlinks covers the
		// classification branch; this one covers the inaccessible-target branch.
		if (typeof process.getuid === "function" && process.getuid() === 0) return; // root bypasses EACCES

		const lockdown = join(scramjetRoot, "locked");
		mkdirSync(lockdown, { recursive: true });
		const inaccessibleTarget = join(lockdown, "mach12:scout.md");
		writeFileSync(inaccessibleTarget, "body");

		mkdirSync(targetDir, { recursive: true });
		const linkPath = join(targetDir, "mach12:vanished.md");
		symlinkSync(inaccessibleTarget, linkPath);

		// Empty registry forces the entry to be considered for prune. The
		// symlink target falls under scramjetRoot so isUnder() returns true.
		const reg: AgentRegistry = new Map();

		chmodSync(lockdown, 0o000);
		let result: ReturnType<typeof ensureAgentBridge>;
		try {
			result = ensureAgentBridge(reg, [scramjetRoot]);
		} finally {
			chmodSync(lockdown, 0o755);
		}

		expect(result.pruned).toEqual([]);
		expect(result.warnings.some((w) => /could not stat/.test(w) && /skipping prune/.test(w))).toBe(true);
		// Live link preserved — we couldn't classify, so we didn't unlink.
		expect(readlinkSync(linkPath)).toBe(inaccessibleTarget);
	});
});
