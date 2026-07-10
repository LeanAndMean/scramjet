import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { capThinkingLevel, registerSubagentTool } from "../src/subagent/index.js";
import { recordingPi } from "./helpers.js";

describe("capThinkingLevel", () => {
	it("returns requested when below parent", () => {
		expect(capThinkingLevel("low", "high")).toBe("low");
	});

	it("returns requested when equal to parent", () => {
		expect(capThinkingLevel("medium", "medium")).toBe("medium");
	});

	it("caps to parent when requested exceeds parent", () => {
		expect(capThinkingLevel("xhigh", "medium")).toBe("medium");
	});

	it("handles 'off' as parent (lowest possible)", () => {
		expect(capThinkingLevel("high", "off")).toBe("off");
	});

	it("handles 'off' as requested (always passes through)", () => {
		expect(capThinkingLevel("off", "high")).toBe("off");
	});

	it("handles 'xhigh' parent (never caps)", () => {
		expect(capThinkingLevel("xhigh", "xhigh")).toBe("xhigh");
		expect(capThinkingLevel("high", "xhigh")).toBe("high");
	});

	it("caps minimal to off when parent is off", () => {
		expect(capThinkingLevel("minimal", "off")).toBe("off");
	});

	it("passes through each level when parent is xhigh", () => {
		const levels = ["off", "minimal", "low", "medium", "high", "xhigh"] as const;
		for (const level of levels) {
			expect(capThinkingLevel(level, "xhigh")).toBe(level);
		}
	});

	it("caps max to parent when parent is below max", () => {
		expect(capThinkingLevel("max", "xhigh")).toBe("xhigh");
		expect(capThinkingLevel("max", "high")).toBe("high");
	});

	it("passes through max when parent is max", () => {
		expect(capThinkingLevel("max", "max")).toBe("max");
	});

	it("passes through each level when parent is max", () => {
		const levels = ["off", "minimal", "low", "medium", "high", "xhigh", "max"] as const;
		for (const level of levels) {
			expect(capThinkingLevel(level, "max")).toBe(level);
		}
	});
});

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

function registeredSubagentTool(thinkingLevel = "high") {
	const { pi, tools } = recordingPi();
	pi.getThinkingLevel = () => thinkingLevel;
	registerSubagentTool(pi);
	return tools[0];
}

function textContent(result: any): string {
	const first = result.content[0];
	return first?.type === "text" ? first.text : "";
}

function argsEchoScript(): string {
	return `process.stdout.write(JSON.stringify({
		type: "message_end",
		message: { role: "assistant", content: [{ type: "text", text: JSON.stringify(process.argv) }] }
	})+"\\n");`;
}

describe("subagent effort — arg construction", () => {
	let tmpDir: string;
	let origArgv1: string;

	beforeEach(() => {
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-effort-test-"));
		origArgv1 = process.argv[1];
		writeProjectAgent(tmpDir, "test-agent.md", ["name: test-agent", "description: Test agent"]);
	});

	afterEach(() => {
		process.argv[1] = origArgv1;
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("passes explicit effort capped at parent level", async () => {
		process.argv[1] = writeFakeInvocation(tmpDir, argsEchoScript());
		const tool = registeredSubagentTool("medium");

		const result = await tool.execute(
			"tool-call-id",
			{ agent: "test-agent", task: "work", effort: "high", agentScope: "project", confirmProjectAgents: false },
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		const argv = JSON.parse(textContent(result));
		const thinkingIdx = argv.indexOf("--thinking");
		expect(thinkingIdx).toBeGreaterThan(-1);
		expect(argv[thinkingIdx + 1]).toBe("medium");
	});

	it("passes explicit effort below parent unchanged", async () => {
		process.argv[1] = writeFakeInvocation(tmpDir, argsEchoScript());
		const tool = registeredSubagentTool("high");

		const result = await tool.execute(
			"tool-call-id",
			{ agent: "test-agent", task: "work", effort: "low", agentScope: "project", confirmProjectAgents: false },
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		const argv = JSON.parse(textContent(result));
		const thinkingIdx = argv.indexOf("--thinking");
		expect(thinkingIdx).toBeGreaterThan(-1);
		expect(argv[thinkingIdx + 1]).toBe("low");
	});

	it("inherits parent level when effort is omitted", async () => {
		process.argv[1] = writeFakeInvocation(tmpDir, argsEchoScript());
		const tool = registeredSubagentTool("high");

		const result = await tool.execute(
			"tool-call-id",
			{ agent: "test-agent", task: "work", agentScope: "project", confirmProjectAgents: false },
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		const argv = JSON.parse(textContent(result));
		const thinkingIdx = argv.indexOf("--thinking");
		expect(thinkingIdx).toBeGreaterThan(-1);
		expect(argv[thinkingIdx + 1]).toBe("high");
	});

	it("passes --thinking off when parent is off and no effort specified", async () => {
		process.argv[1] = writeFakeInvocation(tmpDir, argsEchoScript());
		const tool = registeredSubagentTool("off");

		const result = await tool.execute(
			"tool-call-id",
			{ agent: "test-agent", task: "work", agentScope: "project", confirmProjectAgents: false },
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		const argv = JSON.parse(textContent(result));
		const thinkingIdx = argv.indexOf("--thinking");
		expect(thinkingIdx).toBeGreaterThan(-1);
		expect(argv[thinkingIdx + 1]).toBe("off");
	});

	it("applies per-task effort in parallel mode", async () => {
		process.argv[1] = writeFakeInvocation(tmpDir, argsEchoScript());
		const tool = registeredSubagentTool("high");

		const result = await tool.execute(
			"tool-call-id",
			{
				tasks: [
					{ agent: "test-agent", task: "easy job", effort: "low" },
					{ agent: "test-agent", task: "hard job", effort: "high" },
				],
				agentScope: "project",
				confirmProjectAgents: false,
			},
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		expect(result.isError).toBeUndefined();
		const results = result.details.results;
		expect(results).toHaveLength(2);

		const argv0 = JSON.parse(getFinalOutput(results[0].messages));
		const argv1 = JSON.parse(getFinalOutput(results[1].messages));
		expect(argv0[argv0.indexOf("--thinking") + 1]).toBe("low");
		expect(argv1[argv1.indexOf("--thinking") + 1]).toBe("high");
	});

	it("applies per-step effort in chain mode", async () => {
		process.argv[1] = writeFakeInvocation(tmpDir, argsEchoScript());
		const tool = registeredSubagentTool("xhigh");

		const result = await tool.execute(
			"tool-call-id",
			{
				chain: [
					{ agent: "test-agent", task: "step1", effort: "low" },
					{ agent: "test-agent", task: "step2", effort: "high" },
				],
				agentScope: "project",
				confirmProjectAgents: false,
			},
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		expect(result.isError).toBeUndefined();
		const results = result.details.results;
		expect(results).toHaveLength(2);

		const argv0 = JSON.parse(getFinalOutput(results[0].messages));
		const argv1 = JSON.parse(getFinalOutput(results[1].messages));
		expect(argv0[argv0.indexOf("--thinking") + 1]).toBe("low");
		expect(argv1[argv1.indexOf("--thinking") + 1]).toBe("high");
	});
});

function getFinalOutput(messages: any[]): string {
	for (let i = messages.length - 1; i >= 0; i--) {
		const msg = messages[i];
		if (msg.role === "assistant") {
			for (const part of msg.content) {
				if (part.type === "text") return part.text;
			}
		}
	}
	return "";
}

describe("subagent model inheritance", () => {
	let tmpDir: string;
	let origArgv1: string;

	beforeEach(() => {
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-model-test-"));
		origArgv1 = process.argv[1];
		writeProjectAgent(tmpDir, "test-agent.md", ["name: test-agent", "description: Test agent"]);
	});

	afterEach(() => {
		process.argv[1] = origArgv1;
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("passes parent model as --model when agent has no model", async () => {
		process.argv[1] = writeFakeInvocation(tmpDir, argsEchoScript());
		const tool = registeredSubagentTool("high");

		const result = await tool.execute(
			"tool-call-id",
			{ agent: "test-agent", task: "work", agentScope: "project", confirmProjectAgents: false },
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false, model: { provider: "anthropic", id: "claude-opus-4-6" } },
		);

		const argv = JSON.parse(textContent(result));
		const modelIdx = argv.indexOf("--model");
		expect(modelIdx).toBeGreaterThan(-1);
		expect(argv[modelIdx + 1]).toBe("anthropic/claude-opus-4-6");
	});

	it("agent frontmatter model takes precedence over parent model", async () => {
		process.argv[1] = writeFakeInvocation(tmpDir, argsEchoScript());
		writeProjectAgent(tmpDir, "model-agent.md", [
			"name: model-agent",
			"description: Agent with model",
			"model: openai/gpt-4o",
		]);
		const tool = registeredSubagentTool("high");

		const result = await tool.execute(
			"tool-call-id",
			{ agent: "model-agent", task: "work", agentScope: "project", confirmProjectAgents: false },
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false, model: { provider: "anthropic", id: "claude-opus-4-6" } },
		);

		const argv = JSON.parse(textContent(result));
		const modelIdx = argv.indexOf("--model");
		expect(modelIdx).toBeGreaterThan(-1);
		expect(argv[modelIdx + 1]).toBe("openai/gpt-4o");
	});

	it("no --model flag when both agent.model and ctx.model are undefined", async () => {
		process.argv[1] = writeFakeInvocation(tmpDir, argsEchoScript());
		const tool = registeredSubagentTool("high");

		const result = await tool.execute(
			"tool-call-id",
			{ agent: "test-agent", task: "work", agentScope: "project", confirmProjectAgents: false },
			undefined,
			undefined,
			{ cwd: tmpDir, hasUI: false },
		);

		const argv = JSON.parse(textContent(result));
		const modelIdx = argv.indexOf("--model");
		expect(modelIdx).toBe(-1);
	});
});
