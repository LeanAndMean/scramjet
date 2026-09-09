import { MODELS } from "./models.generated.js";
import type { Api, KnownProvider, Model, ModelThinkingLevel, Usage } from "./types.js";

const modelRegistry: Map<string, Map<string, Model<Api>>> = new Map();

// Initialize registry from MODELS on module load
for (const [provider, models] of Object.entries(MODELS)) {
	const providerModels = new Map<string, Model<Api>>();
	for (const [id, model] of Object.entries(models)) {
		providerModels.set(id, model as Model<Api>);
	}
	modelRegistry.set(provider, providerModels);
}

type ModelApi<
	TProvider extends KnownProvider,
	TModelId extends keyof (typeof MODELS)[TProvider],
> = (typeof MODELS)[TProvider][TModelId] extends { api: infer TApi } ? (TApi extends Api ? TApi : never) : never;

export function getModel<TProvider extends KnownProvider, TModelId extends keyof (typeof MODELS)[TProvider]>(
	provider: TProvider,
	modelId: TModelId,
): Model<ModelApi<TProvider, TModelId>> {
	const providerModels = modelRegistry.get(provider);
	return providerModels?.get(modelId as string) as Model<ModelApi<TProvider, TModelId>>;
}

export function getProviders(): KnownProvider[] {
	return Array.from(modelRegistry.keys()) as KnownProvider[];
}

export function getModels<TProvider extends KnownProvider>(
	provider: TProvider,
): Model<ModelApi<TProvider, keyof (typeof MODELS)[TProvider]>>[] {
	const models = modelRegistry.get(provider);
	return models ? (Array.from(models.values()) as Model<ModelApi<TProvider, keyof (typeof MODELS)[TProvider]>>[]) : [];
}

// SCRAMJET-DIVERGENCE: Validate joint constraints at model ingestion, including dynamic and OAuth producers.
export function validateModelRequestLimits(model: Pick<Model<Api>, "provider" | "id" | "requestLimits">): void {
	const limits = model.requestLimits;
	if (limits === undefined) return;
	if (!Array.isArray(limits) || limits.length === 0) {
		throw new Error(
			`${model.provider}/${model.id}: invalid requestLimits; expected a non-empty endpoint constraint array`,
		);
	}
	for (const [index, limit] of limits.entries()) {
		if (
			!limit ||
			typeof limit.supportsTools !== "boolean" ||
			!Number.isFinite(limit.maxTotalTokens) ||
			limit.maxTotalTokens <= 0 ||
			[limit.maxInputTokens, limit.maxOutputTokens].some(
				(value) => value !== undefined && (!Number.isFinite(value) || value <= 0),
			)
		) {
			throw new Error(
				`${model.provider}/${model.id}: invalid requestLimits[${index}]; expected positive finite token limits and boolean supportsTools`,
			);
		}
	}
}

// SCRAMJET-DIVERGENCE: Keep endpoint input/output combinations together without selecting a provider.
export function getEndpointOutputLimit(model: Model<Api>, inputTokens: number, hasTools: boolean): number {
	if (model.requestLimits === undefined) return Infinity;
	const output = Math.floor(
		Math.max(
			0,
			...model.requestLimits.map((limit) =>
				(hasTools && !limit.supportsTools) || inputTokens > (limit.maxInputTokens ?? Infinity)
					? 0
					: Math.min(limit.maxOutputTokens ?? Infinity, limit.maxTotalTokens - inputTokens),
			),
		),
	);
	if (output < 1) {
		throw new Error(
			`context_length_exceeded: ${model.provider}/${model.id} estimated input ${inputTokens} has no compatible endpoint with output space${hasTools ? " and tools" : ""}; compact or reduce the request and check provider requestLimits.`,
		);
	}
	return output;
}

export function calculateCost<TApi extends Api>(model: Model<TApi>, usage: Usage): Usage["cost"] {
	usage.cost.input = (model.cost.input / 1000000) * usage.input;
	usage.cost.output = (model.cost.output / 1000000) * usage.output;
	usage.cost.cacheRead = (model.cost.cacheRead / 1000000) * usage.cacheRead;
	usage.cost.cacheWrite = (model.cost.cacheWrite / 1000000) * usage.cacheWrite;
	usage.cost.total = usage.cost.input + usage.cost.output + usage.cost.cacheRead + usage.cost.cacheWrite;
	return usage.cost;
}

const EXTENDED_THINKING_LEVELS: ModelThinkingLevel[] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

export function getSupportedThinkingLevels<TApi extends Api>(model: Model<TApi>): ModelThinkingLevel[] {
	if (!model.reasoning) return ["off"];

	return EXTENDED_THINKING_LEVELS.filter((level) => {
		const mapped = model.thinkingLevelMap?.[level];
		if (mapped === null) return false;
		if (level === "xhigh" || level === "max") return mapped !== undefined;
		return true;
	});
}

export function clampThinkingLevel<TApi extends Api>(
	model: Model<TApi>,
	level: ModelThinkingLevel,
): ModelThinkingLevel {
	const availableLevels = getSupportedThinkingLevels(model);
	if (availableLevels.includes(level)) return level;

	const requestedIndex = EXTENDED_THINKING_LEVELS.indexOf(level);
	if (requestedIndex === -1) return availableLevels[0] ?? "off";

	for (let i = requestedIndex; i < EXTENDED_THINKING_LEVELS.length; i++) {
		const candidate = EXTENDED_THINKING_LEVELS[i];
		if (availableLevels.includes(candidate)) return candidate;
	}
	for (let i = requestedIndex - 1; i >= 0; i--) {
		const candidate = EXTENDED_THINKING_LEVELS[i];
		if (availableLevels.includes(candidate)) return candidate;
	}
	return availableLevels[0] ?? "off";
}

/**
 * Check if two models are equal by comparing both their id and provider.
 * Returns false if either model is null or undefined.
 */
export function modelsAreEqual<TApi extends Api>(
	a: Model<TApi> | null | undefined,
	b: Model<TApi> | null | undefined,
): boolean {
	if (!a || !b) return false;
	return a.id === b.id && a.provider === b.provider;
}
