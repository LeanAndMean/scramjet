import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { ModelRecord, ScramjetState } from "./types.ts";

type ActiveModel = NonNullable<ExtensionContext["model"]>;

const DEBOUNCE_MS = 500;

export function buildModelIdentityBlock(model: ModelRecord): string {
	return `# Model Identity
Your model is: ${model.name} (ID: ${model.id}, provider: ${model.provider}).
When model changes occur during this session, they are communicated via messages prefixed with [scramjet].
When posting to GitHub, use this attribution:
- Single model: "Reviewed by ${model.name}"
- Multiple models: describe each model's contribution (e.g., "Reviewed by X (analysis) and Y (posting)")`;
}

function modelRecord(model: ActiveModel, fromTurnIndex: number): ModelRecord {
	return {
		name: model.name,
		id: model.id,
		provider: model.provider,
		fromTurnIndex,
	};
}

function changeMessage(model: ModelRecord): string {
	return `[scramjet] Model changed to: ${model.name} (ID: ${model.id}).`;
}

export function registerModelIdentity(pi: ExtensionAPI, state: ScramjetState): void {
	let latestTurnIndex = 0;
	let debounceTimer: ReturnType<typeof setTimeout> | null = null;
	let pendingModel: ActiveModel | null = null;
	let pendingForInput = false;
	let pendingForNextTurn = false;

	pi.on("session_start", (_event, ctx) => {
		if (!ctx.model) {
			state.currentModel = null;
			state.modelHistory = [];
			return;
		}

		const record = modelRecord(ctx.model, latestTurnIndex);
		state.currentModel = record;
		state.modelHistory = [record];
	});

	pi.on("model_select", (event) => {
		if (event.source === "restore") return;

		if (debounceTimer !== null) clearTimeout(debounceTimer);
		pendingModel = event.model;

		debounceTimer = setTimeout(() => {
			debounceTimer = null;
			if (!pendingModel) return;

			if (state.currentModel && pendingModel.id === state.currentModel.id) {
				pendingModel = null;
				return;
			}

			const record = modelRecord(pendingModel, latestTurnIndex);
			state.currentModel = record;
			state.modelHistory.push(record);
			pendingModel = null;

			const phase = state.lifecycle.phase;
			if (phase === "running" || phase === "probing") {
				pendingForNextTurn = true;
				pendingForInput = false;
			} else {
				pendingForInput = true;
				pendingForNextTurn = false;
			}
		}, DEBOUNCE_MS);
	});

	pi.on("input", (event) => {
		if (!pendingForInput) return;
		pendingForInput = false;
		return { action: "transform" as const, text: `${changeMessage(state.currentModel!)}\n\n${event.text}` };
	});

	pi.on("before_agent_start", (event) => {
		const systemPrompt = state.currentModel
			? `${event.systemPrompt}\n\n${buildModelIdentityBlock(state.currentModel)}`
			: undefined;

		if (pendingForNextTurn) {
			pendingForNextTurn = false;
			if (state.lifecycle.phase !== "probing") {
				return {
					...(systemPrompt ? { systemPrompt } : {}),
					message: {
						customType: "scramjet:model-change",
						content: `${changeMessage(state.currentModel!)} Please continue.`,
						display: true,
					},
				};
			}
		}

		if (!systemPrompt) return {};
		return { systemPrompt };
	});

	pi.on("turn_start", (event) => {
		latestTurnIndex = event.turnIndex;
	});
}
