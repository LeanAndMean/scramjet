/**
 * Stage 3 of issue 244: the coding-agent extension surface for harness-tool invocation.
 *
 * Covers the two structural guarantees the surface must provide:
 *
 * 1. A `"harness-only"` tool is registered and resolvable but never enters the LLM-visible tool
 *    set — never in `agent.state.tools`, never in the tools sent to the provider. This is the
 *    no-masquerade guarantee (issue 244 acceptance: "No public notification masquerade").
 * 2. `AgentSession.invokeHarnessTool` executes a registered tool through the real pipeline: it
 *    persists the same assistant-toolCall + toolResult message pair a model-requested call would
 *    (session-ordering, issue test requirement 8), and that pair is present in the context of the
 *    next non-probe LLM completion request (between-turns injection, issue test requirement 4).
 *
 * Issue 341 layers a *persisted*-settlement boundary on top of Stage 1's Agent-core execution
 * settlement: `invokeHarnessTool` resolves only after the matching tool-result `message_end` has
 * been persisted (so no explicit drain is needed), rejects if that persistence fails after the
 * Agent pipeline ran, rejects a pending invocation on dispose, and rejects a post-dispose call
 * before executing anything.
 *
 * The fixture stands up a minimal real `AgentSession` (fake `streamFn`, in-memory managers, a
 * `DefaultResourceLoader` over an empty tmp dir) so the assertions exercise the actual event,
 * persistence, and context-construction paths rather than mocks.
 */

import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Context, Model } from "@leanandmean/ai";
import { createAssistantMessageEventStream } from "@leanandmean/ai";
import { Type } from "typebox";
import { describe, expect, it } from "vitest";
import { AgentSession } from "../src/core/agent-session.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import { defineTool, type ToolDefinition } from "../src/core/extensions/index.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader } from "../src/core/resource-loader.js";
import type { SessionMessageEntry } from "../src/core/session-manager.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";

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

function assistantMessage(content: AssistantMessage["content"], stopReason: "stop" | "toolUse"): AssistantMessage {
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

function assistantText(text: string): AssistantMessage {
	return assistantMessage([{ type: "text", text }], "stop");
}

function assistantToolCall(name: string, id: string, args: Record<string, unknown>): AssistantMessage {
	return assistantMessage([{ type: "toolCall", id, name, arguments: args }], "toolUse");
}

interface Fixture {
	session: AgentSession;
	sessionManager: SessionManager;
	/** One snapshot per streamFn (LLM completion) call. */
	streamContexts: Array<{ model: Model; messages: Context["messages"]; toolNames: string[] }>;
	/** Drain AgentSession's async persistence queue so session entries can be asserted. */
	drain: () => Promise<void>;
}

/**
 * @param responses Optional scripted assistant reply per LLM call (by 0-based call index). When a
 *   call index is unscripted (or no script is supplied), the mock replies with plain "ok" text,
 *   which terminates the run.
 */
async function createFixture(
	customTools: ToolDefinition[] = [],
	responses?: (callIndex: number) => AssistantMessage | undefined,
): Promise<Fixture> {
	const dir = mkdtempSync(join(tmpdir(), "harness-tool-"));
	const cwd = join(dir, "cwd");
	const agentDir = join(dir, "agent");

	const settingsManager = SettingsManager.inMemory();
	const sessionManager = SessionManager.inMemory(cwd);
	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("openai", "fake");
	const modelRegistry = ModelRegistry.create(authStorage, join(agentDir, "models.json"));
	const resourceLoader = new DefaultResourceLoader({ cwd, agentDir, settingsManager });
	await resourceLoader.reload();

	const streamContexts: Fixture["streamContexts"] = [];
	const agent = new Agent({
		initialState: { systemPrompt: "", model: testModel, tools: [] },
		streamFn: (model, context) => {
			const callIndex = streamContexts.length;
			streamContexts.push({
				model,
				messages: [...context.messages],
				toolNames: (context.tools ?? []).map((t) => t.name),
			});
			const message = responses?.(callIndex) ?? assistantText("ok");
			const stream = createAssistantMessageEventStream();
			stream.push({ type: "start", partial: message });
			stream.push({ type: "done", reason: message.stopReason as "stop" | "toolUse", message });
			return stream;
		},
		getApiKey: async () => "fake",
	});

	const session = new AgentSession({
		agent,
		sessionManager,
		settingsManager,
		cwd,
		resourceLoader,
		modelRegistry,
		customTools,
		sessionStartEvent: { type: "session_start", hasUI: false, mode: "sdk" } as never,
	});

	const drain = () => (session as unknown as { _drainAgentEventQueue(): Promise<void> })._drainAgentEventQueue();

	return { session, sessionManager, streamContexts, drain };
}

/** A harness-only tool that records each invocation. */
function makeNoticeTool(): { tool: ToolDefinition; calls: Array<Record<string, unknown>> } {
	const calls: Array<Record<string, unknown>> = [];
	const tool = defineTool({
		name: "harness_notice",
		label: "Harness Notice",
		description: "System-generated notice. Never model-callable.",
		activation: "harness-only",
		parameters: Type.Object({ note: Type.Optional(Type.String()) }),
		execute: async (_id, params) => {
			calls.push(params as Record<string, unknown>);
			return { content: [{ type: "text", text: "noted" }], details: undefined };
		},
	});
	return { tool, calls };
}

/**
 * A harness-only tool whose `execute` blocks on an external gate, so a caller can hold the Agent
 * pipeline mid-execution (invocation pending) and release it deterministically.
 */
function makeGatedTool(): {
	tool: ToolDefinition;
	calls: Array<Record<string, unknown>>;
	started: Promise<void>;
	release: () => void;
} {
	const calls: Array<Record<string, unknown>> = [];
	let release: () => void = () => {};
	const gate = new Promise<void>((resolve) => {
		release = resolve;
	});
	let signalStarted: () => void = () => {};
	const started = new Promise<void>((resolve) => {
		signalStarted = resolve;
	});
	const tool = defineTool({
		name: "harness_gated",
		label: "Harness Gated",
		description: "Harness-only tool that blocks in execute until released.",
		activation: "harness-only",
		parameters: Type.Object({ note: Type.Optional(Type.String()) }),
		execute: async (_id, params) => {
			calls.push(params as Record<string, unknown>);
			signalStarted();
			await gate;
			return { content: [{ type: "text", text: "noted" }], details: undefined };
		},
	});
	return { tool, calls, started, release };
}

async function flushMicrotasks(): Promise<void> {
	for (let i = 0; i < 20; i++) await Promise.resolve();
}

function makeNormalTool(): ToolDefinition {
	return defineTool({
		name: "normal_custom",
		label: "Normal Custom",
		description: "An ordinary model-callable custom tool.",
		parameters: Type.Object({}),
		execute: async () => ({ content: [{ type: "text", text: "done" }], details: undefined }),
	});
}

function hasHarnessToolCall(messages: Context["messages"]): boolean {
	return messages.some(
		(m) => m.role === "assistant" && m.content.some((c) => c.type === "toolCall" && c.name === "harness_notice"),
	);
}

function hasHarnessToolResult(messages: Context["messages"]): boolean {
	return messages.some((m) => m.role === "toolResult" && m.toolName === "harness_notice");
}

describe("AgentSession harness-tool invocation", () => {
	it("keeps a harness-only tool out of the active set and the provider tool list, but resolvable", async () => {
		const { tool: notice } = makeNoticeTool();
		const { session, streamContexts } = await createFixture([notice, makeNormalTool()]);

		// Registered and resolvable via the full registry...
		expect(session.getToolDefinition("harness_notice")).toBeDefined();
		// ...but never auto-activated into the LLM-visible set.
		expect(session.getActiveToolNames()).not.toContain("harness_notice");
		expect(session.getActiveToolNames()).toContain("normal_custom");

		// And never sent to the provider on a real turn.
		await session.prompt("hi");
		expect(streamContexts).toHaveLength(1);
		expect(streamContexts[0].toolNames).not.toContain("harness_notice");
		expect(streamContexts[0].toolNames).toContain("normal_custom");
	});

	it("cannot be activated even when explicitly requested (structural no-masquerade)", async () => {
		const { tool: notice } = makeNoticeTool();
		const { session } = await createFixture([notice]);

		session.setActiveToolsByName(["harness_notice", "read"]);

		expect(session.getActiveToolNames()).not.toContain("harness_notice");
		expect(session.getActiveToolNames()).toContain("read");
	});

	it("stays pending until the tool-result is persisted, then resolves with immediate evidence (no drain)", async () => {
		const { tool: notice, calls } = makeNoticeTool();
		const { session, sessionManager } = await createFixture([notice]);

		await session.prompt("hi");
		expect(session.isStreaming).toBe(false);

		// Gate the AgentSession-side persistence of the harness tool-result. This step runs in the async
		// event queue AFTER Agent-core execution has already settled `runHarnessTool`, so holding it
		// isolates the persisted-settlement boundary (#341): only `invokeHarnessTool` must wait for it.
		// Under the Stage-1-only (resolve-on-Agent-execution) behavior the invocation would resolve here
		// while persistence is still gated, so `resolved` would be true and the pre-release assertions
		// would fail. White-box hook, consistent with this file's existing `_drainAgentEventQueue` access.
		let releasePersist: () => void = () => {};
		const persistGate = new Promise<void>((resolve) => {
			releasePersist = resolve;
		});
		let signalGateReached: () => void = () => {};
		const gateReached = new Promise<void>((resolve) => {
			signalGateReached = resolve;
		});
		const internal = session as unknown as {
			_processAgentEvent(event: { type: string; message?: unknown }): Promise<void>;
		};
		const originalProcess = internal._processAgentEvent.bind(session);
		internal._processAgentEvent = async (event) => {
			const message = event.message as { role?: string; toolName?: string } | undefined;
			if (event.type === "message_end" && message?.role === "toolResult" && message.toolName === "harness_notice") {
				signalGateReached();
				await persistGate;
			}
			return originalProcess(event);
		};

		let resolved = false;
		const invoke = session.invokeHarnessTool("harness_notice", { note: "x" }).then(() => {
			resolved = true;
		});
		// Deterministic barrier: the queue has reached the tool-result persistence step, which means
		// Agent-core execution already settled `runHarnessTool`. A short flush lets that settlement and
		// any resolve-on-execution propagate before we assert.
		await gateReached;
		await flushMicrotasks();

		// Agent execution already ran (the tool executed), but persistence is gated: the invocation is
		// still pending and the tool-result is not yet in the session.
		expect(calls).toEqual([{ note: "x" }]);
		expect(resolved).toBe(false);
		const gated = sessionManager
			.getBranch()
			.filter((e): e is SessionMessageEntry => e.type === "message")
			.map((e) => e.message);
		expect(gated.some((m) => m.role === "toolResult" && m.toolName === "harness_notice")).toBe(false);

		releasePersist();
		await invoke;
		expect(resolved).toBe(true);

		// No explicit drain: settlement already awaited persistence, so the pair is present immediately.
		const messages = sessionManager
			.getBranch()
			.filter((e): e is SessionMessageEntry => e.type === "message")
			.map((e) => e.message);

		const userIdx = messages.findIndex((m) => m.role === "user");
		const toolCallIdx = messages.findIndex(
			(m) =>
				m.role === "assistant" &&
				Array.isArray(m.content) &&
				m.content.some((c) => c.type === "toolCall" && c.name === "harness_notice"),
		);
		const toolResultIdx = messages.findIndex((m) => m.role === "toolResult" && m.toolName === "harness_notice");

		// Session-ordering invariant (issue test requirement 8): a user message precedes the
		// harness tool transcript entry, and the result follows its call.
		expect(userIdx).toBeGreaterThanOrEqual(0);
		expect(toolCallIdx).toBeGreaterThan(userIdx);
		expect(toolResultIdx).toBeGreaterThan(toolCallIdx);
	});

	it("rejects a second invocation reusing a pending explicit tool-call id", async () => {
		const { tool, calls, started, release } = makeGatedTool();
		const { session } = await createFixture([tool]);

		await session.prompt("hi");

		// Hold the first invocation mid-execute so its acknowledgement stays pending, then a second
		// invocation reusing the same explicit id must reject before touching the Agent.
		const first = session.invokeHarnessTool("harness_gated", { note: "first" }, { toolCallId: "dup-id" });
		first.catch(() => {});
		await started;

		await expect(
			session.invokeHarnessTool("harness_gated", { note: "second" }, { toolCallId: "dup-id" }),
		).rejects.toThrow(/already pending/i);
		// The rejected duplicate never executed.
		expect(calls).toEqual([{ note: "first" }]);

		release();
		await expect(first).resolves.toBeUndefined();
	});

	it("rejects the invocation when persisting the tool-result fails, after Agent execution ran", async () => {
		const { tool: notice, calls } = makeNoticeTool();
		const { session, sessionManager } = await createFixture([notice]);

		await session.prompt("hi");

		const persistError = new Error("appendMessage boom");
		const original = sessionManager.appendMessage.bind(sessionManager);
		sessionManager.appendMessage = ((message: Parameters<typeof original>[0]) => {
			if (message.role === "toolResult" && message.toolName === "harness_notice") {
				throw persistError;
			}
			return original(message);
		}) as typeof original;

		// Agent-core execution succeeds (the tool runs), but persisting the matching tool-result throws;
		// the invocation rejects with that error. Rejection is not artifact absence — the tool DID run.
		await expect(session.invokeHarnessTool("harness_notice", { note: "boom" })).rejects.toBe(persistError);
		expect(calls).toEqual([{ note: "boom" }]);
	});

	it("rejects a pending invocation on dispose and refuses post-dispose invocations", async () => {
		const { tool, calls, started, release } = makeGatedTool();
		const { session } = await createFixture([tool]);

		await session.prompt("hi");

		const pending = session.invokeHarnessTool("harness_gated", { note: "pending" });
		pending.catch(() => {});
		await started;

		session.dispose();
		await expect(pending).rejects.toThrow(/disposed/i);

		// A post-dispose invocation rejects before executing anything.
		const callsBefore = calls.length;
		await expect(session.invokeHarnessTool("harness_gated", { note: "after" })).rejects.toThrow(/disposed/i);
		expect(calls.length).toBe(callsBefore);

		// Release the underlying (now-orphaned) work so the transient run finishes and nothing leaks.
		release();
		await flushMicrotasks();
	});

	it("places the harness toolCall/result in the next LLM completion context (between-turns)", async () => {
		const { tool: notice } = makeNoticeTool();
		const { session, streamContexts } = await createFixture([notice]);

		await session.prompt("hi"); // streamContexts[0]
		await session.invokeHarnessTool("harness_notice", { note: "between" }); // idle
		await session.prompt("again"); // streamContexts[1]

		expect(streamContexts).toHaveLength(2);
		// The first turn's context predates the harness call and must not contain it.
		expect(hasHarnessToolCall(streamContexts[0].messages)).toBe(false);
		// Issue test requirement 4: the next provider request carries the harness tool artifact.
		expect(hasHarnessToolCall(streamContexts[1].messages)).toBe(true);
		expect(hasHarnessToolResult(streamContexts[1].messages)).toBe(true);
	});

	it("delivers a mid-run notice and routes the next intra-run call to a model changed during the run", async () => {
		// End-to-end mid-run scenario (issue 244 Scenario 5 / test requirement 3) at the AgentSession
		// surface: while a run is streaming, a user-style model change plus a harness notice are issued
		// from inside a tool. The notice must be spliced into the next intra-run provider request and
		// that request must route to the newly selected model — verified through the real session,
		// registry, and routing paths rather than a bare Agent.
		const { tool: notice, calls } = makeNoticeTool();
		const secondModel: Model<"openai-chat"> = { ...testModel, id: "second-model", name: "Second Model" };

		let sessionRef!: AgentSession;
		let noticePromise: Promise<void> | undefined;
		const trigger = defineTool({
			name: "trigger_switch",
			label: "Trigger Switch",
			description: "A model-callable tool that switches the model and fires a notice mid-run.",
			parameters: Type.Object({}),
			execute: async () => {
				// Simulate the harness reacting to a mid-run user model change: the model is switched
				// through the canonical session path, and the change is narrated via a harness notice.
				// The notice is a mid-run queued call that cannot drain until this tool returns, so we
				// capture its promise WITHOUT awaiting it here (an inline await would deadlock, #341) and
				// await it externally after the run progresses.
				await sessionRef.setModel(secondModel);
				noticePromise = sessionRef.invokeHarnessTool("harness_notice", { note: "midrun" });
				noticePromise.catch(() => {});
				return { content: [{ type: "text", text: "switched" }], details: undefined };
			},
		});

		const { session, streamContexts } = await createFixture([notice, trigger], (callIndex) =>
			// First call: ask to run trigger_switch. Second call: finish with plain text.
			callIndex === 0 ? assistantToolCall("trigger_switch", "call-1", {}) : undefined,
		);
		sessionRef = session;

		await session.prompt("go");
		// The mid-run notice drained during the run; its promise now settles.
		await expect(noticePromise).resolves.toBeUndefined();

		// The notice actually executed, exactly once.
		expect(calls).toEqual([{ note: "midrun" }]);

		// Two provider calls: the initial one on the original model, the continuation on the new one.
		expect(streamContexts).toHaveLength(2);
		expect(streamContexts[0].model.id).toBe(testModel.id);
		// Routing self-heal: the intra-run continuation goes to the model selected mid-run.
		expect(streamContexts[1].model.id).toBe(secondModel.id);
		// The harness notice pair is present in that continuation's context — before the next LLM call,
		// not deferred to a later user turn.
		expect(hasHarnessToolCall(streamContexts[1].messages)).toBe(true);
		expect(hasHarnessToolResult(streamContexts[1].messages)).toBe(true);
	});

	it("rejects an unknown tool name without persisting anything", async () => {
		const { tool: notice } = makeNoticeTool();
		const { session, sessionManager, drain } = await createFixture([notice]);

		await session.prompt("hi");
		await drain();
		const before = sessionManager.getBranch().length;

		await expect(session.invokeHarnessTool("does_not_exist", {})).rejects.toThrow(
			/no tool with that name is registered/i,
		);

		await drain();
		expect(sessionManager.getBranch().length).toBe(before);
	});
});
