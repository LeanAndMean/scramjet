import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initScramjet } from "../src/index.js";
import { discoverAgents } from "../src/subagent/agents.js";
import { getPiInvocation, registerSubagentTool } from "../src/subagent/index.js";
import { recordingPi } from "./helpers.js";

function writeProjectAgent(tmpDir: string, fileName: string, frontmatter: string[], body = "Agent body.") {
	const agentsDir = path.join(tmpDir, ".scramjet", "agents");
	fs.mkdirSync(agentsDir, { recursive: true });
	fs.writeFileSync(path.join(agentsDir, fileName), ["---", ...frontmatter, "---", "", body].join("\n"));
}

function writeFakeInvocation(tmpDir: string, script: string): string {
	const scriptPath = path.join(tmpDir, "fake-scramjet.js");
	fs.writeFileSync(scriptPath, script);
	return scriptPath;
}

function assistantEvent(text: string, extra: Record<string, unknown> = {}): string {
	return JSON.stringify({
		type: "message_end",
		message: {
			role: "assistant",
			content: [{ type: "text", text }],
			...extra,
		},
	});
}

function registeredSubagentTool() {
	const { pi, tools } = recordingPi();
	registerSubagentTool(pi);
	return tools[0];
}

function textContent(result: any): string {
	const first = result.content[0];
	return first?.type === "text" ? first.text : "";
}

function renderToolResult(tool: any, result: any, expanded: boolean): string {
	const theme = {
		fg: (_color: string, text: string) => text,
		bold: (text: string) => text,
	};
	return tool.renderResult(result, { expanded }, theme, {}).render(120).join("\n");
}

function failedParallelRenderResult() {
	return {
		content: [{ type: "text", text: "Parallel: 0/1 succeeded" }],
		details: {
			mode: "parallel",
			agentScope: "project",
			projectAgentsDir: null,
			results: [
				{
					agent: "test-agent",
					agentSource: "project",
					task: "fail",
					exitCode: 0,
					messages: [{ role: "assistant", content: [{ type: "text", text: "stale assistant output" }] }],
					stderr: "",
					usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0, contextTokens: 0, turns: 0 },
					stopReason: "error",
					errorMessage: "provider failed",
				},
			],
		},
		isError: true,
	};
}

describe("registerSubagentTool — registration", () => {
	it("registers exactly one tool named 'subagent' with renderCall and renderResult", () => {
		const { pi, tools } = recordingPi();
		registerSubagentTool(pi);

		expect(tools).toHaveLength(1);
		expect(tools[0].name).toBe("subagent");
		expect(typeof tools[0].renderCall).toBe("function");
		expect(typeof tools[0].renderResult).toBe("function");
	});
});

describe("initScramjet — subagent wiring", () => {
	it("registers a tool named 'subagent' among all tools", () => {
		const { pi, tools } = recordingPi();
		initScramjet(pi);

		expect(tools.some((t: any) => t.name === "subagent")).toBe(true);
	});
});

describe("discoverAgents — empty directory", () => {
	let tmpDir: string;

	beforeEach(() => {
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-agent-test-"));
	});

	afterEach(() => {
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("returns empty agents array and null projectAgentsDir for a directory with no .scramjet/agents", () => {
		const result = discoverAgents(tmpDir, "project");

		expect(result.agents).toEqual([]);
		expect(result.projectAgentsDir).toBeNull();
	});
});

describe("discoverAgents — happy path with valid agent file", () => {
	let tmpDir: string;

	beforeEach(() => {
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-agent-test-"));
		writeProjectAgent(
			tmpDir,
			"test-agent.md",
			["name: test-agent", "description: A test agent for validation", "tools: read,bash", "model: test-model"],
			"You are a test agent.",
		);
	});

	afterEach(() => {
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("returns agent config with parsed frontmatter fields", () => {
		const result = discoverAgents(tmpDir, "project");

		expect(result.agents).toHaveLength(1);
		expect(result.agents[0]).toMatchObject({
			name: "test-agent",
			description: "A test agent for validation",
			tools: ["read", "bash"],
			model: "test-model",
			source: "project",
			systemPrompt: expect.stringContaining("You are a test agent."),
		});
		expect(result.projectAgentsDir).toBe(path.join(tmpDir, ".scramjet", "agents"));
	});
});

describe("discoverAgents — malformed frontmatter", () => {
	let tmpDir: string;

	beforeEach(() => {
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-agent-test-"));
	});

	afterEach(() => {
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("skips invalid required fields and reports non-string optional fields", () => {
		writeProjectAgent(tmpDir, "bad-name.md", ["name: 123", "description: Invalid agent"]);
		writeProjectAgent(tmpDir, "bad-description.md", ["name: bad-description", "description: 456"]);
		writeProjectAgent(tmpDir, "bad-optional.md", [
			"name: bad-optional",
			"description: Valid agent",
			"tools:",
			"  - bash",
			"model:",
			"  - test-model",
		]);

		const result = discoverAgents(tmpDir, "project");

		expect(result.agents).toHaveLength(1);
		expect(result.agents[0]).toMatchObject({
			name: "bad-optional",
			description: "Valid agent",
			source: "project",
		});
		expect(result.agents[0].tools).toBeUndefined();
		expect(result.agents[0].model).toBeUndefined();
		expect(result.diagnostics).toEqual([
			expect.stringContaining("bad-description.md: frontmatter must include string name and description"),
			expect.stringContaining("bad-name.md: frontmatter must include string name and description"),
			expect.stringContaining("bad-optional.md: ignoring non-string tools frontmatter"),
			expect.stringContaining("bad-optional.md: ignoring non-string model frontmatter"),
		]);
	});

	it("continues discovery and reports diagnostics when one file has invalid YAML", () => {
		const agentsDir = path.join(tmpDir, ".scramjet", "agents");
		fs.mkdirSync(agentsDir, { recursive: true });
		fs.writeFileSync(path.join(agentsDir, "broken.md"), "---\nname: [unterminated\n---\nBody");
		writeProjectAgent(tmpDir, "valid.md", ["name: valid", "description: Valid agent"]);

		const result = discoverAgents(tmpDir, "project");

		expect(result.agents.map((agent) => agent.name)).toEqual(["valid"]);
		expect(result.diagnostics).toEqual([expect.stringContaining("broken.md: invalid YAML frontmatter")]);
	});
});

describe("subagent tool — failure reporting", () => {
	let tmpDir: string;
	let origArgv1: string;

	beforeEach(() => {
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-subagent-test-"));
		origArgv1 = process.argv[1];
		writeProjectAgent(tmpDir, "test-agent.md", ["name: test-agent", "description: Test agent"]);
	});

	afterEach(() => {
		process.argv[1] = origArgv1;
		vi.restoreAllMocks();
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("includes stderr in parallel failure summaries", async () => {
		process.argv[1] = writeFakeInvocation(
			tmpDir,
			'process.stderr.write("child stderr diagnostic\\n"); process.exit(2);\n',
		);
		const tool = registeredSubagentTool();

		const result = await tool.execute(
			"tool-call-id",
			{
				tasks: [{ agent: "test-agent", task: "fail" }],
				agentScope: "project",
				confirmProjectAgents: false,
			},
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		expect(textContent(result)).toContain("[test-agent] failed: child stderr diagnostic");
	});

	it("preserves successful parallel assistant output when the child also writes stderr", async () => {
		process.argv[1] = writeFakeInvocation(
			tmpDir,
			`process.stderr.write("warning only\\n"); process.stdout.write(${JSON.stringify(`${assistantEvent("assistant answer")}\n`)}); process.exit(0);\n`,
		);
		const tool = registeredSubagentTool();

		const result = await tool.execute(
			"tool-call-id",
			{
				tasks: [{ agent: "test-agent", task: "succeed" }],
				agentScope: "project",
				confirmProjectAgents: false,
			},
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		expect(result.isError).toBeUndefined();
		expect(textContent(result)).toContain("[test-agent] completed: assistant answer");
		expect(textContent(result)).not.toContain("warning only");
	});

	it("treats assistant error stopReason as a parallel failure even when exit code is zero", async () => {
		process.argv[1] = writeFakeInvocation(
			tmpDir,
			`process.stdout.write(${JSON.stringify(`${assistantEvent("ignored", { stopReason: "error", errorMessage: "provider failed" })}\n`)}); process.exit(0);\n`,
		);
		const tool = registeredSubagentTool();

		const result = await tool.execute(
			"tool-call-id",
			{
				tasks: [{ agent: "test-agent", task: "fail" }],
				agentScope: "project",
				confirmProjectAgents: false,
			},
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		expect(result.isError).toBe(true);
		expect(textContent(result)).toContain("Parallel: 0/1 succeeded");
		expect(textContent(result)).toContain("[test-agent] failed: provider failed");
	});

	it("includes discovery diagnostics when an unknown agent may have been skipped", async () => {
		writeProjectAgent(tmpDir, "bad-name.md", ["name: 123", "description: Invalid agent"]);
		const tool = registeredSubagentTool();

		const result = await tool.execute(
			"tool-call-id",
			{ agent: "bad-name", task: "run", agentScope: "project", confirmProjectAgents: false },
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		expect(result.isError).toBe(true);
		expect(textContent(result)).toContain("Unknown agent");
		expect(textContent(result)).toContain("Agent discovery warnings");
		expect(textContent(result)).toContain("bad-name.md: frontmatter must include string name and description");
	});

	it("reports signal-killed subprocesses as failures with the signal name", async () => {
		process.argv[1] = writeFakeInvocation(tmpDir, 'process.kill(process.pid, "SIGTERM");\n');
		const tool = registeredSubagentTool();

		const result = await tool.execute(
			"tool-call-id",
			{ agent: "test-agent", task: "die", agentScope: "project", confirmProjectAgents: false },
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		expect(result.isError).toBe(true);
		expect(textContent(result)).toContain("Process killed by SIGTERM");
	});

	it("reports handled signal exit codes with the signal name", async () => {
		process.argv[1] = writeFakeInvocation(tmpDir, "process.exit(143);\n");
		const tool = registeredSubagentTool();

		const result = await tool.execute(
			"tool-call-id",
			{ agent: "test-agent", task: "die", agentScope: "project", confirmProjectAgents: false },
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		expect(result.isError).toBe(true);
		expect(textContent(result)).toContain("Process killed by SIGTERM");
	});

	it("renders failed parallel diagnostics in collapsed mode when assistant text exists", () => {
		const tool = registeredSubagentTool();
		const rendered = renderToolResult(tool, failedParallelRenderResult(), false);

		expect(rendered).toContain("provider failed");
		expect(rendered).not.toContain("stale assistant output");
	});

	it("renders failed parallel diagnostics in expanded mode when assistant text exists", () => {
		const tool = registeredSubagentTool();
		const rendered = renderToolResult(tool, failedParallelRenderResult(), true);

		expect(rendered).toContain("provider failed");
		expect(rendered).not.toContain("stale assistant output");
	});

	it("escalates aborted subprocesses that ignore SIGTERM", async () => {
		const originalSetTimeout = globalThis.setTimeout;
		vi.spyOn(globalThis, "setTimeout").mockImplementation(((handler: any, timeout?: number, ...args: any[]) => {
			return originalSetTimeout(handler, timeout === 5000 ? 10 : timeout, ...args);
		}) as typeof setTimeout);
		const readyPath = path.join(tmpDir, "ready");
		process.argv[1] = writeFakeInvocation(
			tmpDir,
			[
				"const fs = require('node:fs');",
				"process.on('SIGTERM', () => {});",
				`fs.writeFileSync(${JSON.stringify(readyPath)}, 'ready');`,
				"setInterval(() => {}, 1000);",
			].join("\n"),
		);
		const tool = registeredSubagentTool();
		const controller = new AbortController();

		const promise = tool.execute(
			"tool-call-id",
			{ agent: "test-agent", task: "hang", agentScope: "project", confirmProjectAgents: false },
			controller.signal,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);
		for (let i = 0; i < 50 && !fs.existsSync(readyPath); i++) {
			await new Promise((resolve) => originalSetTimeout(resolve, 5));
		}
		expect(fs.existsSync(readyPath)).toBe(true);

		controller.abort();

		await expect(promise).rejects.toThrow("Subagent was aborted");
	});
});

describe("getPiInvocation — fallback", () => {
	let origArgv1: string;
	let origExecPath: string;

	beforeEach(() => {
		origArgv1 = process.argv[1];
		origExecPath = process.execPath;
	});

	afterEach(() => {
		process.argv[1] = origArgv1;
		process.execPath = origExecPath;
	});

	it("falls back to 'scramjet' when argv[1] does not exist and execPath is a generic runtime", () => {
		process.argv[1] = "/nonexistent/path/to/script.js";
		process.execPath = "/usr/bin/node";

		const result = getPiInvocation(["--help"]);

		expect(result.command).toBe("scramjet");
		expect(result.args).toEqual(["--help"]);
	});
});
