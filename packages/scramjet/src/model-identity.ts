import type { AssistantMessage, ToolCall, ToolResultMessage } from "@leanandmean/ai";
import type { ExtensionAPI, ExtensionContext, SessionEntry } from "@leanandmean/coding-agent";
import type { ModelRecord, ScramjetState } from "./types.js";

type ActiveModel = NonNullable<ExtensionContext["model"]>;

export const STABILITY_MS = 500;
export const MAX_STABILITY_WAIT_MS = 5000;

export function buildModelIdentityBlock(model: ModelRecord): string {
	return `# Model Identity
Your model is: ${model.name} (ID: ${model.id}, provider: ${model.provider}).
When model changes occur during this session, they are communicated via tool calls named notify_model_change.
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
			const msg = (entry as any).message;
			if (msg.provider === "scramjet") continue;
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

export async function waitForModelStable(state: ScramjetState): Promise<void> {
	const start = Date.now();
	while (true) {
		const elapsed = Date.now() - state.lastModelSelectTime;
		if (elapsed >= STABILITY_MS) return;
		if (Date.now() - start >= MAX_STABILITY_WAIT_MS) return;
		await new Promise((resolve) => setTimeout(resolve, STABILITY_MS - elapsed));
	}
}

let callCounter = 0;
function generateCallId(model: ModelRecord): string {
	return `scrmdl-${model.id}-${++callCounter}`;
}

function notificationContent(model: ModelRecord, previous: ModelRecord | null): string {
	const lines = [
		`Model changed to: ${model.name} (ID: ${model.id}, provider: ${model.provider}).`,
		`Previous model: ${previous ? `${previous.name} (${previous.id})` : "none"}.`,
		"Update your attribution accordingly for any GitHub posts in this session.",
	];
	return lines.join("\n");
}

export function buildNotificationToolCall(model: ModelRecord, callId: string): ToolCall {
	return {
		type: "toolCall",
		id: callId,
		name: "notify_model_change",
		arguments: { model_name: model.name, model_id: model.id, provider: model.provider },
	};
}

export function buildNotificationPair(
	model: ModelRecord,
	previous: ModelRecord | null,
	callId: string,
): [AssistantMessage, ToolResultMessage] {
	const toolCall = buildNotificationToolCall(model, callId);
	const assistantMsg: AssistantMessage = {
		role: "assistant",
		content: [toolCall],
		api: "messages",
		provider: "scramjet",
		model: "scramjet-harness",
		usage: {
			input: 0,
			output: 0,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: "toolUse",
		timestamp: Date.now(),
	};
	const resultMsg: ToolResultMessage = {
		role: "toolResult",
		toolCallId: callId,
		toolName: "notify_model_change",
		content: [{ type: "text", text: notificationContent(model, previous) }],
		isError: false,
		timestamp: Date.now(),
	};
	return [assistantMsg, resultMsg];
}

export function registerModelIdentity(pi: ExtensionAPI, state: ScramjetState): void {
	let latestTurnIndex = 0;
	let initialModel: ModelRecord | null = null;
	let firstTurnStarted = false;

	pi.registerTool({
		name: "notify_model_change",
		label: "Notify Model Change",
		description: "Notifies the agent of a model change. Harness-injected; not agent-callable.",
		parameters: {
			type: "object",
			properties: {
				model_name: { type: "string", description: "Human-readable model name" },
				model_id: { type: "string", description: "Model identifier" },
				provider: { type: "string", description: "Provider name" },
			},
			required: ["model_name", "model_id", "provider"],
		},
		async execute(_toolCallId, params) {
			const { model_name, model_id, provider } = params as {
				model_name: string;
				model_id: string;
				provider: string;
			};
			const text = [
				`Model changed to: ${model_name} (ID: ${model_id}, provider: ${provider}).`,
				"Update your attribution accordingly for any GitHub posts in this session.",
			].join("\n");
			return {
				content: [{ type: "text" as const, text }],
				details: undefined,
				terminate: true,
			};
		},
	});

	const rebuild = (ctx: ExtensionContext) => {
		latestTurnIndex = 0;
		firstTurnStarted = false;
		state.pendingModelChange = null;
		state.lastModelSelectTime = 0;

		const branch = ctx.sessionManager.getBranch();
		const result = reconstructModelState(branch, ctx.model);

		if (result.currentModel) {
			state.currentModel = result.currentModel;
			state.modelHistory = result.modelHistory;
			initialModel = result.modelHistory[0]!;
			if (result.diverged) {
				state.pendingModelChange = result.currentModel;
			}
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

		state.lastModelSelectTime = Date.now();

		const model = event.model;
		if (state.currentModel && model.id === state.currentModel.id) {
			state.pendingModelChange = null;
			return;
		}

		const record = modelRecord(model, latestTurnIndex);

		if (!firstTurnStarted) {
			state.currentModel = record;
			state.modelHistory = [record];
			initialModel = record;
			return;
		}

		state.pendingModelChange = record;
	});

	pi.on("before_agent_start", async () => {
		const systemPromptSection = initialModel
			? { id: "scramjet:model-identity", text: `\n\n${buildModelIdentityBlock(initialModel)}` }
			: undefined;

		if (state.pendingModelChange && !state.lifecycle.probeInFlight) {
			await waitForModelStable(state);
			if (!state.pendingModelChange) return systemPromptSection ? { systemPromptSection } : {};
			const model = state.pendingModelChange;
			const previous = state.currentModel;
			state.currentModel = model;
			state.modelHistory.push(model);
			state.pendingModelChange = null;
			const callId = generateCallId(model);
			const [assistantMsg, resultMsg] = buildNotificationPair(model, previous, callId);
			return {
				...(systemPromptSection ? { systemPromptSection } : {}),
				preTurnMessages: [assistantMsg, resultMsg],
			};
		}

		if (!systemPromptSection) return {};
		return { systemPromptSection };
	});

	pi.on("message_end", async (event) => {
		const message = event.message;
		if (!message || message.role !== "assistant") return;
		if (!state.pendingModelChange) return;
		if (state.lifecycle.probeInFlight) return;

		const assistantMsg = message as AssistantMessage;
		const hasToolCalls = assistantMsg.content.some((b) => b.type === "toolCall");
		if (!hasToolCalls) return;

		await waitForModelStable(state);
		if (!state.pendingModelChange) return;
		const model = state.pendingModelChange;
		const previous = state.currentModel;
		state.currentModel = model;
		state.modelHistory.push(model);
		state.pendingModelChange = null;

		const callId = generateCallId(model);
		const toolCall = buildNotificationToolCall(model, callId);
		const newContent = [...assistantMsg.content, toolCall];
		return { message: { ...assistantMsg, content: newContent } };
	});

	pi.on("prepare_next_turn", async () => {
		if (!state.pendingModelChange) return;
		if (state.lifecycle.probeInFlight) return;

		await waitForModelStable(state);
		if (!state.pendingModelChange) return;
		const model = state.pendingModelChange;
		const previous = state.currentModel;
		state.currentModel = model;
		state.modelHistory.push(model);
		state.pendingModelChange = null;
		const callId = generateCallId(model);
		const [assistantMsg, resultMsg] = buildNotificationPair(model, previous, callId);
		return { messages: [assistantMsg, resultMsg] };
	});

	pi.on("turn_start", (event) => {
		latestTurnIndex = event.turnIndex;
		firstTurnStarted = true;
	});
}
