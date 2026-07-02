import type { ExtensionAPI, ExtensionContext, SessionEntry } from "@leanandmean/coding-agent";
import type { ModelRecord, ScramjetState } from "./types.js";

type ActiveModel = NonNullable<ExtensionContext["model"]>;

export function buildModelIdentityBlock(model: ModelRecord): string {
	return `# Model Identity
Your model is: ${model.name} (ID: ${model.id}, provider: ${model.provider}).
When model changes occur during this session, they are delivered as scramjet_model_change_notice tool results.
When posting to GitHub, use this attribution:
- Single model: "Reviewed by ${model.name}"
- Multiple models: describe each model's contribution (e.g., "Reviewed by X (analysis) and Y (posting)")`;
}

// Exported so model-change-notice.ts can build the same ModelRecord shape when it
// commits a user-initiated change to state.currentModel/modelHistory (issue 244, Stage 5).
export function modelRecord(model: ActiveModel, fromTurnIndex: number): ModelRecord {
	return {
		name: model.name,
		id: model.id,
		provider: model.provider,
		fromTurnIndex,
	};
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
	let firstTurnStarted = false;

	const rebuild = (ctx: ExtensionContext) => {
		latestTurnIndex = 0;
		firstTurnStarted = false;

		const branch = ctx.sessionManager.getBranch();
		const result = reconstructModelState(branch, ctx.model);

		if (result.currentModel) {
			state.currentModel = result.currentModel;
			state.modelHistory = result.modelHistory;
			initialModel = result.modelHistory[0]!;
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

	// The # Model Identity section is frozen at the first user turn for provider
	// cache stability. Before that turn (firstTurnStarted is false) it tracks the
	// live model, so a pre-first-turn model change — committed to state.currentModel
	// by model-change-notice.ts — is reflected here with no tool call (issue 244,
	// Scenario 1). turn_start latches the section by setting firstTurnStarted.
	pi.on("before_agent_start", () => {
		if (!firstTurnStarted && state.currentModel) {
			initialModel = state.currentModel;
		}
		if (!initialModel) return {};
		return {
			systemPromptSection: { id: "scramjet:model-identity", text: `\n\n${buildModelIdentityBlock(initialModel)}` },
		};
	});

	pi.on("turn_start", (event) => {
		latestTurnIndex = event.turnIndex;
		firstTurnStarted = true;
	});
}
