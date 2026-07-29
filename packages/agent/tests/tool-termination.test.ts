import { type AssistantMessage, createAssistantMessageEventStream, type Model } from "@leanandmean/ai";
import { describe, expect, it, vi } from "vitest";
import { runAgentLoop } from "../src/agent-loop.js";
import type { AgentContext, AgentLoopConfig, AgentMessage, AgentTool, AgentToolResult } from "../src/types.js";

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

function tool(name: string, result: AgentToolResult<undefined>, executed: string[]): AgentTool {
	return {
		name,
		label: name,
		description: name,
		parameters: { type: "object", properties: {} },
		execute: async () => {
			executed.push(name);
			return result;
		},
	};
}

async function runBatch(firstResult: AgentToolResult<undefined>, configOverrides: Partial<AgentLoopConfig> = {}) {
	const executed: string[] = [];
	const tools = [
		tool("first", firstResult, executed),
		tool("second", { content: [{ type: "text", text: "second" }], details: undefined }, executed),
	];
	const responses = [
		assistantMessage(
			[
				{ type: "toolCall", id: "call-1", name: "first", arguments: {} },
				{ type: "toolCall", id: "call-2", name: "second", arguments: {} },
			],
			"toolUse",
		),
		assistantMessage([{ type: "text", text: "continued" }], "stop"),
	];
	let streamCalls = 0;
	const streamFn = () => {
		const message = responses[streamCalls++]!;
		const stream = createAssistantMessageEventStream();
		stream.push({ type: "start", partial: message });
		stream.push({ type: "done", reason: message.stopReason as "toolUse" | "stop", message });
		return stream;
	};
	const context: AgentContext = { systemPrompt: "test", messages: [], tools };
	const config: AgentLoopConfig = {
		model: testModel,
		convertToLlm: (messages) => messages as any,
		...configOverrides,
	};
	const prompt: AgentMessage = { role: "user", content: "run", timestamp: Date.now() };
	await runAgentLoop([prompt], context, config, async () => {}, undefined, streamFn);
	return { executed, streamCalls };
}

describe("tool batch termination", () => {
	it("preserves legacy all-results termination for mixed batches", async () => {
		const result = await runBatch({
			content: [{ type: "text", text: "first" }],
			details: undefined,
			terminate: true,
		});

		expect(result.executed).toEqual(["first", "second"]);
		expect(result.streamCalls).toBe(2);
	});

	it("stops after sibling results finalize when any result requests unconditional termination", async () => {
		const result = await runBatch({
			content: [{ type: "text", text: "first" }],
			details: undefined,
			terminate: true,
			terminationMode: "any",
		});

		expect(result.executed).toEqual(["first", "second"]);
		expect(result.streamCalls).toBe(1);
	});

	it("preserves unconditional termination through afterToolCall result patches", async () => {
		const afterToolCall = vi.fn(async () => ({ content: [{ type: "text" as const, text: "patched" }] }));
		const result = await runBatch(
			{
				content: [{ type: "text", text: "first" }],
				details: undefined,
				terminate: true,
				terminationMode: "any",
			},
			{ afterToolCall },
		);

		expect(afterToolCall).toHaveBeenCalledTimes(2);
		expect(result.streamCalls).toBe(1);
	});
});
