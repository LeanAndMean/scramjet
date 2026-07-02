import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MODEL_CHANGE_NOTICE_TOOL, registerModelChangeNotice } from "../src/model-change-notice.js";
import { registerModelIdentity } from "../src/model-identity.js";
import type { ScramjetState } from "../src/types.js";
import { freshState, lifecycleFor, recordingPi } from "./helpers.js";

function fakeModel(overrides: Record<string, unknown> = {}) {
	return { id: "claude-opus-4-6", name: "Claude Opus 4.6", provider: "anthropic", ...overrides } as any;
}

const GPT = fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" });

// A state seeded as if a session has begun with a user message already sent, so a
// change to a different model is a real (non-no-op) post-first-turn change.
function seededState(overrides: Partial<ScramjetState> = {}): ScramjetState {
	const initial = fakeModel();
	return freshState({
		currentModel: { name: initial.name, id: initial.id, provider: initial.provider, fromTurnIndex: 0 },
		modelHistory: [{ name: initial.name, id: initial.id, provider: initial.provider, fromTurnIndex: 0 }],
		hasUserMessage: true,
		...overrides,
	});
}

function setup(state?: ScramjetState, opts: { withIdentity?: boolean } = {}) {
	const rec = recordingPi();
	const st = state ?? freshState();
	if (opts.withIdentity) registerModelIdentity(rec.pi, st);
	registerModelChangeNotice(rec.pi, st);
	return { ...rec, state: st };
}

function selectEvent(model: unknown, source: "set" | "cycle" | "restore" = "set") {
	return { type: "model_select", model, previousModel: fakeModel(), source };
}

describe("scramjet_model_change_notice tool", () => {
	it("registers a structurally harness-only tool", () => {
		const { tools } = setup();
		const tool = tools.find((t: any) => t.name === MODEL_CHANGE_NOTICE_TOOL);
		expect(tool).toBeDefined();
		expect(tool.activation).toBe("harness-only");
		// Never advertised to the model — it is harness-invoked only.
		expect(tool.promptSnippet).toBeUndefined();
	});

	it("execute returns the change text and echoes the model in details", async () => {
		const { tools } = setup();
		const tool = tools.find((t: any) => t.name === MODEL_CHANGE_NOTICE_TOOL)!;
		const result = await tool.execute(
			"id",
			{ provider: "openai", model: "gpt-5-5", name: "GPT 5.5" },
			undefined,
			undefined,
			{},
		);
		expect(result.content[0].text).toContain("GPT 5.5");
		expect(result.content[0].text).toContain("gpt-5-5");
		expect(result.details).toMatchObject({ provider: "openai", model: "gpt-5-5", name: "GPT 5.5" });
	});
});

describe("hasUserMessage tracking", () => {
	it("marks hasUserMessage on the first input event and never transforms it", async () => {
		const { state, handlers } = setup();
		expect(state.hasUserMessage).toBe(false);

		const input = handlers.get("input")![0];
		const result = await input({ type: "input", text: "hello", source: "interactive" });

		expect(result).toBeUndefined();
		expect(state.hasUserMessage).toBe(true);
	});
});

describe("model_select delivery", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("collapses rapid cycling into one notice for the final model (req 5)", async () => {
		const { emit, state, pi } = setup(seededState());

		await emit("model_select", selectEvent(fakeModel({ id: "m1", name: "M1", provider: "p" }), "cycle"));
		vi.advanceTimersByTime(200);
		await emit("model_select", selectEvent(fakeModel({ id: "m2", name: "M2", provider: "p" }), "cycle"));
		vi.advanceTimersByTime(200);
		await emit("model_select", selectEvent(GPT, "cycle"));
		vi.advanceTimersByTime(500);

		expect(pi.harnessToolCalls).toHaveLength(1);
		expect(pi.harnessToolCalls[0].name).toBe(MODEL_CHANGE_NOTICE_TOOL);
		expect(pi.harnessToolCalls[0].args).toEqual({ provider: "openai", model: "gpt-5-5", name: "GPT 5.5" });
		expect(state.currentModel?.id).toBe("gpt-5-5");
		// Seeded initial + final only; intermediates never settled.
		expect(state.modelHistory).toHaveLength(2);
	});

	it("suppresses a no-op cycle back to the current model", async () => {
		const { emit, state, pi } = setup(seededState());

		await emit("model_select", selectEvent(GPT, "cycle"));
		vi.advanceTimersByTime(200);
		await emit("model_select", selectEvent(fakeModel(), "cycle")); // back to current
		vi.advanceTimersByTime(500);

		expect(pi.harnessToolCalls).toHaveLength(0);
		expect(state.currentModel?.id).toBe("claude-opus-4-6");
		expect(state.modelHistory).toHaveLength(1);
	});

	it("ignores source === 'restore' events", async () => {
		const { emit, state, pi } = setup(seededState());

		await emit("model_select", selectEvent(GPT, "restore"));
		vi.advanceTimersByTime(500);

		expect(pi.harnessToolCalls).toHaveLength(0);
		expect(state.currentModel?.id).toBe("claude-opus-4-6");
	});

	it("delivers immediately when idle and arms no probe or auto-continuation", async () => {
		const { emit, state, pi } = setup(seededState());

		await emit("model_select", selectEvent(GPT));
		vi.advanceTimersByTime(500);

		expect(pi.harnessToolCalls).toHaveLength(1);
		// Transient delivery must not schedule a probe or send any message.
		expect(pi.sent).toHaveLength(0);
		expect(state.lifecycle.probeArmed).toBe(false);
		expect(state.lifecycle.probeInFlight).toBe(false);
		expect(state.pendingNotifyModel).toBeNull();
	});

	it("delivers during active non-probe work (primitive handles queueing, scenario 5)", async () => {
		const { emit, state, pi } = setup(seededState({ lifecycle: lifecycleFor("dormant") }));

		await emit("model_select", selectEvent(GPT));
		vi.advanceTimersByTime(500);

		// Non-probe lifecycle → clear path → invokeHarnessTool (queued mid-run by the primitive).
		expect(pi.harnessToolCalls).toHaveLength(1);
		expect(state.pendingNotifyModel).toBeNull();
	});

	it("never delivers via user messages or input transforms (req 9)", async () => {
		const { emit, pi, handlers } = setup(seededState());

		await emit("model_select", selectEvent(GPT));
		vi.advanceTimersByTime(500);

		const input = handlers.get("input")![0];
		const result = await input({ type: "input", text: "hello", source: "interactive" });

		expect(result).toBeUndefined();
		expect(pi.sent).toHaveLength(0);
		expect(pi.harnessToolCalls).toHaveLength(1);
	});
});

describe("pre-first-turn model change (req 6)", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("updates the system prompt but fires no notice before the first user message", async () => {
		const { emit, state, pi, handlers } = setup(freshState(), { withIdentity: true });
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });

		// No input event → hasUserMessage stays false → pre-first-turn.
		await emit("model_select", selectEvent(GPT));
		vi.advanceTimersByTime(500);

		expect(pi.harnessToolCalls).toHaveLength(0);
		expect(state.currentModel?.id).toBe("gpt-5-5");

		const bas = handlers.get("before_agent_start")![0];
		const result = (await bas({ systemPrompt: "BASE" })) as any;
		expect(result.systemPromptSection.text).toContain("GPT 5.5");
		expect(result.systemPromptSection.text).toContain("gpt-5-5");
	});

	it("freezes the system prompt and switches to notice delivery after the first user message", async () => {
		const { emit, pi, handlers } = setup(freshState(), { withIdentity: true });
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });

		// First user message + first turn latch the identity section.
		const input = handlers.get("input")![0];
		await input({ type: "input", text: "hello", source: "interactive" });
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
		const bas = handlers.get("before_agent_start")![0];
		const frozen = (await bas({ systemPrompt: "BASE" })) as any;

		await emit("model_select", selectEvent(GPT));
		vi.advanceTimersByTime(500);

		// Now a notice fires and the system prompt stays on the initial model.
		expect(pi.harnessToolCalls).toHaveLength(1);
		const after = (await bas({ systemPrompt: "BASE" })) as any;
		expect(after.systemPromptSection.text).toBe(frozen.systemPromptSection.text);
		expect(after.systemPromptSection.text).toContain("Claude Opus 4.6");
		expect(after.systemPromptSection.text).not.toContain("GPT 5.5");
	});
});

describe("probe safety (req 7)", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("defers the notice during a probe and drains it on the next clear agent_end", async () => {
		const { emit, state, pi } = setup(seededState({ lifecycle: lifecycleFor("probing") }));

		await emit("model_select", selectEvent(GPT));
		vi.advanceTimersByTime(500);

		// Gated: no notice, but the change is committed for attribution.
		expect(pi.harnessToolCalls).toHaveLength(0);
		expect(state.pendingNotifyModel?.id).toBe("gpt-5-5");
		expect(state.currentModel?.id).toBe("gpt-5-5");

		// Probe clears; the next agent_end drains the deferred notice on a 0ms tick.
		state.lifecycle = lifecycleFor("idle");
		await emit("agent_end", { messages: [] });
		vi.runAllTimers();

		expect(pi.harnessToolCalls).toHaveLength(1);
		expect(pi.harnessToolCalls[0].args).toEqual({ provider: "openai", model: "gpt-5-5", name: "GPT 5.5" });
		expect(state.pendingNotifyModel).toBeNull();
	});

	it("keeps deferring while the probe is still armed at agent_end", async () => {
		const { emit, state, pi } = setup(seededState({ lifecycle: lifecycleFor("probing") }));

		await emit("model_select", selectEvent(GPT));
		vi.advanceTimersByTime(500);

		// Still probing at agent_end → still deferred.
		await emit("agent_end", { messages: [] });
		vi.runAllTimers();

		expect(pi.harnessToolCalls).toHaveLength(0);
		expect(state.pendingNotifyModel?.id).toBe("gpt-5-5");
	});

	it("coalesces a superseding change while gated to the latest model only", async () => {
		const { emit, state, pi } = setup(seededState({ lifecycle: lifecycleFor("probing") }));

		await emit("model_select", selectEvent(fakeModel({ id: "interim", name: "Interim", provider: "p" })));
		vi.advanceTimersByTime(500);
		expect(state.pendingNotifyModel?.id).toBe("interim");

		await emit("model_select", selectEvent(GPT));
		vi.advanceTimersByTime(500);
		expect(state.pendingNotifyModel?.id).toBe("gpt-5-5");

		state.lifecycle = lifecycleFor("idle");
		await emit("agent_end", { messages: [] });
		vi.runAllTimers();

		expect(pi.harnessToolCalls).toHaveLength(1);
		expect(pi.harnessToolCalls[0].args).toMatchObject({ model: "gpt-5-5" });
	});
});

describe("suppression of agent-initiated switches", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("records the change for attribution but emits no notice", async () => {
		const { emit, state, pi } = setup(seededState({ suppressNextModelNotify: true }));

		await emit("model_select", selectEvent(GPT));

		// Suppression is synchronous: no debounce, flag cleared, history updated.
		expect(state.suppressNextModelNotify).toBe(false);
		expect(state.currentModel?.id).toBe("gpt-5-5");
		expect(state.modelHistory).toHaveLength(2);

		vi.advanceTimersByTime(500);
		expect(pi.harnessToolCalls).toHaveLength(0);
		expect(state.pendingNotifyModel).toBeNull();
	});

	it("drops a probe-deferred user notice that an agent switch supersedes", async () => {
		const { emit, state, pi } = setup(seededState({ lifecycle: lifecycleFor("probing") }));

		// A user change is gated behind the probe.
		const interim = fakeModel({ id: "interim", name: "Interim", provider: "p" });
		await emit("model_select", selectEvent(interim));
		vi.advanceTimersByTime(500);
		expect(state.pendingNotifyModel?.id).toBe("interim");

		// The agent then switches models itself (suppressed). The stale deferred notice
		// must be dropped so it never fires for the superseded model.
		state.suppressNextModelNotify = true;
		await emit("model_select", selectEvent(GPT));
		expect(state.pendingNotifyModel).toBeNull();
		expect(state.currentModel?.id).toBe("gpt-5-5");

		state.lifecycle = lifecycleFor("idle");
		await emit("agent_end", { messages: [] });
		vi.runAllTimers();

		expect(pi.harnessToolCalls).toHaveLength(0);
	});
});
