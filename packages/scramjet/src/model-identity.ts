import type { AssistantMessage, SystemPromptSection } from "@leanandmean/ai";
import type { ExtensionAPI, ExtensionContext, SessionEntry } from "@leanandmean/coding-agent";
import { MODEL_CHANGE_NOTICE_TOOL, type ModelRecord, type ScramjetState } from "./types.js";

type ActiveModel = NonNullable<ExtensionContext["model"]>;

const LEGACY_HARNESS_TOOLS = new Set([MODEL_CHANGE_NOTICE_TOOL, "scramjet_next_step_selection"]);

function sameModel(a: Pick<ModelRecord, "provider" | "id">, b: { provider: string; id: string }): boolean {
	return a.provider === b.provider && a.id === b.id;
}

export function buildModelIdentityBlock(current: ModelRecord, contributors: readonly ModelRecord[] = []): string {
	const contributorLines =
		contributors.length === 0
			? "- No prior material provider responses in this branch."
			: contributors.map((model) => `- ${model.name} (ID: ${model.id}, provider: ${model.provider})`).join("\n");
	return `# Model Identity
Your model for the imminent response is: ${current.name} (ID: ${current.id}, provider: ${current.provider}).
Models that produced material responses in this session branch:
${contributorLines}
When model changes occur during this session, they are delivered as scramjet_model_change_notice tool results.
When posting to GitHub, use honest contribution-aware attribution:
- Credit the current model for work in the imminent response.
- Credit prior models only when their material responses contributed to the published work.
- Single credited model: "Reviewed by ${current.name}"
- Multiple credited models: describe each model's contribution (e.g., "Reviewed by X (analysis) and Y (posting)")`;
}

export function modelRecord(model: ActiveModel, fromTurnIndex: number): ModelRecord {
	return { name: model.name, id: model.id, provider: model.provider, fromTurnIndex };
}

function isLegacyHarnessMessage(message: AssistantMessage): boolean {
	return (
		message.origin === undefined &&
		message.content.length === 1 &&
		message.content[0]?.type === "toolCall" &&
		LEGACY_HARNESS_TOOLS.has(message.content[0].name)
	);
}

function isMaterialProviderMessage(message: AssistantMessage): boolean {
	if (message.origin === "harness" || isLegacyHarnessMessage(message)) return false;
	if (message.stopReason === "error" || message.stopReason === "aborted") return false;
	return message.content.some((part) => {
		if (part.type === "toolCall") return true;
		if (part.type === "text") return part.text.trim() !== "";
		return part.type === "thinking" && part.thinking.trim() !== "";
	});
}

function appendContributor(contributors: ModelRecord[], record: ModelRecord): void {
	if (!contributors.some((existing) => sameModel(existing, record))) contributors.push(record);
}

export interface ReconstructedModelState {
	currentModel: ModelRecord | null;
	modelHistory: ModelRecord[];
	contributors: ModelRecord[];
	diverged: boolean;
}

export function reconstructModelState(
	entries: readonly SessionEntry[],
	ctxModel: ActiveModel | undefined,
	resolveDisplayName?: (provider: string, modelId: string) => string | undefined,
): ReconstructedModelState {
	const history: ModelRecord[] = [];
	const contributors: ModelRecord[] = [];
	let assistantCount = 0;

	for (const entry of entries) {
		if (entry.type === "message") {
			const message = (entry as any).message as AssistantMessage;
			if (message?.role !== "assistant" || !isMaterialProviderMessage(message)) continue;
			appendContributor(contributors, {
				name: resolveDisplayName?.(message.provider, message.model) ?? message.model,
				id: message.model,
				provider: message.provider,
				fromTurnIndex: assistantCount,
			});
			assistantCount++;
		} else if (entry.type === "model_change") {
			const change = entry as { provider: string; modelId: string };
			history.push({
				name: resolveDisplayName?.(change.provider, change.modelId) ?? change.modelId,
				id: change.modelId,
				provider: change.provider,
				fromTurnIndex: assistantCount,
			});
		}
	}

	let currentModel = history.at(-1) ?? null;
	let diverged = false;
	if (ctxModel && (!currentModel || !sameModel(currentModel, ctxModel))) {
		currentModel = modelRecord(ctxModel, assistantCount);
		history.push(currentModel);
		diverged = history.length > 1;
	} else if (ctxModel && currentModel) {
		currentModel.name = ctxModel.name;
	}
	return { currentModel, modelHistory: history, contributors, diverged };
}

function insertIdentity(prompt: string | readonly SystemPromptSection[], text: string): string | SystemPromptSection[] {
	if (typeof prompt === "string") return `${prompt}\n\n${text}`;
	const sections = prompt.slice();
	const volatileIndex = sections.findIndex((section) => section.cacheRetention === "none");
	sections.splice(volatileIndex === -1 ? sections.length : volatileIndex, 0, {
		id: "scramjet:model-identity",
		text: `\n\n${text}`,
	});
	return sections;
}

export function registerModelIdentity(pi: ExtensionAPI, state: ScramjetState): void {
	let pending: { model: ModelRecord; identity: string } | null = null;
	let frozenIdentity: string | null = null;

	const openEpoch = () => {
		pending = null;
		frozenIdentity = null;
		state.identityEpochFrozen = false;
	};

	const rebuild = (ctx: ExtensionContext) => {
		const resolveName = ctx.modelRegistry
			? (provider: string, id: string) => ctx.modelRegistry.find(provider, id)?.name
			: undefined;
		const result = reconstructModelState(ctx.sessionManager.getBranch(), ctx.model, resolveName);
		state.currentModel = result.currentModel;
		state.modelHistory = result.modelHistory;
		state.modelContributors = result.contributors;
		openEpoch();
	};

	pi.on("session_start", (event, ctx) => {
		if (event.reason === "resume" || event.reason === "fork" || event.reason === "reload") {
			rebuild(ctx);
			return;
		}
		state.currentModel = ctx.model ? modelRecord(ctx.model, 0) : null;
		state.modelHistory = state.currentModel ? [state.currentModel] : [];
		state.modelContributors = [];
		openEpoch();
	});
	pi.on("session_tree", (_event, ctx) => rebuild(ctx));
	pi.on("session_compact", () => {
		openEpoch();
		state.pendingNotifyModel = null;
	});

	pi.on("before_provider_call", (event) => {
		const current = modelRecord(event.model, state.modelContributors.length);
		const identity = frozenIdentity ?? buildModelIdentityBlock(current, state.modelContributors);
		if (!frozenIdentity) pending = { model: current, identity };
		return { systemPrompt: insertIdentity(event.systemPrompt ?? "", identity) };
	});

	pi.on("before_provider_request", (event) => {
		if (!frozenIdentity && pending && sameModel(pending.model, event.model)) {
			frozenIdentity = pending.identity;
			state.identityEpochFrozen = true;
		}
	});

	pi.on("message_end", (event, ctx) => {
		const message = event.message as AssistantMessage;
		if (message.role !== "assistant" || !isMaterialProviderMessage(message)) return;
		appendContributor(state.modelContributors, {
			name: ctx.modelRegistry.find(message.provider, message.model)?.name ?? message.model,
			id: message.model,
			provider: message.provider,
			fromTurnIndex: state.modelContributors.length,
		});
	});
}
