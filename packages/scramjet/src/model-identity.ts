/**
 * # Model Identity system-prompt section + attribution ledger (issue 244).
 *
 * The third of the three separated concerns (see model-switch-tool.ts for the
 * agent-callable switch and model-change-notice.ts for user-initiated change delivery).
 * This module owns two things and nothing else:
 *
 * 1. The frozen `# Model Identity` system-prompt section. Before the first user message
 *    it tracks the live model (so a pre-first-turn change is reflected with no tool call —
 *    Scenario 1); once the first user message exists it latches the then-current model and
 *    is never modified again, for provider prompt-cache stability. Later changes arrive as
 *    `scramjet_model_change_notice` tool results, not edits to this section.
 * 2. The attribution ledger (`state.currentModel` / `state.modelHistory`) and its
 *    reconstruction from persisted session entries on resume/fork/session-tree.
 *
 * The pre-first-turn boundary is the shared `state.hasUserMessage` fact: latched live by
 * model-change-notice.ts's `input` observer, and re-derived here from the branch on
 * rebuild so a resumed session past its first user message resumes to notice-based
 * delivery rather than a system-prompt edit.
 */

import type { ExtensionAPI, ExtensionContext, SessionEntry } from "@leanandmean/coding-agent";
import { MODEL_CHANGE_NOTICE_TOOL, type ModelRecord, type ScramjetState } from "./types.js";

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
	// True when the branch contains at least one user message — the pre-first-turn
	// boundary. Drives whether a resumed session delivers later changes via the notice
	// tool (past the boundary) or the still-live system-prompt section (before it).
	hasUserMessage: boolean;
	// The model live at the first user message — the record the frozen # Model Identity
	// section latched in the original session. With multiple pre-first-message changes
	// (A→B→C, then the first message), this is C, not the branch's earliest entry A.
	// Null when the branch has no user message or no model_change entry precedes it.
	modelAtFirstUserMessage: ModelRecord | null;
}

// A synthetic assistant message minted by invokeHarnessTool(scramjet_model_change_notice):
// its sole content block is a toolCall to the notice tool. These are harness-originated
// narration, not real assistant turns, so they are skipped when counting assistant turns —
// otherwise a mid-session user model change would inflate the estimated turn index of every
// subsequent reconstructed model_change entry (issue 244, requirement 13).
function isModelChangeNoticeMessage(message: any): boolean {
	const content = message?.content;
	return (
		Array.isArray(content) &&
		content.length === 1 &&
		content[0]?.type === "toolCall" &&
		content[0]?.name === MODEL_CHANGE_NOTICE_TOOL
	);
}

export function reconstructModelState(
	entries: readonly SessionEntry[],
	ctxModel: ActiveModel | undefined,
): ReconstructedModelState {
	const history: ModelRecord[] = [];
	let assistantCount = 0;
	let hasUserMessage = false;
	let modelAtFirstUserMessage: ModelRecord | null = null;

	for (const entry of entries) {
		if (entry.type === "message") {
			const message = (entry as any).message;
			if (message?.role === "user") {
				if (!hasUserMessage) {
					hasUserMessage = true;
					modelAtFirstUserMessage = history[history.length - 1] ?? null;
				}
			} else if (message?.role === "assistant" && !isModelChangeNoticeMessage(message)) {
				assistantCount++;
			}
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
		return { currentModel: null, modelHistory: [], diverged: false, hasUserMessage, modelAtFirstUserMessage };
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
		return {
			currentModel: divergedRecord,
			modelHistory: history,
			diverged: true,
			hasUserMessage,
			modelAtFirstUserMessage,
		};
	}

	if (ctxModel && ctxModel.id === last.id) {
		last.name = ctxModel.name;
	}

	return { currentModel: last, modelHistory: history, diverged: false, hasUserMessage, modelAtFirstUserMessage };
}

export function registerModelIdentity(pi: ExtensionAPI, state: ScramjetState): void {
	// The model rendered in the frozen # Model Identity section once the first user
	// message exists. Null while pre-first-turn (the section tracks the live model
	// instead — Scenario 1); latched at the first user turn and re-derived on rebuild.
	let frozenModel: ModelRecord | null = null;

	const rebuild = (ctx: ExtensionContext) => {
		const branch = ctx.sessionManager.getBranch();
		const result = reconstructModelState(branch, ctx.model);

		if (result.currentModel) {
			state.currentModel = result.currentModel;
			state.modelHistory = result.modelHistory;
		} else if (ctx.model) {
			const record = modelRecord(ctx.model, 0);
			state.currentModel = record;
			state.modelHistory = [record];
		} else {
			state.currentModel = null;
			state.modelHistory = [];
		}

		// A resumed branch that already contains a user message is past the pre-first-turn
		// boundary: the identity section stays frozen on the model that was live at the
		// first user message, and later changes must arrive as notice tool results (issue
		// 244 structural ordering). A branch with no user message resumes pre-first-turn,
		// so the section tracks the live model again. The fallback covers the unusual
		// branch shape where no model_change entry precedes the first user message.
		state.hasUserMessage = result.hasUserMessage;
		frozenModel = result.hasUserMessage
			? (result.modelAtFirstUserMessage ?? state.modelHistory[0] ?? state.currentModel)
			: null;
	};

	pi.on("session_start", (event, ctx) => {
		if (event.reason === "resume" || event.reason === "fork") {
			rebuild(ctx);
			return;
		}

		// "startup" and "new" (e.g. /clear) begin a fresh conversation with no user message
		// yet, so re-enter the pre-first-turn state: a change before the first message updates
		// the section directly (Scenario 1), never a notice tool (which would have no preceding
		// user message and violate the session-ordering invariant). "reload" re-emits
		// session_start mid-conversation, so it must leave the boundary and frozen section alone.
		if (event.reason !== "reload") {
			frozenModel = null;
			state.hasUserMessage = false;
		}

		if (!ctx.model) {
			state.currentModel = null;
			state.modelHistory = [];
			return;
		}

		const record = modelRecord(ctx.model, 0);
		state.currentModel = record;
		state.modelHistory = [record];
	});

	pi.on("session_tree", (_event, ctx) => {
		rebuild(ctx);
	});

	// Before the first user message (state.hasUserMessage false) the section tracks the
	// live model, so a pre-first-turn change — committed to state.currentModel by
	// model-change-notice.ts — is reflected here with no tool call (Scenario 1). At the
	// first user turn it latches the then-current model (frozenModel) and is frozen for
	// provider cache stability; later changes arrive as scramjet_model_change_notice tool
	// results and leave this section unchanged.
	pi.on("before_agent_start", () => {
		let model: ModelRecord | null;
		if (state.hasUserMessage) {
			// Latch the model live at the first user message, then keep it frozen.
			frozenModel ??= state.currentModel;
			model = frozenModel;
		} else {
			model = state.currentModel;
		}
		if (!model) return {};
		return {
			systemPromptSection: { id: "scramjet:model-identity", text: `\n\n${buildModelIdentityBlock(model)}` },
		};
	});
}
