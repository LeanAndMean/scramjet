/**
 * Tests for the harness-tool-invocation primitive (`Agent.runHarnessTool`).
 *
 * Covers idle immediate execution (with the explicit invariant that no run/turn framing
 * events are emitted), mid-run queueing drained before the next intra-run LLM call, the
 * routing self-heal (a mid-run model change routes the next call), the end-of-run flush for
 * calls queued during the final turn, and provider-safe tool-call id generation.
 *
 * Uses a real `Agent` with a mock `streamFn`, following the precedent in
 * `packages/coding-agent/tests/agent-session-event-queue.test.ts`.
 */

import { type AssistantMessage, createAssistantMessageEventStream, type Model } from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
import { Agent } from "../src/agent.js";
import type { AgentEvent, AgentTool } from "../src/types.js";

const testModel: Model<"openai-chat"> = {
	id: "test-model",
	name: "Test Model",
	api: "openai-chat",
	provider: "openai",
	baseUrl: "https://api.openai.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};

const secondModel: Model<"openai-chat"> = {
	...testModel,
	id: "second-model",
	name: "Second Model",
};

function makeAssistantMessage(content: AssistantMessage["content"], stopReason: "toolUse" | "stop"): AssistantMessage {
	return {
		role: "assistant",
		content,
		api: "openai-chat",
		provider: "openai",
		model: "test-model",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason,
		timestamp: Date.now(),
	};
}

function makeTextAssistantMessage(text: string): AssistantMessage {
	return makeAssistantMessage([{ type: "text", text }], "stop");
}

type StreamCall = { model: Model<any>; toolCallIds: string[] };

/** Mock stream function that records each call's model and the tool-call ids visible in its context. */
function createRecordingStreamFn(messages: AssistantMessage[], order?: string[]) {
	const calls: StreamCall[] = [];
	let callIndex = 0;
	const fn = ((model: Model<any>, context: { messages: any[] }) => {
		calls.push({
			model,
			toolCallIds: context.messages.flatMap((m) =>
				Array.isArray(m.content)
					? m.content.filter((c: any) => c?.type === "toolCall").map((c: any) => c.id as string)
					: [],
			),
		});
		order?.push(`stream-${callIndex + 1}`);
		const message = messages[callIndex++]!;
		const stream = createAssistantMessageEventStream();
		stream.push({ type: "start", partial: message });
		stream.push({ type: "done", reason: message.stopReason as "stop" | "toolUse", message });
		return stream;
	}) as Agent["streamFn"];
	return { fn, calls };
}

function makeHarnessTool(record: string[], order?: string[]): AgentTool {
	return {
		name: "harness_notice",
		label: "Harness Notice",
		description: "harness-only notice tool",
		parameters: { type: "object", properties: { note: { type: "string" } }, required: ["note"] },
		execute: async (_id, args: any) => {
			record.push(args.note);
			order?.push("harness");
			return { content: [{ type: "text", text: `noted: ${args.note}` }], details: undefined };
		},
	};
}

function makeReadTool(execute: AgentTool["execute"]): AgentTool {
	return {
		name: "read",
		label: "Read",
		description: "read a file",
		parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
		execute,
	};
}

describe("Agent.runHarnessTool", () => {
	it("executes an idle harness tool immediately with no run/turn framing", async () => {
		const record: string[] = [];
		const agent = new Agent({ initialState: { model: testModel, tools: [] } });
		const events: AgentEvent[] = [];
		agent.subscribe((event) => {
			events.push(event);
		});

		await agent.runHarnessTool(makeHarnessTool(record), { note: "hello" });

		expect(record).toEqual(["hello"]);

		const types = events.map((event) => event.type);
		expect(types).toEqual([
			"message_start",
			"message_end",
			"tool_execution_start",
			"tool_execution_end",
			"message_start",
			"message_end",
		]);
		// Explicit invariant: transient runs emit no run/turn framing events, so they are invisible
		// to every agent_end-keyed consumer (probe scheduling, compaction, pr-indicator).
		for (const framing of ["agent_start", "turn_start", "turn_end", "agent_end"]) {
			expect(types).not.toContain(framing);
		}

		// The synthetic assistant tool call and its result are persisted to the transcript.
		const messages = agent.state.messages;
		expect(messages.map((m) => m.role)).toEqual(["assistant", "toolResult"]);
		expect((messages[1] as any).content[0].text).toBe("noted: hello");
	});

	it("drains a mid-run harness tool into the live turn before the next LLM call", async () => {
		const record: string[] = [];
		const order: string[] = [];
		const harnessTool = makeHarnessTool(record, order);
		const { fn, calls } = createRecordingStreamFn(
			[
				makeAssistantMessage(
					[{ type: "toolCall", id: "real-1", name: "read", arguments: { path: "a.ts" } }],
					"toolUse",
				),
				makeTextAssistantMessage("done"),
			],
			order,
		);

		let agentRef!: Agent;
		const readTool = makeReadTool(async () => {
			// Queue a harness tool mid-run; it must not execute until the turn boundary.
			await agentRef.runHarnessTool(harnessTool, { note: "midrun" });
			return { content: [{ type: "text", text: "ok" }], details: undefined };
		});
		const agent = new Agent({
			initialState: { model: testModel, tools: [readTool] },
			streamFn: fn,
			getApiKey: async () => "key",
		});
		agentRef = agent;

		await agent.prompt({ role: "user", content: "go", timestamp: Date.now() });

		expect(record).toEqual(["midrun"]);
		expect(calls).toHaveLength(2);
		// The harness tool executed after the first LLM call and before the second one.
		expect(order).toEqual(["stream-1", "harness", "stream-2"]);
		// The first call's context holds only the user message. The second call's context holds the
		// real tool call plus the drained harness tool call — proving the harness call was spliced in
		// before the next LLM request.
		expect(calls[0]!.toolCallIds).toEqual([]);
		expect(calls[1]!.toolCallIds).toContain("real-1");
		expect(calls[1]!.toolCallIds.some((id) => id.startsWith("harness-tool-"))).toBe(true);
	});

	it("routes the next intra-run LLM call to a model changed mid-run", async () => {
		const { fn, calls } = createRecordingStreamFn([
			makeAssistantMessage(
				[{ type: "toolCall", id: "real-1", name: "read", arguments: { path: "a.ts" } }],
				"toolUse",
			),
			makeTextAssistantMessage("done"),
		]);

		let agentRef!: Agent;
		const readTool = makeReadTool(async () => {
			// Simulate a user- or agent-initiated model switch during the run.
			agentRef.state.model = secondModel;
			return { content: [{ type: "text", text: "ok" }], details: undefined };
		});
		const agent = new Agent({
			initialState: { model: testModel, tools: [readTool] },
			streamFn: fn,
			getApiKey: async () => "key",
		});
		agentRef = agent;

		await agent.prompt({ role: "user", content: "go", timestamp: Date.now() });

		expect(calls).toHaveLength(2);
		expect(calls[0]!.model.id).toBe(testModel.id);
		expect(calls[1]!.model.id).toBe(secondModel.id);
	});

	it("flushes a harness tool queued during the run's final turn", async () => {
		const record: string[] = [];
		const harnessTool = makeHarnessTool(record);
		const { fn } = createRecordingStreamFn([makeTextAssistantMessage("done")]);

		let agentRef!: Agent;
		const agent = new Agent({
			initialState: { model: testModel, tools: [] },
			streamFn: fn,
			getApiKey: async () => "key",
		});
		agentRef = agent;
		// Queue during agent_end — after the last prepareNextTurn drain point — so only the
		// end-of-run flush can deliver it. A run is still active here, so this queues.
		agent.subscribe((event) => {
			if (event.type === "agent_end") {
				void agentRef.runHarnessTool(harnessTool, { note: "flushed" });
			}
		});

		await agent.prompt({ role: "user", content: "go", timestamp: Date.now() });

		expect(record).toEqual(["flushed"]);
		const roles = agent.state.messages.map((m) => m.role);
		// user, assistant(text), then the flushed harness assistant(toolCall) + toolResult.
		expect(roles).toEqual(["user", "assistant", "assistant", "toolResult"]);
	});

	it("supports a harness tool whose execute invokes another harness tool while idle", async () => {
		const record: string[] = [];
		const inner = makeHarnessTool(record);
		let agentRef!: Agent;
		const outer: AgentTool = {
			name: "outer_notice",
			label: "Outer Notice",
			description: "harness tool that invokes another harness tool",
			parameters: { type: "object", properties: {}, required: [] },
			execute: async () => {
				await agentRef.runHarnessTool(inner, { note: "inner" });
				return { content: [{ type: "text", text: "outer done" }], details: undefined };
			},
		};
		const agent = new Agent({ initialState: { model: testModel, tools: [] } });
		agentRef = agent;
		const events: AgentEvent[] = [];
		agent.subscribe((event) => {
			events.push(event);
		});

		await agent.runHarnessTool(outer, {});

		expect(record).toEqual(["inner"]);
		const types = events.map((event) => event.type);
		// Both executions complete with valid event delivery — the nested transient run must not
		// invalidate the outer run's remaining events — and still no run/turn framing.
		expect(types.filter((t) => t === "tool_execution_end")).toHaveLength(2);
		for (const framing of ["agent_start", "turn_start", "turn_end", "agent_end"]) {
			expect(types).not.toContain(framing);
		}
	});

	it("makes a new prompt wait for an in-flight transient harness tool", async () => {
		let release!: () => void;
		const gate = new Promise<void>((resolve) => {
			release = resolve;
		});
		const slowTool: AgentTool = {
			name: "slow_notice",
			label: "Slow Notice",
			description: "harness tool that resolves on an external gate",
			parameters: { type: "object", properties: {}, required: [] },
			execute: async () => {
				await gate;
				return { content: [{ type: "text", text: "slow done" }], details: undefined };
			},
		};
		const { fn } = createRecordingStreamFn([makeTextAssistantMessage("done")]);
		const agent = new Agent({
			initialState: { model: testModel, tools: [] },
			streamFn: fn,
			getApiKey: async () => "key",
		});

		const transient = agent.runHarnessTool(slowTool, {});
		const run = agent.prompt({ role: "user", content: "go", timestamp: Date.now() });
		release();
		await Promise.all([transient, run]);

		// The transient pair fully precedes the run's messages — no interleaving.
		const roles = agent.state.messages.map((m) => m.role);
		expect(roles).toEqual(["assistant", "toolResult", "user", "assistant"]);
	});

	it("flushes queued harness tools even when the run fails", async () => {
		const record: string[] = [];
		const harnessTool = makeHarnessTool(record);
		let agentRef!: Agent;
		const agent = new Agent({
			initialState: { model: testModel, tools: [] },
			streamFn: (() => {
				throw new Error("stream exploded");
			}) as Agent["streamFn"],
			getApiKey: async () => "key",
		});
		agentRef = agent;
		agent.subscribe((event) => {
			if (event.type === "message_start" && event.message.role === "user") {
				// Queued while the run is active; the run then fails via a thrown exception.
				void agentRef.runHarnessTool(harnessTool, { note: "queued-before-failure" });
			}
		});

		await agent.prompt({ role: "user", content: "go", timestamp: Date.now() });

		expect(record).toEqual(["queued-before-failure"]);
		expect(agent.state.errorMessage).toContain("stream exploded");
	});

	it("generates provider-safe tool-call ids and honors a supplied id", async () => {
		const agent = new Agent({ initialState: { model: testModel, tools: [] } });
		const ids: string[] = [];
		agent.subscribe((event) => {
			if (event.type === "tool_execution_start") ids.push(event.toolCallId);
		});

		await agent.runHarnessTool(makeHarnessTool([]), { note: "a" });
		await agent.runHarnessTool(makeHarnessTool([]), { note: "b" });
		await agent.runHarnessTool(makeHarnessTool([]), { note: "c" }, { toolCallId: "custom_id-1" });

		expect(ids).toHaveLength(3);
		// Cross-provider-safe: matches Anthropic's ^[a-zA-Z0-9_-]+$, bounded, no raw model-id chars.
		for (const id of ids.slice(0, 2)) {
			expect(id).toMatch(/^[a-zA-Z0-9_-]+$/);
			expect(id.length).toBeLessThanOrEqual(64);
			expect(id).not.toMatch(/[.:/\s]/);
		}
		// Distinct ids avoid collisions with ids already present in a resumed transcript.
		expect(ids[0]).not.toBe(ids[1]);
		// A supplied id is used verbatim.
		expect(ids[2]).toBe("custom_id-1");
	});

	it("reset() warns with queued harness tool names when discarding the queue", async () => {
		const warnings: string[] = [];
		const originalWarn = console.warn;
		console.warn = (msg?: unknown) => {
			warnings.push(String(msg));
		};
		try {
			const { fn } = createRecordingStreamFn([makeTextAssistantMessage("done")]);
			let agentRef!: Agent;
			const agent = new Agent({
				initialState: { model: testModel, tools: [] },
				streamFn: fn,
				getApiKey: async () => "key",
			});
			agentRef = agent;
			agent.subscribe((event) => {
				if (event.type === "message_start" && event.message.role === "user") {
					// Queue a harness call mid-run, then reset before the turn boundary drains it.
					void agentRef.runHarnessTool(makeHarnessTool([]), { note: "discarded" });
					agentRef.reset();
				}
			});

			await agent.prompt({ role: "user", content: "go", timestamp: Date.now() });
		} finally {
			console.warn = originalWarn;
		}

		// The count identifies how many were lost; the names identify which transcript artifacts.
		expect(warnings.some((w) => w.includes("discarded 1 queued harness tool call"))).toBe(true);
		expect(warnings.some((w) => w.includes("harness_notice"))).toBe(true);
	});
});
