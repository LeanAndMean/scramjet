import { BedrockRuntimeClient } from "@aws-sdk/client-bedrock-runtime";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getApiProvider } from "../src/api-registry.js";
import { streamBedrock, streamSimpleBedrock } from "../src/providers/amazon-bedrock.js";
import { streamAnthropic } from "../src/providers/anthropic.js";
import { streamGoogle } from "../src/providers/google.js";
import { streamGoogleVertex } from "../src/providers/google-vertex.js";
import { streamMistral } from "../src/providers/mistral.js";
import { streamOpenAICompletions } from "../src/providers/openai-completions.js";
import { streamOpenAIResponses } from "../src/providers/openai-responses.js";
import { setBedrockProviderModule } from "../src/providers/register-builtins.js";
import type { Api, Model } from "../src/types.js";
import { AssistantMessageEventStream } from "../src/utils/event-stream.js";
import { inspectFailureEvidence } from "../src/utils/failure-evidence.js";
import { isContextOverflow } from "../src/utils/overflow.js";

const context = { messages: [{ role: "user" as const, content: "hello", timestamp: 0 }] };
function model<T extends Api>(api: T, provider: string): Model<T> {
	return {
		id: "test-model",
		name: "test",
		api,
		provider,
		baseUrl: "https://example.com",
		reasoning: false,
		input: ["text"],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 200000,
		maxTokens: 4096,
	};
}
afterEach(() => {
	vi.unstubAllGlobals();
	vi.restoreAllMocks();
	vi.unstubAllEnvs();
	setBedrockProviderModule({ streamBedrock, streamSimpleBedrock });
});
describe("adapter failure evidence", () => {
	it.each([
		[
			"Anthropic",
			() =>
				streamAnthropic(model("anthropic-messages", "anthropic"), context, {
					apiKey: "fake",
					maxRetries: 0,
				}).result(),
		],
		[
			"Chat",
			() =>
				streamOpenAICompletions(model("openai-completions", "openai"), context, {
					apiKey: "fake",
					maxRetries: 0,
				}).result(),
		],
		["Mistral", () => streamMistral(model("mistral-conversations", "mistral"), context, { apiKey: "fake" }).result()],
	] as const)("retains %s status-only 529 using the installed SDK", async (_name, run) => {
		const fetch = vi.fn(async () => new Response(null, { status: 529 }));
		vi.stubGlobal("fetch", fetch);
		const result = await run();
		expect(fetch).toHaveBeenCalledTimes(1);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			status: "valid",
			category: "server",
			transient: true,
		});
	});
	it("classifies accepted Anthropic body absence", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => new Response(null, { status: 200 })),
		);
		const result = await streamAnthropic(model("anthropic-messages", "anthropic"), context, {
			apiKey: "fake",
			maxRetries: 0,
		}).result();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ category: "missing_body" });
	});
	it.each([
		["Chat", streamOpenAICompletions, model("openai-completions", "openai")],
		["Mistral", streamMistral, model("mistral-conversations", "mistral")],
	] as const)("classifies accepted %s body absence", async (_name, run, selected) => {
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => new Response(null, { status: 200, headers: { "content-type": "text/event-stream" } })),
		);
		const result = await (run as typeof streamOpenAICompletions)(selected as Model<"openai-completions">, context, {
			apiKey: "fake",
			maxRetries: 0,
		}).result();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ status: "valid", category: "missing_body" });
	});
	it.each([
		["Google", () => streamGoogle(model("google-generative-ai", "google"), context, { apiKey: "fake" }).result()],
		[
			"Vertex",
			() => streamGoogleVertex(model("google-vertex", "google-vertex"), context, { apiKey: "fake" }).result(),
		],
	] as const)("keeps SDK-obscured %s body absence explicitly unknown", async (_name, run) => {
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => new Response(null, { status: 200, headers: { "content-type": "text/event-stream" } })),
		);
		expect(inspectFailureEvidence((await run()).diagnostics)).toMatchObject({
			status: "valid",
			category: "unknown",
			transient: false,
		});
	});
	it.each([
		[
			"Chat",
			() =>
				streamOpenAICompletions(model("openai-completions", "openai"), context, {
					apiKey: "fake",
					maxRetries: 0,
					onPayload: (payload) => ({ ...(payload as object), metadata: { value: 1n } }),
				}).result(),
		],
		[
			"Anthropic",
			() =>
				streamAnthropic(model("anthropic-messages", "anthropic"), context, {
					apiKey: "fake",
					maxRetries: 0,
					onPayload: (payload) => ({ ...(payload as object), metadata: { value: 1n } }),
				}).result(),
		],
	] as const)("keeps %s zero-fetch SDK serialization local", async (_name, run) => {
		const fetch = vi.fn();
		vi.stubGlobal("fetch", fetch);
		expect(inspectFailureEvidence((await run()).diagnostics)).toMatchObject({ source: "local", transient: false });
		expect(fetch).not.toHaveBeenCalled();
	});
	it.each(["Chat", "Mistral"])("latches %s failed finishes despite later tool completion", async (route) => {
		const chunks = [
			{
				id: "resp",
				model: "test-model",
				created: 1,
				object: "chat.completion.chunk",
				choices: [
					{
						index: 0,
						delta: {
							role: "assistant",
							tool_calls: [
								{ index: 0, id: "call1", type: "function", function: { name: "read", arguments: "{}" } },
							],
						},
						finish_reason: route === "Chat" ? "content_filter" : "error",
					},
				],
			},
			{
				id: "resp",
				model: "test-model",
				created: 1,
				object: "chat.completion.chunk",
				choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }],
			},
		];
		vi.stubGlobal(
			"fetch",
			vi.fn(
				async () =>
					new Response(`${chunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`).join("")}data: [DONE]\n\n`, {
						headers: { "content-type": "text/event-stream" },
					}),
			),
		);
		const result =
			route === "Chat"
				? await streamOpenAICompletions(model("openai-completions", "openai"), context, {
						apiKey: "fake",
						maxRetries: 0,
					}).result()
				: await streamMistral(model("mistral-conversations", "mistral"), context, { apiKey: "fake" }).result();
		expect(result.stopReason).toBe("error");
		expect(result.content).toContainEqual(expect.objectContaining({ type: "toolCall" }));
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ status: "valid", transient: false });
	});
	it("finalizes a rejected lazy iterator and retains partial content", async () => {
		const selected = model("bedrock-converse-stream", "amazon-bedrock");
		const partial = {
			role: "assistant" as const,
			content: [{ type: "text" as const, text: "partial" }],
			api: selected.api,
			model: selected.id,
			provider: selected.provider,
			stopReason: "stop" as const,
			timestamp: 0,
			usage: {
				input: 0,
				output: 0,
				cacheRead: 0,
				cacheWrite: 0,
				totalTokens: 0,
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
			},
		};
		const stream = async function* () {
			yield { type: "start" as const, partial };
			throw new Error("server error context_length_exceeded");
		};
		setBedrockProviderModule({ streamBedrock: stream, streamSimpleBedrock: stream });
		const result = await getApiProvider(selected.api)!.streamSimple(selected, context).result();
		expect(result.stopReason).toBe("error");
		expect(result.content).toEqual(partial.content);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ category: "unknown", transient: false });
	});
	it("forwards a result-only lazy stream without changing its result", async () => {
		const selected = model("bedrock-converse-stream", "amazon-bedrock");
		const result = {
			role: "assistant" as const,
			content: [],
			api: selected.api,
			model: selected.id,
			provider: selected.provider,
			stopReason: "stop" as const,
			timestamp: 0,
			usage: {
				input: 0,
				output: 0,
				cacheRead: 0,
				cacheWrite: 0,
				totalTokens: 0,
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
			},
		};
		const stream = () => {
			const events = new AssistantMessageEventStream();
			events.end(result);
			return events;
		};
		setBedrockProviderModule({ streamBedrock: stream, streamSimpleBedrock: stream });
		expect(await getApiProvider(selected.api)!.streamSimple(selected, context).result()).toEqual(result);
	});
	it("classifies Responses SDK serialization as local preparation with zero fetch calls", async () => {
		const fetch = vi.fn();
		vi.stubGlobal("fetch", fetch);
		const result = await streamOpenAIResponses(model("openai-responses", "openai"), context, {
			apiKey: "fake",
			maxRetries: 0,
			onPayload: (payload) => ({ ...(payload as object), metadata: { value: 1n } }),
		}).result();
		expect(fetch).not.toHaveBeenCalled();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			status: "valid",
			source: "local",
			category: "local",
			transient: false,
		});
	});
	it("classifies Mistral SDK request validation as local", async () => {
		const fetch = vi.fn();
		vi.stubGlobal("fetch", fetch);
		const result = await streamMistral(model("mistral-conversations", "mistral"), context, {
			apiKey: "fake",
			onPayload: () => ({ model: "test-model", messages: [{ role: "user", content: 1n }] }),
		}).result();
		expect(fetch).not.toHaveBeenCalled();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ source: "local", transient: false });
	});
	it.each(["server error context_length_exceeded", "{broken context_length_exceeded"])(
		"retains Anthropic error-event identity independently of prose: %s",
		async (data) => {
			vi.stubGlobal(
				"fetch",
				vi.fn(
					async () =>
						new Response(`event: error\ndata: ${data}\n\n`, { headers: { "content-type": "text/event-stream" } }),
				),
			);
			const result = await streamAnthropic(model("anthropic-messages", "anthropic"), context, {
				apiKey: "fake",
				maxRetries: 0,
			}).result();
			expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
				status: "valid",
				category: "malformed_event",
				transient: false,
			});
		},
	);
	it.each(["new_code", "insufficient_quota", "rate_limit_exceeded", "rate_limit_error", "timeout_error"])(
		"preserves Anthropic streamed rejection %s",
		async (code) => {
			vi.stubGlobal(
				"fetch",
				vi.fn(
					async () =>
						new Response(
							`event: error\ndata: ${JSON.stringify({ type: "error", error: { type: code, message: "server error context_length_exceeded" } })}\n\n`,
							{ headers: { "content-type": "text/event-stream" } },
						),
				),
			);
			const result = await streamAnthropic(model("anthropic-messages", "anthropic"), context, {
				apiKey: "fake",
				maxRetries: 0,
			}).result();
			expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
				status: "valid",
				category:
					code === "new_code"
						? "unknown"
						: code === "insufficient_quota"
							? "quota_exhausted"
							: code === "timeout_error"
								? "timeout"
								: "rate_limit",
				transient: !["new_code", "insufficient_quota"].includes(code),
			});
		},
	);
	it.each([
		["kimi-coding", "Your request exceeded model token limit: 262144 (requested: 300000)"],
		["minimax", "invalid params, context window exceeds limit"],
		["minimax-cn", "invalid params, context window exceeds limit"],
	] as const)("preserves %s overflow without losing rejection vetoes", async (provider, message) => {
		for (const [status, type, overflow] of [
			[400, "invalid_request_error", true],
			[400, "insufficient_quota", false],
			[401, "invalid_request_error", false],
			[403, "invalid_request_error", false],
			[429, "invalid_request_error", false],
		] as const) {
			vi.stubGlobal(
				"fetch",
				vi.fn(
					async () =>
						new Response(JSON.stringify({ error: { type, message } }), {
							status,
							headers: { "content-type": "application/json" },
						}),
				),
			);
			const result = await streamAnthropic(model("anthropic-messages", provider), context, {
				apiKey: "fake",
				maxRetries: 0,
			}).result();
			expect(inspectFailureEvidence(result.diagnostics).status).toBe("valid");
			expect(isContextOverflow(result)).toBe(overflow);
		}
	});
	it("does not authorize an Anthropic rate-limit/quota conflict", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(
				async () =>
					new Response(
						JSON.stringify({
							error: {
								code: "rate_limit_exceeded",
								type: "insufficient_quota",
								message: "server error",
							},
						}),
						{ status: 429, headers: { "content-type": "application/json" } },
					),
			),
		);
		const result = await streamAnthropic(model("anthropic-messages", "anthropic"), context, {
			apiKey: "fake",
			maxRetries: 0,
		}).result();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			status: "valid",
			category: "unknown",
			transient: false,
		});
	});
	it("finalizes Bedrock proxy preparation failure", async () => {
		vi.stubEnv("HTTPS_PROXY", "socks5://localhost:1080");
		vi.stubEnv("NO_PROXY", "");
		try {
			const result = await streamBedrock(model("bedrock-converse-stream", "amazon-bedrock"), context, {
				region: "us-east-1",
			}).result();
			expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ source: "local", transient: false });
		} finally {
			vi.unstubAllEnvs();
		}
	});
	it.each([
		["xai", "This model's maximum prompt length is 131072 but the request contains 537812 tokens"],
		["groq", "Please reduce the length of the messages or completion"],
		["together", "The input (250000 tokens) is longer than the model's context length (200000 tokens)."],
	] as const)("preserves applicable %s overflow at the producer", async (provider, message) => {
		vi.stubGlobal(
			"fetch",
			vi.fn(
				async () =>
					new Response(JSON.stringify({ error: { message } }), {
						status: 400,
						headers: { "content-type": "application/json" },
					}),
			),
		);
		const result = await streamOpenAICompletions(model("openai-completions", provider), context, {
			apiKey: "fake",
			maxRetries: 0,
		}).result();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ category: "context_overflow" });
		vi.stubGlobal(
			"fetch",
			vi.fn(
				async () =>
					new Response(JSON.stringify({ error: { code: "insufficient_quota", message } }), {
						status: 400,
						headers: { "content-type": "application/json" },
					}),
			),
		);
		expect(
			inspectFailureEvidence(
				(
					await streamOpenAICompletions(model("openai-completions", provider), context, {
						apiKey: "fake",
						maxRetries: 0,
					}).result()
				).diagnostics,
			),
		).toMatchObject({ category: "quota_exhausted" });
	});
	it.each(["zai", "openai"])("qualifies the established overflow finish for %s", async (provider) => {
		vi.stubGlobal(
			"fetch",
			vi.fn(
				async () =>
					new Response(
						`data: ${JSON.stringify({ id: "resp", choices: [{ index: 0, delta: {}, finish_reason: "model_context_window_exceeded" }] })}\n\n`,
						{ headers: { "content-type": "text/event-stream" } },
					),
			),
		);
		const result = await streamOpenAICompletions(model("openai-completions", provider), context, {
			apiKey: "fake",
			maxRetries: 0,
		}).result();
		expect(isContextOverflow(result)).toBe(provider === "zai");
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			category: provider === "zai" ? "context_overflow" : "unknown",
		});
	});
	it("preserves Anthropic request_too_large overflow", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(
				async () =>
					new Response(
						JSON.stringify({ error: { type: "request_too_large", message: "Request exceeds the maximum size" } }),
						{ status: 413, headers: { "content-type": "application/json" } },
					),
			),
		);
		expect(
			inspectFailureEvidence(
				(
					await streamAnthropic(model("anthropic-messages", "anthropic"), context, {
						apiKey: "fake",
						maxRetries: 0,
					}).result()
				).diagnostics,
			),
		).toMatchObject({ category: "context_overflow" });
	});
	it("preserves established Cerebras bodyless 413 overflow", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => new Response(null, { status: 413 })),
		);
		expect(
			inspectFailureEvidence(
				(
					await streamOpenAICompletions(model("openai-completions", "cerebras"), context, {
						apiKey: "fake",
						maxRetries: 0,
					}).result()
				).diagnostics,
			),
		).toMatchObject({ category: "context_overflow" });
	});
	it("preserves an anonymous Bedrock throttling event discriminator", async () => {
		vi.spyOn(BedrockRuntimeClient.prototype, "send").mockResolvedValue({
			$metadata: { httpStatusCode: 200 },
			stream: (async function* () {
				yield { throttlingException: { message: "too many tokens" } };
			})(),
		} as never);
		const result = await streamBedrock(model("bedrock-converse-stream", "amazon-bedrock"), context, {
			region: "us-east-1",
		}).result();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ category: "rate_limit", transient: true });
	});
	it.each([
		[
			"Anthropic",
			(onPayload: () => never) =>
				streamAnthropic(model("anthropic-messages", "anthropic"), context, { apiKey: "fake", onPayload }).result(),
		],
		[
			"Chat",
			(onPayload: () => never) =>
				streamOpenAICompletions(model("openai-completions", "openai"), context, {
					apiKey: "fake",
					onPayload,
				}).result(),
		],
		[
			"Mistral",
			(onPayload: () => never) =>
				streamMistral(model("mistral-conversations", "mistral"), context, { apiKey: "fake", onPayload }).result(),
		],
	] as const)("separates %s callbacks without persisting secrets", async (_name, run) => {
		const fetch = vi.fn();
		vi.stubGlobal("fetch", fetch);
		const result = await run(() => {
			throw new Error("private 503 sentinel");
		});
		expect(fetch).not.toHaveBeenCalled();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ source: "callback" });
		expect(JSON.stringify(result)).not.toContain("sentinel");
	});
});
