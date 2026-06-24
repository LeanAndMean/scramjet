import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerCommandLoader } from "../src/commands/index.js";
import {
	type AgentFileEntry,
	buildAgentRegistry,
	buildRegistry,
	type FileEntry,
	parseAgentFile,
	parseAllowedTools,
	parseCommandFile,
} from "../src/commands/loader.js";
import { createLogger } from "../src/logger.js";
import type { AgentDef, CommandDef } from "../src/types.js";
import { freshState } from "./helpers.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURES = resolve(HERE, "fixtures");

type Handler = (event: unknown, ctx?: unknown) => unknown;

function recordingPi() {
	const handlers = new Map<string, Handler[]>();
	const appended: { customType: string; data: unknown }[] = [];
	const pi: any = {
		on(event: string, handler: Handler) {
			const list = handlers.get(event) ?? [];
			list.push(handler);
			handlers.set(event, list);
		},
		appendEntry(customType: string, data: unknown) {
			appended.push({ customType, data });
		},
	};
	return { pi, handlers, appended };
}

describe("parseAllowedTools — input shapes", () => {
	it("returns undefined for absent values", () => {
		expect(parseAllowedTools(undefined)).toBeUndefined();
		expect(parseAllowedTools(null)).toBeUndefined();
	});

	it("returns the trimmed items from a YAML array", () => {
		expect(parseAllowedTools(["Read", " Bash ", "Edit"])).toEqual(["Read", "Bash", "Edit"]);
	});

	it("returns the trimmed items from a comma-separated string", () => {
		expect(parseAllowedTools("Read, Bash , Edit")).toEqual(["Read", "Bash", "Edit"]);
	});

	it("drops non-string array entries silently", () => {
		expect(parseAllowedTools(["Read", 42, null, "Edit"])).toEqual(["Read", "Edit"]);
	});

	it("returns undefined when an array contains only empty/non-string entries", () => {
		expect(parseAllowedTools(["", "  "])).toBeUndefined();
		expect(parseAllowedTools([42, null])).toBeUndefined();
	});

	it("returns undefined for empty string", () => {
		expect(parseAllowedTools("")).toBeUndefined();
		expect(parseAllowedTools("  ,  ")).toBeUndefined();
	});

	it("returns undefined for invalid scalar types", () => {
		expect(parseAllowedTools(42)).toBeUndefined();
		expect(parseAllowedTools({})).toBeUndefined();
	});
});

describe("parseCommandFile — happy paths", () => {
	const SET = "mach12";

	it("parses a valid file into a CommandDef", () => {
		const content = `---
description: A demo command.
allowed-tools: [Read, Bash]
next:
  mode: ask
  hint: User picks next
---
Body text.`;
		const result = parseCommandFile("/abs/mach12:issue-plan.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def).toEqual({
			name: "mach12:issue-plan",
			filePath: "/abs/mach12:issue-plan.md",
			body: "Body text.",
			description: "A demo command.",
			allowedTools: ["Read", "Bash"],
			next: { mode: "ask", hint: "User picks next" },
		});
	});

	it("leaves optional fields unset for a bare file", () => {
		const result = parseCommandFile("/abs/mach12:bare.md", "---\n---\nBody.", SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.description).toBeUndefined();
		expect(result.def.allowedTools).toBeUndefined();
		expect(result.def.next).toBeUndefined();
	});

	it("treats a comma-string allowed-tools as a list", () => {
		const content = "---\nallowed-tools: Read, Bash\n---\nBody.";
		const result = parseCommandFile("/abs/mach12:tools.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.allowedTools).toEqual(["Read", "Bash"]);
	});

	it("strips a leading UTF-8 BOM before parsing frontmatter", () => {
		const content = "﻿---\ndescription: BOM survives\n---\nBody.";
		const result = parseCommandFile("/abs/mach12:bom.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.description).toBe("BOM survives");
	});

	it("rejects files whose basename does not start with the set prefix", () => {
		const result = parseCommandFile("/abs/issue-plan.md", "---\n---\nBody.", SET);
		expect(result.ok).toBe(false);
		if (result.ok) return;
		expect(result.error).toContain("mach12:");
	});

	it("rejects files that are not markdown", () => {
		const result = parseCommandFile("/abs/mach12:issue-plan.txt", "---\n---\nBody.", SET);
		expect(result.ok).toBe(false);
		if (result.ok) return;
		expect(result.error).toContain("markdown");
	});

	it("rejects files with a malformed next block", () => {
		const content = "---\nnext:\n  mode: bogus\n---\nBody.";
		const result = parseCommandFile("/abs/mach12:bad.md", content, SET);
		expect(result.ok).toBe(false);
		if (result.ok) return;
		expect(result.error).toContain("bogus");
	});

	it("ignores an empty-string description", () => {
		const content = "---\ndescription: ''\n---\nBody.";
		const result = parseCommandFile("/abs/mach12:bare.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.description).toBeUndefined();
	});
});

describe("buildRegistry — collision and skip semantics", () => {
	function entry(setName: string, file: string, content: string, scope: "global" | "project" = "global"): FileEntry {
		return { filePath: `/fake/${setName}/${file}`, content, setName, scope };
	}

	const minimal = "---\n---\nBody.";

	it("returns an empty registry and no warnings for empty input", () => {
		const out = buildRegistry([]);
		expect(out.registry.size).toBe(0);
		expect(out.warnings).toEqual([]);
	});

	it("registers a single valid entry", () => {
		const out = buildRegistry([entry("mach12", "mach12:issue-plan.md", minimal)]);
		expect(out.registry.size).toBe(1);
		expect(out.registry.has("mach12:issue-plan")).toBe(true);
		expect(out.warnings).toEqual([]);
	});

	it("registers multiple non-colliding entries", () => {
		const out = buildRegistry([
			entry("mach12", "mach12:a.md", minimal),
			entry("mach12", "mach12:b.md", minimal),
			entry("infra", "infra:rotate.md", minimal),
		]);
		expect(out.registry.size).toBe(3);
	});

	it("global wins when project-local name collides", () => {
		const out = buildRegistry([
			entry("mach12", "mach12:issue-plan.md", minimal, "global"),
			entry("mach12", "mach12:issue-plan.md", minimal, "project"),
		]);
		expect(out.registry.size).toBe(1);
		const def = out.registry.get("mach12:issue-plan");
		expect(def?.filePath).toBe("/fake/mach12/mach12:issue-plan.md");
		expect(out.warnings).toHaveLength(1);
		expect(out.warnings[0]).toContain("project");
		expect(out.warnings[0]).toContain("mach12:issue-plan");
	});

	it("logs and skips malformed files but keeps valid ones in the same batch", () => {
		const out = buildRegistry([
			entry("mach12", "mach12:good.md", minimal),
			entry("mach12", "issue-plan.md", minimal),
			entry("mach12", "mach12:bad.md", "---\nnext:\n  mode: bogus\n---\nBody."),
			entry("mach12", "mach12:also-good.md", minimal),
		]);
		expect(out.registry.size).toBe(2);
		expect(out.registry.has("mach12:good")).toBe(true);
		expect(out.registry.has("mach12:also-good")).toBe(true);
		expect(out.warnings).toHaveLength(2);
	});

	it("preserves input order so later globals win over earlier projects (defensive)", () => {
		const out = buildRegistry([
			entry("mach12", "mach12:x.md", minimal, "global"),
			entry("mach12", "mach12:x.md", minimal, "project"),
			entry("mach12", "mach12:x.md", minimal, "global"),
		]);
		expect(out.registry.size).toBe(1);
		expect(out.warnings).toHaveLength(2);
	});
});

describe("registerCommandLoader — fixture-backed integration", () => {
	let originalCache: string | undefined;
	let originalAgentDir: string | undefined;
	let agentDirSandbox: string;
	let stderrSpy: { mockRestore(): void };

	beforeEach(() => {
		originalCache = process.env.SCRAMJET_CACHE;
		originalAgentDir = process.env.PI_CODING_AGENT_DIR;
		// Isolate the agent bridge's symlink writes into a per-test tmp dir so
		// fixture-backed runs never touch the user's real ~/.scramjet/agent/agents/.
		agentDirSandbox = mkdtempSync(join(tmpdir(), "scramjet-loader-agentdir-"));
		process.env.PI_CODING_AGENT_DIR = agentDirSandbox;
		stderrSpy = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
	});

	afterEach(() => {
		if (originalCache === undefined) delete process.env.SCRAMJET_CACHE;
		else process.env.SCRAMJET_CACHE = originalCache;
		if (originalAgentDir === undefined) delete process.env.PI_CODING_AGENT_DIR;
		else process.env.PI_CODING_AGENT_DIR = originalAgentDir;
		rmSync(agentDirSandbox, { recursive: true, force: true });
		stderrSpy.mockRestore();
	});

	it("registers exactly one resources_discover handler", () => {
		const { pi, handlers } = recordingPi();
		registerCommandLoader(pi, freshState());
		expect(handlers.has("resources_discover")).toBe(true);
		expect(handlers.size).toBe(1);
	});

	it("populates state.registry from global + project fixtures", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "loader-global");
		const { pi, handlers, appended } = recordingPi();
		const state = freshState({ logger: createLogger(pi) });
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		expect(handler).toBeDefined();
		const result = handler?.({
			type: "resources_discover",
			cwd: join(FIXTURES, "loader-project"),
			reason: "startup",
		}) as { promptPaths: string[] };

		expect(state.registry.has("mach12:issue-plan")).toBe(true);
		expect(state.registry.has("mach12:pr-review")).toBe(true);
		expect(state.registry.has("infra:rotate-key")).toBe(true);
		expect(state.registry.has("local:notes")).toBe(true);
		expect(state.registry.has("mach12:broken")).toBe(false);

		const globalIssuePlan = state.registry.get("mach12:issue-plan");
		expect(globalIssuePlan?.filePath).toContain("loader-global");
		expect(globalIssuePlan?.next).toEqual({
			mode: "open",
			candidates: [
				{ name: "mach12:issue-review", hint: "Use when the plan is non-trivial or touches risky areas." },
				{ name: "mach12:issue-implement", hint: "Use when the plan is small and uncontroversial." },
			],
		});
		expect(globalIssuePlan?.allowedTools).toEqual(["Read", "Bash"]);

		expect(result.promptPaths).toEqual([...state.registry.values()].map((d) => d.filePath));
		for (const p of result.promptPaths) {
			expect(p.endsWith(".md")).toBe(true);
		}

		const warnings = appended
			.filter((e) => (e.data as any).level === "warn")
			.map((e) => (e.data as any).message as string);
		expect(warnings.some((m) => m.includes("issue-plan.md") && m.includes("mach12:"))).toBe(true);
		expect(warnings.some((m) => m.includes("mach12:broken"))).toBe(true);
		expect(warnings.some((m) => m.includes("project") && m.includes("mach12:issue-plan"))).toBe(true);
	});

	it("rebuilds the registry on each handler invocation", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "loader-global");
		const { pi, handlers } = recordingPi();
		const state = freshState({ logger: createLogger(pi) });
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		handler?.({ type: "resources_discover", cwd: join(FIXTURES, "loader-project"), reason: "startup" });
		const firstSize = state.registry.size;
		(state.registry as Map<string, CommandDef>).set("ghost:command", {
			name: "ghost:command",
			filePath: "/nope",
			body: "",
		});
		handler?.({ type: "resources_discover", cwd: join(FIXTURES, "loader-project"), reason: "reload" });
		expect(state.registry.size).toBe(firstSize);
		expect(state.registry.has("ghost:command")).toBe(false);
	});

	it("handles missing global root gracefully", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "does-not-exist");
		const { pi, handlers } = recordingPi();
		const state = freshState({ logger: createLogger(pi) });
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		const result = handler?.({
			type: "resources_discover",
			cwd: join(FIXTURES, "loader-project"),
			reason: "startup",
		}) as { promptPaths: string[] };
		expect(state.registry.has("local:notes")).toBe(true);
		expect(result.promptPaths.length).toBeGreaterThan(0);
	});

	it("handles missing project root gracefully", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "loader-global");
		const { pi, handlers } = recordingPi();
		const state = freshState({ logger: createLogger(pi) });
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		const result = handler?.({
			type: "resources_discover",
			cwd: join(FIXTURES, "does-not-exist"),
			reason: "startup",
		}) as { promptPaths: string[] };
		expect(state.registry.has("mach12:issue-plan")).toBe(true);
		expect(result.promptPaths.length).toBeGreaterThan(0);
	});
});

describe("parseAgentFile — happy paths", () => {
	const SET = "mach12";

	it("parses a valid agent file into an AgentDef", () => {
		const content = `---
name: mach12:code-explorer
description: A codebase exploration agent
tools: read, grep, find
---
You are an explorer.`;
		const result = parseAgentFile("/abs/mach12:code-explorer.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def).toEqual({
			name: "mach12:code-explorer",
			filePath: "/abs/mach12:code-explorer.md",
			description: "A codebase exploration agent",
		});
	});

	it("leaves description unset when absent", () => {
		const content = "---\nname: mach12:bare\n---\nBody.";
		const result = parseAgentFile("/abs/mach12:bare.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.description).toBeUndefined();
	});

	it("trims whitespace from name and description", () => {
		const content = "---\nname: '  mach12:padded  '\ndescription: '  padded desc  '\n---\nBody.";
		const result = parseAgentFile("/abs/mach12:padded.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.name).toBe("mach12:padded");
		expect(result.def.description).toBe("padded desc");
	});

	it("strips a leading UTF-8 BOM before parsing frontmatter", () => {
		const content = "﻿---\nname: mach12:bom\ndescription: BOM test\n---\nBody.";
		const result = parseAgentFile("/abs/mach12:bom.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.name).toBe("mach12:bom");
	});

	it("rejects files whose basename does not start with the set prefix", () => {
		const content = "---\nname: wrong\n---\nBody.";
		const result = parseAgentFile("/abs/wrong.md", content, SET);
		expect(result.ok).toBe(false);
		if (result.ok) return;
		expect(result.error).toContain("mach12:");
	});

	it("rejects files that are not markdown", () => {
		const content = "---\nname: mach12:x\n---\nBody.";
		const result = parseAgentFile("/abs/mach12:x.txt", content, SET);
		expect(result.ok).toBe(false);
		if (result.ok) return;
		expect(result.error).toContain("markdown");
	});

	it("rejects files with missing name field", () => {
		const content = "---\ndescription: No name\n---\nBody.";
		const result = parseAgentFile("/abs/mach12:no-name.md", content, SET);
		expect(result.ok).toBe(false);
		if (result.ok) return;
		expect(result.error).toContain("name");
	});

	it("rejects files with empty-string name", () => {
		const content = "---\nname: ''\n---\nBody.";
		const result = parseAgentFile("/abs/mach12:empty.md", content, SET);
		expect(result.ok).toBe(false);
		if (result.ok) return;
		expect(result.error).toContain("name");
	});

	it("ignores an empty-string description", () => {
		const content = "---\nname: mach12:x\ndescription: ''\n---\nBody.";
		const result = parseAgentFile("/abs/mach12:x.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.description).toBeUndefined();
	});
});

describe("buildAgentRegistry — collision and skip semantics", () => {
	function agentEntry(
		setName: string,
		file: string,
		content: string,
		scope: "global" | "project" = "global",
	): AgentFileEntry {
		return { filePath: `/fake/${setName}/${file}`, content, setName, scope };
	}

	const minimal = "---\nname: mach12:test\n---\nBody.";

	it("returns an empty registry and no warnings for empty input", () => {
		const out = buildAgentRegistry([]);
		expect(out.agentRegistry.size).toBe(0);
		expect(out.warnings).toEqual([]);
	});

	it("registers a single valid agent", () => {
		const out = buildAgentRegistry([agentEntry("mach12", "mach12:test.md", minimal)]);
		expect(out.agentRegistry.size).toBe(1);
		expect(out.agentRegistry.has("mach12:test")).toBe(true);
	});

	it("registers multiple non-colliding agents", () => {
		const out = buildAgentRegistry([
			agentEntry("mach12", "mach12:a.md", "---\nname: mach12:a\n---\nBody."),
			agentEntry("mach12", "mach12:b.md", "---\nname: mach12:b\n---\nBody."),
		]);
		expect(out.agentRegistry.size).toBe(2);
	});

	it("global wins when project-local name collides", () => {
		const out = buildAgentRegistry([
			agentEntry("mach12", "mach12:test.md", minimal, "global"),
			agentEntry("mach12", "mach12:test.md", minimal, "project"),
		]);
		expect(out.agentRegistry.size).toBe(1);
		const def = out.agentRegistry.get("mach12:test");
		expect(def?.filePath).toBe("/fake/mach12/mach12:test.md");
		expect(out.warnings).toHaveLength(1);
		expect(out.warnings[0]).toContain("project");
	});

	it("logs and skips malformed agents but keeps valid ones", () => {
		const out = buildAgentRegistry([
			agentEntry("mach12", "mach12:good.md", "---\nname: mach12:good\n---\nBody."),
			agentEntry("mach12", "mach12:bad.md", "---\ndescription: no name\n---\nBody."),
			agentEntry("mach12", "mach12:also-good.md", "---\nname: mach12:also-good\n---\nBody."),
		]);
		expect(out.agentRegistry.size).toBe(2);
		expect(out.agentRegistry.has("mach12:good")).toBe(true);
		expect(out.agentRegistry.has("mach12:also-good")).toBe(true);
		expect(out.warnings).toHaveLength(1);
	});
});

describe("registerCommandLoader — agent discovery integration", () => {
	let originalCache: string | undefined;
	let originalAgentDir: string | undefined;
	let agentDirSandbox: string;
	let stderrSpy: { mockRestore(): void };

	beforeEach(() => {
		originalCache = process.env.SCRAMJET_CACHE;
		originalAgentDir = process.env.PI_CODING_AGENT_DIR;
		agentDirSandbox = mkdtempSync(join(tmpdir(), "scramjet-agent-discovery-"));
		process.env.PI_CODING_AGENT_DIR = agentDirSandbox;
		stderrSpy = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
	});

	afterEach(() => {
		if (originalCache === undefined) delete process.env.SCRAMJET_CACHE;
		else process.env.SCRAMJET_CACHE = originalCache;
		if (originalAgentDir === undefined) delete process.env.PI_CODING_AGENT_DIR;
		else process.env.PI_CODING_AGENT_DIR = originalAgentDir;
		rmSync(agentDirSandbox, { recursive: true, force: true });
		stderrSpy.mockRestore();
	});

	it("populates state.agentRegistry from global + project fixtures", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "loader-global");
		const { pi, handlers } = recordingPi();
		const state = freshState({ logger: createLogger(pi) });
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		handler?.({
			type: "resources_discover",
			cwd: join(FIXTURES, "loader-project"),
			reason: "startup",
		});

		expect(state.agentRegistry.has("mach12:test-explorer")).toBe(true);
		expect(state.agentRegistry.has("mach12:test-reviewer")).toBe(true);

		const explorer = state.agentRegistry.get("mach12:test-explorer");
		expect(explorer?.filePath).toContain("loader-global");
		expect(explorer?.description).toBe("A test agent for codebase exploration");
	});

	it("skips malformed agents and logs warnings", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "loader-global");
		const { pi, handlers, appended } = recordingPi();
		const state = freshState({ logger: createLogger(pi) });
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		handler?.({
			type: "resources_discover",
			cwd: join(FIXTURES, "does-not-exist"),
			reason: "startup",
		});

		const warnings = appended
			.filter((e) => (e.data as any).level === "warn")
			.map((e) => (e.data as any).message as string);
		expect(warnings.some((m) => m.includes("broken-agent") && m.includes("name"))).toBe(true);
		expect(warnings.some((m) => m.includes("wrong-prefix") && m.includes("mach12:"))).toBe(true);
	});

	it("global agents win over project-local on name collision", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "loader-global");
		const { pi, handlers, appended } = recordingPi();
		const state = freshState({ logger: createLogger(pi) });
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		handler?.({
			type: "resources_discover",
			cwd: join(FIXTURES, "loader-project"),
			reason: "startup",
		});

		const explorer = state.agentRegistry.get("mach12:test-explorer");
		expect(explorer?.filePath).toContain("loader-global");
		const warnings = appended
			.filter((e) => (e.data as any).level === "warn")
			.map((e) => (e.data as any).message as string);
		expect(warnings.some((m) => m.includes("project") && m.includes("mach12:test-explorer"))).toBe(true);
	});

	it("rebuilds agent registry on each handler invocation", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "loader-global");
		const { pi, handlers } = recordingPi();
		const state = freshState({ logger: createLogger(pi) });
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		handler?.({ type: "resources_discover", cwd: join(FIXTURES, "loader-project"), reason: "startup" });
		const firstSize = state.agentRegistry.size;
		(state.agentRegistry as Map<string, AgentDef>).set("ghost:agent", { name: "ghost:agent", filePath: "/nope" });
		handler?.({ type: "resources_discover", cwd: join(FIXTURES, "loader-project"), reason: "reload" });
		expect(state.agentRegistry.size).toBe(firstSize);
		expect(state.agentRegistry.has("ghost:agent")).toBe(false);
	});

	it("handles missing agents directory gracefully", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "does-not-exist");
		const { pi, handlers } = recordingPi();
		const state = freshState({ logger: createLogger(pi) });
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		handler?.({
			type: "resources_discover",
			cwd: join(FIXTURES, "does-not-exist"),
			reason: "startup",
		});
		expect(state.agentRegistry.size).toBe(0);
	});
});
