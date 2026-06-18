import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { ModelRecord, ScramjetState } from "./types.ts";

type ActiveModel = NonNullable<ExtensionContext["model"]>;

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

export function registerModelIdentity(pi: ExtensionAPI, state: ScramjetState): void {
	let latestTurnIndex = 0;

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

	pi.on("before_agent_start", (event) => {
		if (!state.currentModel) return {};
		return { systemPrompt: `${event.systemPrompt}\n\n${buildModelIdentityBlock(state.currentModel)}` };
	});

	pi.on("turn_start", (event) => {
		latestTurnIndex = event.turnIndex;
	});
}
