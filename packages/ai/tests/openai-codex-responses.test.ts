import { Type } from "typebox";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getModel } from "../src/models.js";
import {
	streamOpenAICodexResponses,
	streamSimpleOpenAICodexResponses,
} from "../src/providers/openai-codex-responses.js";
import type { AssistantMessage, Context, Model, ModelThinkingLevel, ToolResultMessage } from "../src/types.js";
import { isContextOverflow } from "../src/utils/overflow.js";

const failureModel: Model<"openai-codex-responses"> = {
	id: "gpt-5.6-sol",
	name: "GPT-5.6 Sol",
	api: "openai-codex-responses",
	provider: "openai-codex",
	baseUrl: "https://example.com/backend-api",
	reasoning: true,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 1_050_000,
	maxTokens: 128_000,
};

const context: Context = {
	messages: [{ role: "user", content: "hello", timestamp: 0 }],
};

const apiKey = `x.${btoa(JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "test" } }))}.x`;

const efforts = ["low", "medium", "high", "xhigh", "max"] as const;
const astraModel = getModel("openai-codex", "gpt-6-astra");
const toolContext: Context = {
	messages: context.messages,
	tools: [
		{
			name: "read",
			description: "Read a file",
			parameters: Type.Object({ path: Type.String() }),
		},
	],
};

afterEach(() => {
	vi.unstubAllGlobals();
});

async function streamFailedResponse(error: { code?: string; message: string }) {
	const body = `data: ${JSON.stringify({ type: "response.failed", response: { error } })}\n\n`;
	vi.stubGlobal(
		"fetch",
		vi.fn(async () => new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } })),
	);
	return streamOpenAICodexResponses(failureModel, context, { apiKey, transport: "sse" }).result();
}

describe("openai-codex response failures", () => {
	it("does not classify generic invalid_request_body as overflow", async () => {
		const result = await streamFailedResponse({ code: "invalid_request_body", message: "Request failed" });
		expect(isContextOverflow(result, failureModel.contextWindow)).toBe(false);
	});
	it("does not add an unsupported output allocation field", async () => {
		let payload: Record<string, unknown> | undefined;
		await streamSimpleOpenAICodexResponses(failureModel, context, {
			apiKey,
			transport: "sse",
			maxTokens: 2048,
			onPayload: (value) => {
				payload = value as Record<string, unknown>;
				throw new Error("halt-before-network");
			},
		}).result();
		expect(payload).toBeDefined();
		expect(payload).not.toHaveProperty("max_output_tokens");
	});
	it("preserves the provider error code for overflow classification", async () => {
		const result = await streamFailedResponse({ code: "context_length_exceeded", message: "Request failed" });

		expect(result.stopReason).toBe("error");
		expect(result.errorMessage).toBe("Request failed (context_length_exceeded)");
		expect(isContextOverflow(result, failureModel.contextWindow)).toBe(true);
	});

	it("preserves the provider message when no code is present", async () => {
		const result = await streamFailedResponse({ message: "Request failed" });

		expect(result.errorMessage).toBe("Request failed");
		expect(isContextOverflow(result, failureModel.contextWindow)).toBe(false);
	});

	it("preserves non-overflow codes without misclassifying them", async () => {
		const result = await streamFailedResponse({ code: "permission_denied", message: "Request failed" });

		expect(result.errorMessage).toBe("Request failed (permission_denied)");
		expect(isContextOverflow(result, failureModel.contextWindow)).toBe(false);
	});
});

function sse(events: Record<string, unknown>[]): Response {
	return new Response(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(""), {
		status: 200,
		headers: { "content-type": "text/event-stream" },
	});
}

function completedResponse(text = "done"): Response {
	const item = {
		type: "message",
		id: "msg_1",
		role: "assistant",
		status: "completed",
		content: [{ type: "output_text", text, annotations: [] }],
	};
	return sse([
		{ type: "response.created", response: { id: "resp_1" } },
		{ type: "response.output_item.added", output_index: 0, item: { ...item, content: [] } },
		{
			type: "response.content_part.added",
			item_id: item.id,
			output_index: 0,
			content_index: 0,
			part: { type: "output_text", text: "", annotations: [] },
		},
		{ type: "response.output_text.delta", item_id: item.id, output_index: 0, content_index: 0, delta: text },
		{ type: "response.output_item.done", output_index: 0, item },
		{ type: "response.completed", response: { id: "resp_1", status: "completed" } },
	]);
}

function toolCallResponse(): Response {
	const item = {
		type: "function_call",
		id: "fc_read_1",
		call_id: "call_read_1",
		name: "read",
		arguments: '{"path":"README.md"}',
		status: "completed",
	};
	return sse([
		{ type: "response.created", response: { id: "resp_tool" } },
		{ type: "response.output_item.added", output_index: 0, item: { ...item, arguments: "" } },
		{
			type: "response.function_call_arguments.delta",
			item_id: item.id,
			output_index: 0,
			delta: item.arguments,
		},
		{
			type: "response.function_call_arguments.done",
			item_id: item.id,
			output_index: 0,
			arguments: item.arguments,
		},
		{ type: "response.output_item.done", output_index: 0, item },
		{ type: "response.completed", response: { id: "resp_tool", status: "completed" } },
	]);
}

function stubCodexFetch(responses: Response[]) {
	const requests: { url: string; headers: Headers; body: Record<string, unknown> }[] = [];
	const fetchMock = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
		const request = input instanceof Request ? input : new Request(input, init);
		requests.push({
			url: request.url,
			headers: new Headers(request.headers),
			body: (await request.json()) as Record<string, unknown>,
		});
		const response = responses.shift();
		if (!response) throw new Error("Unexpected request");
		return response;
	});
	vi.stubGlobal("fetch", fetchMock);
	return requests;
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

describe("GPT-6 Astra through OpenAI Codex Responses", () => {
	it.each(efforts)("preserves %s at final HTTP serialization", async (effort) => {
		const requests = stubCodexFetch([completedResponse()]);
		const result = await streamSimpleOpenAICodexResponses(astraModel, context, {
			apiKey,
			reasoning: effort,
			transport: "sse",
		}).result();

		expect(result.stopReason).toBe("stop");
		expect(requests[0].url).toBe("https://chatgpt.com/backend-api/codex/responses");
		expect(requests[0].body.model).toBe("gpt-6-astra");
		expect(requests[0].body.reasoning).toEqual(expect.objectContaining({ effort }));
	});

	it.each(["off", "minimal"] as const)("clamps inherited %s to low", async (reasoning) => {
		const requests = stubCodexFetch([completedResponse()]);
		await streamSimpleOpenAICodexResponses(astraModel, context, {
			apiKey,
			reasoning: reasoning as ModelThinkingLevel,
			transport: "sse",
		}).result();

		expect(requests[0].body.reasoning).toEqual(expect.objectContaining({ effort: "low" }));
	});

	it("does not synthesize reasoning when no effort is selected", async () => {
		const requests = stubCodexFetch([completedResponse()]);
		await streamSimpleOpenAICodexResponses(astraModel, context, { apiKey, transport: "sse" }).result();

		expect(requests[0].body).not.toHaveProperty("reasoning");
	});

	it("streams a tool call and continues after its result", async () => {
		const requests = stubCodexFetch([toolCallResponse(), completedResponse("continued")]);
		const first = await streamSimpleOpenAICodexResponses(astraModel, toolContext, {
			apiKey,
			reasoning: "low",
			transport: "sse",
		}).result();
		expect(first.stopReason).toBe("toolUse");
		expect(first.content).toContainEqual({
			type: "toolCall",
			id: "call_read_1|fc_read_1",
			name: "read",
			arguments: { path: "README.md" },
		});

		const second = await streamSimpleOpenAICodexResponses(astraModel, continuationContext(first), {
			apiKey,
			reasoning: "low",
			transport: "sse",
		}).result();
		expect(second.stopReason).toBe("stop");
		expect(second.content).toContainEqual(expect.objectContaining({ type: "text", text: "continued" }));
		expect(requests[1].body.input).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					type: "function_call",
					id: "fc_read_1",
					call_id: "call_read_1",
				}),
				expect.objectContaining({
					type: "function_call_output",
					call_id: "call_read_1",
					output: "file contents",
				}),
			]),
		);
	});
});
