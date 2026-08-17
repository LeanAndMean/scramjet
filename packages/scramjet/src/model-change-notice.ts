/**
 * scramjet_model_change_notice tool + user-initiated model-change delivery (issue 244, Stage 5).
 *
 * This module owns `model_select` handling entirely. It is the second of the three
 * separated concerns (see model-switch-tool.ts for agent-callable switching and
 * model-identity.ts for the frozen system-prompt section + attribution ledger):
 * communicating a *user-initiated* model change to the agent through a real, replayable
 * tool artifact rather than a user-role message or context mutation.
 *
 * The notice tool is `activation: "harness-only"` — never provider-visible, never
 * model-callable — and is executed for real through `pi.invokeHarnessTool`, so its
 * `tool_execution_*` events, `ToolResultMessage`, persistence, and TUI rows are
 * identical to any other tool by construction (not emulation).
 *
 * Delivery, per acceptance criteria:
 * - **Suppression:** an agent-initiated switch (switch_scramjet_model set
 *   `state.suppressNextModelNotify` before `pi.setModel`, which emits `model_select`
 *   synchronously) records the change for attribution but fires no notice — the switch
 *   tool's own row is the transcript record. The flag is read/cleared synchronously here.
 * - **Debounce + coalescing:** rapid user cycling within 500ms collapses to one settle
 *   for the final model; intermediate models never reach a delivery attempt.
 * - **Open identity epoch:** selections update `state.currentModel` synchronously. A
 *   selection is suppressed when the first dispatched request uses it, but retained for
 *   normal notice delivery when an older prepared request freezes a different model.
 * - **Probe safety:** if a probe is armed/in-flight at settle, delivery is deferred
 *   (`state.pendingNotifyModel`) and drained on the next non-probe `agent_end` — the
 *   notice never appears in a probe provider call.
 * - **Routing** is handled by the Stage 2 primitive: `invokeHarnessTool` executes
 *   idle-immediate or queues mid-run transparently, and `prepareNextTurn` routes the
 *   next intra-run LLM call to the newly-selected model.
 *
 * Routing state (`state.currentModel` and `state.modelHistory`) commits synchronously
 * on selection. Debounce controls only notice delivery, while contributor attribution
 * is derived separately from material provider-origin responses.
 */

import type { ExtensionAPI, ExtensionContext } from "@leanandmean/coding-agent";
import { Type } from "typebox";
import { modelRecord } from "./model-identity.js";
import { MODEL_CHANGE_NOTICE_TOOL, type ModelRecord, type ScramjetState } from "./types.js";

type ActiveModel = NonNullable<ExtensionContext["model"]>;

const DEBOUNCE_MS = 500;

function sameModel(a: Pick<ModelRecord, "provider" | "id">, b: Pick<ModelRecord, "provider" | "id">): boolean {
	return a.provider === b.provider && a.id === b.id;
}

export { MODEL_CHANGE_NOTICE_TOOL };

const PARAMETERS = Type.Object({
	provider: Type.String({ description: "Provider of the newly-selected model." }),
	model: Type.String({ description: "ID of the newly-selected model." }),
	name: Type.String({ description: "Display name of the newly-selected model." }),
});

export function buildNoticeText(params: { provider: string; model: string; name: string }): string {
	return (
		`The active model changed to ${params.name} (ID: ${params.model}, provider: ${params.provider}). ` +
		"This was a user-initiated change; continue with the new model."
	);
}

export function registerModelChangeNotice(pi: ExtensionAPI, state: ScramjetState): void {
	// A structurally harness-only tool: the harness invokes it to record a user-initiated
	// model change; the model can never call it (never in the provider-visible tool set).
	pi.registerTool({
		name: MODEL_CHANGE_NOTICE_TOOL,
		label: "Model Change Notice",
		description:
			"System-generated notice that the active harness model changed mid-session. Invoked by the " +
			"harness (never by the model) to record a user-initiated model switch as a real tool row in " +
			"the transcript.",
		activation: "harness-only",
		parameters: PARAMETERS,
		async execute(_toolCallId, params) {
			return {
				content: [{ type: "text", text: buildNoticeText(params) }],
				details: { provider: params.provider, model: params.model, name: params.name },
			};
		},
	});

	let latestTurnIndex = 0;
	let debounceTimer: ReturnType<typeof setTimeout> | null = null;
	let pendingRecord: ModelRecord | null = null;
	let debounceOrigin: ModelRecord | null = null;

	function clearDebounce() {
		if (debounceTimer !== null) {
			clearTimeout(debounceTimer);
			debounceTimer = null;
		}
		pendingRecord = null;
		debounceOrigin = null;
		state.pendingOpenEpochNotifyModel = null;
	}

	function commitModel(model: ActiveModel): ModelRecord | null {
		if (state.currentModel && model.provider === state.currentModel.provider && model.id === state.currentModel.id) {
			return null;
		}
		const record = modelRecord(model, latestTurnIndex);
		state.currentModel = record;
		state.modelHistory.push(record);
		return record;
	}

	function deliverNotice(record: ModelRecord): void {
		void pi
			.invokeHarnessTool(MODEL_CHANGE_NOTICE_TOOL, {
				provider: record.provider,
				model: record.id,
				name: record.name,
			})
			.catch((err) => {
				state.logger.warn("model-notice", "model-change notice delivery failed", {
					provider: record.provider,
					model: record.id,
					error: err instanceof Error ? err.message : String(err),
				});
			});
	}

	function settle(record: ModelRecord): void {
		// Probe-gated: defer past the probe so the notice never lands in a probe provider
		// call. Store only the latest (structural coalescing); drained on agent_end.
		if (state.lifecycle.probeArmed || state.lifecycle.probeInFlight) {
			state.pendingNotifyModel = record;
			return;
		}

		// Clear: deliver now. invokeHarnessTool routes idle-immediate vs mid-run-queue
		// transparently based on isStreaming (Stage 2 primitive).
		state.pendingNotifyModel = null;
		deliverNotice(record);
	}

	pi.on("model_select", (event) => {
		if (event.source === "restore") return;
		const previous = state.currentModel;
		const record = commitModel(event.model);

		if (state.suppressNextModelNotify) {
			state.suppressNextModelNotify = false;
			clearDebounce();
			state.pendingNotifyModel = null;
			return;
		}
		if (!record) return;

		state.pendingNotifyModel = null;
		state.pendingOpenEpochNotifyModel = null;
		if (debounceTimer !== null) {
			clearTimeout(debounceTimer);
		} else {
			debounceOrigin = previous;
		}
		pendingRecord = record;
		debounceTimer = setTimeout(() => {
			debounceTimer = null;
			const latest = pendingRecord;
			pendingRecord = null;
			const returnedToOrigin =
				latest && debounceOrigin && latest.provider === debounceOrigin.provider && latest.id === debounceOrigin.id;
			debounceOrigin = null;
			if (!latest || returnedToOrigin) return;
			if (!state.identityEpochFrozen) {
				state.pendingOpenEpochNotifyModel = latest;
				return;
			}
			if (!state.identityEpochModel || !sameModel(latest, state.identityEpochModel)) settle(latest);
		}, DEBOUNCE_MS);
	});

	pi.on("before_provider_request", (event) => {
		const record = state.pendingOpenEpochNotifyModel;
		if (!record || !state.identityEpochFrozen || !state.identityEpochModel) return;
		state.pendingOpenEpochNotifyModel = null;
		if (!sameModel(record, event.model)) settle(record);
	});

	// Drain a probe-deferred notice once the probe clears. Deferred on a setTimeout(0)
	// so it lands after the run settles (isStreaming still true during agent_end) on the
	// idle path, mirroring auto-continue.ts's probe/dispatch defer — including the
	// lifecycleGeneration/pending staleness re-check idiom.
	pi.on("agent_end", () => {
		const record = state.pendingNotifyModel;
		if (!record) return;
		if (state.lifecycle.probeArmed || state.lifecycle.probeInFlight) return;
		const scheduledGeneration = state.lifecycleGeneration;
		setTimeout(() => {
			if (state.pendingNotifyModel !== record) return;
			if (state.lifecycleGeneration !== scheduledGeneration) return;
			if (state.lifecycle.probeArmed || state.lifecycle.probeInFlight) return;
			state.pendingNotifyModel = null;
			deliverNotice(record);
		}, 0);
	});

	pi.on("turn_start", (event) => {
		latestTurnIndex = event.turnIndex;
	});

	// Rebuilds clear transient notice state; model-identity.ts independently reconstructs
	// routing and contributors and opens a new request-bound identity epoch.
	pi.on("session_start", () => {
		clearDebounce();
		state.pendingNotifyModel = null;
		latestTurnIndex = 0;
	});

	pi.on("session_tree", () => {
		clearDebounce();
		state.pendingNotifyModel = null;
		latestTurnIndex = 0;
	});

	pi.on("session_compact", () => {
		clearDebounce();
		state.pendingNotifyModel = null;
	});

	pi.on("session_shutdown", () => {
		clearDebounce();
	});
}
