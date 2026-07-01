import type { AssistantMessage, ToolCall, ToolResultMessage } from "@leanandmean/ai";
import type { SessionEntry } from "@leanandmean/coding-agent";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
	buildNotificationPair,
	buildNotificationToolCall,
	MAX_STABILITY_WAIT_MS,
	reconstructModelState,
	registerModelIdentity,
	STABILITY_MS,
	waitForModelStable,
} from "../src/model-identity.js";
import { freshState, lifecycleFor, recordingPi } from "./helpers.js";

function fakeModel(overrides: Record<string, unknown> = {}) {
	return {
		id: "claude-opus-4-6",
		name: "Claude Opus 4.6",
		provider: "anthropic",
		...overrides,
	} as any;
}

function setup() {
	const { pi, tools, handlers, emit } = recordingPi();
	const state = freshState();
	registerModelIdentity(pi, state);
	return { pi, tools, handlers, emit, state };
}

function fakeAssistantMessage(toolCalls: ToolCall[] = [], text = ""): AssistantMessage {
	const content: AssistantMessage["content"] = [];
	if (text) content.push({ type: "text", text });
	content.push(...toolCalls);
	return {
		role: "assistant",
		content,
		api: "messages",
		provider: "anthropic",
		model: "claude-opus-4-6",
		usage: {
			input: 0,
			output: 0,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: toolCalls.length > 0 ? "toolUse" : "stop",
		timestamp: Date.now(),
	};
}

function fakeToolCall(name = "bash", id = "tc-1"): ToolCall {
	return { type: "toolCall", id, name, arguments: { command: "ls" } };
}

describe("registerModelIdentity", () => {
	it("registers session_start, before_agent_start, turn_start, model_select, message_end, and prepare_next_turn handlers", () => {
		const { handlers } = setup();
		expect(handlers.get("session_start")).toHaveLength(1);
		expect(handlers.get("before_agent_start")).toHaveLength(1);
		expect(handlers.get("turn_start")).toHaveLength(1);
		expect(handlers.get("model_select")).toHaveLength(1);
		expect(handlers.get("message_end")).toHaveLength(1);
		expect(handlers.get("prepare_next_turn")).toHaveLength(1);
	});

	it("does not register an input handler", () => {
		const { handlers } = setup();
		expect(handlers.get("input")).toBeUndefined();
	});

	it("registers the notify_model_change tool", () => {
		const { tools } = setup();
		const tool = tools.find((t: any) => t.name === "notify_model_change");
		expect(tool).toBeDefined();
		expect(tool.parameters.required).toEqual(["model_name", "model_id", "provider"]);
	});

	it("captures the initial model on session_start", async () => {
		const { emit, state } = setup();

		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });

		expect(state.currentModel).toEqual({
			name: "Claude Opus 4.6",
			id: "claude-opus-4-6",
			provider: "anthropic",
			fromTurnIndex: 0,
		});
		expect(state.modelHistory).toEqual([state.currentModel]);
	});

	it("leaves model state empty and omits the prompt block when ctx.model is undefined", async () => {
		const { handlers, emit, state } = setup();

		await emit("session_start", { type: "session_start", reason: "startup" }, { model: undefined });
		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as Record<string, unknown>;

		expect(state.currentModel).toBeNull();
		expect(state.modelHistory).toEqual([]);
		expect(result).toEqual({});
	});

	it("appends stable model identity context on every before_agent_start", async () => {
		const { handlers, emit } = setup();

		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		const handler = handlers.get("before_agent_start")![0];
		const first = (await handler({ systemPrompt: "BASE" })) as any;
		await emit("turn_start", { type: "turn_start", turnIndex: 3, timestamp: 123 });
		const second = (await handler({ systemPrompt: "BASE" })) as any;

		expect(first).toEqual(second);
		expect(Object.keys(first)).toEqual(["systemPromptSection"]);
		expect(first.systemPromptSection.id).toBe("scramjet:model-identity");
		expect(first.systemPromptSection.text).toContain("\n\n# Model Identity");
		expect(first.systemPromptSection.text).toContain(
			"Your model is: Claude Opus 4.6 (ID: claude-opus-4-6, provider: anthropic).",
		);
		expect(first.systemPromptSection.text).toContain("tool calls named notify_model_change");
		expect(first.systemPromptSection.text).toContain('Single model: "Reviewed by Claude Opus 4.6"');
	});

	it("does not duplicate model history on later turn_start events", async () => {
		const { emit, state } = setup();

		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 1, timestamp: 123 });
		await emit("turn_start", { type: "turn_start", turnIndex: 2, timestamp: 456 });

		expect(state.modelHistory).toEqual([state.currentModel]);
		expect(state.currentModel?.fromTurnIndex).toBe(0);
	});
});

// ─── Mandatory acceptance tests (7) ───────────────────────────────────────────

describe("mandatory acceptance tests", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	const gpt5 = fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" });

	async function initAndStartTurn(emit: any, model = fakeModel()) {
		await emit("session_start", { type: "session_start", reason: "startup" }, { model });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
	}

	it("mid-run injection: model change tool call injected into assistant message with tool calls", async () => {
		const { handlers, emit, state } = setup();
		await initAndStartTurn(emit);

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		expect(state.pendingModelChange).not.toBeNull();

		const msgHandler = handlers.get("message_end")![0];
		const assistantMsg = fakeAssistantMessage([fakeToolCall()], "Here are results");
		const result = (await msgHandler({ type: "message_end", message: assistantMsg })) as any;

		expect(result).toBeDefined();
		expect(result.message.role).toBe("assistant");
		const injected = result.message.content.find(
			(b: any) => b.type === "toolCall" && b.name === "notify_model_change",
		);
		expect(injected).toBeDefined();
		expect(injected.arguments.model_name).toBe("GPT 5.5");
		expect(injected.arguments.model_id).toBe("gpt-5-5");
		expect(injected.arguments.provider).toBe("openai");
		expect(state.pendingModelChange).toBeNull();
	});

	it("between-turns injection: model change delivered via preTurnMessages in before_agent_start", async () => {
		const { handlers, emit, state } = setup();
		await initAndStartTurn(emit);

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		expect(state.pendingModelChange).not.toBeNull();

		const basHandler = handlers.get("before_agent_start")![0];
		const result = (await basHandler({ systemPrompt: "BASE" })) as any;

		expect(result.preTurnMessages).toBeDefined();
		expect(result.preTurnMessages).toHaveLength(2);
		const [assistantMsg, resultMsg] = result.preTurnMessages as [AssistantMessage, ToolResultMessage];
		expect(assistantMsg.role).toBe("assistant");
		expect(assistantMsg.content[0].type).toBe("toolCall");
		expect((assistantMsg.content[0] as ToolCall).name).toBe("notify_model_change");
		expect(resultMsg.role).toBe("toolResult");
		expect(resultMsg.toolName).toBe("notify_model_change");
		expect(resultMsg.content[0]).toEqual(expect.objectContaining({ type: "text" }));
		expect(state.pendingModelChange).toBeNull();
	});

	it("coalescing: multiple rapid model changes produce exactly one notification", async () => {
		const { handlers, emit, state } = setup();
		await initAndStartTurn(emit);

		const mid = fakeModel({ id: "mid-model", name: "Mid", provider: "mid" });
		await emit("model_select", { type: "model_select", model: mid, previousModel: fakeModel(), source: "cycle" });
		vi.advanceTimersByTime(200);
		await emit("model_select", { type: "model_select", model: gpt5, previousModel: mid, source: "cycle" });
		vi.advanceTimersByTime(200);
		const model3 = fakeModel({ id: "model-3", name: "Model 3", provider: "test" });
		await emit("model_select", { type: "model_select", model: model3, previousModel: gpt5, source: "cycle" });
		vi.advanceTimersByTime(STABILITY_MS);

		expect(state.pendingModelChange).not.toBeNull();
		expect(state.pendingModelChange!.id).toBe("model-3");

		const basHandler = handlers.get("before_agent_start")![0];
		const result = (await basHandler({ systemPrompt: "BASE" })) as any;

		expect(result.preTurnMessages).toHaveLength(2);
		const toolCall = (result.preTurnMessages[0] as AssistantMessage).content[0] as ToolCall;
		expect(toolCall.arguments.model_id).toBe("model-3");
		expect(toolCall.arguments.model_name).toBe("Model 3");
	});

	it("pre-first-turn: model change before turn_start updates system prompt, no tool call", async () => {
		const { handlers, emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });

		// Model change BEFORE any turn_start
		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });

		expect(state.pendingModelChange).toBeNull();

		const basHandler = handlers.get("before_agent_start")![0];
		const result = (await basHandler({ systemPrompt: "BASE" })) as any;

		expect(result.preTurnMessages).toBeUndefined();
		expect(result.systemPromptSection.text).toContain("GPT 5.5");
		expect(result.systemPromptSection.text).toContain("gpt-5-5");
	});

	it("probe safety: model change during probe is not delivered until probe completes", async () => {
		const { handlers, emit, state } = setup();
		await initAndStartTurn(emit);
		state.lifecycle = lifecycleFor("probing");

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		// before_agent_start during probe: no preTurnMessages
		const basHandler = handlers.get("before_agent_start")![0];
		const probeResult = (await basHandler({ systemPrompt: "BASE" })) as any;
		expect(probeResult.preTurnMessages).toBeUndefined();
		expect(state.pendingModelChange).not.toBeNull();

		// message_end during probe: no injection
		const msgHandler = handlers.get("message_end")![0];
		const probeMsg = fakeAssistantMessage([fakeToolCall()]);
		const msgResult = (await msgHandler({ type: "message_end", message: probeMsg })) as any;
		expect(msgResult).toBeUndefined();
		expect(state.pendingModelChange).not.toBeNull();

		// Probe completes — next before_agent_start delivers
		state.lifecycle = lifecycleFor("running");
		const postProbe = (await basHandler({ systemPrompt: "BASE" })) as any;
		expect(postProbe.preTurnMessages).toHaveLength(2);
		expect(state.pendingModelChange).toBeNull();
	});

	it("session ordering: no tool call fires before the first user message exists", async () => {
		const { handlers, emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });

		// Model change BEFORE turn_start (no user message yet)
		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });

		// before_agent_start should NOT produce preTurnMessages
		const basHandler = handlers.get("before_agent_start")![0];
		const result = (await basHandler({ systemPrompt: "BASE" })) as any;
		expect(result.preTurnMessages).toBeUndefined();
		expect(state.pendingModelChange).toBeNull();

		// After turn_start (user message exists), model change should produce preTurnMessages
		await emit("turn_start", { type: "turn_start", turnIndex: 1, timestamp: 123 });
		const model3 = fakeModel({ id: "model-3", name: "Model 3", provider: "test" });
		await emit("model_select", { type: "model_select", model: model3, previousModel: gpt5, source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		const result2 = (await basHandler({ systemPrompt: "BASE" })) as any;
		expect(result2.preTurnMessages).toHaveLength(2);
	});

	it("no user-message delivery: no input handler registered, delivery is tool calls only", async () => {
		const { handlers, emit } = setup();
		await initAndStartTurn(emit);

		// No input handler exists
		expect(handlers.get("input")).toBeUndefined();

		// before_agent_start returns preTurnMessages (not message)
		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		const basHandler = handlers.get("before_agent_start")![0];
		const result = (await basHandler({ systemPrompt: "BASE" })) as any;
		expect(result.message).toBeUndefined();
		expect(result.preTurnMessages).toBeDefined();
		expect(result.preTurnMessages[0].role).toBe("assistant");
		expect(result.preTurnMessages[1].role).toBe("toolResult");
	});
});

// ─── Supporting tests ─────────────────────────────────────────────────────────

describe("message_end injection guards", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	const gpt5 = fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" });

	async function initAndStartTurn(emit: any) {
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
	}

	it("skips when no pending model change", async () => {
		const { handlers, emit, state } = setup();
		await initAndStartTurn(emit);
		expect(state.pendingModelChange).toBeNull();

		const msgHandler = handlers.get("message_end")![0];
		const result = await msgHandler({ type: "message_end", message: fakeAssistantMessage([fakeToolCall()]) });
		expect(result).toBeUndefined();
	});

	it("skips non-assistant messages", async () => {
		const { handlers, emit, state } = setup();
		await initAndStartTurn(emit);
		state.pendingModelChange = { name: "GPT 5.5", id: "gpt-5-5", provider: "openai", fromTurnIndex: 0 };

		const msgHandler = handlers.get("message_end")![0];
		const result = await msgHandler({ type: "message_end", message: { role: "user", content: "hi" } });
		expect(result).toBeUndefined();
		expect(state.pendingModelChange).not.toBeNull();
	});

	it("skips text-only assistant messages (no tool calls), preserves pending", async () => {
		const { handlers, emit, state } = setup();
		await initAndStartTurn(emit);

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		const msgHandler = handlers.get("message_end")![0];
		const textOnlyMsg = fakeAssistantMessage([], "Just some text");
		const result = await msgHandler({ type: "message_end", message: textOnlyMsg });
		expect(result).toBeUndefined();
		expect(state.pendingModelChange).not.toBeNull();
	});

	it("injects at end of content array", async () => {
		const { handlers, emit } = setup();
		await initAndStartTurn(emit);

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		const msgHandler = handlers.get("message_end")![0];
		const tc1 = fakeToolCall("bash", "tc-1");
		const tc2 = fakeToolCall("read", "tc-2");
		const assistantMsg = fakeAssistantMessage([tc1, tc2], "Working on it");
		const result = (await msgHandler({ type: "message_end", message: assistantMsg })) as any;

		// Original blocks preserved in order, notification appended
		expect(result.message.content[0]).toEqual({ type: "text", text: "Working on it" });
		expect(result.message.content[1]).toBe(tc1);
		expect(result.message.content[2]).toBe(tc2);
		expect(result.message.content[3].type).toBe("toolCall");
		expect(result.message.content[3].name).toBe("notify_model_change");
	});

	it("injected tool call has scrmdl- prefix ID and correct arguments", async () => {
		const { handlers, emit } = setup();
		await initAndStartTurn(emit);

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		const msgHandler = handlers.get("message_end")![0];
		const result = (await msgHandler({
			type: "message_end",
			message: fakeAssistantMessage([fakeToolCall()]),
		})) as any;

		const injected = result.message.content.find((b: any) => b.name === "notify_model_change");
		expect(injected.id).toMatch(/^scrmdl-/);
		expect(injected.arguments).toEqual({ model_name: "GPT 5.5", model_id: "gpt-5-5", provider: "openai" });
	});
});

describe("prepare_next_turn handler", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	const gpt5 = fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" });

	async function initAndStartTurn(emit: any) {
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
	}

	it("delivers pending model change via messages", async () => {
		const { handlers, emit, state } = setup();
		await initAndStartTurn(emit);

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		const pntHandler = handlers.get("prepare_next_turn")![0];
		const result = (await pntHandler({ type: "prepare_next_turn" })) as any;

		expect(result).toBeDefined();
		expect(result.messages).toHaveLength(2);
		expect(result.messages[0].role).toBe("assistant");
		expect(result.messages[1].role).toBe("toolResult");
		expect(state.pendingModelChange).toBeNull();
		expect(state.currentModel!.id).toBe("gpt-5-5");
	});

	it("returns undefined when no pending change", async () => {
		const { handlers, emit } = setup();
		await initAndStartTurn(emit);

		const pntHandler = handlers.get("prepare_next_turn")![0];
		const result = await pntHandler({ type: "prepare_next_turn" });
		expect(result).toBeUndefined();
	});

	it("skips during probe (preserves pending)", async () => {
		const { handlers, emit, state } = setup();
		await initAndStartTurn(emit);
		state.lifecycle = lifecycleFor("probing");

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		const pntHandler = handlers.get("prepare_next_turn")![0];
		const result = await pntHandler({ type: "prepare_next_turn" });
		expect(result).toBeUndefined();
		expect(state.pendingModelChange).not.toBeNull();
	});

	it("waits for stability before delivering", async () => {
		const { handlers, emit, state } = setup();
		await initAndStartTurn(emit);

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		// Don't advance timers — stability not yet reached

		const pntHandler = handlers.get("prepare_next_turn")![0];
		const promise = pntHandler({ type: "prepare_next_turn" });
		vi.advanceTimersByTime(STABILITY_MS);
		const result = (await promise) as any;

		expect(result.messages).toHaveLength(2);
		expect(state.pendingModelChange).toBeNull();
	});
});

describe("tool execution", () => {
	it("returns structured content with terminate: true", async () => {
		const { tools } = setup();
		const tool = tools.find((t: any) => t.name === "notify_model_change");
		const result = await tool.execute("call-1", { model_name: "GPT 5.5", model_id: "gpt-5-5", provider: "openai" });

		expect(result.content).toHaveLength(1);
		expect(result.content[0].type).toBe("text");
		expect(result.content[0].text).toContain("Model changed to: GPT 5.5");
		expect(result.content[0].text).toContain("gpt-5-5");
		expect(result.content[0].text).toContain("openai");
		expect(result.terminate).toBe(true);
	});
});

describe("before_agent_start fallback delivery", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	const gpt5 = fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" });

	it("delivers via preTurnMessages and clears pending", async () => {
		const { handlers, emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		const basHandler = handlers.get("before_agent_start")![0];
		const result = (await basHandler({ systemPrompt: "BASE" })) as any;

		expect(result.preTurnMessages).toHaveLength(2);
		expect(state.pendingModelChange).toBeNull();

		// Second call: no preTurnMessages
		const result2 = (await basHandler({ systemPrompt: "BASE" })) as any;
		expect(result2.preTurnMessages).toBeUndefined();
	});

	it("skips during probe (preserves pending)", async () => {
		const { handlers, emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
		state.lifecycle = lifecycleFor("probing");

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		const basHandler = handlers.get("before_agent_start")![0];
		const result = (await basHandler({ systemPrompt: "BASE" })) as any;

		expect(result.preTurnMessages).toBeUndefined();
		expect(state.pendingModelChange).not.toBeNull();
	});
});

// ─── model_select debounce and state ──────────────────────────────────────────

describe("model_select behavior", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	const gpt5 = fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" });

	it("sets pendingModelChange immediately (no debounce on pending)", async () => {
		const { emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });

		expect(state.pendingModelChange).not.toBeNull();
		expect(state.pendingModelChange!.id).toBe("gpt-5-5");
	});

	it("stamps lastModelSelectTime on every model_select", async () => {
		const { emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });

		const before = Date.now();
		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });

		expect(state.lastModelSelectTime).toBeGreaterThanOrEqual(before);
	});

	it("collapses rapid cycling to only the final model", async () => {
		const { emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });

		const mid = fakeModel({ id: "mid-model", name: "Mid", provider: "mid" });
		await emit("model_select", { type: "model_select", model: mid, previousModel: fakeModel(), source: "cycle" });
		await emit("model_select", { type: "model_select", model: gpt5, previousModel: mid, source: "cycle" });

		expect(state.pendingModelChange!.id).toBe("gpt-5-5");
	});

	it("suppresses no-op selection (same as current)", async () => {
		const { emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });

		await emit("model_select", {
			type: "model_select",
			model: fakeModel(),
			previousModel: fakeModel(),
			source: "set",
		});
		expect(state.pendingModelChange).toBeNull();
	});

	it("clears pending when cycling back to current model (A→B→A)", async () => {
		const { emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "cycle" });
		expect(state.pendingModelChange!.id).toBe("gpt-5-5");

		// Cycle back to original model
		await emit("model_select", { type: "model_select", model: fakeModel(), previousModel: gpt5, source: "cycle" });
		expect(state.pendingModelChange).toBeNull();
		expect(state.currentModel!.id).toBe("claude-opus-4-6");
	});

	it("suppresses source === 'restore' events", async () => {
		const { emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "restore" });
		expect(state.pendingModelChange).toBeNull();
		expect(state.lastModelSelectTime).toBe(0);
	});

	it("assigns correct fromTurnIndex to pending model record", async () => {
		const { emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 5, timestamp: 999 });

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		expect(state.pendingModelChange?.fromTurnIndex).toBe(5);
	});

	it("keeps system prompt showing initial model after model change (cache-friendly)", async () => {
		const { handlers, emit } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });

		const basHandler = handlers.get("before_agent_start")![0];
		const before = (await basHandler({ systemPrompt: "BASE" })) as any;

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(STABILITY_MS);

		// Deliver the pending change
		await basHandler({ systemPrompt: "BASE" });

		// After delivery, system prompt still shows initial model
		const after = (await basHandler({ systemPrompt: "BASE" })) as any;
		expect(after.systemPromptSection.text).toBe(before.systemPromptSection.text);
		expect(after.systemPromptSection.text).toContain("Claude Opus 4.6");
		expect(after.systemPromptSection.text).not.toContain("GPT 5.5");
	});
});

// ─── Pre-first-turn behavior ─────────────────────────────────────────────────

describe("pre-first-turn model change", () => {
	it("updates initialModel directly, no pendingModelChange", async () => {
		const { handlers, emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });

		const gpt5 = fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" });
		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });

		expect(state.pendingModelChange).toBeNull();
		expect(state.currentModel?.id).toBe("gpt-5-5");

		const basHandler = handlers.get("before_agent_start")![0];
		const result = (await basHandler({ systemPrompt: "BASE" })) as any;
		expect(result.preTurnMessages).toBeUndefined();
		expect(result.systemPromptSection.text).toContain("GPT 5.5");
	});

	it("firstTurnStarted is cleared on rebuild", async () => {
		const { handlers, emit } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 1, timestamp: 123 });

		// Rebuild via resume
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const ctx = { sessionManager: { getBranch: () => entries }, model: fakeModel() };
		await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

		// Model change after rebuild but before turn_start → pre-first-turn again
		const gpt5 = fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" });
		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });

		const basHandler = handlers.get("before_agent_start")![0];
		const result = (await basHandler({ systemPrompt: "BASE" })) as any;
		expect(result.preTurnMessages).toBeUndefined();
		expect(result.systemPromptSection.text).toContain("GPT 5.5");
	});
});

// ─── Stability gate ──────────────────────────────────────────────────────────

describe("waitForModelStable", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("resolves immediately when stable", async () => {
		const state = freshState();
		state.lastModelSelectTime = Date.now() - STABILITY_MS - 1;
		const promise = waitForModelStable(state);
		await vi.advanceTimersByTimeAsync(0);
		await promise;
	});

	it("waits until 500ms of silence", async () => {
		const state = freshState();
		state.lastModelSelectTime = Date.now();
		let resolved = false;
		const promise = waitForModelStable(state).then(() => {
			resolved = true;
		});

		await vi.advanceTimersByTimeAsync(400);
		expect(resolved).toBe(false);

		await vi.advanceTimersByTimeAsync(100);
		await promise;
		expect(resolved).toBe(true);
	});

	it("extends wait when new stamp arrives", async () => {
		const state = freshState();
		state.lastModelSelectTime = Date.now();
		let resolved = false;
		const promise = waitForModelStable(state).then(() => {
			resolved = true;
		});

		await vi.advanceTimersByTimeAsync(400);
		expect(resolved).toBe(false);

		// New model select resets the timer
		state.lastModelSelectTime = Date.now();
		await vi.advanceTimersByTimeAsync(400);
		expect(resolved).toBe(false);

		await vi.advanceTimersByTimeAsync(100);
		await promise;
		expect(resolved).toBe(true);
	});

	it("resolves after MAX_STABILITY_WAIT_MS even if stamp keeps updating", async () => {
		const state = freshState();
		state.lastModelSelectTime = Date.now();
		let resolved = false;
		const promise = waitForModelStable(state).then(() => {
			resolved = true;
		});

		// Keep resetting stamp every 400ms — without the cap this would loop forever
		for (let i = 0; i < 12; i++) {
			await vi.advanceTimersByTimeAsync(400);
			state.lastModelSelectTime = Date.now();
		}

		// Should resolve once MAX_STABILITY_WAIT_MS is exceeded
		await vi.advanceTimersByTimeAsync(MAX_STABILITY_WAIT_MS);
		await promise;
		expect(resolved).toBe(true);
	});
});

// ─── Builder functions ────────────────────────────────────────────────────────

describe("buildNotificationToolCall", () => {
	it("produces a ToolCall with correct shape", () => {
		const model = { name: "GPT 5.5", id: "gpt-5-5", provider: "openai", fromTurnIndex: 0 };
		const tc = buildNotificationToolCall(model, "scrmdl-abc123");

		expect(tc.type).toBe("toolCall");
		expect(tc.id).toBe("scrmdl-abc123");
		expect(tc.name).toBe("notify_model_change");
		expect(tc.arguments).toEqual({ model_name: "GPT 5.5", model_id: "gpt-5-5", provider: "openai" });
	});
});

describe("buildNotificationPair", () => {
	it("produces [AssistantMessage, ToolResultMessage] pair", () => {
		const model = { name: "GPT 5.5", id: "gpt-5-5", provider: "openai", fromTurnIndex: 0 };
		const prev = { name: "Claude", id: "claude-opus-4-6", provider: "anthropic", fromTurnIndex: 0 };
		const [am, rm] = buildNotificationPair(model, prev, "scrmdl-test");

		expect(am.role).toBe("assistant");
		expect(am.content).toHaveLength(1);
		expect(am.content[0].type).toBe("toolCall");
		expect((am.content[0] as ToolCall).id).toBe("scrmdl-test");
		expect(am.stopReason).toBe("toolUse");
		expect(am.provider).toBe("scramjet");

		expect(rm.role).toBe("toolResult");
		expect(rm.toolCallId).toBe("scrmdl-test");
		expect(rm.toolName).toBe("notify_model_change");
		expect(rm.isError).toBe(false);
		expect(rm.content[0]).toEqual(expect.objectContaining({ type: "text" }));
		expect((rm.content[0] as any).text).toContain("GPT 5.5");
		expect((rm.content[0] as any).text).toContain("Claude");
	});
});

// ─── Rebuild / resume ─────────────────────────────────────────────────────────

describe("resume reconstruction integration", () => {
	function ctxWithBranchAndModel(entries: SessionEntry[], model?: any) {
		return { sessionManager: { getBranch: () => entries }, model };
	}

	it("reconstructs model state on session_start with reason resume", async () => {
		const { emit, state } = setup();
		const entries = [
			modelChangeEntry("anthropic", "claude-opus-4-6"),
			messageEntry("user"),
			messageEntry("assistant"),
			modelChangeEntry("openai", "gpt-5-5"),
		];
		const ctx = ctxWithBranchAndModel(entries, fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" }));

		await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

		expect(state.modelHistory).toHaveLength(2);
		expect(state.currentModel!.id).toBe("gpt-5-5");
		expect(state.currentModel!.name).toBe("GPT 5.5");
	});

	it("reconstructs model state on session_start with reason fork", async () => {
		const { emit, state } = setup();
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const ctx = ctxWithBranchAndModel(entries, fakeModel());

		await emit("session_start", { type: "session_start", reason: "fork" }, ctx);

		expect(state.modelHistory).toHaveLength(1);
		expect(state.currentModel!.name).toBe("Claude Opus 4.6");
	});

	it("keeps startup behavior for reason startup", async () => {
		const { emit, state } = setup();

		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });

		expect(state.modelHistory).toHaveLength(1);
		expect(state.currentModel!.name).toBe("Claude Opus 4.6");
		expect(state.currentModel!.fromTurnIndex).toBe(0);
	});

	it("reconstructs model state on session_tree", async () => {
		const { emit, state } = setup();
		const entries = [
			modelChangeEntry("anthropic", "claude-opus-4-6"),
			messageEntry("user"),
			messageEntry("assistant"),
			modelChangeEntry("openai", "gpt-5-5"),
		];
		const ctx = ctxWithBranchAndModel(entries, fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" }));

		await emit("session_tree", { type: "session_tree", newLeafId: "leaf1", oldLeafId: "leaf0" }, ctx);

		expect(state.modelHistory).toHaveLength(2);
		expect(state.currentModel!.id).toBe("gpt-5-5");
	});

	it("detects divergence on resume and sets pendingModelChange", async () => {
		const { emit, state } = setup();
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const ctx = ctxWithBranchAndModel(entries, fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" }));

		await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

		expect(state.pendingModelChange).not.toBeNull();
		expect(state.pendingModelChange!.id).toBe("gpt-5-5");
	});

	it("does not set pendingModelChange when model matches on resume", async () => {
		const { emit, state } = setup();
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const ctx = ctxWithBranchAndModel(entries, fakeModel());

		await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

		expect(state.pendingModelChange).toBeNull();
	});

	it("resets history when branch has no model_change entries", async () => {
		const { emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });

		const ctx = ctxWithBranchAndModel([], fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" }));
		await emit("session_tree", { type: "session_tree", newLeafId: "leaf1", oldLeafId: "leaf0" }, ctx);

		expect(state.currentModel!.id).toBe("gpt-5-5");
		expect(state.modelHistory).toHaveLength(1);
	});

	it("resets to null when branch is empty and ctx.model is undefined", async () => {
		const { emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });

		const ctx = ctxWithBranchAndModel([], undefined);
		await emit("session_tree", { type: "session_tree", newLeafId: "leaf1", oldLeafId: "leaf0" }, ctx);

		expect(state.currentModel).toBeNull();
		expect(state.modelHistory).toEqual([]);
	});

	it("clears pendingModelChange on rebuild", async () => {
		const { emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });

		// Set a pending change
		const gpt5 = fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" });
		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		expect(state.pendingModelChange).not.toBeNull();

		// Rebuild clears it (model matches, so no divergence)
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const ctx = ctxWithBranchAndModel(entries, fakeModel());
		await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

		expect(state.pendingModelChange).toBeNull();
		expect(state.lastModelSelectTime).toBe(0);
	});
});

// ─── reconstructModelState (pure function) ────────────────────────────────────

function modelChangeEntry(provider: string, modelId: string, id?: string): SessionEntry {
	return {
		type: "model_change",
		id: id ?? `mc-${Math.random()}`,
		parentId: null,
		timestamp: "0",
		provider,
		modelId,
	} as any;
}

function messageEntry(role: "user" | "assistant", id?: string): SessionEntry {
	return {
		type: "message",
		id: id ?? `msg-${Math.random()}`,
		parentId: null,
		timestamp: "0",
		message: { role, content: "test" },
	} as any;
}

describe("reconstructModelState", () => {
	it("returns empty state when no model_change entries exist", () => {
		const result = reconstructModelState([], undefined);
		expect(result.currentModel).toBeNull();
		expect(result.modelHistory).toEqual([]);
		expect(result.diverged).toBe(false);
	});

	it("reconstructs a single model from one model_change entry", () => {
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const result = reconstructModelState(entries, undefined);

		expect(result.currentModel).toEqual({
			name: "claude-opus-4-6",
			id: "claude-opus-4-6",
			provider: "anthropic",
			fromTurnIndex: 0,
		});
		expect(result.modelHistory).toHaveLength(1);
		expect(result.diverged).toBe(false);
	});

	it("uses ctx.model name for the last entry when available", () => {
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const ctxModel = fakeModel({ id: "claude-opus-4-6", name: "Claude Opus 4.6", provider: "anthropic" });
		const result = reconstructModelState(entries, ctxModel);

		expect(result.currentModel!.name).toBe("Claude Opus 4.6");
		expect(result.currentModel!.id).toBe("claude-opus-4-6");
	});

	it("reconstructs multiple models with estimated turn indices", () => {
		const entries = [
			modelChangeEntry("anthropic", "claude-opus-4-6"),
			messageEntry("user"),
			messageEntry("assistant"),
			messageEntry("user"),
			messageEntry("assistant"),
			modelChangeEntry("openai", "gpt-5-5"),
			messageEntry("user"),
			messageEntry("assistant"),
			modelChangeEntry("anthropic", "claude-sonnet-4"),
		];
		const result = reconstructModelState(entries, undefined);

		expect(result.modelHistory).toHaveLength(3);
		expect(result.modelHistory[0]!.fromTurnIndex).toBe(0);
		expect(result.modelHistory[1]!.fromTurnIndex).toBe(2);
		expect(result.modelHistory[2]!.fromTurnIndex).toBe(3);
		expect(result.currentModel!.id).toBe("claude-sonnet-4");
	});

	it("detects divergence when ctx.model differs from last entry", () => {
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const ctxModel = fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" });
		const result = reconstructModelState(entries, ctxModel);

		expect(result.diverged).toBe(true);
		expect(result.currentModel!.id).toBe("gpt-5-5");
		expect(result.currentModel!.name).toBe("GPT 5.5");
		expect(result.modelHistory).toHaveLength(2);
		expect(result.modelHistory[1]!.id).toBe("gpt-5-5");
	});

	it("does not detect divergence when ctx.model matches last entry", () => {
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const ctxModel = fakeModel({ id: "claude-opus-4-6", name: "Claude Opus 4.6", provider: "anthropic" });
		const result = reconstructModelState(entries, ctxModel);

		expect(result.diverged).toBe(false);
		expect(result.modelHistory).toHaveLength(1);
	});

	it("does not detect divergence when ctx.model is undefined", () => {
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const result = reconstructModelState(entries, undefined);

		expect(result.diverged).toBe(false);
	});

	it("ignores non-model_change entries", () => {
		const entries = [
			messageEntry("user"),
			messageEntry("assistant"),
			{ type: "custom", id: "x", parentId: null, timestamp: "0", customType: "test", data: {} } as any,
		];
		const result = reconstructModelState(entries, undefined);

		expect(result.currentModel).toBeNull();
		expect(result.modelHistory).toEqual([]);
	});

	it("excludes synthetic (provider: scramjet) assistant messages from turn index count", () => {
		const syntheticEntry = {
			type: "message",
			id: "synthetic-1",
			parentId: null,
			timestamp: "0",
			message: { role: "assistant", content: [], provider: "scramjet" },
		} as any;
		const entries = [
			modelChangeEntry("anthropic", "claude-opus-4-6"),
			messageEntry("user"),
			messageEntry("assistant"),
			syntheticEntry,
			messageEntry("user"),
			messageEntry("assistant"),
			modelChangeEntry("openai", "gpt-5-5"),
		];
		const result = reconstructModelState(entries, undefined);

		// Without filtering, turnIndex would be 3 (counting synthetic). With filtering, it's 2.
		expect(result.modelHistory[1]!.fromTurnIndex).toBe(2);
	});
});
