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
		let harnessPromise!: Promise<void>;
		const readTool = makeReadTool(async () => {
			// Capture the promise mid-run WITHOUT awaiting it: the queued call cannot drain until this
			// tool returns, so an inline await would deadlock. Record its settlement into the shared
			// timeline — it must land AFTER the tool executes ("harness"), proving the promise resolves
			// on execution and not at enqueue time.
			harnessPromise = agentRef.runHarnessTool(harnessTool, { note: "midrun" });
			harnessPromise.then(
				() => order.push("harness-settled"),
				() => order.push("harness-settled"),
			);
			return { content: [{ type: "text", text: "ok" }], details: undefined };
		});
		const agent = new Agent({
			initialState: { model: testModel, tools: [readTool] },
			streamFn: fn,
			getApiKey: async () => "key",
		});
		agentRef = agent;

		await agent.prompt({ role: "user", content: "go", timestamp: Date.now() });
		// Settlement (#341): the promise resolves only after the queued call actually executes.
		await expect(harnessPromise).resolves.toBeUndefined();

		// The promise settled only AFTER the queued call executed — not at enqueue time. A regression
		// that resolved at enqueue would push "harness-settled" before "harness".
		expect(order.indexOf("harness-settled")).toBeGreaterThan(order.indexOf("harness"));
		expect(record).toEqual(["midrun"]);
		expect(calls).toHaveLength(2);
		// The harness tool executed after the first LLM call and before the second one.
		expect(order.filter((e) => e !== "harness-settled")).toEqual(["stream-1", "harness", "stream-2"]);
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

	it("reset() rejects a queued harness tool and warns with its name", async () => {
		const warnings: string[] = [];
		const originalWarn = console.warn;
		console.warn = (msg?: unknown) => {
			warnings.push(String(msg));
		};
		let queuedPromise!: Promise<void>;
		let rejection: unknown;
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
					// Queue a harness call mid-run, observe its rejection immediately (before reset can
					// reject it), then reset before the turn boundary drains it.
					queuedPromise = agentRef.runHarnessTool(makeHarnessTool([]), { note: "discarded" });
					queuedPromise.then(
						() => {},
						(err) => {
							rejection = err;
						},
					);
					agentRef.reset();
				}
			});

			await agent.prompt({ role: "user", content: "go", timestamp: Date.now() });
		} finally {
			console.warn = originalWarn;
		}

		// The queued call's promise rejects rather than silently resolving — an awaiting caller sees the
		// discard (#341).
		await expect(queuedPromise).rejects.toThrow(/discarded unsettled harness tool call/);
		expect(rejection).toBeInstanceOf(Error);
		// The count identifies how many were lost; the names identify which transcript artifacts.
		expect(warnings.some((w) => w.includes("discarded 1 unsettled harness tool call"))).toBe(true);
		expect(warnings.some((w) => w.includes("harness_notice"))).toBe(true);
	});

	it("rejects an in-flight transient harness tool on reset yet completes the underlying work", async () => {
		const originalWarn = console.warn;
		console.warn = () => {};
		let release!: () => void;
		const gate = new Promise<void>((resolve) => {
			release = resolve;
		});
		let executed = false;
		const slowTool: AgentTool = {
			name: "slow_notice",
			label: "Slow Notice",
			description: "harness tool that resolves on an external gate",
			parameters: { type: "object", properties: {}, required: [] },
			execute: async () => {
				await gate;
				executed = true;
				return { content: [{ type: "text", text: "slow done" }], details: undefined };
			},
		};
		const agent = new Agent({ initialState: { model: testModel, tools: [] } });
		// The underlying transient work finishes with the tool-result message_end even after the
		// public promise is revoked; observe that event deterministically.
		let underlyingDone!: () => void;
		const underlyingComplete = new Promise<void>((resolve) => {
			underlyingDone = resolve;
		});
		agent.subscribe((event) => {
			if (event.type === "message_end" && event.message.role === "toolResult") underlyingDone();
		});

		try {
			// Idle transient run (not queued). Observe its rejection immediately, then reset while it is
			// gated — outside any queue.
			const transient = agent.runHarnessTool(slowTool, {});
			const rejection = expect(transient).rejects.toThrow(/discarded unsettled harness tool call/);
			agent.reset();
			await rejection;

			// The public guarantee was revoked, but the underlying work still runs to completion.
			release();
			await underlyingComplete;
			expect(executed).toBe(true);
		} finally {
			console.warn = originalWarn;
		}
	});

	it("fulfills the promise (with an isError result) when the tool's execute throws", async () => {
		const throwingTool: AgentTool = {
			name: "boom",
			label: "Boom",
			description: "harness tool whose execute throws",
			parameters: { type: "object", properties: {}, required: [] },
			execute: async () => {
				throw new Error("tool exploded");
			},
		};
		const agent = new Agent({ initialState: { model: testModel, tools: [] } });

		// A tool's own execute() error is not an infrastructure failure: the promise resolves.
		await expect(agent.runHarnessTool(throwingTool, {})).resolves.toBeUndefined();

		const result = agent.state.messages.find((m) => m.role === "toolResult");
		expect(result).toBeDefined();
		expect((result as any).isError).toBe(true);
		expect((result as any).content[0].text).toContain("tool exploded");
	});

	it("rejects the failing and remnant final-flush calls without erasing partial artifacts", async () => {
		const originalWarn = console.warn;
		console.warn = () => {};
		const { fn } = createRecordingStreamFn([makeTextAssistantMessage("done")]);
		let agentRef!: Agent;
		const agent = new Agent({
			initialState: { model: testModel, tools: [] },
			streamFn: fn,
			getApiKey: async () => "key",
		});
		agentRef = agent;

		const makeNamed = (name: string): AgentTool => ({
			name,
			label: name,
			description: "harness tool",
			parameters: { type: "object", properties: {}, required: [] },
			execute: async () => ({ content: [{ type: "text", text: "ok" }], details: undefined }),
		});

		let listenerFailed = false;
		let p1!: Promise<void>;
		let p2!: Promise<void>;
		agent.subscribe((event) => {
			// Queue two calls during the run's final turn (agent_end), each observed immediately.
			if (event.type === "agent_end") {
				p1 = agentRef.runHarnessTool(makeNamed("harness_a"), {});
				p1.catch(() => {});
				p2 = agentRef.runHarnessTool(makeNamed("harness_b"), {});
				p2.catch(() => {});
				return;
			}
			// Force an infrastructure failure on the first call's tool-result event: a listener throw
			// escapes the pipeline during the final flush.
			if (
				event.type === "message_end" &&
				event.message.role === "toolResult" &&
				event.message.toolName === "harness_a" &&
				!listenerFailed
			) {
				listenerFailed = true;
				throw new Error("listener exploded");
			}
		});

		// The run's promise rejects with the drain failure — the failure remains observable.
		await expect(agent.prompt({ role: "user", content: "go", timestamp: Date.now() })).rejects.toThrow(
			"listener exploded",
		);

		// The failing call and the un-executed remnant both reject.
		await expect(p1).rejects.toThrow("listener exploded");
		await expect(p2).rejects.toThrow("listener exploded");

		// harness_b never ran (its record was rejected as a remnant), but harness_a's assistant
		// tool-call message was already emitted before the failure — rejection is not artifact absence.
		expect(
			agent.state.messages.some(
				(m) => m.role === "assistant" && m.content.some((c) => c.type === "toolCall" && c.name === "harness_a"),
			),
		).toBe(true);
		expect(agent.state.messages.some((m) => m.role === "toolResult" && m.toolName === "harness_b")).toBe(false);

		console.warn = originalWarn;
	});
});
