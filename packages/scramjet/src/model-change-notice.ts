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
 * - **Pre-first-turn:** before the first user message (`state.hasUserMessage`), a change
 *   only updates `state.currentModel`; model-identity.ts reflects it in the system prompt
 *   with no tool call (Scenario 1).
 * - **Probe safety:** if a probe is armed/in-flight at settle, delivery is deferred
 *   (`state.pendingNotifyModel`) and drained on the next non-probe `agent_end` — the
 *   notice never appears in a probe provider call.
 * - **Routing** is handled by the Stage 2 primitive: `invokeHarnessTool` executes
 *   idle-immediate or queues mid-run transparently, and `prepareNextTurn` routes the
 *   next intra-run LLM call to the newly-selected model.
 *
 * `state.modelHistory`/`currentModel` are committed at settle regardless of delivery
 * gating, so attribution stays accurate while narration defers.
 */

import type { ExtensionAPI, ExtensionContext } from "@leanandmean/coding-agent";
import { Type } from "typebox";
import { modelRecord } from "./model-identity.js";
import { MODEL_CHANGE_NOTICE_TOOL, type ModelRecord, type ScramjetState } from "./types.js";

type ActiveModel = NonNullable<ExtensionContext["model"]>;

const DEBOUNCE_MS = 500;

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
	let pendingModel: ActiveModel | null = null;

	function clearDebounce() {
		if (debounceTimer !== null) {
			clearTimeout(debounceTimer);
			debounceTimer = null;
		}
		pendingModel = null;
	}

	// Commit a change to the attribution ledger. Returns the new record, or null when
	// the target equals the current model (a no-op cycle contributes no history entry).
	function commitModel(model: ActiveModel): ModelRecord | null {
		if (state.currentModel && model.id === state.currentModel.id) return null;
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

	function settle(model: ActiveModel): void {
		const record = commitModel(model);
		if (!record) return;

		// Pre-first-turn: the system prompt reflects the change directly (model-identity.ts
		// reads state.currentModel while the identity section is still live). No tool call.
		if (!state.hasUserMessage) {
			state.pendingNotifyModel = null;
			return;
		}

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

		// Agent-initiated switch: record for attribution, emit no notice. Also drop any
		// earlier probe-deferred user notice — the model has moved on again, so a queued
		// notice about the prior model is now stale and must not fire.
		if (state.suppressNextModelNotify) {
			state.suppressNextModelNotify = false;
			clearDebounce();
			state.pendingNotifyModel = null;
			commitModel(event.model);
			return;
		}

		if (debounceTimer !== null) clearTimeout(debounceTimer);
		pendingModel = event.model;
		debounceTimer = setTimeout(() => {
			debounceTimer = null;
			const model = pendingModel;
			pendingModel = null;
			if (model) settle(model);
		}, DEBOUNCE_MS);
	});

	// Observe (never transform) user input to mark that the first user message exists.
	// Harness sendMessage traffic (probes, notices) never emits `input`, so this stays
	// user-originated.
	pi.on("input", () => {
		if (!state.hasUserMessage) state.hasUserMessage = true;
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

	// Transient delivery state is cleared across rebuilds. state.hasUserMessage is NOT
	// reset here — it is a shared fact reconstructed from the branch by model-identity.ts's
	// rebuild (which runs on these same events, registered first), so a resumed session
	// past its first user message stays past the pre-first-turn boundary (issue 244, Stage 6).
	pi.on("session_start", (event) => {
		if (event.reason === "reload") return;
		clearDebounce();
		state.pendingNotifyModel = null;
		latestTurnIndex = 0;
	});

	pi.on("session_tree", () => {
		clearDebounce();
		state.pendingNotifyModel = null;
		latestTurnIndex = 0;
	});

	pi.on("session_shutdown", () => {
		clearDebounce();
	});
}
