import {
	type AssistantMessage,
	createAssistantMessageEventStream,
	type Model,
	type SystemPromptSection,
} from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
import { Agent } from "../src/agent.js";
import type { AgentTool } from "../src/types.js";

const modelA: Model<"openai-chat"> = {
	id: "model-a",
	name: "Model A",
	api: "openai-chat",
	provider: "openai",
	baseUrl: "https://api.openai.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};
const modelB = { ...modelA, id: "model-b", name: "Model B" };
const usage = {
	input: 0,
	output: 0,
	cacheRead: 0,
	cacheWrite: 0,
	totalTokens: 0,
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
};

function assistant(
	model: Model<any>,
	content: AssistantMessage["content"],
	stopReason: "toolUse" | "stop",
): AssistantMessage {
	return {
		role: "assistant",
		content,
		api: model.api,
		provider: model.provider,
		model: model.id,
		usage,
		stopReason,
		timestamp: Date.now(),
	};
}

function streamOf(message: AssistantMessage) {
	const stream = createAssistantMessageEventStream();
	stream.push({ type: "start", partial: message });
	stream.push({ type: "done", reason: message.stopReason as "toolUse" | "stop", message });
	return stream;
}

describe("beforeProviderCall", () => {
	it("isolates request context from stored state", async () => {
		const storedPrompt: SystemPromptSection[] = [{ id: "stored", text: "stored" }];
		let capturedMessageCount = 0;
		let capturedToolCount = 0;
		const agent = new Agent({
			initialState: { model: modelA, systemPrompt: storedPrompt, tools: [] },
			beforeProviderCall: async (context) => {
				(context.systemPrompt as SystemPromptSection[]).push({ id: "request", text: "request" });
				context.messages.push({ role: "user", content: "request-only", timestamp: Date.now() });
				context.tools?.push({
					name: "request-only",
					label: "Request only",
					description: "request only",
					parameters: { type: "object", properties: {} },
					execute: async () => ({ content: [], details: undefined }),
				});
				return context;
			},
			streamFn: ((_model, context) => {
				capturedMessageCount = context.messages.length;
				capturedToolCount = context.tools?.length ?? 0;
				expect(context.systemPrompt).toHaveLength(2);
				return streamOf(assistant(modelA, [{ type: "text", text: "done" }], "stop"));
			}) as Agent["streamFn"],
		});

		await agent.prompt("go");

		expect(capturedMessageCount).toBe(2);
		expect(capturedToolCount).toBe(1);
		expect(agent.state.systemPrompt).toEqual(storedPrompt);
		expect(agent.state.tools).toEqual([]);
		expect(agent.state.messages).toHaveLength(2);
	});

	it("observes refreshed routing and changes only the request-local prompt", async () => {
		const routed: string[] = [];
		const prompts: Array<string | SystemPromptSection[] | undefined> = [];
		let call = 0;
		let agent!: Agent;
		const tool: AgentTool = {
			name: "switch",
			label: "Switch",
			description: "switch model",
			parameters: { type: "object", properties: {} },
			execute: async () => {
				agent.state.model = modelB;
				return { content: [{ type: "text", text: "ok" }], details: undefined };
			},
		};
		agent = new Agent({
			initialState: { model: modelA, systemPrompt: "stored", tools: [tool] },
			beforeProviderCall: async (context, model) => {
				routed.push(model.id);
				return { ...context, systemPrompt: `request-${model.id}` };
			},
			streamFn: ((model, context) => {
				prompts.push(context.systemPrompt);
				return streamOf(
					call++ === 0
						? assistant(model, [{ type: "toolCall", id: "switch-1", name: "switch", arguments: {} }], "toolUse")
						: assistant(model, [{ type: "text", text: "done" }], "stop"),
				);
			}) as Agent["streamFn"],
		});

		await agent.prompt("go");

		expect(routed).toEqual(["model-a", "model-b"]);
		expect(prompts).toEqual(["request-model-a", "request-model-b"]);
		expect(agent.state.systemPrompt).toBe("stored");
	});
});

describe("assistant origin", () => {
	it("marks provider, harness, and Agent-local failure messages", async () => {
		const providerAgent = new Agent({
			initialState: { model: modelA },
			streamFn: (() => streamOf(assistant(modelA, [{ type: "text", text: "done" }], "stop"))) as Agent["streamFn"],
		});
		await providerAgent.prompt("go");
		expect((providerAgent.state.messages.at(-1) as AssistantMessage).origin).toBe("provider");

		const harnessAgent = new Agent({ initialState: { model: modelA } });
		await harnessAgent.runHarnessTool(
			{
				name: "notice",
				label: "Notice",
				description: "notice",
				parameters: { type: "object", properties: {} },
				execute: async () => ({ content: [{ type: "text", text: "ok" }], details: undefined }),
			},
			{},
		);
		expect((harnessAgent.state.messages[0] as AssistantMessage).origin).toBe("harness");

		const failureAgent = new Agent({
			initialState: { model: modelA },
			streamFn: (() => {
				throw new Error("boom");
			}) as Agent["streamFn"],
		});
		await failureAgent.prompt("go");
		expect((failureAgent.state.messages.at(-1) as AssistantMessage).origin).toBe("harness");
	});
});
