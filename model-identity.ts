import type { ExtensionAPI, ExtensionContext, SessionEntry } from "@earendil-works/pi-coding-agent";
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

export interface ReconstructedModelState {
	currentModel: ModelRecord | null;
	modelHistory: ModelRecord[];
	diverged: boolean;
}

export function reconstructModelState(
	entries: readonly SessionEntry[],
	ctxModel: ActiveModel | undefined,
): ReconstructedModelState {
	const history: ModelRecord[] = [];
	let assistantCount = 0;

	for (const entry of entries) {
		if (entry.type === "message" && (entry as any).message?.role === "assistant") {
			assistantCount++;
		} else if (entry.type === "model_change") {
			const mc = entry as { provider: string; modelId: string };
			history.push({
				name: mc.modelId,
				id: mc.modelId,
				provider: mc.provider,
				fromTurnIndex: assistantCount,
			});
		}
	}

	if (history.length === 0) {
		return { currentModel: null, modelHistory: [], diverged: false };
	}

	const last = history[history.length - 1]!;

	if (ctxModel && ctxModel.id !== last.id) {
		const divergedRecord: ModelRecord = {
			name: ctxModel.name,
			id: ctxModel.id,
			provider: ctxModel.provider,
			fromTurnIndex: assistantCount,
		};
		history.push(divergedRecord);
		return { currentModel: divergedRecord, modelHistory: history, diverged: true };
	}

	if (ctxModel && ctxModel.id === last.id) {
		last.name = ctxModel.name;
	}

	return { currentModel: last, modelHistory: history, diverged: false };
}

export function registerModelIdentity(pi: ExtensionAPI, state: ScramjetState): void {
	let latestTurnIndex = 0;
	let initialModel: ModelRecord | null = null;
	let debounceTimer: ReturnType<typeof setTimeout> | null = null;
	let pendingModel: ActiveModel | null = null;
	let pendingForInput = false;
	let pendingForNextTurn = false;

	const rebuild = (ctx: ExtensionContext) => {
		latestTurnIndex = 0;
		pendingForInput = false;
		pendingForNextTurn = false;
		if (debounceTimer !== null) {
			clearTimeout(debounceTimer);
			debounceTimer = null;
		}
		pendingModel = null;

		const branch = ctx.sessionManager.getBranch();
		const result = reconstructModelState(branch, ctx.model);

		if (result.currentModel) {
			state.currentModel = result.currentModel;
			state.modelHistory = result.modelHistory;
			initialModel = result.modelHistory[0]!;
			if (result.diverged) pendingForInput = true;
		} else if (ctx.model) {
			const record = modelRecord(ctx.model, latestTurnIndex);
			state.currentModel = record;
			state.modelHistory = [record];
			initialModel = record;
		} else {
			state.currentModel = null;
			state.modelHistory = [];
			initialModel = null;
		}
	};

	pi.on("session_start", (event, ctx) => {
		if (event.reason === "resume" || event.reason === "fork") {
			rebuild(ctx);
			return;
		}

		if (!ctx.model) {
			state.currentModel = null;
			state.modelHistory = [];
			return;
		}

		const record = modelRecord(ctx.model, latestTurnIndex);
		state.currentModel = record;
		state.modelHistory = [record];
		initialModel = record;
	});

	pi.on("session_tree", (_event, ctx) => {
		rebuild(ctx);
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
		const systemPrompt = initialModel
			? `${event.systemPrompt}\n\n${buildModelIdentityBlock(initialModel)}`
			: undefined;

		if (pendingForNextTurn) {
			if (state.lifecycle.phase !== "probing") {
				pendingForNextTurn = false;
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
