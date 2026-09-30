import { afterEach, describe, expect, it, vi } from "vitest";
import { getModel } from "../src/models.js";
import { streamOpenAICodexResponses } from "../src/providers/openai-codex-responses.js";
import { failureFromProviderError, inspectFailureEvidence } from "../src/utils/failure-evidence.js";
import { isContextOverflow } from "../src/utils/overflow.js";

const model = getModel("openai-codex", "gpt-6-astra");
const apiKey = `x.${btoa(JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "test" } }))}.x`;
const context = { messages: [{ role: "user" as const, content: "hello", timestamp: 0 }] };
const run = (options: Parameters<typeof streamOpenAICodexResponses>[2] = {}) =>
	streamOpenAICodexResponses(model, context, { apiKey, transport: "sse", ...options }).result();
afterEach(() => {
	vi.unstubAllGlobals();
	vi.useRealTimers();
});
describe("Codex retry boundaries", () => {
	it("rejects a malformed URL before fetch with local detail", async () => {
		const fetch = vi.fn();
		vi.stubGlobal("fetch", fetch);
		const result = await streamOpenAICodexResponses({ ...model, baseUrl: "not-a-url" }, context, {
			apiKey,
			transport: "sse",
			maxRetries: 2,
		}).result();
		expect(fetch).not.toHaveBeenCalled();
		expect(result.errorMessage).toMatch(/failed to parse url.*not-a-url/i);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			status: "valid",
			source: "local",
			transient: false,
		});
	});
	it("does not retry an unsupported fetch rejection or discard its detail", async () => {
		const fetch = vi.fn(async () => {
			throw new TypeError("unsupported local sentinel");
		});
		vi.stubGlobal("fetch", fetch);
		const result = await run({ maxRetries: 2 });
		expect(fetch).toHaveBeenCalledTimes(1);
		expect(result.errorMessage).toBe("unsupported local sentinel");
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ category: "unknown", transient: false });
	});
	it.each(["ECONNRESET", "UND_ERR_CONNECT_TIMEOUT"])(
		"retries observed fetch %s with exact bounded attempts",
		async (code) => {
			vi.useFakeTimers();
			const fetch = vi.fn(async () => {
				throw new TypeError("fetch failed", { cause: { code } });
			});
			vi.stubGlobal("fetch", fetch);
			const pending = run({ maxRetries: 1 });
			await vi.runAllTimersAsync();
			const result = await pending;
			expect(fetch).toHaveBeenCalledTimes(2);
			expect(result.errorMessage).toBe("fetch failed");
			expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
				category: code === "ECONNRESET" ? "transport" : "timeout",
				transient: true,
			});
		},
	);
	it.each([401, 503])("retains status and delay policy when HTTP %s body reading fails", async (status) => {
		const fetch = vi.fn(
			async () =>
				new Response(
					new ReadableStream({
						start(controller) {
							controller.error(new Error("terminated"));
						},
					}),
					{ status, headers: { "retry-after": "120" } },
				),
		);
		vi.stubGlobal("fetch", fetch);
		const result = await run();
		expect(fetch).toHaveBeenCalledTimes(1);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject(
			status === 401
				? { category: "authentication", transient: false }
				: { category: "server", suppression: "server_delay_exceeds_limit" },
		);
	});
	it.each([400, 401, 403, 409])("does not retry HTTP %s with misleading body", async (status) => {
		const fetch = vi.fn(async () => new Response("rate limit server error", { status }));
		vi.stubGlobal("fetch", fetch);
		const result = await run();
		expect(fetch).toHaveBeenCalledTimes(1);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ status: "valid", transient: false });
	});
	it.each(["usage_limit_reached", "usage_not_included"])("never retries explicit quota %s", async (code) => {
		const fetch = vi.fn(
			async () => new Response(JSON.stringify({ error: { code, message: "server error" } }), { status: 429 }),
		);
		vi.stubGlobal("fetch", fetch);
		const result = await run();
		expect(fetch).toHaveBeenCalledTimes(1);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			category: "quota_exhausted",
			transient: false,
		});
	});
	it.each([429, 503])("retries HTTP %s with an unfamiliar provider code", async (status) => {
		const fetch = vi.fn(
			async () =>
				new Response(JSON.stringify({ error: { code: "new_provider_code" } }), {
					status,
					headers: { "retry-after-ms": "0" },
				}),
		);
		vi.stubGlobal("fetch", fetch);
		const result = await run({ maxRetries: 1 });
		expect(fetch).toHaveBeenCalledTimes(2);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			category: status === 429 ? "rate_limit" : "server",
			transient: true,
		});
	});
	it("preserves the outer HTTP quota veto before Codex retries", async () => {
		const body = {
			code: "insufficient_quota",
			error: { type: "rate_limit_error", message: "try later" },
		};
		expect(failureFromProviderError({ status: 429, ...body })).toEqual({
			schemaVersion: 1,
			kind: "provider",
			category: "unknown",
		});
		const fetch = vi.fn(
			async () =>
				new Response(JSON.stringify(body), {
					status: 429,
					headers: { "content-type": "application/json", "retry-after-ms": "0" },
				}),
		);
		vi.stubGlobal("fetch", fetch);
		const result = await run({ maxRetries: 1 });
		expect(result.stopReason).toBe("error");
		expect.soft(fetch).toHaveBeenCalledTimes(1);
		expect.soft(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			status: "valid",
			category: "unknown",
			transient: false,
		});
	});
	it("honors ordinary 429 and explicit maxRetries", async () => {
		const fetch = vi.fn(
			async () =>
				new Response(JSON.stringify({ error: { code: "rate_limit_exceeded" } }), {
					status: 429,
					headers: { "retry-after-ms": "0" },
				}),
		);
		vi.stubGlobal("fetch", fetch);
		const result = await run({ maxRetries: 2 });
		expect(fetch).toHaveBeenCalledTimes(3);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ category: "rate_limit", transient: true });
	});
	it.each([NaN, Infinity, -1, 0.5])("rejects invalid direct count %s before transport", async (maxRetries) => {
		const fetch = vi.fn();
		vi.stubGlobal("fetch", fetch);
		const result = await run({ maxRetries });
		expect(fetch).not.toHaveBeenCalled();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ source: "local" });
	});
	it("keeps response callbacks out of the retry loop and private", async () => {
		const fetch = vi.fn(async () => new Response("", { status: 503 }));
		vi.stubGlobal("fetch", fetch);
		const result = await run({
			onResponse: () => {
				throw new Error("secret 429 sentinel");
			},
		});
		expect(fetch).toHaveBeenCalledTimes(1);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ source: "callback" });
		expect(JSON.stringify(result)).not.toContain("sentinel");
	});
	it("distinguishes successful missing body from HTTP rejection without a body", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => new Response(null, { status: 200 })),
		);
		expect(inspectFailureEvidence((await run()).diagnostics)).toMatchObject({ category: "missing_body" });
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => new Response(null, { status: 503 })),
		);
		expect(inspectFailureEvidence((await run({ maxRetries: 0 })).diagnostics)).toMatchObject({ category: "server" });
	});
	it("falls back to SSE after a pre-open WebSocket error", async () => {
		class FailingWebSocket extends EventTarget {
			readyState = 0;
			constructor() {
				super();
				queueMicrotask(() =>
					this.dispatchEvent(Object.assign(new Event("error"), { message: "upstream disconnected" })),
				);
			}
			send() {}
			close() {
				this.readyState = 3;
			}
		}
		vi.stubGlobal("WebSocket", FailingWebSocket);
		const fetch = vi.fn(
			async () =>
				new Response(
					'data: {"type":"response.completed","response":{"status":"completed","output":[],"usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0}}}\n\n',
					{ headers: { "content-type": "text/event-stream" } },
				),
		);
		vi.stubGlobal("fetch", fetch);
		const result = await run({ transport: "websocket" });
		expect(fetch).toHaveBeenCalledTimes(1);
		expect(result.stopReason).toBe("stop");
		expect(inspectFailureEvidence(result.diagnostics)).toEqual({ status: "absent" });
	});
	it("classifies a started WebSocket error event without SSE fallback and retains failed tool content", async () => {
		class FailingWebSocket extends EventTarget {
			readyState = 1;
			constructor() {
				super();
				queueMicrotask(() => this.dispatchEvent(new Event("open")));
			}
			send() {
				setTimeout(() => {
					this.dispatchEvent(
						new MessageEvent("message", {
							data: JSON.stringify({ type: "response.created", response: { id: "resp_1" } }),
						}),
					);
					this.dispatchEvent(
						new MessageEvent("message", {
							data: JSON.stringify({
								type: "response.output_item.added",
								output_index: 0,
								item: {
									type: "function_call",
									id: "fc_1",
									call_id: "call_1",
									name: "read",
									arguments: "",
								},
							}),
						}),
					);
					setTimeout(
						() => this.dispatchEvent(Object.assign(new Event("error"), { message: "upstream disconnected" })),
						0,
					);
				}, 0);
			}
			close() {
				this.readyState = 3;
			}
		}
		vi.stubGlobal("WebSocket", FailingWebSocket);
		const fetch = vi.fn();
		vi.stubGlobal("fetch", fetch);
		const result = await streamOpenAICodexResponses(model, context, { apiKey, transport: "websocket" }).result();
		expect(fetch).not.toHaveBeenCalled();
		expect(result.stopReason).toBe("error");
		expect(result.content).toContainEqual(expect.objectContaining({ type: "toolCall" }));
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			status: "valid",
			category: "transport",
			family: "request_failure",
			transient: true,
		});
	});
	it.each([
		["api", "unknown", false],
		["protocol", "malformed_event", false],
		["abnormal_close", "transport", true],
		["policy_close", "unknown", false],
		["normal_close", "unknown", false],
		["semantic", "rate_limit", true],
	] as const)("classifies started WebSocket %s independently of prose", async (kind, category, transient) => {
		class FailingWebSocket extends EventTarget {
			readyState = 1;
			constructor() {
				super();
				queueMicrotask(() => this.dispatchEvent(new Event("open")));
			}
			send() {
				setTimeout(() => {
					this.dispatchEvent(
						new MessageEvent("message", {
							data: JSON.stringify({ type: "response.created", response: { id: "resp_1" } }),
						}),
					);
					setTimeout(() => {
						const prose = "server error context_length_exceeded";
						if (kind.endsWith("close")) {
							this.dispatchEvent(
								Object.assign(new Event("close"), {
									code: kind === "abnormal_close" ? 1006 : kind === "policy_close" ? 1008 : 1000,
									reason: prose,
									wasClean: kind !== "abnormal_close",
								}),
							);
						} else {
							this.dispatchEvent(
								new MessageEvent("message", {
									data:
										kind === "protocol"
											? prose
											: JSON.stringify({
													type: "response.failed",
													response: {
														error: {
															message: prose,
															...(kind === "semantic" ? { code: "rate_limit_exceeded" } : {}),
														},
													},
												}),
								}),
							);
						}
					}, 0);
				}, 0);
			}
			close() {
				this.readyState = 3;
			}
		}
		vi.stubGlobal("WebSocket", FailingWebSocket);
		const fetch = vi.fn();
		vi.stubGlobal("fetch", fetch);
		const result = await streamOpenAICodexResponses(model, context, { apiKey, transport: "websocket" }).result();
		expect(fetch).not.toHaveBeenCalled();
		expect(result.stopReason).toBe("error");
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			status: "valid",
			family: "request_failure",
			category,
			transient,
		});
		expect(isContextOverflow(result)).toBe(false);
	});
	it("classifies code-less SSE API failures as unknown", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(
				async () =>
					new Response(
						'data: {"type":"response.failed","response":{"error":{"message":"server error context_length_exceeded"}}}\n\n',
						{ headers: { "content-type": "text/event-stream" } },
					),
			),
		);
		const result = await run();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			status: "valid",
			category: "unknown",
			transient: false,
		});
		expect(isContextOverflow(result)).toBe(false);
	});
	it.each([0, 3])("persists excessive server delay suppression even with %s inner retries", async (maxRetries) => {
		const fetch = vi.fn(async () => new Response(null, { status: 503, headers: { "retry-after": "120" } }));
		vi.stubGlobal("fetch", fetch);
		const result = await run({ maxRetries });
		expect(fetch).toHaveBeenCalledTimes(1);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			category: "server",
			suppression: "server_delay_exceeds_limit",
		});
	});
	it("suppresses unrepresentable server delays without persisting the value", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => new Response(null, { status: 429, headers: { "retry-after": "1e309" } })),
		);
		const result = await run({ maxRetryDelayMs: 0 });
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ suppression: "invalid_server_delay" });
		expect(JSON.stringify(result.diagnostics)).not.toContain("1e309");
	});
	it("chunks uncapped server waits and cancels without a retry", async () => {
		vi.useFakeTimers();
		const fetch = vi.fn(async () => new Response(null, { status: 429, headers: { "retry-after-ms": "2147483648" } }));
		vi.stubGlobal("fetch", fetch);
		const controller = new AbortController();
		const result = run({ maxRetryDelayMs: 0, signal: controller.signal });
		await vi.advanceTimersByTimeAsync(0);
		expect(fetch).toHaveBeenCalledTimes(1);
		await vi.advanceTimersByTimeAsync(2_147_483_647);
		expect(fetch).toHaveBeenCalledTimes(1);
		controller.abort();
		expect((await result).stopReason).toBe("aborted");
		expect(vi.getTimerCount()).toBe(0);
	});
});
