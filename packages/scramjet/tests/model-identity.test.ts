import type { SessionEntry } from "@leanandmean/coding-agent";
import { describe, expect, it } from "vitest";
import { MODEL_CHANGE_NOTICE_TOOL } from "../src/model-change-notice.js";
import { reconstructModelState, registerModelIdentity } from "../src/model-identity.js";
import { freshState, recordingPi } from "./helpers.js";

// Stage 5 (issue 244) moved all model_select debounce/delivery machinery out of
// model-identity.ts into model-change-notice.ts. Stage 6 completes the separation: this
// module now owns only the frozen # Model Identity system-prompt section and the
// reconstruction ledger. The pre-first-turn boundary is the shared state.hasUserMessage
// fact — latched live by model-change-notice.ts's `input` observer (covered in
// model-change-notice.test.ts) and re-derived from the branch here on rebuild.

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
	return { pi, handlers, emit, state };
}

const STARTUP = { type: "session_start", reason: "startup" } as const;

describe("registerModelIdentity", () => {
	it("registers session_start, session_tree, and before_agent_start handlers only", () => {
		const { handlers } = setup();
		expect(handlers.get("session_start")).toHaveLength(1);
		expect(handlers.get("session_tree")).toHaveLength(1);
		expect(handlers.get("before_agent_start")).toHaveLength(1);
		// The section is now frozen by state.hasUserMessage, not a local turn latch, so
		// model-identity no longer needs a turn_start handler.
		expect(handlers.get("turn_start")).toBeUndefined();
	});

	it("captures the initial model on session_start", async () => {
		const { emit, state } = setup();

		await emit("session_start", STARTUP, { model: fakeModel() });

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

		await emit("session_start", STARTUP, { model: undefined });
		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as Record<string, unknown>;

		expect(state.currentModel).toBeNull();
		expect(state.modelHistory).toEqual([]);
		expect(result).toEqual({});
	});

	it("appends stable model identity context on every before_agent_start once latched", async () => {
		const { handlers, emit, state } = setup();

		await emit("session_start", STARTUP, { model: fakeModel() });
		state.hasUserMessage = true; // first user message exists → section latches
		const handler = handlers.get("before_agent_start")![0];
		const first = (await handler({ systemPrompt: "BASE" })) as {
			systemPromptSection: { id: string; text: string };
			systemPrompt?: unknown;
			message?: unknown;
		};
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

	it("freezes the section to the model live at the first user message (cache stability)", async () => {
		const { handlers, emit, state } = setup();
		await emit("session_start", STARTUP, { model: fakeModel() });
		const handler = handlers.get("before_agent_start")![0];

		// First user message latches the section, then the live model changes underneath it.
		state.hasUserMessage = true;
		const before = (await handler({ systemPrompt: "BASE" })) as any;
		state.currentModel = { name: "GPT 5.5", id: "gpt-5-5", provider: "openai", fromTurnIndex: 1 };
		const after = (await handler({ systemPrompt: "BASE" })) as any;

		expect(after.systemPromptSection.text).toBe(before.systemPromptSection.text);
		expect(after.systemPromptSection.text).toContain("Claude Opus 4.6");
		expect(after.systemPromptSection.text).not.toContain("GPT 5.5");
	});

	it("re-enters the pre-first-turn state on a fresh session (reason new, e.g. /clear)", async () => {
		const { emit, state } = setup();
		await emit("session_start", STARTUP, { model: fakeModel() });
		state.hasUserMessage = true; // user sent messages in the prior session

		// /clear starts a brand-new empty session (reason "new").
		await emit("session_start", { type: "session_start", reason: "new" }, { model: fakeModel() });

		// A stale hasUserMessage here would make a pre-first-turn change fire a notice with
		// no preceding user message (session-ordering-invariant violation).
		expect(state.hasUserMessage).toBe(false);
	});

	it("preserves the boundary across a mid-conversation reload", async () => {
		const { emit, state } = setup();
		await emit("session_start", STARTUP, { model: fakeModel() });
		state.hasUserMessage = true; // conversation is past its first user message

		// reload re-emits session_start mid-conversation without clearing the branch.
		await emit("session_start", { type: "session_start", reason: "reload" }, { model: fakeModel() });

		expect(state.hasUserMessage).toBe(true);
	});

	it("reflects a pre-first-turn model change, then freezes it at the first user message (Scenario 1)", async () => {
		const { handlers, emit, state } = setup();
		await emit("session_start", STARTUP, { model: fakeModel() });
		const handler = handlers.get("before_agent_start")![0];

		// Change committed before any user message → section tracks it live (no tool call).
		state.currentModel = { name: "GPT 5.5", id: "gpt-5-5", provider: "openai", fromTurnIndex: 0 };
		const preTurn = (await handler({ systemPrompt: "BASE" })) as any;
		expect(preTurn.systemPromptSection.text).toContain("GPT 5.5");
		expect(preTurn.systemPromptSection.text).toContain("gpt-5-5");

		// First user message latches the *changed* model, not the session's original.
		state.hasUserMessage = true;
		const latched = (await handler({ systemPrompt: "BASE" })) as any;
		expect(latched.systemPromptSection.text).toContain("GPT 5.5");

		// A later change no longer moves the frozen section — it arrives as a notice.
		state.currentModel = { name: "Claude Sonnet 4", id: "claude-sonnet-4", provider: "anthropic", fromTurnIndex: 2 };
		const frozen = (await handler({ systemPrompt: "BASE" })) as any;
		expect(frozen.systemPromptSection.text).toContain("GPT 5.5");
		expect(frozen.systemPromptSection.text).not.toContain("Claude Sonnet 4");
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

// A harness-minted notice pair: a single-toolCall assistant message followed by its
// toolResult (role "toolResult", never "user"/"assistant"). Shapes match what
// invokeHarnessTool(scramjet_model_change_notice) persists.
function noticeCallEntry(id?: string): SessionEntry {
	return {
		type: "message",
		id: id ?? "msg-notice-call",
		parentId: null,
		timestamp: "0",
		message: {
			role: "assistant",
			content: [{ type: "toolCall", id: "tc-notice", name: MODEL_CHANGE_NOTICE_TOOL, arguments: {} }],
		},
	} as any;
}

function noticeResultEntry(id?: string): SessionEntry {
	return {
		type: "message",
		id: id ?? "msg-notice-result",
		parentId: null,
		timestamp: "0",
		message: { role: "toolResult", toolName: MODEL_CHANGE_NOTICE_TOOL, content: [{ type: "text", text: "noted" }] },
	} as any;
}

describe("reconstructModelState", () => {
	it("returns empty state when no model_change entries exist", () => {
		const result = reconstructModelState([], undefined);
		expect(result.currentModel).toBeNull();
		expect(result.modelHistory).toEqual([]);
		expect(result.diverged).toBe(false);
		expect(result.hasUserMessage).toBe(false);
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

	it("skips a scramjet_model_change_notice assistant message when counting turns (requirement 13)", () => {
		const entries = [
			modelChangeEntry("anthropic", "claude-opus-4-6"),
			messageEntry("user"),
			messageEntry("assistant"), // one real assistant turn
			noticeCallEntry(), // harness-minted notice call — must NOT count
			noticeResultEntry(), // its toolResult — role "toolResult", also not counted
			modelChangeEntry("openai", "gpt-5-5"),
		];
		const result = reconstructModelState(entries, undefined);

		expect(result.modelHistory).toHaveLength(2);
		// Without the guard the notice assistant message would inflate this to 2.
		expect(result.modelHistory[1]!.fromTurnIndex).toBe(1);
	});

	it("counts a real assistant message that merely calls another tool", () => {
		// Guard is specific to a *sole* scramjet_model_change_notice toolCall: a normal
		// tool-using assistant turn still counts.
		const entries = [
			modelChangeEntry("anthropic", "claude-opus-4-6"),
			messageEntry("user"),
			{
				type: "message",
				id: "m",
				parentId: null,
				timestamp: "0",
				message: { role: "assistant", content: [{ type: "toolCall", id: "t", name: "read", arguments: {} }] },
			} as any,
			modelChangeEntry("openai", "gpt-5-5"),
		];
		const result = reconstructModelState(entries, undefined);

		expect(result.modelHistory[1]!.fromTurnIndex).toBe(1);
	});

	it("reports hasUserMessage from the presence of a user message", () => {
		expect(reconstructModelState([messageEntry("user")], undefined).hasUserMessage).toBe(true);
		expect(reconstructModelState([messageEntry("assistant")], undefined).hasUserMessage).toBe(false);
		// A notice toolResult is role "toolResult", not a user message.
		expect(reconstructModelState([noticeResultEntry()], undefined).hasUserMessage).toBe(false);
	});

	it("captures the model live at the first user message, not the branch's earliest", () => {
		// A→B→C all before the first user message; the frozen section latched C live.
		const entries = [
			modelChangeEntry("anthropic", "model-a"),
			modelChangeEntry("anthropic", "model-b"),
			modelChangeEntry("openai", "model-c"),
			messageEntry("user"),
			messageEntry("assistant"),
			modelChangeEntry("anthropic", "model-d"), // post-first-message change: must not move it
			messageEntry("user", "msg-user-2"),
		];
		const result = reconstructModelState(entries, undefined);

		expect(result.modelAtFirstUserMessage!.id).toBe("model-c");
		expect(result.currentModel!.id).toBe("model-d");
	});

	it("reports a null modelAtFirstUserMessage when nothing precedes the first user message", () => {
		expect(reconstructModelState([messageEntry("user")], undefined).modelAtFirstUserMessage).toBeNull();
		expect(reconstructModelState([], undefined).modelAtFirstUserMessage).toBeNull();
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

	it("resolves display names from a registry when provided (F3)", () => {
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")];
		const resolver = (provider: string, modelId: string) => {
			if (provider === "anthropic" && modelId === "claude-opus-4-6") return "Claude Opus 4.6";
			return undefined;
		};
		const result = reconstructModelState(entries, undefined, resolver);

		expect(result.currentModel!.name).toBe("Claude Opus 4.6");
		expect(result.currentModel!.id).toBe("claude-opus-4-6");
	});

	it("falls back to model id when the resolver returns undefined (F3)", () => {
		const entries = [modelChangeEntry("anthropic", "unknown-model")];
		const resolver = () => undefined;
		const result = reconstructModelState(entries, undefined, resolver);

		expect(result.currentModel!.name).toBe("unknown-model");
	});

	it("ignores non-model_change entries", () => {
		const entries = [
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

		await emit("session_start", STARTUP, { model: fakeModel() });

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
		await emit("session_start", STARTUP, { model: fakeModel() });
		expect(state.currentModel).not.toBeNull();

		const ctx = ctxWithBranchAndModel([], fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" }));
		await emit("session_tree", { type: "session_tree", newLeafId: "leaf1", oldLeafId: "leaf0" }, ctx);

		expect(state.currentModel!.id).toBe("gpt-5-5");
		expect(state.currentModel!.name).toBe("GPT 5.5");
		expect(state.modelHistory).toHaveLength(1);
	});

	it("resets to null when branch is empty and ctx.model is undefined", async () => {
		const { emit, state } = setup();
		await emit("session_start", STARTUP, { model: fakeModel() });

		const ctx = ctxWithBranchAndModel([], undefined);
		await emit("session_tree", { type: "session_tree", newLeafId: "leaf1", oldLeafId: "leaf0" }, ctx);

		expect(state.currentModel).toBeNull();
		expect(state.modelHistory).toEqual([]);
	});

	it("resumes past the pre-first-turn boundary when the branch has a user message", async () => {
		const { handlers, emit, state } = setup();
		const entries = [
			modelChangeEntry("anthropic", "claude-opus-4-6"),
			messageEntry("user"),
			messageEntry("assistant"),
		];
		const ctx = ctxWithBranchAndModel(entries, fakeModel());
		await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

		// hasUserMessage reconstructed → a later change delivers via notice, not a prompt edit.
		expect(state.hasUserMessage).toBe(true);

		const handler = handlers.get("before_agent_start")![0];
		const first = (await handler({ systemPrompt: "BASE" })) as any;
		expect(first.systemPromptSection.text).toContain("Claude Opus 4.6");

		// The section is frozen: a subsequent model change must not move it.
		state.currentModel = { name: "GPT 5.5", id: "gpt-5-5", provider: "openai", fromTurnIndex: 3 };
		const after = (await handler({ systemPrompt: "BASE" })) as any;
		expect(after.systemPromptSection.text).toBe(first.systemPromptSection.text);
		expect(after.systemPromptSection.text).not.toContain("GPT 5.5");
	});

	it("resumes pre-first-turn when the branch has no user message (section tracks live)", async () => {
		const { handlers, emit, state } = setup();
		const entries = [modelChangeEntry("anthropic", "claude-opus-4-6")]; // no user message yet
		const ctx = ctxWithBranchAndModel(entries, fakeModel());
		await emit("session_start", { type: "session_start", reason: "fork" }, ctx);

		expect(state.hasUserMessage).toBe(false);

		const handler = handlers.get("before_agent_start")![0];
		const first = (await handler({ systemPrompt: "BASE" })) as any;
		expect(first.systemPromptSection.text).toContain("Claude Opus 4.6");

		// Pre-first-turn: a change is reflected live in the still-unfrozen section.
		state.currentModel = { name: "GPT 5.5", id: "gpt-5-5", provider: "openai", fromTurnIndex: 0 };
		const after = (await handler({ systemPrompt: "BASE" })) as any;
		expect(after.systemPromptSection.text).toContain("GPT 5.5");
	});

	it("freezes the resumed section on the model live at the first user message, not the earliest", async () => {
		const { handlers, emit } = setup();
		// Two pre-first-message changes: the original session froze on model-c.
		const entries = [
			modelChangeEntry("anthropic", "model-a"),
			modelChangeEntry("openai", "model-c"),
			messageEntry("user"),
			messageEntry("assistant"),
		];
		const ctx = ctxWithBranchAndModel(entries, fakeModel({ id: "model-c", name: "Model C", provider: "openai" }));
		await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as any;
		expect(result.systemPromptSection.text).toContain("model-c");
		expect(result.systemPromptSection.text).not.toContain("model-a");
	});

	it("freezes the section to the reconstructed initial model on a diverged resume", async () => {
		const { handlers, emit, state } = setup();
		const entries = [
			modelChangeEntry("anthropic", "claude-opus-4-6"),
			messageEntry("user"),
			messageEntry("assistant"),
		];
		// ctx.model diverges from the last recorded change.
		const ctx = ctxWithBranchAndModel(entries, fakeModel({ id: "gpt-5-5", name: "GPT 5.5", provider: "openai" }));
		await emit("session_start", { type: "session_start", reason: "resume" }, ctx);

		// Ledger tracks the diverged current model for attribution...
		expect(state.currentModel!.id).toBe("gpt-5-5");
		expect(state.modelHistory).toHaveLength(2);
		expect(state.hasUserMessage).toBe(true);

		// ...but the frozen identity section reflects the session's initial model.
		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as any;
		expect(result.systemPromptSection.text).toContain("claude-opus-4-6");
		expect(result.systemPromptSection.text).not.toContain("GPT 5.5");
	});
});
