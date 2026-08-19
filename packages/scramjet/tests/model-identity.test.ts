import type { SessionEntry } from "@leanandmean/coding-agent";
import { describe, expect, it } from "vitest";
import { reconstructModelState, registerModelIdentity } from "../src/model-identity.js";
import { freshState, recordingPi } from "./helpers.js";

function fakeModel(overrides: Record<string, unknown> = {}) {
	return { id: "model-a", name: "Model A", provider: "provider-a", ...overrides } as any;
}

function setup() {
	const rec = recordingPi();
	const state = freshState();
	registerModelIdentity(rec.pi, state);
	return { ...rec, state };
}

function modelChange(provider: string, modelId: string): SessionEntry {
	return { type: "model_change", provider, modelId } as any;
}

function assistant(overrides: Record<string, unknown> = {}): SessionEntry {
	return {
		type: "message",
		message: {
			role: "assistant",
			origin: "provider",
			provider: "provider-a",
			model: "model-a",
			content: [{ type: "text", text: "work" }],
			stopReason: "stop",
			...overrides,
		},
	} as any;
}

const STARTUP = { type: "session_start", reason: "startup" } as const;

describe("request-bound identity epochs", () => {
	it("registers provider-bound, compaction, reconstruction, and contributor handlers", () => {
		const { handlers } = setup();
		for (const event of [
			"session_start",
			"session_tree",
			"session_compact",
			"before_provider_call",
			"before_provider_request",
			"message_end",
		]) {
			expect(handlers.get(event)).toHaveLength(1);
		}
		expect(handlers.get("before_agent_start")).toBeUndefined();
		expect(handlers.get("input")).toBeUndefined();
	});

	it("stays open through preparation and freezes only at matching provider dispatch", async () => {
		const { handlers, emit, state } = setup();
		await emit("session_start", STARTUP, { model: fakeModel() });
		const prepare = handlers.get("before_provider_call")![0];
		const dispatch = handlers.get("before_provider_request")![0];
		const modelB = fakeModel({ id: "model-b", name: "Model B", provider: "provider-b" });

		const first = (await prepare({ model: fakeModel(), systemPrompt: "BASE" })) as any;
		const second = (await prepare({ model: modelB, systemPrompt: "BASE" })) as any;
		expect(first.systemPrompt).toContain("Model A");
		expect(second.systemPrompt).toContain("Model B");
		expect(state.identityEpochFrozen).toBe(false);

		await dispatch({ model: fakeModel(), payload: {} });
		expect(state.identityEpochFrozen).toBe(false);
		await dispatch({ model: modelB, payload: {} });
		expect(state.identityEpochFrozen).toBe(true);
		expect(state.identityEpochModel).toMatchObject({ provider: "provider-b", id: "model-b" });

		const frozen = (await prepare({ model: fakeModel(), systemPrompt: "CHANGED" })) as any;
		expect(frozen.systemPrompt).toContain("CHANGED");
		expect(frozen.systemPrompt).toContain("Model B");
		expect(frozen.systemPrompt).not.toContain("Model A");
		expect(frozen.systemPrompt.split("# Model Identity")[1]).toBe(second.systemPrompt.split("# Model Identity")[1]);
	});

	it("inserts structured identity before the volatile tail without mutating input", async () => {
		const { handlers, emit } = setup();
		await emit("session_start", STARTUP, { model: fakeModel() });
		const sections = [
			{ id: "stable", text: "stable" },
			{ id: "volatile", text: "volatile", cacheRetention: "none" },
		];
		const result = (await handlers.get("before_provider_call")![0]({
			model: fakeModel(),
			systemPrompt: sections,
		})) as any;

		expect(sections).toHaveLength(2);
		expect(result.systemPrompt.map((section: any) => section.id)).toEqual([
			"stable",
			"scramjet:model-identity",
			"volatile",
		]);
	});

	it("successful compaction reopens the epoch and clears deferred notice state", async () => {
		const { handlers, emit, state } = setup();
		await emit("session_start", STARTUP, { model: fakeModel() });
		const prepare = handlers.get("before_provider_call")![0];
		await prepare({ model: fakeModel(), systemPrompt: "BASE" });
		await emit("before_provider_request", { model: fakeModel(), payload: {} });
		state.pendingNotifyModel = { name: "B", id: "b", provider: "p", fromTurnIndex: 0 };

		await emit("session_compact", { type: "session_compact" });
		expect(state.identityEpochFrozen).toBe(false);
		expect(state.identityEpochModel).toBeNull();
		expect(state.pendingNotifyModel).toBeNull();
		const result = (await prepare({
			model: fakeModel({ id: "model-b", name: "Model B", provider: "provider-b" }),
			systemPrompt: "NEW",
		})) as any;
		expect(result.systemPrompt).toContain("Model B");
	});

	it("adds ordered unique contributors only from material provider responses", async () => {
		const { emit, state } = setup();
		const ctx = { modelRegistry: { find: (_provider: string, id: string) => ({ name: id.toUpperCase() }) } };
		await emit("message_end", { message: (assistant() as any).message }, ctx);
		await emit("message_end", { message: (assistant() as any).message }, ctx);
		await emit(
			"message_end",
			{ message: (assistant({ model: "model-b", provider: "provider-b" }) as any).message },
			ctx,
		);
		await emit("message_end", { message: (assistant({ origin: "harness", model: "model-c" }) as any).message }, ctx);
		await emit("message_end", { message: (assistant({ content: [], model: "model-d" }) as any).message }, ctx);
		await emit(
			"message_end",
			{ message: (assistant({ stopReason: "error", model: "model-e" }) as any).message },
			ctx,
		);

		expect(state.modelContributors.map((model) => `${model.provider}/${model.id}`)).toEqual([
			"provider-a/model-a",
			"provider-b/model-b",
		]);
	});
});

describe("reconstructModelState", () => {
	it("keeps routing history separate from provider-response contributors", () => {
		const entries = [
			modelChange("provider-a", "model-a"),
			assistant(),
			modelChange("provider-b", "selection-only"),
			assistant({ provider: "provider-b", model: "model-b" }),
		];
		const result = reconstructModelState(entries, undefined);

		expect(result.modelHistory.map((model) => model.id)).toEqual(["model-a", "selection-only"]);
		expect(result.contributors.map((model) => model.id)).toEqual(["model-a", "model-b"]);
	});

	it("excludes harness, empty, failed, aborted, and known legacy synthetic messages", () => {
		const entries = [
			assistant({ origin: "harness" }),
			assistant({ content: [] }),
			assistant({ stopReason: "error" }),
			assistant({ stopReason: "aborted" }),
			assistant({
				origin: undefined,
				content: [{ type: "toolCall", id: "t", name: "scramjet_model_change_notice", arguments: {} }],
			}),
			assistant({
				origin: undefined,
				content: [{ type: "toolCall", id: "t", name: "scramjet_next_step_selection", arguments: {} }],
			}),
		];
		expect(reconstructModelState(entries, undefined).contributors).toEqual([]);
	});

	it("counts real tool-use and keeps same IDs under different providers distinct", () => {
		const entries = [
			assistant({ content: [{ type: "toolCall", id: "t", name: "read", arguments: {} }] }),
			assistant({ provider: "provider-b" }),
		];
		expect(reconstructModelState(entries, undefined).contributors.map((model) => model.provider)).toEqual([
			"provider-a",
			"provider-b",
		]);
	});

	it("reconstructs the exact current provider and reopens on resume", async () => {
		const { emit, state } = setup();
		const entries = [modelChange("provider-a", "same-id"), assistant()];
		const current = fakeModel({ id: "same-id", name: "Other Same ID", provider: "provider-b" });
		await emit(
			"session_start",
			{ type: "session_start", reason: "resume" },
			{
				model: current,
				modelRegistry: { find: () => undefined },
				sessionManager: { getBranch: () => entries },
			},
		);

		expect(state.currentModel).toMatchObject({ provider: "provider-b", id: "same-id" });
		expect(state.modelContributors).toHaveLength(1);
		expect(state.identityEpochFrozen).toBe(false);
	});
});
