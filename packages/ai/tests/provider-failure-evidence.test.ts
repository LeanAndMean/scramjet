import { BedrockRuntimeClient } from "@aws-sdk/client-bedrock-runtime";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getApiProvider } from "../src/api-registry.js";
import { streamBedrock, streamSimpleBedrock } from "../src/providers/amazon-bedrock.js";
import { streamAnthropic } from "../src/providers/anthropic.js";
import { streamAzureOpenAIResponses } from "../src/providers/azure-openai-responses.js";
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
	it.each([
		["github-copilot", "prompt token count of 200000 exceeds the limit of 128000"],
		["llama.cpp", "the request exceeds the available context size, try increasing it"],
		["lmstudio", "tokens to keep from the initial prompt is greater than the context length"],
		["ollama", "prompt too long; exceeded max context length by 200 tokens"],
	] as const)("retains established %s overflow and rejection vetoes", async (provider, message) => {
		for (const [status, code, overflow] of [
			[400, "invalid_request_error", true],
			[400, "insufficient_quota", false],
			[401, "invalid_request_error", false],
			[403, "invalid_request_error", false],
			[404, "invalid_request_error", false],
			[429, "invalid_request_error", false],
		] as const) {
			const fetch = vi.fn(
				async () =>
					new Response(JSON.stringify({ error: { code, message } }), {
						status,
						headers: { "content-type": "application/json" },
					}),
			);
			vi.stubGlobal("fetch", fetch);
			const result = await streamOpenAICompletions(model("openai-completions", provider), context, {
				apiKey: "fake",
				maxRetries: 0,
			}).result();
			expect(fetch).toHaveBeenCalledTimes(1);
			expect(result.stopReason).toBe("error");
			expect(inspectFailureEvidence(result.diagnostics).status).toBe("valid");
			expect(isContextOverflow(result)).toBe(overflow);
		}
	});
	it.each(["Chat", "Anthropic"] as const)(
		"normalizes actual %s SDK transport and timeout identities",
		async (route) => {
			for (const category of ["timeout", "transport"] as const) {
				const fetch = vi.fn(async () => {
					if (category === "timeout") throw new DOMException("fixture timeout", "AbortError");
					throw new TypeError("fetch failed", {
						cause: Object.assign(new Error("socket failed"), { code: "ECONNRESET" }),
					});
				});
				vi.stubGlobal("fetch", fetch);
				const result = await (route === "Chat"
					? streamOpenAICompletions(model("openai-completions", "openai"), context, {
							apiKey: "fake",
							maxRetries: 0,
						})
					: streamAnthropic(model("anthropic-messages", "anthropic"), context, { apiKey: "fake", maxRetries: 0 })
				).result();
				expect(fetch).toHaveBeenCalledTimes(1);
				expect(result.stopReason).toBe("error");
				expect(result.errorMessage).toBe(category === "timeout" ? "Request timed out." : "Connection error.");
				expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
					status: "valid",
					category,
					transient: true,
				});
			}
		},
	);
	it.each(["Chat", "Anthropic"] as const)(
		"does not turn %s caller cancellation into transient evidence",
		async (route) => {
			const controller = new AbortController();
			const fetch = vi.fn(async () => {
				controller.abort();
				throw new DOMException("caller cancellation", "AbortError");
			});
			vi.stubGlobal("fetch", fetch);
			const options = { apiKey: "fake", maxRetries: 0, signal: controller.signal };
			const result = await (route === "Chat"
				? streamOpenAICompletions(model("openai-completions", "openai"), context, options)
				: streamAnthropic(model("anthropic-messages", "anthropic"), context, options)
			).result();
			expect(fetch).toHaveBeenCalledTimes(1);
			expect(result.stopReason).toBe("aborted");
			expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ status: "valid", transient: false });
		},
	);
	it.each(["Chat", "Anthropic"] as const)(
		"keeps unsupported %s rejection closed despite timeout prose",
		async (route) => {
			vi.stubGlobal(
				"fetch",
				vi.fn(
					async () =>
						new Response(
							JSON.stringify({
								error: {
									code: "novel",
									type: "novel",
									message: "Connection error. Request timed out.",
								},
							}),
							{ status: 400, headers: { "content-type": "application/json" } },
						),
				),
			);
			const result = await (route === "Chat"
				? streamOpenAICompletions(model("openai-completions", "openai"), context, { apiKey: "fake", maxRetries: 0 })
				: streamAnthropic(model("anthropic-messages", "anthropic"), context, { apiKey: "fake", maxRetries: 0 })
			).result();
			expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
				status: "valid",
				category: "invalid_request",
				transient: false,
			});
		},
	);
	it.each([false, true])("retains the Anthropic SDK envelope quota veto (compatible=%s)", async (compatible) => {
		vi.stubGlobal(
			"fetch",
			vi.fn(
				async () =>
					new Response(
						JSON.stringify({
							code: compatible ? "rate_limit_exceeded" : "insufficient_quota",
							error: { type: "rate_limit_error", message: "try later" },
						}),
						{ status: 429, headers: { "content-type": "application/json" } },
					),
			),
		);
		const result = await streamAnthropic(model("anthropic-messages", "anthropic"), context, {
			apiKey: "fake",
			maxRetries: 0,
		}).result();
		expect(result.stopReason).toBe("error");
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			status: "valid",
			category: compatible ? "rate_limit" : "unknown",
			transient: compatible,
		});
	});
	it.each(["Chat", "Anthropic"] as const)("protects %s partial-tool EOF and terminal success", async (route) => {
		for (const terminal of [false, true]) {
			const anthropicEvents = [
				{ type: "message_start", message: { id: "msg", usage: { input_tokens: 1, output_tokens: 0 } } },
				{
					type: "content_block_start",
					index: 0,
					content_block: { type: "tool_use", id: "call", name: "read", input: {} },
				},
				{
					type: "content_block_delta",
					index: 0,
					delta: { type: "input_json_delta", partial_json: '{"path":"partial"}' },
				},
				...(terminal
					? [
							{ type: "content_block_stop", index: 0 },
							{ type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 1 } },
							{ type: "message_stop" },
						]
					: []),
			];
			const chatChunks = [
				{
					id: "msg",
					choices: [
						{
							index: 0,
							delta: {
								role: "assistant",
								tool_calls: [
									{
										index: 0,
										id: "call",
										type: "function",
										function: { name: "read", arguments: '{"path":"partial"}' },
									},
								],
							},
							finish_reason: null,
						},
					],
				},
				...(terminal ? [{ id: "msg", choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] }] : []),
			];
			const body =
				route === "Anthropic"
					? anthropicEvents.map((event) => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join("")
					: `${chatChunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`).join("")}data: [DONE]\n\n`;
			vi.stubGlobal(
				"fetch",
				vi.fn(async () => new Response(body, { headers: { "content-type": "text/event-stream" } })),
			);
			const result = await (route === "Chat"
				? streamOpenAICompletions(model("openai-completions", "openai"), context, { apiKey: "fake", maxRetries: 0 })
				: streamAnthropic(model("anthropic-messages", "anthropic"), context, { apiKey: "fake", maxRetries: 0 })
			).result();
			expect(result.content).toEqual([
				{ type: "toolCall", id: "call", name: "read", arguments: { path: "partial" } },
			]);
			expect(result.stopReason).toBe(terminal ? "toolUse" : "error");
			if (terminal) expect(inspectFailureEvidence(result.diagnostics)).toEqual({ status: "absent" });
			else
				expect(result.diagnostics?.filter((d) => d.type === "request_failure").map((d) => d.details)).toEqual([
					{ schemaVersion: 1, kind: "stream", reason: "missing_terminal_event" },
				]);
		}
	});
	it("preserves startless Anthropic SSE success", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(
				async () =>
					new Response('event: ping\ndata: {"type":"ping"}\n\n', {
						headers: { "content-type": "text/event-stream" },
					}),
			),
		);
		const result = await streamAnthropic(model("anthropic-messages", "anthropic"), context, {
			apiKey: "fake",
			maxRetries: 0,
		}).result();
		expect(result.stopReason).toBe("stop");
		expect(inspectFailureEvidence(result.diagnostics)).toEqual({ status: "absent" });
	});
	it.each([
		[undefined, "missing_body", false],
		[{ internalServerException: { message: "fixture" } }, "server", true],
		[{ serviceUnavailableException: { message: "fixture" } }, "server", true],
		[{ validationException: { message: "server error context_length_exceeded" } }, "invalid_request", false],
		[{ modelStreamErrorException: { message: "server error context_length_exceeded" } }, "unknown", false],
	] as const)("preserves Bedrock event evidence for %s", async (event, category, transient) => {
		const send = vi.spyOn(BedrockRuntimeClient.prototype, "send").mockResolvedValue({
			$metadata: { httpStatusCode: 200 },
			...(event
				? {
						stream: (async function* () {
							yield event;
						})(),
					}
				: {}),
		} as never);
		const result = await streamBedrock(model("bedrock-converse-stream", "amazon-bedrock"), context, {
			region: "us-east-1",
		}).result();
		expect(send).toHaveBeenCalledTimes(1);
		expect(result.stopReason).toBe("error");
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ status: "valid", category, transient });
		expect(isContextOverflow(result)).toBe(false);
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
	it.each(["OpenAI", "Azure"] as const)(
		"keeps %s SDK preparation guidance actionable and private with zero fetch calls",
		async (route) => {
			const fetch = vi.fn();
			vi.stubGlobal("fetch", fetch);
			for (const value of [
				1n,
				{
					toJSON() {
						throw new Error("private serialization sentinel");
					},
				},
			]) {
				const options = {
					apiKey: "fake",
					maxRetries: 0,
					onPayload: (payload: unknown) => ({ ...(payload as object), metadata: { value } }),
				};
				const result = await (route === "OpenAI"
					? streamOpenAIResponses(model("openai-responses", "openai"), context, options)
					: streamAzureOpenAIResponses(model("azure-openai-responses", "azure-openai-responses"), context, options)
				).result();
				expect(result.stopReason).toBe("error");
				expect(result.errorMessage).toMatch(/JSON-serializable/);
				expect(result.errorMessage).toMatch(/base URL and headers/);
				expect(JSON.stringify(result)).not.toContain("private serialization sentinel");
				expect(fetch).not.toHaveBeenCalled();
				expect(result.diagnostics).toContainEqual(
					expect.objectContaining({
						type: "request_failure",
						details: { schemaVersion: 1, kind: "local", reason: "request_preparation" },
					}),
				);
				expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
					status: "valid",
					source: "local",
					category: "local",
					transient: false,
				});
			}
		},
	);
	it.each(["OpenAI key", "Azure key", "Azure endpoint", "Azure URL"] as const)(
		"retains controlled configuration guidance for %s without fetching",
		async (configuration) => {
			for (const name of [
				"OPENAI_API_KEY",
				"AZURE_OPENAI_API_KEY",
				"AZURE_OPENAI_BASE_URL",
				"AZURE_OPENAI_RESOURCE_NAME",
			]) {
				vi.stubEnv(name, "");
			}
			const fetch = vi.fn();
			vi.stubGlobal("fetch", fetch);
			const result = await (configuration === "OpenAI key"
				? streamOpenAIResponses(model("openai-responses", "openai"), context)
				: streamAzureOpenAIResponses(
						{ ...model("azure-openai-responses", "azure-openai-responses"), baseUrl: "" },
						context,
						{
							apiKey: configuration === "Azure key" ? undefined : "fake",
							azureBaseUrl: configuration === "Azure URL" ? "private-invalid-url-sentinel" : undefined,
						},
					)
			).result();
			expect(result.stopReason).toBe("error");
			expect(result.errorMessage).toContain(
				configuration === "OpenAI key"
					? "Set OPENAI_API_KEY"
					: configuration === "Azure key"
						? "Set AZURE_OPENAI_API_KEY"
						: configuration === "Azure endpoint"
							? "Set AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME"
							: "Invalid Azure OpenAI base URL",
			);
			if (configuration === "Azure URL") expect(result.errorMessage).toContain("azureBaseUrl");
			expect(JSON.stringify(result)).not.toContain("private-invalid-url-sentinel");
			expect(fetch).not.toHaveBeenCalled();
			expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
				status: "valid",
				source: "local",
				category: "local",
				transient: false,
			});
		},
	);
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
