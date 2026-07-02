import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerModelChangeNotice } from "../src/model-change-notice.js";
import { registerModelSwitchTool } from "../src/model-switch-tool.js";
import { freshState, recordingPi } from "./helpers.js";

// Integration test (S12): wires the agent-callable `switch_scramjet_model` tool together
// with the `model_select` handler that owns user-initiated notice delivery, driving both
// through their real code paths on one shared pi. The two halves are otherwise only tested
// in isolation with hand-seeded intermediate state, which is exactly what let the F1 strand
// slip through: the switch tool sets `suppressNextModelNotify` and the notice handler
// consumes it, but no test composed them, so a stranded flag went unnoticed.

interface FakeModel {
	provider: string;
	id: string;
	name: string;
}

const INITIAL: FakeModel = { provider: "anthropic", id: "claude-opus-4-6", name: "Claude Opus 4.6" };
const X: FakeModel = { provider: "anthropic", id: "claude-opus-4-8", name: "Claude Opus 4.8" };
const Y: FakeModel = { provider: "openai", id: "gpt-5-5", name: "GPT 5.5" };

function record(m: FakeModel) {
	return { name: m.name, id: m.id, provider: m.provider, fromTurnIndex: 0 };
}

function fakeModelRegistry(models: FakeModel[]) {
	return {
		find: (provider: string, id: string) => models.find((m) => m.provider === provider && m.id === id),
		getAvailable: () => models,
	};
}

function integrationSetup() {
	const rec = recordingPi();
	// Seeded past the first user message so notices actually deliver (not pre-first-turn),
	// with the attribution ledger sitting on the initial model.
	const state = freshState({
		currentModel: record(INITIAL),
		modelHistory: [record(INITIAL)],
		hasUserMessage: true,
	});
	registerModelChangeNotice(rec.pi, state);
	registerModelSwitchTool(rec.pi, state);

	// liveModel mirrors agent.state.model: updated synchronously by setModel and by a user
	// UI switch, with model_select emitted only when the model actually changes — mirroring
	// AgentSession._emitModelSelect's modelsAreEqual early-return, the behavior at the heart
	// of F1 (a same-target setModel emits no model_select, so the handler never clears the flag).
	let liveModel: FakeModel = INITIAL;
	rec.pi.setModel = async (model: FakeModel) => {
		const changed = model.id !== liveModel.id;
		liveModel = model;
		if (changed) await rec.emit("model_select", { type: "model_select", model, source: "set" });
		return true;
	};
	async function uiSwitch(model: FakeModel) {
		liveModel = model;
		await rec.emit("model_select", { type: "model_select", model, source: "set" });
	}

	const registry = fakeModelRegistry([INITIAL, X, Y]);
	const switchTool = rec.tools.find((t: any) => t.name === "switch_scramjet_model");
	if (!switchTool) throw new Error("switch_scramjet_model tool not registered");
	const agentSwitch = (m: FakeModel) =>
		switchTool.execute("call-id", { provider: m.provider, model: m.id }, undefined, undefined, {
			modelRegistry: registry,
		} as any) as Promise<any>;

	return { ...rec, state, uiSwitch, agentSwitch };
}

describe("switch tool + model_select handler integration (S12 / F1)", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("does not strand the suppression flag when the agent switches to a model the user just picked", async () => {
		const { state, pi, uiSwitch, agentSwitch } = integrationSetup();
		const deliveredModels = () => pi.harnessToolCalls.map((c: any) => c.args.model);

		// 1. User switches to X via the UI. agent.state.model becomes X immediately; the notice
		//    debounce arms but state.currentModel (the ledger) still lags at INITIAL for up to 500ms.
		await uiSwitch(X);
		expect(state.currentModel?.id).toBe(INITIAL.id);

		// 2. Within that window, the agent calls switch_scramjet_model(X). The same-model guard
		//    compares the lagging ledger (INITIAL), so it does not treat this as a no-op and
		//    proceeds; setModel(X) is a live no-op that emits no model_select, so the handler
		//    never runs and cannot clear the flag it set. The success-path clear must.
		const result = await agentSwitch(X);
		expect(result.details).toMatchObject({ switched: true });
		expect(state.suppressNextModelNotify).toBe(false);

		// 3. The user's X switch settles and delivers its notice.
		vi.advanceTimersByTime(500);
		expect(deliveredModels()).toEqual([X.id]);

		// 4. A subsequent genuine user change to Y must still deliver its notice. If the flag had
		//    stranded true, this model_select would hit the suppress branch and be swallowed.
		await uiSwitch(Y);
		vi.advanceTimersByTime(500);

		expect(deliveredModels()).toEqual([X.id, Y.id]);
		expect(state.currentModel?.id).toBe(Y.id);
	});
});
