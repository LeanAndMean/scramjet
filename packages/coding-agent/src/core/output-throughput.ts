import type { AssistantMessage, AssistantMessageEvent } from "@leanandmean/ai";

const LIVE_WINDOW_MS = 2000;
const BYTES_PER_TOKEN = 4;
const MAX_SAMPLES_PER_MODEL = 20;
const encoder = new TextEncoder();

export interface OutputThroughputIdentity {
	provider: string;
	model: string;
}

export interface OutputThroughputSample extends OutputThroughputIdentity {
	responseModel?: string;
	outputTokens: number;
	durationMs: number;
	observedAt: number;
}

export interface OutputThroughputTrackerOptions {
	monotonicNow?: () => number;
	wallNow?: () => number;
}

interface Observation {
	time: number;
	tokens: number;
}

interface ActiveMeasurement extends OutputThroughputIdentity {
	generation: number;
	observations: Observation[];
}

export class OutputThroughputTracker {
	private readonly monotonicNow: () => number;
	private readonly wallNow: () => number;
	private active?: ActiveMeasurement;
	private nextGeneration = 0;

	constructor(options: OutputThroughputTrackerOptions = {}) {
		this.monotonicNow = options.monotonicNow ?? (() => performance.now());
		this.wallNow = options.wallNow ?? (() => Date.now());
	}

	get generation(): number {
		return this.nextGeneration;
	}

	start(identity: OutputThroughputIdentity): number {
		const generation = ++this.nextGeneration;
		this.active = { ...identity, generation, observations: [] };
		return generation;
	}

	reset(): void {
		this.nextGeneration++;
		this.active = undefined;
	}

	observe(generation: number, event: AssistantMessageEvent): void {
		if (generation !== this.active?.generation || !isGeneratedDelta(event) || event.delta.length === 0) return;
		const time = this.monotonicNow();
		if (!Number.isFinite(time)) return;
		this.active.observations.push({ time, tokens: encoder.encode(event.delta).byteLength / BYTES_PER_TOKEN });
	}

	liveRate(): number | undefined {
		if (!this.active) return undefined;
		const now = this.monotonicNow();
		if (!Number.isFinite(now)) return undefined;
		const cutoff = now - LIVE_WINDOW_MS;
		const retained = this.active.observations.filter((observation) => observation.time >= cutoff);
		const first = retained[0];
		if (!first) return undefined;
		const elapsedMs = now - first.time;
		if (!(elapsedMs > 0)) return undefined;
		const tokens = retained.reduce((total, observation) => total + observation.tokens, 0);
		const rate = (tokens * 1000) / elapsedMs;
		return Number.isFinite(rate) && rate > 0 ? rate : undefined;
	}

	finalize(generation: number, message: AssistantMessage): OutputThroughputSample | undefined {
		if (generation !== this.active?.generation) return undefined;
		const active = this.active;
		this.reset();
		const first = active.observations[0];
		const last = active.observations.at(-1);
		const outputTokens = message.usage.output;
		if (
			message.origin !== "provider" ||
			message.provider !== active.provider ||
			message.model !== active.model ||
			!isSuccessfulStop(message.stopReason) ||
			!Number.isFinite(outputTokens) ||
			outputTokens <= 0 ||
			active.observations.length < 2 ||
			!first ||
			!last
		) {
			return undefined;
		}
		const durationMs = last.time - first.time;
		if (!Number.isFinite(durationMs) || durationMs <= 0) return undefined;
		const observedAt = this.wallNow();
		if (!Number.isFinite(observedAt)) return undefined;
		return {
			provider: active.provider,
			model: active.model,
			...(message.responseModel ? { responseModel: message.responseModel } : {}),
			outputTokens,
			durationMs,
			observedAt,
		};
	}
}

export class OutputThroughputHistory {
	private readonly samplesByModel = new Map<string, OutputThroughputSample[]>();

	add(sample: OutputThroughputSample): void {
		if (!isValidSample(sample)) return;
		const key = modelKey(sample.provider, sample.model);
		const samples = [...(this.samplesByModel.get(key) ?? []), { ...sample }]
			.sort((left, right) => left.observedAt - right.observedAt)
			.slice(-MAX_SAMPLES_PER_MODEL);
		this.samplesByModel.set(key, samples);
	}

	samples(provider: string, model: string): readonly OutputThroughputSample[] {
		return (this.samplesByModel.get(modelKey(provider, model)) ?? []).map((sample) => ({ ...sample }));
	}

	allSamples(): readonly OutputThroughputSample[] {
		return [...this.samplesByModel.values()].flat().map((sample) => ({ ...sample }));
	}

	median(provider: string, model: string): number | undefined {
		const rates = this.samples(provider, model)
			.map((sample) => (sample.outputTokens * 1000) / sample.durationMs)
			.filter((rate) => Number.isFinite(rate) && rate > 0)
			.sort((left, right) => left - right);
		if (rates.length === 0) return undefined;
		const middle = Math.floor(rates.length / 2);
		return rates.length % 2 === 0 ? ((rates[middle - 1] ?? 0) + (rates[middle] ?? 0)) / 2 : rates[middle];
	}
}

export function formatLiveOutputRate(rate: number): string | undefined {
	return formatRate(rate, true);
}

export function formatHistoricalOutputRate(rate: number): string | undefined {
	return formatRate(rate, false);
}

function formatRate(rate: number, approximate: boolean): string | undefined {
	if (!Number.isFinite(rate) || rate <= 0) return undefined;
	const rounded = Math.round(rate);
	return approximate ? `~${rounded} tok/s` : `${rounded} tok/s est.`;
}

function isGeneratedDelta(
	event: AssistantMessageEvent,
): event is Extract<AssistantMessageEvent, { type: "text_delta" | "thinking_delta" | "toolcall_delta" }> {
	return event.type === "text_delta" || event.type === "thinking_delta" || event.type === "toolcall_delta";
}

function isSuccessfulStop(stopReason: AssistantMessage["stopReason"]): boolean {
	return stopReason === "stop" || stopReason === "length" || stopReason === "toolUse";
}

function isValidSample(sample: OutputThroughputSample): boolean {
	return (
		sample.provider.length > 0 &&
		sample.model.length > 0 &&
		Number.isFinite(sample.outputTokens) &&
		sample.outputTokens > 0 &&
		Number.isFinite(sample.durationMs) &&
		sample.durationMs > 0 &&
		Number.isFinite(sample.observedAt)
	);
}

function modelKey(provider: string, model: string): string {
	return `${provider.length}:${provider}${model}`;
}
