import { type AssistantMessage, createAssistantMessageEventStream, type Model } from "@leanandmean/ai";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Agent } from "../src/agent.js";
import type { AgentEvent, AgentTool } from "../src/types.js";

const model: Model<"openai-chat"> = {
	id: "test-model",
	name: "Test Model",
	api: "openai-chat",
	provider: "openai",
	baseUrl: "https://api.openai.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};

function assistant(stopReason: AssistantMessage["stopReason"] = "stop"): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text: "done" }],
		api: model.api,
		provider: model.provider,
		model: model.id,
		usage: { input: 0, output: 84, cacheRead: 0, cacheWrite: 0 },
		stopReason,
		timestamp: Date.now(),
	};
}

afterEach(() => vi.restoreAllMocks());

describe("provider response duration", () => {
	for (const terminal of [true, false]) {
		for (const normalizedStart of [true, false]) {
			describe(`${terminal ? "terminal event" : "iterator exhaustion"}, ${normalizedStart ? "normalized" : "synthetic"} start`, () => {
				it.each([
					{ preparation: 0, listener: 0, result: 0, end: 0 },
					{ preparation: 700, listener: 0, result: 0, end: 0 },
					{ preparation: 0, listener: 300, result: 0, end: 0 },
					{ preparation: 0, listener: 0, result: 400, end: 0 },
					{ preparation: 0, listener: 0, result: 0, end: 900 },
				])("measures only invocation through final result: %j", async (delays) => {
					let now = 10_000;
					vi.spyOn(performance, "now").mockImplementation(() => now);
					const advance = async (duration: number) => {
						now += duration;
					};
					const message = assistant();
					const events: AgentEvent[] = [];
					const agent = new Agent({
						initialState: { model },
						transformContext: async (messages) => {
							await advance(delays.preparation);
							return messages;
						},
						beforeProviderCall: async () => {
							await advance(delays.preparation);
							return undefined;
						},
						convertToLlm: async () => {
							await advance(delays.preparation);
							return [];
						},
						getApiKey: async () => {
							await advance(delays.preparation);
							return "offline-key";
						},
						streamFn: async () => {
							await advance(500);
							const stream = createAssistantMessageEventStream();
							stream[Symbol.asyncIterator] = async function* () {
								await advance(1000);
								if (normalizedStart) yield { type: "start", partial: message };
								await advance(500);
								if (terminal) yield { type: "done", reason: "stop", message };
							};
							vi.spyOn(stream, "result").mockImplementation(async () => {
								await advance(delays.result);
								return message;
							});
							return stream;
						},
					});
					agent.subscribe(async (event) => {
						events.push(event);
						if (event.type === "message_start" && event.message.role === "assistant") {
							await advance(delays.listener);
						}
						if (event.type === "message_end") await advance(delays.end);
					});

					await agent.prompt("go");

					const ends = events.filter((event) => event.type === "message_end");
					expect(ends).toHaveLength(2);
					expect(ends[0]).not.toHaveProperty("responseDurationMs");
					expect(ends[1]).toMatchObject({
						responseDurationMs: 2000 + delays.result + (normalizedStart ? delays.listener : 0),
						message: { origin: "provider" },
					});
					expect(events.map((event) => event.type)).toEqual([
						"agent_start",
						"turn_start",
						"message_start",
						"message_end",
						"message_start",
						"message_end",
						"turn_end",
						"agent_end",
					]);
					expect(message).not.toHaveProperty("responseDurationMs");
					expect(agent.state.messages.at(-1)).not.toHaveProperty("responseDurationMs");
				});
			});
		}
	}

	it.each(["error", "aborted"] as const)("preserves provider %s endings with duration", async (stopReason) => {
		let now = 0;
		vi.spyOn(performance, "now").mockImplementation(() => now);
		const message = assistant(stopReason);
		const events: AgentEvent[] = [];
		const agent = new Agent({
			initialState: { model },
			streamFn: () => {
				now = 250;
				const stream = createAssistantMessageEventStream();
				stream.push({ type: "error", reason: stopReason, error: message });
				return stream;
			},
		});
		agent.subscribe((event) => {
			events.push(event);
		});
		await agent.prompt("go");
		expect(events.find((event) => event.type === "message_end" && event.message.role === "assistant")).toMatchObject({
			responseDurationMs: 250,
			message: { origin: "provider", stopReason },
		});
	});

	it("omits timing from harness assistant, tool, and local-failure endings", async () => {
		const events: AgentEvent[] = [];
		const tool: AgentTool = {
			name: "notice",
			label: "Notice",
			description: "notice",
			parameters: { type: "object", properties: {} },
			execute: async () => ({ content: [], details: undefined }),
		};
		const agent = new Agent({
			initialState: { model },
			streamFn: () => {
				throw new Error("local failure");
			},
		});
		agent.subscribe((event) => {
			events.push(event);
		});
		await agent.runHarnessTool(tool, {});
		await agent.prompt("go");
		const ends = events.filter((event) => event.type === "message_end");
		expect(ends.map((event) => event.message.role)).toEqual(["assistant", "toolResult", "user", "assistant"]);
		for (const event of ends) expect(event).not.toHaveProperty("responseDurationMs");
	});
});
