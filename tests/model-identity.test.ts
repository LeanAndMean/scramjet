import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerModelIdentity } from "../model-identity.ts";
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
		const first = (await handler({ systemPrompt: "BASE" })) as { systemPrompt: string; message?: unknown };
		await emit("turn_start", { type: "turn_start", turnIndex: 3, timestamp: 123 });
		const second = (await handler({ systemPrompt: "BASE" })) as { systemPrompt: string; message?: unknown };

		expect(first).toEqual(second);
		expect(Object.keys(first)).toEqual(["systemPrompt"]);
		expect(first.message).toBeUndefined();
		expect(first.systemPrompt).toContain("BASE\n\n# Model Identity");
		expect(first.systemPrompt).toContain(
			"Your model is: Claude Opus 4.6 (ID: claude-opus-4-6, provider: anthropic).",
		);
		expect(first.systemPrompt).toContain("messages prefixed with [scramjet]");
		expect(first.systemPrompt).toContain('Single model: "Reviewed by Claude Opus 4.6"');
		expect(first.systemPrompt).toContain("Multiple models: describe each model's contribution");
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
			state.lifecycle = lifecycleFor("probing");

			await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
			vi.advanceTimersByTime(500);

			const basHandler = handlers.get("before_agent_start")![0];
			const result = (await basHandler({ systemPrompt: "BASE" })) as any;

			expect(result.message).toBeUndefined();
			expect(result.systemPrompt).toContain("GPT 5.5");
		});
	});

	it("updates system prompt to reflect new model after change", async () => {
		const { handlers, emit, state } = setupWithModel();
		await initModel(emit);
		state.lifecycle = { phase: "idle" };

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(500);

		const basHandler = handlers.get("before_agent_start")![0];
		const result = (await basHandler({ systemPrompt: "BASE" })) as any;

		expect(result.systemPrompt).toContain("Your model is: GPT 5.5 (ID: gpt-5-5, provider: openai).");
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
		state.lifecycle = lifecycleFor("running");

		await emit("model_select", { type: "model_select", model: gpt5, previousModel: fakeModel(), source: "set" });
		vi.advanceTimersByTime(500);

		// Simulate lifecycle transitioning to probing before before_agent_start fires
		state.lifecycle = lifecycleFor("probing");

		const basHandler = handlers.get("before_agent_start")![0];
		const result = (await basHandler({ systemPrompt: "BASE" })) as any;

		expect(result.message).toBeUndefined();
		expect(result.systemPrompt).toContain("GPT 5.5");
	});

	it("flags are mutually exclusive across phase transitions", async () => {
		const { handlers, emit, state } = setupWithModel();
		await initModel(emit);
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
