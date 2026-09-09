import { Type } from "typebox";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getModel } from "../src/models.js";
import { streamSimpleOpenAIResponses } from "../src/providers/openai-responses.js";
import type { AssistantMessage, Context, Model, ModelThinkingLevel, ToolResultMessage } from "../src/types.js";

const efforts = ["low", "medium", "high", "xhigh", "max"] as const;
const openaiModel = getModel("openai", "gpt-6-astra");
const copilotModel = getModel("github-copilot", "gpt-6-astra");
const apiKey = "test-key";

const context: Context = {
	messages: [{ role: "user", content: "hello", timestamp: 0 }],
};

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
		{
			type: "response.completed",
			response: {
				id: "resp_1",
				status: "completed",
				usage: {
					input_tokens: 1,
					output_tokens: 1,
					total_tokens: 2,
					input_tokens_details: { cached_tokens: 0 },
				},
			},
		},
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

function stubFetch(responses: Response[]) {
	const requests: Request[] = [];
	const fetchMock = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
		const request = input instanceof Request ? input : new Request(input, init);
		requests.push(request.clone());
		const response = responses.shift();
		if (!response) throw new Error("Unexpected request");
		return response;
	});
	vi.stubGlobal("fetch", fetchMock);
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

async function assertToolContinuation(model: Model<"openai-responses">) {
	const requests = stubFetch([toolCallResponse(), completedResponse("continued")]);
	const first = await streamSimpleOpenAIResponses(model, toolContext, { apiKey, reasoning: "low" }).result();
	expect(first.stopReason).toBe("toolUse");
	expect(first.content).toContainEqual({
		type: "toolCall",
		id: "call_read_1|fc_read_1",
		name: "read",
		arguments: { path: "README.md" },
	});

	const second = await streamSimpleOpenAIResponses(model, continuationContext(first), {
		apiKey,
		reasoning: "low",
	}).result();
	expect(second.stopReason).toBe("stop");
	expect(second.content).toContainEqual(expect.objectContaining({ type: "text", text: "continued" }));

	const continuation = await requestBody(requests[1]);
	expect(continuation.input).toEqual(
		expect.arrayContaining([
			expect.objectContaining({
				type: "function_call",
				id: "fc_read_1",
				call_id: "call_read_1",
				name: "read",
			}),
			expect.objectContaining({
				type: "function_call_output",
				call_id: "call_read_1",
				output: "file contents",
			}),
		]),
	);
	return requests;
}

describe.each([
	["OpenAI", openaiModel],
	["GitHub Copilot", copilotModel],
] as const)("GPT-6 Astra through %s Responses", (_name, model) => {
	it.each(efforts)("preserves %s at final HTTP serialization", async (effort) => {
		const requests = stubFetch([completedResponse()]);
		const result = await streamSimpleOpenAIResponses(model, context, { apiKey, reasoning: effort }).result();

		expect(result.stopReason).toBe("stop");
		const body = await requestBody(requests[0]);
		expect(body.model).toBe("gpt-6-astra");
		expect(body.reasoning).toEqual(expect.objectContaining({ effort }));
	});

	it.each(["off", "minimal"] as const)("clamps inherited %s to low", async (reasoning) => {
		const requests = stubFetch([completedResponse()]);
		await streamSimpleOpenAIResponses(model, context, {
			apiKey,
			reasoning: reasoning as ModelThinkingLevel,
		}).result();

		const body = await requestBody(requests[0]);
		expect(body.reasoning).toEqual(expect.objectContaining({ effort: "low" }));
	});

	it("does not synthesize reasoning when no effort is selected", async () => {
		const requests = stubFetch([completedResponse()]);
		await streamSimpleOpenAIResponses(model, context, { apiKey }).result();

		const body = await requestBody(requests[0]);
		expect(body).not.toHaveProperty("reasoning");
	});

	it("streams a tool call and continues after its result", async () => {
		await assertToolContinuation(model);
	});
});

describe("GitHub Copilot Responses request contract", () => {
	it("sends static and dynamic headers, including vision intent", async () => {
		const requests = stubFetch([completedResponse()]);
		const imageContext: Context = {
			messages: [
				{
					role: "user",
					content: [
						{ type: "text", text: "inspect" },
						{ type: "image", data: "aW1hZ2U=", mimeType: "image/png" },
					],
					timestamp: 0,
				},
			],
		};
		await streamSimpleOpenAIResponses(copilotModel, imageContext, { apiKey, reasoning: "low" }).result();

		const request = requests[0];
		expect(request.url).toBe("https://api.individual.githubcopilot.com/responses");
		expect(request.headers.get("user-agent")).toBe("GitHubCopilotChat/0.35.0");
		expect(request.headers.get("editor-version")).toBe("vscode/1.107.0");
		expect(request.headers.get("editor-plugin-version")).toBe("copilot-chat/0.35.0");
		expect(request.headers.get("copilot-integration-id")).toBe("vscode-chat");
		expect(request.headers.get("openai-intent")).toBe("conversation-edits");
		expect(request.headers.get("x-initiator")).toBe("user");
		expect(request.headers.get("copilot-vision-request")).toBe("true");
	});

	it("serializes compound Responses IDs when history enters Astra from another provider", async () => {
		const requests = stubFetch([completedResponse()]);
		const foreignAssistant: AssistantMessage = {
			role: "assistant",
			content: [
				{
					type: "toolCall",
					id: "foreign-call|foreign-item",
					name: "read",
					arguments: { path: "README.md" },
				},
			],
			api: "openai-responses",
			provider: "openai",
			model: "gpt-6-astra",
			usage: {
				input: 1,
				output: 1,
				cacheRead: 0,
				cacheWrite: 0,
				totalTokens: 2,
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
			},
			stopReason: "toolUse",
			timestamp: 1,
		};
		const foreignResult: ToolResultMessage = {
			role: "toolResult",
			toolCallId: "foreign-call|foreign-item",
			toolName: "read",
			content: [{ type: "text", text: "file contents" }],
			isError: false,
			timestamp: 2,
		};

		await streamSimpleOpenAIResponses(
			copilotModel,
			{ messages: [context.messages[0], foreignAssistant, foreignResult] },
			{ apiKey, reasoning: "low" },
		).result();

		const body = await requestBody(requests[0]);
		const input = body.input as Record<string, unknown>[];
		const call = input.find((item) => item.type === "function_call");
		const output = input.find((item) => item.type === "function_call_output");
		expect(call).toEqual(
			expect.objectContaining({
				id: expect.stringMatching(/^fc_/),
				call_id: "foreign-call",
			}),
		);
		expect(output).toEqual(expect.objectContaining({ call_id: "foreign-call", output: "file contents" }));
		expect(requests[0].headers.get("x-initiator")).toBe("agent");
	});
});
