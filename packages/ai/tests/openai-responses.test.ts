import { Type } from "typebox";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getModel } from "../src/models.js";
import { streamAzureOpenAIResponses } from "../src/providers/azure-openai-responses.js";
import {
	streamOpenAIResponses,
	streamSimpleOpenAIResponses,
	validateResponsesProviderFailure,
} from "../src/providers/openai-responses.js";
import {
	createResponsesSdkRequestObserver,
	normalizeResponsesFailure,
} from "../src/providers/openai-responses-shared.js";
import type { AssistantMessage, Context, Model, ModelThinkingLevel, ToolResultMessage } from "../src/types.js";

const efforts = ["low", "medium", "high", "xhigh", "max"] as const;
const openaiModel = getModel("openai", "gpt-6-astra");
const copilotModel = getModel("github-copilot", "gpt-6-astra");
const azureModel = getModel("azure-openai-responses", "gpt-4");
const copilotHeaders = copilotModel.headers;

function copilotResponsesModel(
	id: string,
	thinkingLevelMap: Model<"openai-responses">["thinkingLevelMap"],
): Model<"openai-responses"> {
	return {
		...copilotModel,
		id,
		name: id,
		thinkingLevelMap,
	};
}

const grok46Model = copilotResponsesModel("grok-4.6", {
	off: null,
	minimal: null,
	xhigh: "xhigh",
	max: null,
});
const maiModel = copilotResponsesModel("mai-code-1.1-flash", {
	off: null,
	minimal: null,
	xhigh: null,
	max: null,
});
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

function jsonError(status: number, error?: Record<string, unknown>): Response {
	return new Response(error ? JSON.stringify({ error }) : undefined, {
		status,
		headers: { "content-type": "application/json", "retry-after-ms": "0" },
	});
}

function stubFetch(outcomes: Array<Response | { throws: unknown }>) {
	const requests: Request[] = [];
	const fetchMock = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
		const request = input instanceof Request ? input : new Request(input, init);
		requests.push(request.clone());
		const outcome = outcomes.shift();
		if (!outcome) throw new Error("Unexpected request");
		if ("throws" in outcome) throw outcome.throws;
		return outcome;
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

describe("OpenAI Responses failure normalization", () => {
	async function failureFrom(response: Response, model = openaiModel) {
		stubFetch([response]);
		return streamSimpleOpenAIResponses(model, context, { apiKey, maxRetries: 0 }).result();
	}

	function providerDetails(message: AssistantMessage) {
		return message.diagnostics?.find((diagnostic) => diagnostic.type === "provider_failure")?.details;
	}

	function sdkRetryDetails(message: AssistantMessage) {
		return message.diagnostics?.find((diagnostic) => diagnostic.type === "sdk_request_retry")?.details;
	}

	it("records SDK recovery from an HTTP rate limit", async () => {
		const requests = stubFetch([jsonError(429), completedResponse()]);
		const result = await streamSimpleOpenAIResponses(openaiModel, context, { apiKey, maxRetries: 1 }).result();

		expect(result.stopReason).toBe("stop");
		expect(requests).toHaveLength(2);
		expect(sdkRetryDetails(result)).toEqual({
			schemaVersion: 1,
			layer: "openai_sdk_request",
			outcome: "recovered",
			reason: "accepted_after_retry",
			observedAttemptCount: 2,
			attempts: [
				{ ordinal: 0, result: "response", status: 429 },
				{ ordinal: 1, result: "response", status: 200 },
			],
			truncated: false,
		});
	});

	it("records SDK recovery from a transport failure", async () => {
		const requests = stubFetch([{ throws: new TypeError("private transport detail") }, completedResponse()]);
		const result = await streamSimpleOpenAIResponses(openaiModel, context, { apiKey, maxRetries: 1 }).result();

		expect(result.stopReason).toBe("stop");
		expect(requests).toHaveLength(2);
		expect(sdkRetryDetails(result)).toEqual({
			schemaVersion: 1,
			layer: "openai_sdk_request",
			outcome: "recovered",
			reason: "accepted_after_retry",
			observedAttemptCount: 2,
			attempts: [
				{ ordinal: 0, result: "transport", category: "connection" },
				{ ordinal: 1, result: "response", status: 200 },
			],
			truncated: false,
		});
		expect(JSON.stringify(result)).not.toContain("private transport detail");
	});

	it("records exhausted and non-retryable terminal request outcomes", async () => {
		stubFetch([jsonError(429), jsonError(429)]);
		const exhausted = await streamSimpleOpenAIResponses(openaiModel, context, { apiKey, maxRetries: 1 }).result();
		expect(sdkRetryDetails(exhausted)).toEqual({
			schemaVersion: 1,
			layer: "openai_sdk_request",
			outcome: "exhausted",
			reason: "configured_limit_reached",
			observedAttemptCount: 2,
			attempts: [
				{ ordinal: 0, result: "response", status: 429 },
				{ ordinal: 1, result: "response", status: 429 },
			],
			truncated: false,
		});

		stubFetch([jsonError(429), jsonError(429), jsonError(429)]);
		const defaultExhausted = await streamSimpleOpenAIResponses(openaiModel, context, { apiKey }).result();
		expect(sdkRetryDetails(defaultExhausted)).toEqual(
			expect.objectContaining({
				outcome: "exhausted",
				reason: "configured_limit_reached",
				observedAttemptCount: 3,
			}),
		);

		stubFetch([jsonError(400)]);
		const nonRetryable = await streamSimpleOpenAIResponses(openaiModel, context, {
			apiKey,
			maxRetries: 2,
		}).result();
		expect(sdkRetryDetails(nonRetryable)).toEqual(
			expect.objectContaining({
				outcome: "not_attempted",
				reason: "non_retryable_request",
				observedAttemptCount: 1,
			}),
		);

		stubFetch([jsonError(429), jsonError(400)]);
		const terminalAfterRetry = await streamSimpleOpenAIResponses(openaiModel, context, {
			apiKey,
			maxRetries: 2,
		}).result();
		expect(sdkRetryDetails(terminalAfterRetry)).toEqual({
			schemaVersion: 1,
			layer: "openai_sdk_request",
			outcome: "exhausted",
			reason: "terminal_after_retry",
			observedAttemptCount: 2,
			attempts: [
				{ ordinal: 0, result: "response", status: 429 },
				{ ordinal: 1, result: "response", status: 400 },
			],
			truncated: false,
		});
	});

	it("distinguishes accepted-stream failures from request retries", async () => {
		const streamFailure = await failureFrom(sse([{ type: "error", code: "server_error" }]));
		expect(sdkRetryDetails(streamFailure)).toEqual(
			expect.objectContaining({
				outcome: "not_attempted",
				reason: "stream_already_accepted",
				observedAttemptCount: 1,
			}),
		);

		stubFetch([jsonError(429), sse([{ type: "error", code: "server_error" }])]);
		const recoveredThenFailed = await streamSimpleOpenAIResponses(openaiModel, context, {
			apiKey,
			maxRetries: 1,
		}).result();
		expect(sdkRetryDetails(recoveredThenFailed)).toEqual(
			expect.objectContaining({
				outcome: "recovered",
				reason: "accepted_after_retry",
				observedAttemptCount: 2,
			}),
		);
		expect(providerDetails(recoveredThenFailed)).toEqual(expect.objectContaining({ phase: "stream" }));
	});

	it("keeps observer records bounded and degrades invalid ordinals to call order", async () => {
		const inputs: Array<string | URL | Request> = [];
		const inits: Array<RequestInit | undefined> = [];
		const responses = Array.from({ length: 9 }, (_, index) => new Response(null, { status: 500 + (index % 2) }));
		const observer = createResponsesSdkRequestObserver(async (input, init) => {
			inputs.push(input);
			inits.push(init);
			return responses.shift()!;
		});

		for (let index = 0; index < 9; index++) {
			await observer.fetch("https://example.test", {
				headers: { "x-stainless-retry-count": index === 0 ? "invalid" : String(index) },
			});
		}
		const diagnostic = observer.diagnosticForFailure(8);
		expect(diagnostic.observedAttemptCount).toBe(9);
		expect(diagnostic.attempts.map((attempt) => attempt.ordinal)).toEqual([0, 1, 2, 3, 5, 6, 7, 8]);
		expect(diagnostic.truncated).toBe(true);
		expect(inputs).toHaveLength(9);
		expect(inits).toHaveLength(9);
	});

	it("returns the exact response, rethrows the exact transport value, and does not consume bodies", async () => {
		const response = new Response("unconsumed");
		const responseInput = new Request("https://example.test/response");
		const responseInit = { headers: { "x-stainless-retry-count": "0" } };
		const responseFetch = vi.fn(async () => response);
		const responseObserver = createResponsesSdkRequestObserver(responseFetch);
		expect(await responseObserver.fetch(responseInput, responseInit)).toBe(response);
		expect(responseFetch).toHaveBeenCalledWith(responseInput, responseInit);
		expect(response.bodyUsed).toBe(false);

		const thrown = { sensitive: "not retained" };
		const throwingFetch = vi.fn(async () => {
			throw thrown;
		});
		const throwingObserver = createResponsesSdkRequestObserver(throwingFetch);
		await expect(throwingObserver.fetch("https://example.test/error")).rejects.toBe(thrown);
		expect(JSON.stringify(throwingObserver.diagnosticForFailure(0))).not.toContain("not retained");
	});

	it("records recovery for custom and Azure shared routes", async () => {
		const customModel = { ...openaiModel, provider: "custom-responses", baseUrl: "https://custom.example/v1" };
		stubFetch([jsonError(429), completedResponse()]);
		const custom = await streamSimpleOpenAIResponses(customModel, context, { apiKey, maxRetries: 1 }).result();
		expect(sdkRetryDetails(custom)).toEqual(expect.objectContaining({ outcome: "recovered" }));

		stubFetch([jsonError(429), completedResponse()]);
		const azure = await streamAzureOpenAIResponses(azureModel, context, {
			apiKey,
			azureBaseUrl: "https://example.openai.azure.com/openai/v1",
			maxRetries: 1,
		}).result();
		expect(sdkRetryDetails(azure)).toEqual(expect.objectContaining({ outcome: "recovered" }));
	});

	it("normalizes a bare provider error without undefined placeholders", async () => {
		const result = await failureFrom(sse([{ type: "error" }]));

		expect(result.stopReason).toBe("error");
		expect(result.errorMessage).toBe("OpenAI Responses returned a malformed error event.");
		expect(result.errorMessage).not.toContain("undefined");
		expect(result.diagnostics).toEqual([
			{
				type: "provider_failure",
				timestamp: expect.any(Number),
				details: {
					schemaVersion: 1,
					layer: "openai_responses",
					phase: "stream",
					kind: "malformed_event",
					category: "malformed_event",
					retryDisposition: "unknown",
					detailSource: "none",
				},
			},
			{
				type: "sdk_request_retry",
				timestamp: expect.any(Number),
				details: {
					schemaVersion: 1,
					layer: "openai_sdk_request",
					outcome: "not_attempted",
					reason: "stream_already_accepted",
					observedAttemptCount: 1,
					attempts: [{ ordinal: 0, result: "response", status: 200 }],
					truncated: false,
				},
			},
			{
				type: "gateway_observability",
				timestamp: expect.any(Number),
				details: {
					schemaVersion: 1,
					layer: "gateway_service_internal",
					outcome: "unobservable",
					reason: "no_structured_evidence",
				},
			},
		]);
	});

	it.each([
		[
			"top-level code",
			{ type: "error", code: "rate_limit_exceeded", message: "sensitive provider prose" },
			"rate_limit",
			"transient",
			"provider_code",
			"OpenAI Responses request was rate limited.",
		],
		[
			"message-only error",
			{ type: "error", message: "request timeout at /sensitive/path" },
			"timeout",
			"transient",
			"message_category",
			"OpenAI Responses request timed out.",
		],
		[
			"nested SDK-intercepted error",
			{ type: "error", error: { code: "authentication_error", message: "secret-key-value" } },
			"authentication",
			"non_transient",
			"provider_code",
			"OpenAI Responses authentication failed.",
		],
	] as const)("normalizes a %s", async (_name, event, category, disposition, source, message) => {
		const result = await failureFrom(sse([event]));

		expect(result.errorMessage).toBe(message);
		expect(providerDetails(result)).toEqual(
			expect.objectContaining({
				schemaVersion: 1,
				phase: "stream",
				kind: "provider_event",
				category,
				retryDisposition: disposition,
				detailSource: source,
			}),
		);
	});

	it.each([
		[
			"complete response.failed",
			{ type: "response.failed", response: { error: { code: "content_filter", message: "private prompt" } } },
			"content_rejection",
			"provider_code",
		],
		[
			"partial response.failed",
			{ type: "response.failed", response: { incomplete_details: { reason: "server error in tenant secret" } } },
			"server",
			"message_category",
		],
		["empty response.failed", { type: "response.failed", response: {} }, "malformed_event", "none"],
	] as const)("normalizes a %s", async (_name, event, category, source) => {
		const result = await failureFrom(sse([event]));
		expect(providerDetails(result)).toEqual(expect.objectContaining({ category, detailSource: source }));
	});

	it("gives context overflow precedence over conflicting transient evidence", async () => {
		const result = await failureFrom(
			sse([{ type: "error", code: "rate_limit_exceeded", message: "maximum context length exceeded" }]),
		);

		expect(result.errorMessage).toBe("OpenAI Responses context length was exceeded.");
		expect(providerDetails(result)).toEqual(
			expect.objectContaining({ category: "context_overflow", retryDisposition: "non_transient" }),
		);
	});

	it("gives supported top-level evidence precedence over conflicting nested evidence", () => {
		const result = normalizeResponsesFailure(
			{
				code: "rate_limit_exceeded",
				error: { code: "authentication_error", message: "private nested provider prose" },
			},
			"stream",
		);

		expect(result.message).toBe("OpenAI Responses request was rate limited.");
		expect(result.diagnostic).toEqual(
			expect.objectContaining({
				category: "rate_limit",
				retryDisposition: "transient",
				detailSource: "provider_code",
				providerCode: "rate_limit_exceeded",
			}),
		);
		expect(JSON.stringify(result)).not.toContain("private nested provider prose");
	});

	it.each([
		[jsonError(429), "rate_limit", "http_status"],
		[
			jsonError(400, { code: "invalid_request_error", message: "private request body" }),
			"invalid_request",
			"provider_code",
		],
	] as const)(
		"normalizes request failures without a body or with SDK API fields",
		async (response, category, source) => {
			const result = await failureFrom(response);
			expect(providerDetails(result)).toEqual(
				expect.objectContaining({ phase: "request", kind: "http", category, detailSource: source }),
			);
		},
	);

	it.each([
		["OpenAI", streamOpenAIResponses, openaiModel, {}],
		["Azure", streamAzureOpenAIResponses, azureModel, { azureBaseUrl: "https://example.openai.azure.com/openai/v1" }],
	] as const)(
		"keeps %s payload callback failures out of provider diagnostics",
		async (_name, streamFn, model, extra) => {
			const result = await streamFn(model as never, context, {
				apiKey,
				...extra,
				onPayload: async () => {
					throw new TypeError("private callback rate limit sentinel");
				},
			} as never).result();

			expect(result.stopReason).toBe("error");
			expect(result.errorMessage).toBe("OpenAI Responses payload callback failed.");
			expect(result.diagnostics).toBeUndefined();
			expect(JSON.stringify(result)).not.toContain("private callback rate limit sentinel");
		},
	);

	it.each([
		["OpenAI", streamOpenAIResponses, openaiModel, {}],
		["Azure", streamAzureOpenAIResponses, azureModel, { azureBaseUrl: "https://example.openai.azure.com/openai/v1" }],
	] as const)(
		"keeps %s payload callback failures private when the request is also aborted",
		async (_name, streamFn, model, extra) => {
			const controller = new AbortController();
			controller.abort();
			const result = await streamFn(model as never, context, {
				apiKey,
				...extra,
				signal: controller.signal,
				onPayload: async () => {
					throw new TypeError("private aborted callback sentinel");
				},
			} as never).result();

			expect(result.stopReason).toBe("aborted");
			expect(result.errorMessage).toBe("OpenAI Responses payload callback failed.");
			expect(result.diagnostics).toBeUndefined();
			expect(JSON.stringify(result)).not.toContain("private aborted callback sentinel");
		},
	);

	it("normalizes request transport failures without retaining the thrown message", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => Promise.reject(new TypeError("private host and request details"))),
		);

		const result = await streamSimpleOpenAIResponses(openaiModel, context, { apiKey, maxRetries: 0 }).result();

		expect(result.errorMessage).toBe("OpenAI Responses request failed during transport.");
		expect(providerDetails(result)).toEqual(
			expect.objectContaining({ phase: "request", kind: "transport", category: "transport" }),
		);
		expect(sdkRetryDetails(result)).toEqual(
			expect.objectContaining({
				outcome: "not_attempted",
				reason: "configured_zero",
				observedAttemptCount: 1,
				attempts: [{ ordinal: 0, result: "transport", category: "connection" }],
			}),
		);
		expect(JSON.stringify(result)).not.toContain("private host");
	});

	it.each([
		["OpenAI", streamOpenAIResponses, openaiModel, {}],
		["Azure", streamAzureOpenAIResponses, azureModel, { azureBaseUrl: "https://example.openai.azure.com/openai/v1" }],
	] as const)(
		"keeps %s response callback failures out of provider diagnostics",
		async (_name, streamFn, model, extra) => {
			const response = completedResponse();
			stubFetch([response]);
			const result = await streamFn(model as never, context, {
				apiKey,
				...extra,
				onResponse: () => {
					throw new Error("private callback rate limit sentinel");
				},
			} as never).result();

			expect(result.stopReason).toBe("error");
			expect(result.errorMessage).toBe("OpenAI Responses response callback failed.");
			expect(result.diagnostics).toBeUndefined();
			expect(response.bodyUsed).toBe(false);
			expect(JSON.stringify(result)).not.toContain("private callback rate limit sentinel");
		},
	);

	it.each([
		["OpenAI", streamOpenAIResponses, openaiModel, {}],
		["Azure", streamAzureOpenAIResponses, azureModel, { azureBaseUrl: "https://example.openai.azure.com/openai/v1" }],
	] as const)(
		"keeps %s response callback failures private when the callback also aborts",
		async (_name, streamFn, model, extra) => {
			const controller = new AbortController();
			const response = completedResponse();
			stubFetch([response]);
			const result = await streamFn(model as never, context, {
				apiKey,
				...extra,
				signal: controller.signal,
				onResponse: () => {
					controller.abort();
					throw new Error("private aborted response callback sentinel");
				},
			} as never).result();

			expect(result.stopReason).toBe("aborted");
			expect(result.errorMessage).toBe("OpenAI Responses response callback failed.");
			expect(result.diagnostics).toBeUndefined();
			expect(response.bodyUsed).toBe(false);
			expect(JSON.stringify(result)).not.toContain("private aborted response callback sentinel");
		},
	);

	it("does not add failure diagnostics to a successful stream", async () => {
		stubFetch([completedResponse()]);
		const result = await streamSimpleOpenAIResponses(openaiModel, context, { apiKey }).result();
		expect(result.diagnostics).toBeUndefined();
	});

	it("does not retain unrestricted provider data", async () => {
		const sentinels = ["secret-header", "https://private.example/path", "resp_private", "/home/private/file"];
		const result = await failureFrom(
			sse([
				{ type: "error", code: "unsupported_private_code", message: sentinels.join(" "), stack: "secret-stack" },
			]),
		);
		const serialized = JSON.stringify(result);

		for (const sentinel of sentinels) expect(serialized).not.toContain(sentinel);
		expect(serialized).not.toContain("secret-stack");
		expect(Object.keys(providerDetails(result) ?? {}).sort()).toEqual(
			["category", "detailSource", "kind", "layer", "phase", "retryDisposition", "schemaVersion"].sort(),
		);
	});

	it("normalizes custom shared-route failures", async () => {
		const customModel = { ...openaiModel, provider: "custom-responses", baseUrl: "https://custom.example/v1" };
		const result = await failureFrom(sse([{ type: "error", code: "server_error" }]), customModel);
		expect(providerDetails(result)).toEqual(expect.objectContaining({ category: "server" }));
	});

	it("normalizes representative Azure failures", async () => {
		stubFetch([sse([{ type: "error", code: "permission_denied" }])]);
		const result = await streamAzureOpenAIResponses(azureModel, context, {
			apiKey,
			azureBaseUrl: "https://example.openai.azure.com/openai/v1",
			maxRetries: 0,
		}).result();
		expect(providerDetails(result)).toEqual(
			expect.objectContaining({ category: "permission", retryDisposition: "non_transient" }),
		);
	});

	it("accepts canonical provider failure builder outputs", () => {
		const failures = [
			normalizeResponsesFailure({ status: 429 }, "request"),
			normalizeResponsesFailure({ status: 418 }, "request"),
			normalizeResponsesFailure({ status: 418, message: "rate limit" }, "request"),
			normalizeResponsesFailure(new TypeError("private transport failure"), "request"),
			normalizeResponsesFailure({ code: "server_error" }, "stream"),
			normalizeResponsesFailure({ message: "rate limit" }, "stream"),
			normalizeResponsesFailure({ status: 400, message: "maximum context length exceeded" }, "request"),
			normalizeResponsesFailure({}, "stream"),
		];

		for (const failure of failures) {
			expect(
				validateResponsesProviderFailure([{ type: "provider_failure", timestamp: 0, details: failure.diagnostic }]),
			).toEqual({
				status: "valid",
				category: failure.diagnostic.category,
				retryDisposition: failure.diagnostic.retryDisposition,
			});
		}
	});

	it("rejects noncanonical provider failure tuples", () => {
		const base = normalizeResponsesFailure({ code: "server_error" }, "stream").diagnostic;
		const malformed = [
			{ ...base, kind: "transport", category: "transport", detailSource: "none" },
			{
				...base,
				phase: "request",
				kind: "transport",
				category: "transport",
				detailSource: "none",
				httpStatus: 503,
			},
			{ ...base, kind: "http", detailSource: "http_status", providerCode: undefined },
			{
				...base,
				kind: "http",
				category: "server",
				detailSource: "http_status",
				httpStatus: 503,
				providerCode: "server_error",
			},
			{
				...base,
				kind: "http",
				category: "rate_limit",
				retryDisposition: "transient",
				detailSource: "message_category",
				httpStatus: 400,
				providerCode: undefined,
			},
			{ ...base, phase: "request", kind: "provider_event", httpStatus: 503 },
			{
				...base,
				category: "rate_limit",
				detailSource: "message_category",
				providerCode: "rate_limit_exceeded",
			},
			{
				...base,
				kind: "malformed_event",
				category: "malformed_event",
				retryDisposition: "unknown",
				detailSource: "none",
				providerCode: "server_error",
			},
			{ ...base, category: "server", detailSource: "none", providerCode: undefined },
		];

		for (const details of malformed) {
			expect(validateResponsesProviderFailure([{ type: "provider_failure", timestamp: 0, details }])).toEqual({
				status: "malformed",
			});
		}
	});

	it("validates exactly one closed provider failure diagnostic", async () => {
		const result = await failureFrom(sse([{ type: "error", code: "server_error" }]));
		expect(validateResponsesProviderFailure(result.diagnostics)).toEqual({
			status: "valid",
			category: "server",
			retryDisposition: "transient",
		});
		expect(validateResponsesProviderFailure(undefined)).toEqual({ status: "absent" });
		expect(validateResponsesProviderFailure({ type: "provider_failure" })).toEqual({ status: "malformed" });
		for (const diagnostic of [null, "provider_failure", 1, true, {}]) {
			expect(validateResponsesProviderFailure([diagnostic])).toEqual({ status: "absent" });
		}
		expect(validateResponsesProviderFailure([{ type: "usage", timestamp: 0, details: { tokens: 1 } }])).toEqual({
			status: "absent",
		});
		expect(
			validateResponsesProviderFailure([
				{ type: "provider_failure", timestamp: 0, details: { ...providerDetails(result), extra: true } },
			]),
		).toEqual({ status: "malformed" });
		expect(
			validateResponsesProviderFailure([
				{
					type: "provider_failure",
					timestamp: 0,
					details: { ...providerDetails(result), kind: { toString: () => "provider_event" } },
				},
			]),
		).toEqual({ status: "malformed" });
		expect(
			validateResponsesProviderFailure([
				{
					type: "provider_failure",
					timestamp: 0,
					details: {
						...providerDetails(result),
						providerCode: "rate_limit_exceeded",
						category: "server",
						detailSource: "provider_code",
					},
				},
			]),
		).toEqual({ status: "malformed" });
		expect(validateResponsesProviderFailure([result.diagnostics![0], result.diagnostics![0]])).toEqual({
			status: "duplicate",
		});
	});
});

describe("GitHub Copilot exact Responses model contracts", () => {
	it.each([
		[grok46Model, "xhigh", "xhigh"],
		[grok46Model, "minimal", "low"],
		[maiModel, "high", "high"],
		[maiModel, "xhigh", "high"],
	] as const)("maps $0.id $1 to $2", async (model, reasoning, expected) => {
		const requests = stubFetch([completedResponse()]);
		await streamSimpleOpenAIResponses(model, context, {
			apiKey,
			reasoning: reasoning as ModelThinkingLevel,
		}).result();
		expect((await requestBody(requests[0])).reasoning).toEqual(expect.objectContaining({ effort: expected }));
	});

	it("streams a MAI tool call and continues with Copilot headers", async () => {
		const requests = await assertToolContinuation(maiModel);
		expect(requests[0].url).toBe("https://api.individual.githubcopilot.com/responses");
		for (const [name, value] of Object.entries(copilotHeaders ?? {})) {
			expect(requests[0].headers.get(name)).toBe(value);
		}
		expect(requests[0].headers.get("openai-intent")).toBe("conversation-edits");
		expect(requests[0].headers.get("x-initiator")).toBe("user");
		expect(requests[1].headers.get("x-initiator")).toBe("agent");
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
