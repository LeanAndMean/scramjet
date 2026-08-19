import type { AssistantMessage, AssistantMessageEvent } from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
import {
	formatHistoricalOutputRate,
	formatLiveOutputRate,
	OutputThroughputHistory,
	OutputThroughputTracker,
} from "../src/core/output-throughput.js";

const identity = { provider: "test-provider", model: "requested-model" };

function message(overrides: Partial<AssistantMessage> = {}): AssistantMessage {
	return {
		role: "assistant",
		origin: "provider",
		content: [],
		api: "openai-responses",
		provider: identity.provider,
		model: identity.model,
		usage: {
			input: 0,
			output: 40,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 40,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: "stop",
		timestamp: 1,
		...overrides,
	} as AssistantMessage;
}

function delta(type: "text_delta" | "thinking_delta" | "toolcall_delta", value: string): AssistantMessageEvent {
	return { type, contentIndex: 0, delta: value, partial: message() } as AssistantMessageEvent;
}

describe("OutputThroughputTracker", () => {
	it("counts all generated delta types as fractional UTF-8 token estimates", () => {
		let now = 0;
		const tracker = new OutputThroughputTracker({ monotonicNow: () => now, wallNow: () => 10 });
		const generation = tracker.start(identity);

		tracker.observe(generation, delta("text_delta", "a"));
		now = 500;
		tracker.observe(generation, delta("thinking_delta", "é"));
		now = 1000;
		tracker.observe(generation, delta("toolcall_delta", "b"));

		expect(tracker.liveRate()).toBeCloseTo(1);
	});

	it("uses a two-second rolling window and decays fully during a stall", () => {
		let now = 0;
		const tracker = new OutputThroughputTracker({ monotonicNow: () => now });
		const generation = tracker.start(identity);
		tracker.observe(generation, delta("text_delta", "1234"));
		now = 1000;
		tracker.observe(generation, delta("text_delta", "1234"));

		expect(tracker.liveRate()).toBe(2);
		now = 2500;
		expect(tracker.liveRate()).toBeCloseTo(1 / 1.5);
		now = 3100;
		expect(tracker.liveRate()).toBeUndefined();
	});

	it("ignores empty and marker events and rejects stale generations", () => {
		let now = 0;
		const tracker = new OutputThroughputTracker({ monotonicNow: () => now });
		const stale = tracker.start(identity);
		tracker.observe(stale, delta("text_delta", ""));
		const current = tracker.start(identity);
		now = 1000;
		tracker.observe(stale, delta("text_delta", "1234"));
		tracker.observe(current, { type: "text_start", contentIndex: 0, partial: message() });

		expect(tracker.liveRate()).toBeUndefined();
		expect(tracker.finalize(stale, message())).toBeUndefined();
	});

	it("qualifies final samples from exact output usage over first-to-last delta time", () => {
		let now = 100;
		const tracker = new OutputThroughputTracker({ monotonicNow: () => now, wallNow: () => 1234 });
		const generation = tracker.start(identity);
		tracker.observe(generation, delta("text_delta", "1234"));
		now = 600;
		tracker.observe(generation, delta("text_delta", "5678"));
		now = 5000;

		expect(tracker.finalize(generation, message({ responseModel: "routed-model" }))).toEqual({
			provider: identity.provider,
			model: identity.model,
			responseModel: "routed-model",
			outputTokens: 40,
			durationMs: 500,
			observedAt: 1234,
		});
		expect(tracker.liveRate()).toBeUndefined();
	});

	it("keeps full-response timing after rolling observations age out", () => {
		let now = 0;
		const tracker = new OutputThroughputTracker({ monotonicNow: () => now });
		const generation = tracker.start(identity);
		tracker.observe(generation, delta("text_delta", "a"));
		now = 3000;
		tracker.observe(generation, delta("text_delta", "b"));
		now = 6000;
		expect(tracker.liveRate()).toBeUndefined();
		expect(tracker.finalize(generation, message())?.durationMs).toBe(3000);
	});

	it.each(["stop", "length", "toolUse"] as const)("accepts the %s terminal reason", (stopReason) => {
		let now = 0;
		const tracker = new OutputThroughputTracker({ monotonicNow: () => now });
		const generation = tracker.start(identity);
		tracker.observe(generation, delta("text_delta", "a"));
		now = 1;
		tracker.observe(generation, delta("text_delta", "b"));
		expect(tracker.finalize(generation, message({ stopReason }))).toBeDefined();
	});

	it.each([
		["error response", { stopReason: "error" }],
		["aborted response", { stopReason: "aborted" }],
		["harness response", { origin: "harness" }],
		["wrong provider", { provider: "other" }],
		["wrong model", { model: "other" }],
		["zero output", { usage: { ...message().usage, output: 0 } }],
		["non-finite output", { usage: { ...message().usage, output: Number.NaN } }],
	] as const)("rejects a %s", (_label, overrides) => {
		let now = 0;
		const tracker = new OutputThroughputTracker({ monotonicNow: () => now });
		const generation = tracker.start(identity);
		tracker.observe(generation, delta("text_delta", "a"));
		now = 1;
		tracker.observe(generation, delta("text_delta", "b"));
		expect(tracker.finalize(generation, message(overrides as Partial<AssistantMessage>))).toBeUndefined();
	});

	it("rejects fewer than two observations and zero-duration observations", () => {
		const now = 0;
		const tracker = new OutputThroughputTracker({ monotonicNow: () => now });
		let generation = tracker.start(identity);
		tracker.observe(generation, delta("text_delta", "ab"));
		expect(tracker.finalize(generation, message())).toBeUndefined();

		generation = tracker.start(identity);
		tracker.observe(generation, delta("text_delta", "a"));
		tracker.observe(generation, delta("text_delta", "b"));
		expect(tracker.finalize(generation, message())).toBeUndefined();
	});

	it("increments generation on reset and superseding starts", () => {
		const tracker = new OutputThroughputTracker();
		const first = tracker.start(identity);
		const second = tracker.start(identity);
		tracker.reset();
		const third = tracker.generation;
		expect(second).toBeGreaterThan(first);
		expect(third).toBeGreaterThan(second);
	});
});

describe("OutputThroughputHistory", () => {
	it("keys provider and requested model without collisions and retains provenance only", () => {
		const history = new OutputThroughputHistory();
		history.add({ provider: "a", model: "bc", outputTokens: 10, durationMs: 1000, observedAt: 1 });
		history.add({
			provider: "ab",
			model: "c",
			responseModel: "routed",
			outputTokens: 30,
			durationMs: 1000,
			observedAt: 2,
		});
		expect(history.median("a", "bc")).toBe(10);
		expect(history.median("ab", "c")).toBe(30);
		expect(history.median("ab", "routed")).toBeUndefined();
	});

	it("returns deterministic odd and even medians", () => {
		const history = new OutputThroughputHistory();
		for (const rate of [30, 10, 20, 40]) {
			history.add({ ...identity, outputTokens: rate, durationMs: 1000, observedAt: rate });
		}
		expect(history.median(identity.provider, identity.model)).toBe(25);
		history.add({ ...identity, outputTokens: 50, durationMs: 1000, observedAt: 50 });
		expect(history.median(identity.provider, identity.model)).toBe(30);
	});

	it("retains only the latest 20 samples per requested key", () => {
		const history = new OutputThroughputHistory();
		for (let index = 1; index <= 21; index++) {
			history.add({ ...identity, outputTokens: index, durationMs: 1000, observedAt: index });
		}
		expect(history.samples(identity.provider, identity.model)).toHaveLength(20);
		expect(history.samples(identity.provider, identity.model)[0]?.outputTokens).toBe(2);
	});
});

describe("output throughput formatting", () => {
	it("distinguishes approximate live rates from historical estimates", () => {
		expect(formatLiveOutputRate(41.6)).toBe("~42 tok/s");
		expect(formatHistoricalOutputRate(41.6)).toBe("42 tok/s est.");
		expect(formatLiveOutputRate(0)).toBeUndefined();
		expect(formatHistoricalOutputRate(Number.NaN)).toBeUndefined();
	});
});
