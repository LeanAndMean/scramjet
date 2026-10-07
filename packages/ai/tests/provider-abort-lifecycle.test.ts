import { spawnSync } from "node:child_process";
import { getEventListeners, setMaxListeners } from "node:events";
import { createServer, type ServerResponse } from "node:http";
import Anthropic from "@anthropic-ai/sdk";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getModel } from "../src/models.js";
import { streamAnthropic } from "../src/providers/anthropic.js";
import { streamAzureOpenAIResponses } from "../src/providers/azure-openai-responses.js";
import { streamOpenAICompletions } from "../src/providers/openai-completions.js";
import { streamOpenAIResponses } from "../src/providers/openai-responses.js";
import type { AssistantMessage, Context, StreamOptions } from "../src/types.js";
import type { AssistantMessageEventStream } from "../src/utils/event-stream.js";
import { inspectFailureEvidence } from "../src/utils/failure-evidence.js";
import { createProviderAbortScope } from "../src/utils/provider-abort-scope.js";

const context: Context = { messages: [{ role: "user", content: "hello", timestamp: 0 }] };
const nativeFetch = globalThis.fetch;
const chatPartial = [
	{ id: "msg_1", choices: [{ index: 0, delta: { role: "assistant", content: "done" }, finish_reason: null }] },
];
const chatCompleted = [
	...chatPartial,
	{
		id: "msg_1",
		choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
		usage: { prompt_tokens: 2, completion_tokens: 1, total_tokens: 3 },
	},
];
const anthropicPartial = [
	{ type: "message_start", message: { id: "msg_1", usage: { input_tokens: 2, output_tokens: 0 } } },
	{ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
	{ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "done" } },
];
const anthropicCompleted = [
	...anthropicPartial,
	{ type: "content_block_stop", index: 0 },
	{ type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 1 } },
	{ type: "message_stop" },
];
const routes = [
	{
		name: "Chat",
		partial: chatPartial,
		completed: chatCompleted,
		malformed: [...chatPartial, { error: { message: "malformed stream" } }],
		stream: (options: StreamOptions, baseUrl = "http://localhost:1/v1") =>
			streamOpenAICompletions({ ...getModel("openai", "gpt-4o"), baseUrl }, context, {
				apiKey: "test-key",
				...options,
			}),
	},
	...[false, true].map((injected) => ({
		name: injected ? "Anthropic injected client" : "Anthropic",
		partial: anthropicPartial,
		completed: anthropicCompleted,
		malformed: [...anthropicPartial, { type: "error" }],
		stream: (options: StreamOptions, baseUrl = "http://localhost:1/v1") =>
			streamAnthropic({ ...getModel("anthropic", "claude-sonnet-4-5"), baseUrl }, context, {
				apiKey: "test-key",
				...(injected
					? {
							client: new Anthropic({
								apiKey: "test-key",
								baseURL: baseUrl,
								fetch: (input, init) => fetch(input, init),
							}),
						}
					: {}),
				...options,
			}),
	})),
	{
		name: "OpenAI Responses",
		stream: (options: StreamOptions, baseUrl = "http://localhost:1/v1") =>
			streamOpenAIResponses({ ...getModel("openai", "gpt-6-astra"), baseUrl }, context, {
				apiKey: "test-key",
				...options,
			}),
	},
	{
		name: "Azure Responses",
		stream: (options: StreamOptions, baseUrl = "http://localhost:1/v1") =>
			streamAzureOpenAIResponses(getModel("azure-openai-responses", "gpt-4"), context, {
				apiKey: "test-key",
				azureBaseUrl: baseUrl,
				...options,
			}),
	},
];

function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((done) => {
		resolve = done;
	});
	return { promise, resolve };
}

function sse(events: Record<string, unknown>[]) {
	return events
		.map(
			(event) =>
				`${typeof event.type === "string" && !event.type.startsWith("response.") ? `event: ${event.type}\n` : ""}data: ${JSON.stringify(event)}\n\n`,
		)
		.join("");
}

const partialEvents = [
	{ type: "response.created", response: { id: "resp_1" } },
	{
		type: "response.output_item.added",
		output_index: 0,
		item: { type: "message", id: "msg_1", role: "assistant", content: [] },
	},
	{
		type: "response.content_part.added",
		output_index: 0,
		content_index: 0,
		part: { type: "output_text", text: "", annotations: [] },
	},
	{ type: "response.output_text.delta", output_index: 0, content_index: 0, delta: "done" },
];
const completedEvents = [
	...partialEvents,
	{
		type: "response.output_item.done",
		output_index: 0,
		item: { type: "message", id: "msg_1", content: [{ type: "output_text", text: "done", annotations: [] }] },
	},
	{
		type: "response.completed",
		response: {
			id: "resp_1",
			status: "completed",
			usage: { input_tokens: 2, output_tokens: 1, total_tokens: 3, input_tokens_details: { cached_tokens: 0 } },
		},
	},
];

function httpError(status: number): Response {
	return new Response(JSON.stringify({ error: { message: "failure", code: "server_error" } }), {
		status,
		headers: { "content-type": "application/json", "retry-after-ms": "1" },
	});
}

function observeFetch(implementation: typeof fetch) {
	const signals: AbortSignal[] = [];
	const mock = vi.fn<typeof fetch>((input, init) => {
		const signal = init?.signal ?? (input instanceof Request ? input.signal : undefined);
		if (!signal) throw new Error("SDK transport signal missing");
		signals.push(signal);
		return implementation(input, init);
	});
	vi.stubGlobal("fetch", mock);
	return { signals, mock };
}

function assertFinalCleanup(caller: AbortSignal | undefined, signals: AbortSignal[], baseline = 0) {
	if (caller) {
		expect(getEventListeners(caller, "abort")).toHaveLength(baseline);
		expect(caller.aborted).toBe(false);
	}
	for (const signal of signals) expect(signal.aborted).toBe(true);
}

async function terminalChecks(stream: AssistantMessageEventStream, check: (result: AssistantMessage) => void) {
	const result = stream.result().then((message) => {
		check(message);
		return message;
	});
	const iteration = (async () => {
		for await (const event of stream) {
			if (event.type === "done" || event.type === "error")
				check(event.type === "done" ? event.message : event.error);
		}
	})();
	const [message] = await Promise.all([result, iteration]);
	return message;
}

async function bounded<T>(promise: Promise<T>): Promise<T> {
	let timer: ReturnType<typeof setTimeout> | undefined;
	try {
		return await Promise.race([
			promise,
			new Promise<never>((_, reject) => {
				timer = setTimeout(() => reject(new Error("Lifecycle handshake timed out")), 2000);
			}),
		]);
	} finally {
		clearTimeout(timer);
	}
}

async function loopback(handler: (response: ServerResponse) => void) {
	const closed = deferred<void>();
	const server = createServer((request, response) => {
		request.resume();
		response.on("close", () => closed.resolve());
		response.writeHead(200, { "content-type": "text/event-stream" });
		handler(response);
	});
	await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
	const address = server.address();
	if (!address || typeof address === "string") throw new Error("Missing loopback address");
	return {
		url: `http://127.0.0.1:${address.port}/v1`,
		closed: closed.promise,
		close: () => {
			server.closeAllConnections();
			return new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
		},
	};
}

afterEach(() => {
	vi.unstubAllGlobals();
});

describe.each(routes)("$name operation lifetime", (route) => {
	const { stream } = route;
	const partial = "partial" in route ? route.partial : partialEvents;
	const completed = "completed" in route ? route.completed : completedEvents;
	const malformed = "malformed" in route ? route.malformed : [...partialEvents, { type: "error" }];
	const response = (events = completed) =>
		new Response(sse(events), { headers: { "content-type": "text/event-stream" } });
	it("restores caller listeners after each of twelve successful operations, before either terminal consumer", async () => {
		const caller = new AbortController();
		setMaxListeners(10, caller.signal);
		const existing = () => {};
		caller.signal.addEventListener("abort", existing);
		const { signals } = observeFetch(async () => response());
		const warnings: Error[] = [];
		const onWarning = (warning: Error) => warnings.push(warning);
		process.on("warning", onWarning);
		try {
			for (let index = 0; index < 12; index++) {
				await terminalChecks(stream({ signal: caller.signal }), (result) => {
					assertFinalCleanup(caller.signal, signals, 1);
					expect(result.stopReason).toBe("stop");
					expect(result.content).toContainEqual(expect.objectContaining({ type: "text", text: "done" }));
					expect(result.usage).toMatchObject({ input: 2, output: 1, totalTokens: 3 });
				});
			}
			await new Promise<void>((resolve) => setImmediate(resolve));
			expect(warnings.filter((warning) => warning.name === "MaxListenersExceededWarning")).toEqual([]);
		} finally {
			process.removeListener("warning", onWarning);
			caller.signal.removeEventListener("abort", existing);
			caller.abort();
		}
	});

	it("disposes successful transport without a caller", async () => {
		const { signals } = observeFetch(async () => response());
		await terminalChecks(stream({}), (result) => {
			assertFinalCleanup(undefined, signals);
			expect(result.stopReason).toBe("stop");
		});
	});

	it.each([true, false])(
		"aborts an unread native body on response callback failure (caller: %s)",
		async (withCaller) => {
			const server = await loopback((response) => response.write(sse([partial[0]])));
			const caller = withCaller ? new AbortController() : undefined;
			const { signals } = observeFetch(nativeFetch);
			try {
				await terminalChecks(
					stream(
						{
							signal: caller?.signal,
							onResponse: () => {
								throw new Error("private callback sentinel");
							},
						},
						server.url,
					),
					(result) => {
						assertFinalCleanup(caller?.signal, signals);
						expect(result.stopReason).toBe("error");
						expect(result.diagnostics).toEqual([
							expect.objectContaining({
								type: "request_failure",
								details: { schemaVersion: 1, kind: "callback", callback: "onResponse" },
							}),
						]);
						expect(JSON.stringify(result)).not.toContain("private callback sentinel");
					},
				);
				await bounded(server.closed);
			} finally {
				caller?.abort();
				await server.close();
			}
		},
	);

	it.each([
		{ name: "retry recovery", statuses: [503, 200], stopReason: "stop", outcome: "recovered" },
		{ name: "retry exhaustion", statuses: [503, 503], stopReason: "error", outcome: "exhausted" },
		{ name: "non-retryable request", statuses: [400], stopReason: "error", outcome: "not_attempted" },
	])("cleans up $name without changing attempts or diagnostics", async ({ statuses, stopReason, outcome }) => {
		const caller = new AbortController();
		const pending = [...statuses];
		const { signals, mock } = observeFetch(async () => {
			const status = pending.shift();
			if (status === undefined) throw new Error("Unexpected retry");
			return status === 200 ? response() : httpError(status);
		});
		await terminalChecks(stream({ signal: caller.signal, maxRetries: 1 }), (result) => {
			assertFinalCleanup(caller.signal, signals);
			expect(result.stopReason).toBe(stopReason);
			expect(mock).toHaveBeenCalledTimes(statuses.length);
			if (!route.name.includes("Responses")) {
				if (stopReason === "stop") {
					expect(inspectFailureEvidence(result.diagnostics)).toEqual({ status: "absent" });
					expect(result.usage).toMatchObject({ input: 2, output: 1, totalTokens: 3 });
				} else {
					expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
						status: "valid",
						category: statuses[0] === 400 ? "invalid_request" : "server",
						transient: statuses[0] !== 400,
					});
				}
			}
			if (route.name.includes("Responses"))
				expect(result.diagnostics).toContainEqual(
					expect.objectContaining({
						type: "sdk_request_retry",
						details: expect.objectContaining({ outcome, observedAttemptCount: statuses.length }),
					}),
				);
		});
	});

	it("cleans up payload callback failure without making a request", async () => {
		const caller = new AbortController();
		const { mock } = observeFetch(async () => response());
		await terminalChecks(
			stream({
				signal: caller.signal,
				onPayload: () => {
					throw new Error("private payload sentinel");
				},
			}),
			(result) => {
				assertFinalCleanup(caller.signal, []);
				expect(mock).not.toHaveBeenCalled();
				expect(result.stopReason).toBe("error");
				expect(result.diagnostics).toEqual([
					expect.objectContaining({
						type: "request_failure",
						details: { schemaVersion: 1, kind: "callback", callback: "onPayload" },
					}),
				]);
			},
		);
	});

	it.each([
		{ name: "incomplete", events: partial, category: "missing_terminal_event" },
		{ name: "malformed", events: malformed, category: "malformed_event" },
	])("cleans up $name streams while preserving partial output", async ({ events, category }) => {
		const caller = new AbortController();
		const { signals } = observeFetch(async () => response(events));
		await terminalChecks(stream({ signal: caller.signal }), (result) => {
			assertFinalCleanup(caller.signal, signals);
			expect(result.stopReason).toBe("error");
			expect(result.content).toContainEqual(expect.objectContaining({ type: "text", text: "done" }));
			expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
				status: "valid",
				category: route.name === "Chat" && category === "malformed_event" ? "unknown" : category,
			});
			if (route.name.includes("Responses"))
				expect(result.diagnostics).toContainEqual(
					expect.objectContaining({
						type: "provider_failure",
						details: expect.objectContaining({ phase: "stream", category }),
					}),
				);
		});
	});

	it("does not fetch with a pre-aborted caller", async () => {
		const caller = new AbortController();
		caller.abort("already cancelled");
		const { mock } = observeFetch(async () => response());
		const result = await stream({ signal: caller.signal }).result();
		expect(result.stopReason).toBe("aborted");
		expect(mock).not.toHaveBeenCalled();
		expect(getEventListeners(caller.signal, "abort")).toHaveLength(0);
	});

	it("interrupts a pending request", async () => {
		const caller = new AbortController();
		const ready = deferred<void>();
		const { signals } = observeFetch((_input, init) => {
			ready.resolve();
			return new Promise<Response>((_resolve, reject) => {
				init!.signal!.addEventListener("abort", () => reject(new Error("cancelled")), { once: true });
			});
		});
		const result = stream({ signal: caller.signal }).result();
		await bounded(ready.promise);
		expect(getEventListeners(caller.signal, "abort")).toHaveLength(1);
		caller.abort();
		expect((await bounded(result)).stopReason).toBe("aborted");
		expect(getEventListeners(caller.signal, "abort")).toHaveLength(0);
		expect(signals[0].aborted).toBe(true);
	});

	it("keeps cancellation connected while a response callback is held", async () => {
		const caller = new AbortController();
		const ready = deferred<void>();
		const release = deferred<void>();
		const server = await loopback((response) => response.write(sse([partial[0]])));
		const { signals } = observeFetch(nativeFetch);
		try {
			const result = stream(
				{
					signal: caller.signal,
					onResponse: async () => {
						ready.resolve();
						await release.promise;
					},
				},
				server.url,
			).result();
			await bounded(ready.promise);
			expect(getEventListeners(caller.signal, "abort")).toHaveLength(1);
			expect(signals[0].aborted).toBe(false);
			caller.abort();
			expect(signals[0].aborted).toBe(true);
			await bounded(server.closed);
			release.resolve();
			expect((await bounded(result)).stopReason).toBe("aborted");
			expect(getEventListeners(caller.signal, "abort")).toHaveLength(0);
		} finally {
			caller.abort();
			release.resolve();
			await server.close();
		}
	});

	it("interrupts native body reads after partial output", async () => {
		const caller = new AbortController();
		const server = await loopback((response) => response.write(sse(partial)));
		const { signals } = observeFetch(nativeFetch);
		try {
			const events = stream({ signal: caller.signal }, server.url);
			const ready = deferred<void>();
			const consuming = (async () => {
				for await (const event of events) if (event.type === "text_delta") ready.resolve();
			})();
			const result = events.result();
			await bounded(ready.promise);
			expect(getEventListeners(caller.signal, "abort")).toHaveLength(1);
			expect(signals[0].aborted).toBe(false);
			caller.abort();
			const message = await bounded(result);
			expect(message.stopReason).toBe("aborted");
			expect(message.content).toContainEqual(expect.objectContaining({ type: "text", text: "done" }));
			expect(getEventListeners(caller.signal, "abort")).toHaveLength(0);
			await bounded(consuming);
			await bounded(server.closed);
		} finally {
			caller.abort();
			await server.close();
		}
	});

	it("does not start another attempt after cancellation during retry backoff", async () => {
		const caller = new AbortController();
		const retrying = deferred<void>();
		const failure = httpError(503);
		const getHeader = failure.headers.get.bind(failure.headers);
		vi.spyOn(failure.headers, "get").mockImplementation((name) => {
			if (name === "retry-after-ms") {
				retrying.resolve();
				return "50";
			}
			return getHeader(name);
		});
		const { mock } = observeFetch(async () => failure);
		const result = stream({ signal: caller.signal, maxRetries: 2 }).result();
		await bounded(retrying.promise);
		caller.abort();
		expect((await bounded(result)).stopReason).toBe("aborted");
		expect(mock).toHaveBeenCalledTimes(1);
		expect(getEventListeners(caller.signal, "abort")).toHaveLength(0);
	});
});

describe("provider abort scope", () => {
	it("captures the real abort-listener warning with an explicit test-only limit", () => {
		const child = spawnSync(
			process.execPath,
			[
				"-e",
				'const { setMaxListeners } = require("node:events"); const signal = new AbortController().signal; setMaxListeners(10, signal); for (let i = 0; i < 11; i++) signal.addEventListener("abort", () => {});',
			],
			{ encoding: "utf8", timeout: 2000, env: { ...process.env, NODE_OPTIONS: "" } },
		);
		expect(child.error).toBeUndefined();
		expect(child.status).toBe(0);
		expect(child.stderr).toMatch(/MaxListenersExceededWarning:.*11 abort listeners/);
	});

	it("owns a distinct signal, forwards reasons, and never removes unrelated caller listeners", () => {
		const caller = new AbortController();
		const existing = () => {};
		caller.signal.addEventListener("abort", existing);
		const scope = createProviderAbortScope(caller.signal);
		expect(scope.signal).not.toBe(caller.signal);
		expect(getEventListeners(caller.signal, "abort")).toHaveLength(2);
		const reason = new Error("cancelled by caller");
		caller.abort(reason);
		expect(scope.signal.aborted).toBe(true);
		expect(scope.signal.reason).toBe(reason);
		scope.dispose();
		scope.dispose();
		expect(getEventListeners(caller.signal, "abort")).toEqual([existing]);
		expect(scope.signal.reason).toBe(reason);
	});

	it("disposes without aborting the caller", () => {
		const caller = new AbortController();
		const scope = createProviderAbortScope(caller.signal);
		expect(getEventListeners(caller.signal, "abort")).toHaveLength(1);
		scope.dispose();
		scope.dispose();
		expect(scope.signal.aborted).toBe(true);
		expect(caller.signal.aborted).toBe(false);
		expect(getEventListeners(caller.signal, "abort")).toHaveLength(0);
	});

	it("immediately forwards an already-aborted caller without adding a listener", () => {
		const caller = new AbortController();
		caller.abort("pre-aborted");
		const scope = createProviderAbortScope(caller.signal);
		expect(scope.signal.aborted).toBe(true);
		expect(scope.signal.reason).toBe("pre-aborted");
		expect(getEventListeners(caller.signal, "abort")).toHaveLength(0);
		scope.dispose();
		expect(scope.signal.reason).toBe("pre-aborted");
	});

	it("owns and disposes a signal without a caller", () => {
		const scope = createProviderAbortScope();
		expect(scope.signal.aborted).toBe(false);
		scope.dispose();
		scope.dispose();
		expect(scope.signal.aborted).toBe(true);
	});
});
