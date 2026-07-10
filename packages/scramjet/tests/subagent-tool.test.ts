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

function renderToolCall(tool: any, args: any): string {
	const theme = {
		fg: (_color: string, text: string) => text,
		bold: (text: string) => text,
	};
	return tool.renderCall(args, theme, {}).render(120).join("\n");
}

function renderToolResult(tool: any, result: any, expanded: boolean, args?: any): string {
	const theme = {
		fg: (_color: string, text: string) => text,
		bold: (text: string) => text,
	};
	return tool
		.renderResult(result, { expanded }, theme, { args: args ?? {} })
		.render(120)
		.join("\n");
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

describe("subagent tool — chain mode", () => {
	let tmpDir: string;
	let origArgv1: string;

	beforeEach(() => {
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-subagent-test-"));
		origArgv1 = process.argv[1];
		writeProjectAgent(tmpDir, "echo-agent.md", ["name: echo-agent", "description: Echo agent"]);
	});

	afterEach(() => {
		process.argv[1] = origArgv1;
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("substitutes {previous} placeholder with prior step output", async () => {
		process.argv[1] = writeFakeInvocation(
			tmpDir,
			`const task = process.argv[process.argv.length - 1];
			const match = task.match(/Task: (.*)/);
			const text = match ? match[1] : task;
			process.stdout.write(${JSON.stringify("")}+JSON.stringify({
				type: "message_end",
				message: { role: "assistant", content: [{ type: "text", text }] }
			})+"\\n");
			`,
		);
		const tool = registeredSubagentTool();

		const result = await tool.execute(
			"tool-call-id",
			{
				chain: [
					{ agent: "echo-agent", task: "step-one-output" },
					{ agent: "echo-agent", task: "received: {previous}" },
				],
				agentScope: "project",
				confirmProjectAgents: false,
			},
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		expect(textContent(result)).toContain("received: step-one-output");
		expect(result.isError).toBeUndefined();
	});

	it("stops at first error and reports the failing step", async () => {
		process.argv[1] = writeFakeInvocation(tmpDir, 'process.stderr.write("step failed\\n"); process.exit(1);\n');
		const tool = registeredSubagentTool();

		const result = await tool.execute(
			"tool-call-id",
			{
				chain: [
					{ agent: "echo-agent", task: "will-fail" },
					{ agent: "echo-agent", task: "should-not-run {previous}" },
				],
				agentScope: "project",
				confirmProjectAgents: false,
			},
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		expect(result.isError).toBe(true);
		expect(textContent(result)).toContain("Chain stopped at step 1");
		expect(textContent(result)).toContain("step failed");
		expect(result.details.results).toHaveLength(1);
	});
});

describe("subagent tool — validation guards", () => {
	let tmpDir: string;

	beforeEach(() => {
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-subagent-test-"));
		writeProjectAgent(tmpDir, "test-agent.md", ["name: test-agent", "description: Test agent"]);
	});

	afterEach(() => {
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("rejects when multiple modes are specified", async () => {
		const tool = registeredSubagentTool();

		const result = await tool.execute(
			"tool-call-id",
			{
				agent: "test-agent",
				task: "do something",
				tasks: [{ agent: "test-agent", task: "also do something" }],
				agentScope: "project",
				confirmProjectAgents: false,
			},
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		expect(textContent(result)).toContain("Invalid parameters");
		expect(textContent(result)).toContain("exactly one mode");
	});

	it("rejects when parallel tasks exceed MAX_PARALLEL_TASKS", async () => {
		const tool = registeredSubagentTool();
		const tasks = Array.from({ length: 9 }, (_, i) => ({ agent: "test-agent", task: `task-${i}` }));

		const result = await tool.execute(
			"tool-call-id",
			{ tasks, agentScope: "project", confirmProjectAgents: false },
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		expect(textContent(result)).toContain("Too many parallel tasks (9)");
		expect(textContent(result)).toContain("Max is 8");
	});
});

describe("discoverAgents — scope override", () => {
	let tmpDir: string;

	beforeEach(() => {
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-agent-test-"));
	});

	afterEach(() => {
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("project agent with same name shadows user agent in 'both' scope", () => {
		writeProjectAgent(tmpDir, "shared-agent.md", ["name: shared-agent", "description: project version"]);

		const userDir = path.join(os.tmpdir(), `scramjet-agent-user-${Date.now()}`);
		fs.mkdirSync(userDir, { recursive: true });
		fs.writeFileSync(
			path.join(userDir, "shared-agent.md"),
			["---", "name: shared-agent", "description: user version", "---", "", "body"].join("\n"),
		);

		vi.doMock("@leanandmean/coding-agent", async (importOriginal) => {
			const original = (await importOriginal()) as any;
			return {
				...original,
				getAgentDir: () => path.dirname(userDir),
			};
		});

		return import("../src/subagent/agents.js").then(({ discoverAgents: discover }) => {
			const result = discover(tmpDir, "both");

			const sharedAgent = result.agents.find((a: any) => a.name === "shared-agent");
			expect(sharedAgent).toBeDefined();
			expect(sharedAgent!.description).toBe("project version");
			expect(sharedAgent!.source).toBe("project");

			fs.rmSync(userDir, { recursive: true, force: true });
			vi.restoreAllMocks();
		});
	});
});

describe("discoverAgents — error diagnostics", () => {
	let tmpDir: string;

	beforeEach(() => {
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-agent-test-"));
	});

	afterEach(() => {
		const scramjetDir = path.join(tmpDir, ".scramjet");
		const agentsDir = path.join(scramjetDir, "agents");
		try {
			fs.chmodSync(scramjetDir, 0o755);
		} catch {}
		try {
			fs.chmodSync(agentsDir, 0o755);
		} catch {}
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("reports diagnostic when readdirSync fails on an existing directory", () => {
		const agentsDir = path.join(tmpDir, ".scramjet", "agents");
		fs.mkdirSync(agentsDir, { recursive: true });
		fs.chmodSync(agentsDir, 0o000);

		const result = discoverAgents(tmpDir, "project");

		expect(result.agents).toEqual([]);
		expect(result.diagnostics).toEqual([expect.stringContaining("failed to read agent directory")]);
	});

	it("reports diagnostic when statSync fails with EACCES during directory walk", () => {
		const scramjetDir = path.join(tmpDir, ".scramjet");
		const agentsDir = path.join(scramjetDir, "agents");
		fs.mkdirSync(agentsDir, { recursive: true });
		fs.chmodSync(scramjetDir, 0o000);

		const result = discoverAgents(tmpDir, "project");

		expect(result.agents).toEqual([]);
		expect(result.projectAgentsDir).toBeNull();
		expect(result.diagnostics).toEqual([expect.stringContaining("cannot check directory")]);
	});

	it("reports diagnostic when readFileSync fails on a dangling symlink", () => {
		const agentsDir = path.join(tmpDir, ".scramjet", "agents");
		fs.mkdirSync(agentsDir, { recursive: true });
		fs.symlinkSync("/nonexistent/target.md", path.join(agentsDir, "dangling.md"));

		const result = discoverAgents(tmpDir, "project");

		expect(result.agents).toEqual([]);
		expect(result.diagnostics).toEqual([expect.stringContaining("failed to read agent file")]);
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

describe("renderCall — parallel mode", () => {
	it("shows all tasks without truncation for 5+ tasks", () => {
		const tool = registeredSubagentTool();
		const tasks = Array.from({ length: 6 }, (_, i) => ({
			agent: `agent-${i}`,
			task: `Task number ${i} with a long description that exceeds forty characters easily`,
		}));

		const rendered = renderToolCall(tool, { tasks });

		for (let i = 0; i < 6; i++) {
			expect(rendered).toContain(`agent-${i}`);
			expect(rendered).toContain(`Task number ${i} with a long description that exceeds forty characters easily`);
		}
		expect(rendered).not.toContain("... +");
		expect(rendered).not.toContain("more");
	});

	it("displays effort per task using resolved level", () => {
		const tool = registeredSubagentTool();
		const tasks = [
			{ agent: "reviewer", task: "Review code", effort: "medium" },
			{ agent: "analyzer", task: "Analyze types", effort: "xhigh" },
		];

		const rendered = renderToolCall(tool, { tasks });

		expect(rendered).toContain("[Effort:medium]");
		// xhigh is capped at parent level (high in recordingPi)
		expect(rendered).toContain("[Effort:high]");
	});

	it("shows parent effort level when no explicit effort specified", () => {
		const tool = registeredSubagentTool();
		const tasks = [{ agent: "reviewer", task: "Review code" }];

		const rendered = renderToolCall(tool, { tasks });

		// Parent level from recordingPi is "high"
		expect(rendered).toContain("[Effort:high]");
	});
});

describe("renderCall — chain mode", () => {
	it("shows all steps without truncation for 4+ steps", () => {
		const tool = registeredSubagentTool();
		const chain = Array.from({ length: 5 }, (_, i) => ({
			agent: `step-agent-${i}`,
			task: `Step ${i} task with enough text to verify no truncation occurs at all`,
		}));

		const rendered = renderToolCall(tool, { chain });

		for (let i = 0; i < 5; i++) {
			expect(rendered).toContain(`step-agent-${i}`);
			expect(rendered).toContain(`Step ${i} task with enough text to verify no truncation occurs at all`);
		}
		expect(rendered).not.toContain("... +");
		expect(rendered).not.toContain("more");
	});

	it("strips {previous} placeholder from chain task text", () => {
		const tool = registeredSubagentTool();
		const chain = [
			{ agent: "first", task: "Initial task" },
			{ agent: "second", task: "Process {previous} and continue" },
		];

		const rendered = renderToolCall(tool, { chain });

		expect(rendered).toContain("Process  and continue");
		expect(rendered).not.toContain("{previous}");
	});

	it("displays effort per step", () => {
		const tool = registeredSubagentTool();
		const chain = [
			{ agent: "explorer", task: "Explore", effort: "low" },
			{ agent: "architect", task: "Design", effort: "high" },
		];

		const rendered = renderToolCall(tool, { chain });

		expect(rendered).toContain("[Effort:low]");
		expect(rendered).toContain("[Effort:high]");
	});
});

describe("renderCall — single mode", () => {
	it("shows full task text without truncation", () => {
		const tool = registeredSubagentTool();
		const longTask = "Analyze the entire codebase architecture and provide a comprehensive report on patterns";

		const rendered = renderToolCall(tool, { agent: "explorer", task: longTask });

		expect(rendered).toContain(longTask);
		expect(rendered).not.toContain("...");
	});

	it("displays effort for single mode", () => {
		const tool = registeredSubagentTool();

		const rendered = renderToolCall(tool, { agent: "explorer", task: "Explore", effort: "medium" });

		expect(rendered).toContain("[Effort:medium]");
	});

	it("shows parent effort when no explicit effort", () => {
		const tool = registeredSubagentTool();

		const rendered = renderToolCall(tool, { agent: "explorer", task: "Explore" });

		expect(rendered).toContain("[Effort:high]");
	});
});

describe("renderResult model and effort", () => {
	function singleResult(overrides: any = {}) {
		return {
			content: [{ type: "text", text: "done" }],
			details: {
				mode: "single",
				agentScope: "user",
				projectAgentsDir: null,
				results: [
					{
						agent: "test-agent",
						agentSource: "user",
						task: "do something",
						exitCode: 0,
						messages: [{ role: "assistant", content: [{ type: "text", text: "result" }] }],
						stderr: "",
						usage: {
							input: 100,
							output: 50,
							cacheRead: 0,
							cacheWrite: 0,
							cost: 0.01,
							contextTokens: 150,
							turns: 1,
						},
						model: "claude-sonnet-4-20250514",
						...overrides,
					},
				],
			},
		};
	}

	function parallelResult(count: number, overrides: any = {}) {
		return {
			content: [{ type: "text", text: `Parallel: ${count}/${count} succeeded` }],
			details: {
				mode: "parallel",
				agentScope: "user",
				projectAgentsDir: null,
				results: Array.from({ length: count }, (_, i) => ({
					agent: `agent-${i}`,
					agentSource: "user",
					task: `task ${i}`,
					exitCode: 0,
					messages: [{ role: "assistant", content: [{ type: "text", text: `output ${i}` }] }],
					stderr: "",
					usage: { input: 100, output: 50, cacheRead: 0, cacheWrite: 0, cost: 0.01, contextTokens: 150, turns: 1 },
					model: "claude-sonnet-4-20250514",
					...overrides,
				})),
			},
		};
	}

	function chainResult(count: number, overrides: any = {}) {
		return {
			content: [{ type: "text", text: "chain done" }],
			details: {
				mode: "chain",
				agentScope: "user",
				projectAgentsDir: null,
				results: Array.from({ length: count }, (_, i) => ({
					agent: `step-agent-${i}`,
					agentSource: "user",
					task: `step task ${i}`,
					step: i + 1,
					exitCode: 0,
					messages: [{ role: "assistant", content: [{ type: "text", text: `step output ${i}` }] }],
					stderr: "",
					usage: { input: 100, output: 50, cacheRead: 0, cacheWrite: 0, cost: 0.01, contextTokens: 150, turns: 1 },
					model: "claude-opus-4-20250514",
					...overrides,
				})),
			},
		};
	}

	it("shows model in single expanded header", () => {
		const tool = registeredSubagentTool();
		const rendered = renderToolResult(tool, singleResult(), true, { effort: "high" });
		expect(rendered).toContain("claude-sonnet-4-20250514");
	});

	it("shows model in single collapsed header", () => {
		const tool = registeredSubagentTool();
		const rendered = renderToolResult(tool, singleResult(), false, { effort: "high" });
		expect(rendered).toContain("claude-sonnet-4-20250514");
	});

	it("shows effort in single result header", () => {
		const tool = registeredSubagentTool();
		const rendered = renderToolResult(tool, singleResult(), true, { effort: "medium" });
		expect(rendered).toContain("[Effort:medium]");
	});

	it("omits model when undefined", () => {
		const tool = registeredSubagentTool();
		const rendered = renderToolResult(tool, singleResult({ model: undefined }), true, { effort: "high" });
		expect(rendered).not.toContain("claude-sonnet");
		expect(rendered).toContain("test-agent");
	});

	it("shows model and effort in parallel expanded headers", () => {
		const tool = registeredSubagentTool();
		const args = {
			tasks: [
				{ agent: "agent-0", task: "task 0", effort: "low" },
				{ agent: "agent-1", task: "task 1", effort: "medium" },
			],
		};
		const rendered = renderToolResult(tool, parallelResult(2), true, args);
		expect(rendered).toContain("claude-sonnet-4-20250514");
		expect(rendered).toContain("[Effort:low]");
		expect(rendered).toContain("[Effort:medium]");
	});

	it("shows model and effort in parallel collapsed headers", () => {
		const tool = registeredSubagentTool();
		const args = {
			tasks: [
				{ agent: "agent-0", task: "task 0", effort: "high" },
				{ agent: "agent-1", task: "task 1" },
			],
		};
		const rendered = renderToolResult(tool, parallelResult(2), false, args);
		expect(rendered).toContain("claude-sonnet-4-20250514");
		expect(rendered).toContain("[Effort:high]");
	});

	it("shows model and effort in chain expanded headers", () => {
		const tool = registeredSubagentTool();
		const args = {
			chain: [
				{ agent: "step-agent-0", task: "step 0", effort: "low" },
				{ agent: "step-agent-1", task: "step 1", effort: "high" },
			],
		};
		const rendered = renderToolResult(tool, chainResult(2), true, args);
		expect(rendered).toContain("claude-opus-4-20250514");
		expect(rendered).toContain("[Effort:low]");
		expect(rendered).toContain("[Effort:high]");
	});

	it("shows model and effort in chain collapsed headers", () => {
		const tool = registeredSubagentTool();
		const args = {
			chain: [
				{ agent: "step-agent-0", task: "step 0", effort: "medium" },
				{ agent: "step-agent-1", task: "step 1" },
			],
		};
		const rendered = renderToolResult(tool, chainResult(2), false, args);
		expect(rendered).toContain("claude-opus-4-20250514");
		expect(rendered).toContain("[Effort:medium]");
	});

	it("resolves effort against parent level", () => {
		const tool = registeredSubagentTool();
		const args = { effort: "xhigh" };
		const rendered = renderToolResult(tool, singleResult(), true, args);
		// Parent level is "high" (from mock), xhigh should cap to high
		expect(rendered).toContain("[Effort:high]");
	});

	it("shows parent effort when no explicit effort in args", () => {
		const tool = registeredSubagentTool();
		const rendered = renderToolResult(tool, singleResult(), false, {});
		expect(rendered).toContain("[Effort:high]");
	});

	it("renders chain result when results has fewer entries than args.chain", () => {
		const tool = registeredSubagentTool();
		const result = {
			content: [{ type: "text", text: "chain done" }],
			details: {
				mode: "chain",
				agentScope: "user",
				projectAgentsDir: null,
				results: [
					{
						agent: "step-agent-0",
						agentSource: "user",
						task: "first task",
						step: 1,
						exitCode: 1,
						messages: [{ role: "assistant", content: [{ type: "text", text: "failed" }] }],
						stderr: "error occurred",
						usage: {
							input: 100,
							output: 50,
							cacheRead: 0,
							cacheWrite: 0,
							cost: 0.01,
							contextTokens: 150,
							turns: 1,
						},
						model: "claude-sonnet-4-20250514",
						stopReason: "error",
						errorMessage: "step failed",
					},
				],
			},
		};
		const args = {
			chain: [
				{ agent: "step-agent-0", task: "first task", effort: "low" },
				{ agent: "step-agent-1", task: "second task", effort: "medium" },
				{ agent: "step-agent-2", task: "third task", effort: "high" },
			],
		};

		const expanded = renderToolResult(tool, result, true, args);
		const collapsed = renderToolResult(tool, result, false, args);

		expect(expanded).toContain("step-agent-0");
		expect(expanded).toContain("[Effort:low]");
		expect(expanded).toContain("0/1 steps");
		expect(expanded).not.toContain("step-agent-1");
		expect(collapsed).toContain("step-agent-0");
		expect(collapsed).toContain("0/1 steps");
	});

	it("shows explicit effort uncapped when getThinkingLevel throws", () => {
		const { pi, tools } = recordingPi();
		pi.getThinkingLevel = () => {
			throw new Error("no thinking level available");
		};
		registerSubagentTool(pi);
		const tool = tools[0];

		const callRendered = renderToolCall(tool, { agent: "explorer", task: "Explore", effort: "xhigh" });
		expect(callRendered).toContain("[Effort:xhigh]");

		const resultRendered = renderToolResult(tool, singleResult(), true, { effort: "xhigh" });
		expect(resultRendered).toContain("[Effort:xhigh]");
	});

	it("shows no effort badge when getThinkingLevel throws and no explicit effort", () => {
		const { pi, tools } = recordingPi();
		pi.getThinkingLevel = () => {
			throw new Error("no thinking level available");
		};
		registerSubagentTool(pi);
		const tool = tools[0];

		const callRendered = renderToolCall(tool, { agent: "explorer", task: "Explore" });
		expect(callRendered).not.toContain("[Effort:");

		const resultRendered = renderToolResult(tool, singleResult(), true, {});
		expect(resultRendered).not.toContain("[Effort:");
	});
});

describe("renderResult — parallel progress accuracy", () => {
	function partialParallelResult(results: any[]) {
		return {
			content: [{ type: "text", text: "in progress" }],
			details: {
				mode: "parallel",
				agentScope: "user",
				projectAgentsDir: null,
				results,
			},
		};
	}

	function makeResult(overrides: any = {}) {
		return {
			agent: "test-agent",
			agentSource: "user",
			task: "some task",
			exitCode: 0,
			messages: [],
			stderr: "",
			usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0, contextTokens: 0, turns: 0 },
			model: "claude-sonnet-4-20250514",
			...overrides,
		};
	}

	it("shows running indicator when some tasks have not finalized", () => {
		const tool = registeredSubagentTool();
		const results = [
			makeResult({ agent: "done-agent", exitCode: 0, messages: [{ role: "assistant", content: [{ type: "text", text: "output" }] }] }),
			makeResult({ agent: "running-agent-1", exitCode: -1 }),
			makeResult({ agent: "running-agent-2", exitCode: -1 }),
		];
		const rendered = renderToolResult(tool, partialParallelResult(results), false);
		expect(rendered).toContain("⏳");
		expect(rendered).toContain("1/3 done, 2 running");
	});

	it("shows (running...) for tasks with no output that are still running", () => {
		const tool = registeredSubagentTool();
		const results = [
			makeResult({ agent: "done-agent", exitCode: 0, messages: [{ role: "assistant", content: [{ type: "text", text: "output" }] }] }),
			makeResult({ agent: "running-agent", exitCode: -1 }),
		];
		const rendered = renderToolResult(tool, partialParallelResult(results), false);
		expect(rendered).toContain("(running...)");
	});

	it("shows all-running state when no tasks have finalized", () => {
		const tool = registeredSubagentTool();
		const results = [
			makeResult({ agent: "agent-0", exitCode: -1 }),
			makeResult({ agent: "agent-1", exitCode: -1 }),
			makeResult({ agent: "agent-2", exitCode: -1 }),
		];
		const rendered = renderToolResult(tool, partialParallelResult(results), false);
		expect(rendered).toContain("⏳");
		expect(rendered).toContain("0/3 done, 3 running");
		expect(rendered).not.toContain("✓");
	});

	it("shows success only when all tasks have finalized successfully", () => {
		const tool = registeredSubagentTool();
		const results = [
			makeResult({ agent: "agent-0", exitCode: 0, messages: [{ role: "assistant", content: [{ type: "text", text: "out 0" }] }] }),
			makeResult({ agent: "agent-1", exitCode: 0, messages: [{ role: "assistant", content: [{ type: "text", text: "out 1" }] }] }),
		];
		const rendered = renderToolResult(tool, partialParallelResult(results), false);
		expect(rendered).toContain("✓");
		expect(rendered).toContain("2/2 tasks");
		expect(rendered).not.toContain("running");
	});
});
