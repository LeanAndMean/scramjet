import type { SessionEntry } from "@leanandmean/coding-agent";
import { describe, expect, it } from "vitest";
import { reconstructModelState, registerModelIdentity } from "../src/model-identity.js";
import { freshState, recordingPi } from "./helpers.js";

// Stage 5 (issue 244) moved all model_select debounce/delivery machinery out of
// model-identity.ts into model-change-notice.ts. This file now covers only the
// retained surface: the frozen # Model Identity system-prompt section and the
// reconstruction ledger. Delivery behavior is covered in model-change-notice.test.ts.
// The reconstruction guard for notice entries and hasUserMessage branch
// reconstruction land in Stage 6.

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
		expect(first.systemPromptSection.text).toContain("scramjet_model_change_notice tool results");
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

	it("keeps the system prompt on the initial model after the first turn latches it", async () => {
		const { handlers, emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		const handler = handlers.get("before_agent_start")![0];
		const before = (await handler({ systemPrompt: "BASE" })) as any;

		// First turn latches the section, then the live model changes underneath it.
		await emit("turn_start", { type: "turn_start", turnIndex: 0, timestamp: 0 });
		state.currentModel = { name: "GPT 5.5", id: "gpt-5-5", provider: "openai", fromTurnIndex: 1 };

		const after = (await handler({ systemPrompt: "BASE" })) as any;

		expect(after.systemPromptSection.text).toBe(before.systemPromptSection.text);
		expect(after.systemPromptSection.text).toContain("Claude Opus 4.6");
		expect(after.systemPromptSection.text).not.toContain("GPT 5.5");
	});

	it("reflects a pre-first-turn model change in the system prompt", async () => {
		const { handlers, emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });

		// Change committed before any turn_start → section tracks it live.
		state.currentModel = { name: "GPT 5.5", id: "gpt-5-5", provider: "openai", fromTurnIndex: 0 };

		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as any;

		expect(result.systemPromptSection.text).toContain("GPT 5.5");
		expect(result.systemPromptSection.text).toContain("gpt-5-5");
	});
});

function modelChangeEntry(provider: string, modelId: string, id?: string): SessionEntry {
	return {
		type: "model_change",
		id: id ?? `mc-${provider}-${modelId}`,
		parentId: null,
		timestamp: "0",
		provider,
		modelId,
	} as any;
}

function messageEntry(role: "user" | "assistant", id?: string): SessionEntry {
	return {
		type: "message",
		id: id ?? `msg-${role}`,
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

	it("resets history when branch has no model_change entries", async () => {
		const { emit, state } = setup();
		await emit("session_start", { type: "session_start", reason: "startup" }, { model: fakeModel() });
		expect(state.currentModel).not.toBeNull();

		const ctx = ctxWithBranchAndModel([], fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" }));
		await emit("session_tree", { type: "session_tree", newLeafId: "leaf1", oldLeafId: "leaf0" }, ctx);

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
});
