import { describe, expect, it } from "vitest";
import { registerModelIdentity } from "../model-identity.ts";
import { freshState, recordingPi } from "./helpers.ts";

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
