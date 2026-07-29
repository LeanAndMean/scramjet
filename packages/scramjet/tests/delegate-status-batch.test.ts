import type { AgentContext, AgentLoopConfig, AgentMessage, AgentTool } from "@leanandmean/agent";
import { runAgentLoop } from "@leanandmean/agent";
import { type AssistantMessage, createAssistantMessageEventStream, type Model } from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
import { registerCommandStatusTool } from "../src/command-status.js";
import { registerDelegateTool } from "../src/delegate.js";
import type { CommandDef } from "../src/types.js";
import { freshState, lifecycleFor, recordingPi } from "./helpers.js";

const testModel: Model<"openai-chat"> = {
	id: "test-model",
	name: "Test Model",
	api: "openai-chat",
	provider: "openai",
	baseUrl: "https://api.openai.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};

function assistantMessage(content: AssistantMessage["content"], stopReason: "toolUse" | "stop"): AssistantMessage {
	return {
		role: "assistant",
		content,
		api: "openai-chat",
		provider: "openai",
		model: testModel.id,
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason,
		timestamp: Date.now(),
	};
}

function def(name: string, delegateOnly = false): CommandDef {
	return {
		name,
		filePath: `/fake/${name}.md`,
		body: `${name} body`,
		...(delegateOnly ? { delegateOnly: true } : {}),
	};
}

async function runMixedBatch(order: ["delegate", "status"] | ["status", "delegate"]) {
	const caller = def("test:caller");
	const target = def("test:subcommand", true);
	const state = freshState({
		registry: new Map([
			[caller.name, caller],
			[target.name, target],
		]),
		lifecycle: lifecycleFor("running", caller.name),
	});
	const { pi, tools } = recordingPi();
	registerDelegateTool(pi, state);
	registerCommandStatusTool(pi, state);
	const toolCalls = {
		delegate: {
			type: "toolCall" as const,
			id: "delegate-call",
			name: "delegate",
			arguments: { command: target.name, args: "" },
		},
		status: {
			type: "toolCall" as const,
			id: "status-call",
			name: "report_scramjet_command_status",
			arguments: { status: "completed", summary: "done" },
		},
	};
	const responses = [
		assistantMessage(
			order.map((name) => toolCalls[name]),
			"toolUse",
		),
		assistantMessage([{ type: "text", text: "continued after report" }], "stop"),
	];
	let streamCalls = 0;
	const streamFn = () => {
		const message = responses[streamCalls++]!;
		const stream = createAssistantMessageEventStream();
		stream.push({ type: "start", partial: message });
		stream.push({ type: "done", reason: message.stopReason as "toolUse" | "stop", message });
		return stream;
	};
	const context: AgentContext = { systemPrompt: "test", messages: [], tools: tools as AgentTool[] };
	const config: AgentLoopConfig = { model: testModel, convertToLlm: (messages) => messages as any };
	const prompt: AgentMessage = { role: "user", content: "run", timestamp: Date.now() };
	const messages = await runAgentLoop([prompt], context, config, async () => {}, undefined, streamFn);
	return { messages, state, streamCalls };
}

describe("delegate and terminal status in one tool batch", () => {
	it.each([
		["delegate", "status"],
		["status", "delegate"],
	] as const)("does not start another provider turn for [%s, %s]", async (first, second) => {
		const result = await runMixedBatch([first, second]);
		const toolResults = result.messages.filter((message) => message.role === "toolResult");

		expect(result.streamCalls).toBe(1);
		expect(toolResults).toHaveLength(2);
		expect(result.state.lifecycle.lastReport?.status).toBe("completed");
	});
});
