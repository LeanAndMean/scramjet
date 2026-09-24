import { Type } from "typebox";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getModel } from "../src/models.js";
import { streamSimpleOpenAICompletions } from "../src/providers/openai-completions.js";
import { buildBaseOptions, clampReasoning } from "../src/providers/simple-options.js";
import type { AssistantMessage, Context, Model, ToolResultMessage } from "../src/types.js";

const copilotHeaders = {
	"User-Agent": "GitHubCopilotChat/0.35.0",
	"Editor-Version": "vscode/1.107.0",
	"Editor-Plugin-Version": "copilot-chat/0.35.0",
	"Copilot-Integration-Id": "vscode-chat",
	"X-GitHub-Api-Version": "2026-06-01",
};

function copilotModel(id: string, thinkingLevelMap: Model<"openai-completions">["thinkingLevelMap"]) {
	return {
		id,
		name: id,
		api: "openai-completions",
		provider: "github-copilot",
		baseUrl: "https://api.individual.githubcopilot.com",
		headers: copilotHeaders,
		reasoning: true,
		input: ["text", "image"],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 1000000,
		maxTokens: 64000,
		thinkingLevelMap,
		compat: { supportsStore: false, supportsDeveloperRole: false, supportsReasoningEffort: true },
	} satisfies Model<"openai-completions">;
}

const fableModel = copilotModel("claude-fable-5.1", {
	off: null,
	minimal: null,
	xhigh: "xhigh",
	max: "max",
});
const kimiModel = copilotModel("kimi-k3", {
	off: null,
	minimal: null,
	medium: null,
	xhigh: null,
	max: "max",
});
const apiKey = "test-key";
const context: Context = { messages: [{ role: "user", content: "hello", timestamp: 0 }] };
const toolContext: Context = {
	messages: context.messages,
	tools: [{ name: "read", description: "Read a file", parameters: Type.Object({ path: Type.String() }) }],
};

function chatSse(chunks: Record<string, unknown>[]): Response {
	return new Response(`${chunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`).join("")}data: [DONE]\n\n`, {
		status: 200,
		headers: { "content-type": "text/event-stream" },
	});
}

function completedChatResponse(text = "done"): Response {
	return chatSse([
		{ id: "chat_1", object: "chat.completion.chunk", choices: [{ index: 0, delta: { content: text } }] },
		{ id: "chat_1", object: "chat.completion.chunk", choices: [{ index: 0, delta: {}, finish_reason: "stop" }] },
	]);
}

function toolCallChatResponse(): Response {
	return chatSse([
		{
			id: "chat_tool",
			object: "chat.completion.chunk",
			choices: [
				{
					index: 0,
					delta: {
						tool_calls: [
							{
								index: 0,
								id: "call_read_1",
								type: "function",
								function: { name: "read", arguments: '{"path":"README.md"}' },
							},
						],
					},
				},
			],
		},
		{
			id: "chat_tool",
			object: "chat.completion.chunk",
			choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }],
		},
	]);
}

function stubFetch(responses: Response[]) {
	const requests: Request[] = [];
	vi.stubGlobal(
		"fetch",
		vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
			const request = input instanceof Request ? input : new Request(input, init);
			requests.push(request.clone());
			const response = responses.shift();
			if (!response) throw new Error("Unexpected request");
			return response;
		}),
	);
	return requests;
}

async function requestBody(request: Request): Promise<Record<string, unknown>> {
	return (await request.json()) as Record<string, unknown>;
}

function continuationContext(assistant: AssistantMessage): Context {
	const toolCall = assistant.content.find((block) => block.type === "toolCall");
	if (!toolCall || toolCall.type !== "toolCall") throw new Error("Expected tool call");
	const result: ToolResultMessage = {
		role: "toolResult",
		toolCallId: toolCall.id,
		toolName: toolCall.name,
		content: [{ type: "text", text: "file contents" }],
		isError: false,
		timestamp: 1,
	};
	return { messages: [...context.messages, assistant, result], tools: toolContext.tools };
}

afterEach(() => {
	vi.unstubAllGlobals();
});

describe("output defaults", () => {
	it("does not restrict OpenRouter routes using an aggregate output maximum", () => {
		const model = getModel("openrouter", "qwen/qwen3-14b");
		expect(model.maxTokens).toBe(16384);
		expect(buildBaseOptions(model).maxTokens).toBeUndefined();
		expect(buildBaseOptions(model, { maxTokens: 8192 }).maxTokens).toBe(8192);
	});
	it("omits implicit output on the actual OpenRouter wire payload", async () => {
		const model = getModel("openrouter", "qwen/qwen3-14b");
		let payload: Record<string, unknown> | undefined;
		await streamSimpleOpenAICompletions(
			model,
			{ messages: [{ role: "user", content: "hello", timestamp: 0 }] },
			{
				apiKey: "test",
				onPayload: (value) => {
					payload = value as Record<string, unknown>;
					throw new Error("halt-before-network");
				},
			},
		).result();
		expect(payload).toBeDefined();
		expect(payload).not.toHaveProperty("max_tokens");
		expect(payload).not.toHaveProperty("max_completion_tokens");
	});
	it("preserves ordinary provider defaults", () => {
		const model = getModel("openai", "gpt-5.4");
		expect(buildBaseOptions(model).maxTokens).toBe(model.maxTokens);
	});
});

describe("GitHub Copilot Completions request contracts", () => {
	it.each(["xhigh", "max"] as const)("preserves Fable %s at final HTTP serialization", async (reasoning) => {
		const requests = stubFetch([completedChatResponse()]);
		await streamSimpleOpenAICompletions(fableModel, context, { apiKey, reasoning }).result();
		expect((await requestBody(requests[0])).reasoning_effort).toBe(reasoning);
	});

	it.each([
		["medium", "high"],
		["max", "max"],
	] as const)("maps Kimi %s to %s", async (reasoning, expected) => {
		const requests = stubFetch([completedChatResponse()]);
		await streamSimpleOpenAICompletions(kimiModel, context, { apiKey, reasoning }).result();
		expect((await requestBody(requests[0])).reasoning_effort).toBe(expected);
	});

	it("streams a tool call, sends Copilot headers, and continues after its result", async () => {
		const requests = stubFetch([toolCallChatResponse(), completedChatResponse("continued")]);
		const first = await streamSimpleOpenAICompletions(kimiModel, toolContext, {
			apiKey,
			reasoning: "medium",
		}).result();
		expect(first.stopReason).toBe("toolUse");
		expect(first.content).toContainEqual({
			type: "toolCall",
			id: "call_read_1",
			name: "read",
			arguments: { path: "README.md" },
		});

		const second = await streamSimpleOpenAICompletions(kimiModel, continuationContext(first), {
			apiKey,
			reasoning: "max",
		}).result();
		expect(second.stopReason).toBe("stop");
		expect(second.content).toContainEqual(expect.objectContaining({ type: "text", text: "continued" }));

		const firstBody = await requestBody(requests[0]);
		const secondBody = await requestBody(requests[1]);
		expect(firstBody.reasoning_effort).toBe("high");
		expect(secondBody.reasoning_effort).toBe("max");
		expect(secondBody.messages).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ role: "assistant", tool_calls: expect.any(Array) }),
				expect.objectContaining({ role: "tool", tool_call_id: "call_read_1", content: "file contents" }),
			]),
		);
		expect(requests[0].url).toBe("https://api.individual.githubcopilot.com/chat/completions");
		expect(requests[0].headers.get("user-agent")).toBe("GitHubCopilotChat/0.35.0");
		expect(requests[0].headers.get("x-github-api-version")).toBe("2026-06-01");
		expect(requests[0].headers.get("openai-intent")).toBe("conversation-edits");
		expect(requests[0].headers.get("x-initiator")).toBe("user");
		expect(requests[1].headers.get("x-initiator")).toBe("agent");
	});
});

describe("clampReasoning", () => {
	it("clamps xhigh to high", () => {
		expect(clampReasoning("xhigh")).toBe("high");
	});

	it("clamps max to high", () => {
		expect(clampReasoning("max")).toBe("high");
	});

	it("passes through other levels unchanged", () => {
		expect(clampReasoning("low")).toBe("low");
		expect(clampReasoning("medium")).toBe("medium");
		expect(clampReasoning("high")).toBe("high");
	});

	it("passes through undefined", () => {
		expect(clampReasoning(undefined)).toBeUndefined();
	});
});
