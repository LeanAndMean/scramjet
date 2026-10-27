import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent, type AgentEvent } from "@leanandmean/agent";
import { type AssistantMessage, createAssistantMessageEventStream, type Model } from "@leanandmean/ai";
import { Type } from "typebox";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgentSession } from "../src/core/agent-session.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader } from "../src/core/resource-loader.js";
import { createAgentSession } from "../src/core/sdk.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";

const google = vi.hoisted(() => ({ stream: vi.fn() }));
vi.mock("@google/genai", async (original) => {
	const actual = await original<typeof import("@google/genai")>();
	return {
		...actual,
		GoogleGenAI: class {
			models = { generateContentStream: google.stream };
		},
	};
});

const model = {
	id: "requested-model",
	name: "Offline Model",
	api: "openai-completions" as const,
	provider: "openai",
	baseUrl: "https://example.test",
	reasoning: false,
	input: ["text" as const],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 10000,
	maxTokens: 100,
};
const directories: string[] = [];
const sessions: AgentSession[] = [];

function assistant(overrides: Partial<AssistantMessage> = {}): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text: "done" }],
		api: model.api,
		provider: model.provider,
		model: model.id,
		responseModel: "routed-model",
		usage: {
			input: 10,
			output: 84,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 94,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: "stop",
		timestamp: 1,
		...overrides,
	};
}

async function profile() {
	const dir = await mkdtemp(join(tmpdir(), "session-throughput-"));
	directories.push(dir);
	const settingsManager = SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false } });
	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey(model.provider, "offline-key");
	const modelRegistry = ModelRegistry.inMemory(authStorage);
	const resourceLoader = new DefaultResourceLoader({
		cwd: dir,
		agentDir: dir,
		settingsManager,
		noExtensions: true,
		noSkills: true,
		noPromptTemplates: true,
	});
	await resourceLoader.reload();
	return { dir, settingsManager, authStorage, modelRegistry, resourceLoader };
}

async function fixture(
	options: { times?: number[]; normalizedStart?: boolean; duration?: number; adapterModel?: Model<any> } = {},
) {
	const resources = await profile();
	let now = 0;
	vi.spyOn(performance, "now").mockImplementation(() => now);
	let response = assistant();
	const agent = new Agent({
		initialState: { model: options.adapterModel ?? model },
		getApiKey: async () => "offline-key",
		streamFn: options.adapterModel
			? undefined
			: () => {
					const start = now;
					const message = response;
					const stream = createAssistantMessageEventStream();
					stream[Symbol.asyncIterator] = async function* () {
						if (options.normalizedStart !== false) yield { type: "start", partial: message };
						for (const time of options.times ?? []) {
							now = start + time;
							yield { type: "text_delta", contentIndex: 0, delta: "abcd", partial: message };
						}
						now = start + (options.duration ?? 2000);
						if (message.stopReason === "error" || message.stopReason === "aborted") {
							yield { type: "error", reason: message.stopReason, error: message };
						} else {
							yield { type: "done", reason: message.stopReason, message };
						}
					};
					vi.spyOn(stream, "result").mockResolvedValue(message);
					return stream;
				},
	});
	const session = new AgentSession({
		agent,
		...resources,
		cwd: resources.dir,
		sessionManager: SessionManager.inMemory(resources.dir),
		outputThroughputHistoryPath: join(resources.dir, "output-throughput-history-v2.json"),
	});
	sessions.push(session);
	return {
		...resources,
		agent,
		session,
		setResponse: (message: AssistantMessage) => {
			response = message;
		},
		advance: (ms: number) => {
			now += ms;
		},
	};
}

afterEach(async () => {
	for (const session of sessions.splice(0)) {
		await session.flushOutputThroughputHistory();
		session.dispose();
	}
	vi.restoreAllMocks();
	vi.unstubAllGlobals();
	google.stream.mockReset();
	await Promise.all(directories.splice(0).map((dir) => rm(dir, { recursive: true, force: true })));
});

describe("Session whole-response throughput", () => {
	it("measures Responses hidden output independently of summaries and synthetic separators", async () => {
		const route = { ...model, api: "openai-responses" as const, reasoning: true };
		const { agent, session, advance } = await fixture({ adapterModel: route });
		const deltas: string[] = [];
		agent.subscribe((event) => {
			if (event.type === "message_update" && event.assistantMessageEvent.type === "thinking_delta") {
				deltas.push(event.assistantMessageEvent.delta);
			}
		});
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => {
				advance(1500);
				const item = { id: "reasoning_1", type: "reasoning", summary: [] };
				const events = [
					{ type: "response.created", response: { id: "response_1" } },
					{ type: "response.output_item.added", output_index: 0, item },
					{ type: "response.reasoning_summary_part.added", part: { type: "summary_text", text: "" } },
					{ type: "response.reasoning_summary_text.delta", delta: "Brief summary" },
					{ type: "response.reasoning_summary_part.done" },
					{ type: "response.output_item.done", output_index: 0, item },
					{
						type: "response.completed",
						response: {
							id: "response_1",
							status: "completed",
							usage: {
								input_tokens: 10,
								output_tokens: 84,
								total_tokens: 94,
								input_tokens_details: { cached_tokens: 0 },
								output_tokens_details: { reasoning_tokens: 80 },
							},
						},
					},
				];
				return new Response(
					new ReadableStream({
						start(controller) {
							advance(500);
							controller.enqueue(
								new TextEncoder().encode(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join("")),
							);
							controller.close();
						},
					}),
					{ headers: { "content-type": "text/event-stream" } },
				);
			}),
		);
		await agent.prompt("offline reasoning");
		expect(deltas).toEqual(["Brief summary", "\n\n"]);
		expect(agent.state.messages.at(-1)).toMatchObject({ stopReason: "stop", usage: { output: 84 } });
		expect(session.medianOutputRate).toBe(42);
	});

	it.each(["error", "aborted"] as const)(
		"rejects Google %s then measures thought-inclusive whole-object tool output",
		async (failure) => {
			const route = { ...model, api: "google-generative-ai" as const, provider: "google", reasoning: true };
			const { agent, session, advance } = await fixture({ adapterModel: route });
			const deltas: string[] = [];
			agent.subscribe((event) => {
				if (event.type === "message_update" && event.assistantMessageEvent.type === "toolcall_delta") {
					deltas.push(event.assistantMessageEvent.delta);
				}
			});
			agent.state.tools = [
				{
					name: "finish",
					label: "Finish",
					description: "Finish offline fixture",
					parameters: Type.Object({}),
					execute: async () => ({ content: [{ type: "text", text: "done" }], details: {}, terminate: true }),
				},
			];
			google.stream.mockImplementationOnce(async () => {
				advance(2000);
				if (failure === "aborted") agent.abort();
				throw new Error("offline failure");
			});
			await agent.prompt("fail");
			expect(agent.state.messages.at(-1)).toMatchObject({ stopReason: failure });
			expect(session.medianOutputRate).toBeUndefined();
			google.stream.mockImplementationOnce(async () => {
				advance(1500);
				return (async function* () {
					advance(500);
					yield {
						candidates: [
							{
								finishReason: "STOP",
								content: {
									parts: [
										{ text: "brief thought", thought: true },
										{ functionCall: { name: "finish", args: { result: "done" } } },
									],
								},
							},
						],
						usageMetadata: {
							promptTokenCount: 10,
							candidatesTokenCount: 4,
							thoughtsTokenCount: 80,
							totalTokenCount: 94,
						},
					};
				})();
			});
			await agent.prompt("success");
			expect(deltas).toEqual(['{"result":"done"}']);
			await session.flushOutputThroughputHistory();
			expect(session.outputThroughputHistory.allSamples()).toMatchObject([
				{ provider: "google", model: model.id, outputTokens: 84, durationMs: 2000 },
			]);
			expect(session.outputThroughputHistory.allSamples()).toHaveLength(1);
			expect(session.medianOutputRate).toBe(42);
		},
	);

	it.each([
		{ times: [500, 1500] },
		{ times: [1999, 2000] },
		{ times: [2000] },
		{ times: [] },
		{ times: [], normalizedStart: false },
	])("measures 84 tokens over 2000 ms for $times / start=$normalizedStart", async (options) => {
		const { agent, session } = await fixture(options);
		await agent.prompt("offline");
		expect(session.medianOutputRate).toBe(42);
		await session.flushOutputThroughputHistory();
		const history = await session.refreshOutputThroughputHistory();
		expect(history.median(model.provider, model.id)).toBe(42);
		expect(history.median(model.provider, "routed-model")).toBeUndefined();
		expect(history.latestSample(model.provider, model.id)).toMatchObject({
			outputTokens: 84,
			durationMs: 2000,
			responseModel: "routed-model",
		});
	});

	it("captures duration before slow queued Session hooks", async () => {
		const { agent, session, advance } = await fixture();
		session.subscribe(async (event) => {
			if (event.type === "message_end" && event.message.role === "assistant") {
				await Promise.resolve();
				advance(9000);
			}
		});
		await agent.prompt("offline");
		expect(session.medianOutputRate).toBe(42);
	});

	it.each(["error", "aborted"] as const)("excludes %s and recovers on the next response", async (stopReason) => {
		const { agent, session, setResponse } = await fixture({ times: [1000, 2000] });
		setResponse(assistant({ stopReason }));
		await agent.prompt("fail");
		await session.flushOutputThroughputHistory();
		expect(session.medianOutputRate).toBeUndefined();
		expect(session.outputThroughputHistory.allSamples()).toEqual([]);
		setResponse(assistant());
		await agent.prompt("success");
		expect(session.medianOutputRate).toBe(42);
	});

	it.each([undefined, 0, -1, Number.NaN, Number.POSITIVE_INFINITY])(
		"does not accept missing or invalid event duration %s",
		async (duration) => {
			const { session } = await fixture({ times: [1000, 2000] });
			const event: AgentEvent = {
				type: "message_end",
				message: { ...assistant(), origin: "provider" },
				responseDurationMs: duration,
			};
			const handle = (session as unknown as { _handleAgentEvent(event: AgentEvent): Promise<void> | void })
				._handleAgentEvent;
			await handle({ type: "message_start", message: event.message });
			await handle(event);
			expect(session.medianOutputRate).toBeUndefined();
			expect(session.outputThroughputHistory.allSamples()).toEqual([]);
		},
	);

	it("abort resets an in-flight measurement and ignores its late ending", async () => {
		const { agent, session } = await fixture();
		const message = { ...assistant(), origin: "provider" as const };
		const handle = (session as unknown as { _handleAgentEvent(event: AgentEvent): Promise<void> | void })
			._handleAgentEvent;
		await handle({ type: "message_start", message });
		await handle({
			type: "message_update",
			message,
			assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: "abcd", partial: message },
		});
		expect(session.outputThroughputActive).toBe(true);
		const generation = session.outputThroughputGeneration;
		await session.abort();
		expect(session.outputThroughputGeneration).toBeGreaterThan(generation);
		expect(session.outputThroughputActive).toBe(false);
		await handle({ type: "message_end", message, responseDurationMs: 2000 });
		expect(session.medianOutputRate).toBeUndefined();
		await agent.prompt("success");
		expect(session.medianOutputRate).toBe(42);
	});

	it("retains a local sample when persistence rejects the explicit path", async () => {
		const { agent, session } = await fixture();
		const legacy = JSON.stringify({ version: 1, samples: [] });
		await writeFile(session.outputThroughputHistoryPath, legacy);
		await agent.prompt("offline");
		await session.flushOutputThroughputHistory();
		expect(session.outputThroughputHistory.allSamples()).toEqual([]);
		expect(session.medianOutputRate).toBe(42);
		expect(await readFile(session.outputThroughputHistoryPath, "utf8")).toBe(legacy);
	});

	it("uses fresh v2 history on SDK reopen without touching v1 history", async () => {
		const { agent, session, dir, ...resources } = await fixture();
		const legacyPath = join(dir, "output-throughput-history.json");
		const legacy = JSON.stringify({
			version: 1,
			samples: [{ provider: model.provider, model: model.id, outputTokens: 999, durationMs: 1, observedAt: 1 }],
		});
		await writeFile(legacyPath, legacy);
		const open = async () => {
			const { session: reopened } = await createAgentSession({
				...resources,
				cwd: dir,
				agentDir: dir,
				model,
				sessionManager: SessionManager.inMemory(dir),
				noTools: "all",
			});
			sessions.push(reopened);
			return reopened;
		};
		const cold = await open();
		expect(cold.outputThroughputHistoryPath).toBe(join(dir, "output-throughput-history-v2.json"));
		expect(cold.medianOutputRate).toBeUndefined();
		await agent.prompt("offline");
		await session.flushOutputThroughputHistory();
		const reopened = await open();
		expect(reopened.medianOutputRate).toBe(42);
		expect(reopened.outputThroughputHistory.median(model.provider, model.id)).toBe(42);
		expect(await readFile(legacyPath, "utf8")).toBe(legacy);
	});
});
