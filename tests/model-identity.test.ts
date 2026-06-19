import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { reconstructModelState, registerModelIdentity } from "../model-identity.ts";
import { freshState, lifecycleFor, recordingPi } from "./helpers.ts";

function fakeModel(overrides: Record<string, unknown> = {}) {
	return {
		id: "claude-opus-4-6",
		name: "Claude Opus 4.6",
		provider: "anthropic",
		...overrides,
	} as any;
}

function setup() {
	const { pi, handlers, emit } = recordingPi();
	const state = freshState();
	registerModelIdentity(pi, state);
	return { handlers, emit, state };
}

describe("registerModelIdentity", () => {
	it("registers session_start, before_agent_start, and turn_start handlers", () => {
		const { handlers } = setup();
		expect(handlers.get("session_start")).toHaveLength(1);
		expect(handlers.get("before_agent_start")).toHaveLength(1);
		expect(handlers.get("turn_start")).toHaveLength(1);
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
		const first = (await handler({ systemPrompt: "BASE" })) as {
			systemPromptSection: { id: string; text: string };
			systemPrompt?: unknown;
			message?: unknown;
		};
		await emit("turn_start", { type: "turn_start", turnIndex: 3, timestamp: 123 });
		const second = (await handler({ systemPrompt: "BASE" })) as typeof first;

		expect(first).toEqual(second);
		expect(Object.keys(first)).toEqual(["systemPromptSection"]);
		expect(first.systemPrompt).toBeUndefined();
		expect(first.message).toBeUndefined();
		expect(first.systemPromptSection.id).toBe("scramjet:model-identity");
		expect(first.systemPromptSection.text).toContain("\n\n# Model Identity");
		expect(first.systemPromptSection.text).not.toContain("BASE");
		expect(first.systemPromptSection.text).toContain(
			"Your model is: Claude Opus 4.6 (ID: claude-opus-4-6, provider: anthropic).",
		);
		expect(first.systemPromptSection.text).toContain("messages prefixed with [scramjet]");
		expect(first.systemPromptSection.text).toContain('Single model: "Reviewed by Claude Opus 4.6"');
		expect(first.systemPromptSection.text).toContain("Multiple models: describe each model's contribution");
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

describe("model_select debounce and delivery", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	const gpt5 = fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" });

	function setupWithModel() {
		const { handlers, emit, state } = setup();
		return { handlers, emit, state };
	}

	async function initModel(emit: any, model = fakeModel()) {
		await emit("session_start", { type: "session_start", reason: "startup" }, { model });
	}

	it("registers a model_select handler", () => {
		const { handlers } = setupWithModel();
		expect(handlers.get("model_select")).toHaveLength(1);
	});

	it("does not fire before 500ms debounce settles", async () => {
		const { emit, state } = setupWithModel();
		await initModel(emit);

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "cycle" });
		vi.advanceTimersByTime(400);

		expect(state.currentModel?.id).toBe("claude-opus-4-6");
	});

	it("commits model change after 500ms debounce", async () => {
		const { emit, state } = setupWithModel();
		await initModel(emit);

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(500);

		expect(state.currentModel?.id).toBe("gpt-5-5");
		expect(state.currentModel?.name).toBe("GPT 5.5");
		expect(state.modelHistory).toHaveLength(2);
		expect(state.modelHistory[1]?.id).toBe("gpt-5-5");
	});

	it("collapses rapid cycling to only the final model", async () => {
		const { emit, state } = setupWithModel();
		await initModel(emit);

		const mid = fakeModel({ id: "mid-model", name: "Mid", provider: "mid" });
		await emit("model_select", { type: "model_select", model: mid, previousModel: fakeModel(), source: "cycle" });
		vi.advanceTimersByTime(200);
		await emit("model_select", { type: "model_select", model: gpt5, previousModel: mid, source: "cycle" });
		vi.advanceTimersByTime(500);

		expect(state.currentModel?.id).toBe("gpt-5-5");
		expect(state.modelHistory).toHaveLength(2);
	});

	it("suppresses no-op cycling (pending === current)", async () => {
		const { emit, state } = setupWithModel();
		await initModel(emit);

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "cycle" });
		vi.advanceTimersByTime(200);
		// Cycle back to original
		await emit("model_select", {
			type: "model_select",
			model: fakeModel(),
			previousModel: gpt5,
			source: "cycle",
		});
		vi.advanceTimersByTime(500);

		expect(state.currentModel?.id).toBe("claude-opus-4-6");
		expect(state.modelHistory).toHaveLength(1);
	});

	it("suppresses source === 'restore' events", async () => {
		const { emit, state } = setupWithModel();
		await initModel(emit);

		await emit("model_select", {
			type: "model_select",
			model: gpt5,
			previousModel: fakeModel(),
			source: "restore",
		});
		vi.advanceTimersByTime(500);

		expect(state.currentModel?.id).toBe("claude-opus-4-6");
		expect(state.modelHistory).toHaveLength(1);
	});

	describe("delivery: waiting for input (idle/waiting/dormant)", () => {
		it("stores pendingForInput and transforms input text", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
			state.lifecycle = { phase: "idle" };

			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			const inputHandler = handlers.get("input")![0];
			const result = (await inputHandler({ type: "input", text: "hello", source: "interactive" })) as any;

			expect(result.action).toBe("transform");
			expect(result.text).toContain("[scramjet] Model changed to: GPT 5.5 (ID: gpt-5-5).");
			expect(result.text).toContain("hello");
			expect(result.text).not.toContain("Please continue");
		});

		it("clears the pending flag after input transform", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
			state.lifecycle = { phase: "idle" };

			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			const inputHandler = handlers.get("input")![0];
			await inputHandler({ type: "input", text: "hello", source: "interactive" });
			const secondResult = (await inputHandler({ type: "input", text: "world", source: "interactive" })) as any;

			expect(secondResult).toBeUndefined();
		});

		it("works in waiting phase", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
			state.lifecycle = lifecycleFor("waiting");

			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			const inputHandler = handlers.get("input")![0];
			const result = (await inputHandler({ type: "input", text: "answer", source: "interactive" })) as any;

			expect(result.action).toBe("transform");
			expect(result.text).toContain("[scramjet] Model changed to: GPT 5.5");
		});

		it("works in dormant phase", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
			state.lifecycle = lifecycleFor("dormant");

			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			const inputHandler = handlers.get("input")![0];
			const result = (await inputHandler({ type: "input", text: "go", source: "interactive" })) as any;

			expect(result.action).toBe("transform");
		});
	});

	describe("delivery: agent working (running/probing)", () => {
		it("stores pendingForNextTurn and delivers via before_agent_start message", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
			state.lifecycle = lifecycleFor("running");

			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			const basHandler = handlers.get("before_agent_start")![0];
			const result = (await basHandler({ systemPrompt: "BASE" })) as any;

			expect(result.message).toBeDefined();
			expect(result.message.customType).toBe("scramjet:model-change");
			expect(result.message.content).toContain("[scramjet] Model changed to: GPT 5.5 (ID: gpt-5-5).");
			expect(result.message.content).toContain("Please continue.");
			expect(result.message.display).toBe(true);
		});

		it("clears the pending flag after delivery", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
			state.lifecycle = lifecycleFor("running");

			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			const basHandler = handlers.get("before_agent_start")![0];
			await basHandler({ systemPrompt: "BASE" });
			const secondResult = (await basHandler({ systemPrompt: "BASE" })) as any;

			expect(secondResult.message).toBeUndefined();
		});

		it("skips message delivery during probing phase (avoids probe interference)", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
			state.lifecycle = lifecycleFor("probing");

			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			const basHandler = handlers.get("before_agent_start")![0];
			const result = (await basHandler({ systemPrompt: "BASE" })) as any;

			expect(result.message).toBeUndefined();
			expect(result.systemPromptSection.text).toContain("Claude Opus 4.6");
		});

		it("delivers model change notification after probing phase completes", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
			state.lifecycle = lifecycleFor("running");

			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			// Transition to probing before before_agent_start fires
			state.lifecycle = lifecycleFor("probing");
			const basHandler = handlers.get("before_agent_start")![0];
			const probingResult = (await basHandler({ systemPrompt: "BASE" })) as any;

			// Message suppressed during probing, but flag preserved
			expect(probingResult.message).toBeUndefined();

			// Phase returns to running — message should now be delivered
			state.lifecycle = lifecycleFor("running");
			const runningResult = (await basHandler({ systemPrompt: "BASE" })) as any;

			expect(runningResult.message).toBeDefined();
			expect(runningResult.message.content).toContain("[scramjet] Model changed to: GPT 5.5");
			expect(runningResult.message.content).toContain("Please continue.");
		});
	});

	describe("slash-command input guard", () => {
		it("does not transform slash-command input", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
			state.lifecycle = { phase: "idle" };

			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			const inputHandler = handlers.get("input")![0];
			const result = (await inputHandler({
				type: "input",
				text: "/mach12:issue-implement 55 1",
				source: "interactive",
			})) as any;

			expect(result).toBeUndefined();
		});

		it("protects whitespace-prefixed slash commands", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
			state.lifecycle = { phase: "idle" };

			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			const inputHandler = handlers.get("input")![0];
			const result = (await inputHandler({
				type: "input",
				text: "  /mach12:pr-create 55",
				source: "interactive",
			})) as any;

			expect(result).toBeUndefined();
		});

		it("redirects blocked notification to before_agent_start message", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
			state.lifecycle = { phase: "idle" };

			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			const inputHandler = handlers.get("input")![0];
			await inputHandler({ type: "input", text: "/mach12:issue-implement 55 1", source: "interactive" });

			state.lifecycle = lifecycleFor("running");
			const basHandler = handlers.get("before_agent_start")![0];
			const result = (await basHandler({ systemPrompt: "BASE" })) as any;

			expect(result.message).toBeDefined();
			expect(result.message.content).toContain("[scramjet] Model changed to: GPT 5.5");
			expect(result.message.content).toContain("Please continue.");
		});

		it("protects slash commands after rebuild-divergence sets pendingForInput", async () => {
			const { handlers, emit } = setupWithModel();
			const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
			const ctx = {
				sessionManager: { getBranch: () => entries },
				model: fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" }),
			};

			await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

			const inputHandler = handlers.get("input")![0];
			const result = (await inputHandler({ type: "input", text: "/scramjet on", source: "interactive" })) as any;

			expect(result).toBeUndefined();
		});
	});

	describe("pre-first-turn model change", () => {
		it("updates initialModel directly without setting pendingForInput", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			state.lifecycle = { phase: "idle" };

			// model_select before any turn_start → pre-first-turn
			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			// No pendingForInput — input should pass through untransformed
			const inputHandler = handlers.get("input")![0];
			const result = (await inputHandler({ type: "input", text: "hello", source: "interactive" })) as any;
			expect(result).toBeUndefined();

			// System prompt should reflect the NEW model
			const basHandler = handlers.get("before_agent_start")![0];
			const basResult = (await basHandler({ systemPrompt: "BASE" })) as any;
			expect(basResult.systemPromptSection.text).toContain("GPT 5.5");
			expect(basResult.systemPromptSection.text).toContain("gpt-5-5");
		});

		it("post-first-turn model change still uses input transform", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			state.lifecycle = { phase: "idle" };

			// turn_start marks the first turn
			await emit("turn_start", { type: "turn_start", turnIndex: 1, timestamp: 123 });

			// model_select AFTER turn_start → post-first-turn, should use input transform
			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			const inputHandler = handlers.get("input")![0];
			const result = (await inputHandler({ type: "input", text: "hello", source: "interactive" })) as any;
			expect(result.action).toBe("transform");
			expect(result.text).toContain("[scramjet] Model changed to: GPT 5.5");
		});

		it("firstTurnStarted is cleared on rebuild", async () => {
			const { handlers, emit, state } = setupWithModel();
			await initModel(emit);
			state.lifecycle = { phase: "idle" };

			// Mark first turn as started
			await emit("turn_start", { type: "turn_start", turnIndex: 1, timestamp: 123 });

			// Rebuild via resume — should clear firstTurnStarted
			const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
			const ctx = {
				sessionManager: { getBranch: () => entries },
				model: fakeModel(),
			};
			await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

			// model_select after rebuild but before new turn_start → pre-first-turn again
			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			// No pendingForInput — input should pass through untransformed
			const inputHandler = handlers.get("input")![0];
			const result = (await inputHandler({ type: "input", text: "hello", source: "interactive" })) as any;
			expect(result).toBeUndefined();

			// System prompt should reflect the new model
			const basHandler = handlers.get("before_agent_start")![0];
			const basResult = (await basHandler({ systemPrompt: "BASE" })) as any;
			expect(basResult.systemPromptSection.text).toContain("GPT 5.5");
		});
	});

	it("keeps system prompt showing initial model after model change (cache-friendly)", async () => {
		const { handlers, emit, state } = setupWithModel();
		await initModel(emit);
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
		state.lifecycle = { phase: "idle" };

		const basHandler = handlers.get("before_agent_start")![0];
		const before = (await basHandler({ systemPrompt: "BASE" })) as any;

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(500);

		const after = (await basHandler({ systemPrompt: "BASE" })) as any;

		expect(after.systemPromptSection.text).toBe(before.systemPromptSection.text);
		expect(after.systemPromptSection.text).toContain(
			"Your model is: Claude Opus 4.6 (ID: claude-opus-4-6, provider: anthropic).",
		);
		expect(after.systemPromptSection.text).not.toContain("GPT 5.5");
	});

	it("assigns correct fromTurnIndex to new model record", async () => {
		const { emit, state } = setupWithModel();
		await initModel(emit);
		await emit("turn_start", { type: "turn_start", turnIndex: 5, timestamp: 999 });

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(500);

		expect(state.currentModel?.fromTurnIndex).toBe(5);
	});

	it("does not inject message into probe turn (probing phase)", async () => {
		const { handlers, emit, state } = setupWithModel();
		await initModel(emit);
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
		state.lifecycle = lifecycleFor("running");

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(500);

		// Simulate lifecycle transitioning to probing before before_agent_start fires
		state.lifecycle = lifecycleFor("probing");

		const basHandler = handlers.get("before_agent_start")![0];
		const result = (await basHandler({ systemPrompt: "BASE" })) as any;

		expect(result.message).toBeUndefined();
		expect(result.systemPromptSection.text).toContain("Claude Opus 4.6");
	});

	it("flags are mutually exclusive across phase transitions", async () => {
		const { handlers, emit, state } = setupWithModel();
		await initModel(emit);
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
		state.lifecycle = lifecycleFor("running");

		// First change during running
		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(500);

		// Phase transitions to idle, second change
		state.lifecycle = { phase: "idle" };
		const model3 = fakeModel({ id: "model-3", name: "Model 3", provider: "test" });
		await emit("model_select", { type: "model_select", model: model3, previousModel: gpt5, source: "set" });
		vi.advanceTimersByTime(500);

		// Only the input path should fire, not before_agent_start message
		const inputHandler = handlers.get("input")![0];
		const inputResult = (await inputHandler({ type: "input", text: "hi", source: "interactive" })) as any;
		expect(inputResult.action).toBe("transform");
		expect(inputResult.text).toContain("Model 3");

		const basHandler = handlers.get("before_agent_start")![0];
		const basResult = (await basHandler({ systemPrompt: "BASE" })) as any;
		expect(basResult.message).toBeUndefined();
	});
});

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
});

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

	it("detects divergence on resume and stores pendingForInput", async () => {
		const { handlers, emit } = setup();
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const ctx = ctxWithBranchAndModel(entries, fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" }));

		await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

		// Verify divergence is detected by checking input transform fires
		const inputHandler = handlers.get("input")![0];
		const result = (await inputHandler({ type: "input", text: "hello", source: "interactive" })) as any;

		expect(result.action).toBe("transform");
		expect(result.text).toContain("[scramjet] Model changed to: GPT 5.5 (ID: gpt-5-5).");
		expect(result.text).toContain("hello");
	});

	it("does not set pendingForInput when model matches on resume", async () => {
		const { handlers, emit } = setup();
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const ctx = ctxWithBranchAndModel(entries, fakeModel());

		await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

		const inputHandler = handlers.get("input")![0];
		const result = (await inputHandler({ type: "input", text: "hello", source: "interactive" })) as any;

		expect(result).toBeUndefined();
	});

	it("resets history when branch has no model_change entries", async () => {
		const { emit, state } = setup();
		// First set up state
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		expect(state.currentModel).not.toBeNull();

		// Now navigate to a branch with no model entries
		const ctx = ctxWithBranchAndModel([], fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" }));
		await emit("session_tree", { type: "session_tree", newLeafId: "leaf1", oldLeafId: "leaf0" }, ctx);

		// Should reset and use ctx.model as initial
		expect(state.currentModel!.id).toBe("gpt-5-5");
		expect(state.currentModel!.name).toBe("GPT 5.5");
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

	it("clears pending flags on rebuild", async () => {
		const { handlers, emit, state } = setup();
		// Create a pending change from stage 2 behavior
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
		state.lifecycle = { phase: "idle" };

		vi.useFakeTimers();
		const gpt5 = fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" });
		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(500);
		vi.useRealTimers();

		// Should have pending
		const inputHandler = handlers.get("input")![0];
		let result = (await inputHandler({ type: "input", text: "test", source: "interactive" })) as any;
		expect(result?.action).toBe("transform");

		// Now rebuild — this should clear any leftover pending flags
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const ctx = ctxWithBranchAndModel(entries, fakeModel());
		await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

		// No pending change since ctx.model matches
		result = (await inputHandler({ type: "input", text: "test2", source: "interactive" })) as any;
		expect(result).toBeUndefined();
	});
});
