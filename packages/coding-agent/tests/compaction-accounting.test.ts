import type { AgentMessage } from "@leanandmean/agent";
import type { AssistantMessage } from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
import {
	DEFAULT_COMPACTION_SETTINGS,
	estimateContextTokens,
	estimateTokens,
	getLastAssistantUsage,
	prepareCompaction,
} from "../src/core/compaction/compaction.js";
import { buildSessionContext, SessionManager } from "../src/core/session-manager.js";

const model = { provider: "openai", id: "accounting-model" };
function assistant(overrides: Partial<AssistantMessage> = {}): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text: "provider content" }],
		api: "openai-completions",
		provider: model.provider,
		model: model.id,
		usage: { input: 249_990, output: 10, cacheRead: 0, cacheWrite: 0, totalTokens: 250_000 },
		stopReason: "toolUse",
		timestamp: 1,
		...overrides,
	};
}
const harness = assistant({
	origin: "harness",
	content: [{ type: "text", text: "harness content" }],
	usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0 },
});
const tool: AgentMessage = {
	role: "toolResult",
	toolCallId: "call",
	toolName: "read",
	content: [{ type: "text", text: "x".repeat(192) }],
	isError: false,
	timestamp: 1,
};

describe("compaction accounting", () => {
	it("keeps the provider anchor across synthetic zero usage and estimates all trailing content", () => {
		const trailingTokens = estimateTokens(harness) + estimateTokens(tool);
		expect(estimateContextTokens([assistant(), harness, tool])).toEqual({
			tokens: 250_000 + trailingTokens,
			usageTokens: 250_000,
			trailingTokens,
			lastUsageIndex: 0,
		});
	});

	it.each([
		{ origin: "harness" as const },
		{ stopReason: "error" as const },
		{ stopReason: "aborted" as const },
		...(["input", "output", "cacheRead", "cacheWrite", "totalTokens"] as const).flatMap((key) =>
			[NaN, Infinity, -1].map((value) => ({ usage: { ...assistant().usage, [key]: value } })),
		),
		{ usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0 } },
		{ usage: { input: Number.MAX_VALUE, output: Number.MAX_VALUE, cacheRead: 0, cacheWrite: 0 } },
	])("skips invalid usage but estimates its content: %j", (overrides) => {
		const invalid = assistant(overrides);
		const trailing = estimateTokens(invalid) + estimateTokens(tool);
		expect(estimateContextTokens([assistant(), invalid, tool])).toMatchObject({
			tokens: 250_000 + trailing,
			lastUsageIndex: 0,
		});
		const manager = SessionManager.inMemory();
		for (const message of [assistant(), invalid, tool]) manager.appendMessage(message);
		expect(getLastAssistantUsage(manager.getBranch())).toEqual(assistant().usage);
		expect(estimateContextTokens([invalid, tool])).toMatchObject({ tokens: trailing, lastUsageIndex: null });
	});

	it.each([undefined, "provider"] as const)("accepts valid %s provenance and existing arithmetic", (origin) => {
		const declared = assistant({ origin, usage: { ...assistant().usage, totalTokens: 123 } });
		expect(estimateContextTokens([declared]).tokens).toBe(123);
		const components = assistant({ origin, usage: { ...assistant().usage, totalTokens: 0 } });
		expect(estimateContextTokens([components]).tokens).toBe(250_000);
	});

	it.each(["compaction", "model_change"] as const)("qualifies usage after %s by entry order", (boundary) => {
		const manager = SessionManager.inMemory();
		manager.appendMessage({ role: "user", content: "discarded".repeat(100), timestamp: 1 });
		const kept = manager.appendMessage(assistant({ timestamp: 9000 }));
		if (boundary === "compaction") manager.appendCompaction("previous summary".repeat(40), kept, 999_999);
		else manager.appendModelChange(model.provider, model.id);
		manager.appendMessage(harness);
		manager.appendMessage(tool);
		let entries = manager.getBranch();
		for (const entry of entries) entry.timestamp = new Date(9000).toISOString();
		const messages = buildSessionContext(entries).messages;
		if (boundary === "compaction") expect(messages).toHaveLength(4);
		expect(prepareCompaction(entries, DEFAULT_COMPACTION_SETTINGS, model)!.tokensBefore).toBe(
			messages.reduce((sum, message) => sum + estimateTokens(message), 0),
		);
		manager.appendMessage(assistant({ timestamp: 0 }));
		manager.appendMessage(tool);
		entries = manager.getBranch();
		for (const entry of entries) entry.timestamp = new Date(9000).toISOString();
		expect(prepareCompaction(entries, DEFAULT_COMPACTION_SETTINGS, model)!.tokensBefore).toBe(
			250_000 + estimateTokens(tool),
		);
	});

	it.each([model, { provider: "other", id: model.id }, { provider: model.provider, id: "other" }])(
		"qualifies against current selection: %j",
		(selected) => {
			const manager = SessionManager.inMemory();
			manager.appendMessage(assistant());
			manager.appendMessage(harness);
			const expected =
				selected === model
					? 250_000 + estimateTokens(harness)
					: estimateTokens(assistant()) + estimateTokens(harness);
			expect(prepareCompaction(manager.getBranch(), DEFAULT_COMPACTION_SETTINGS, selected)!.tokensBefore).toBe(
				expected,
			);
		},
	);

	it("supports two-argument routing reconstruction without harness identity", () => {
		const manager = SessionManager.inMemory();
		manager.appendMessage(assistant());
		manager.appendMessage({ ...harness, provider: "harness", model: "synthetic" });
		expect(prepareCompaction(manager.getBranch(), DEFAULT_COMPACTION_SETTINGS)!.tokensBefore).toBe(
			250_000 + estimateTokens(harness),
		);
		manager.appendModelChange("other", "other");
		expect(prepareCompaction(manager.getBranch(), DEFAULT_COMPACTION_SETTINGS)!.tokensBefore).toBe(
			estimateTokens(assistant()) + estimateTokens(harness),
		);
	});

	it("estimates every message when routing cannot be reconstructed", () => {
		const manager = SessionManager.inMemory();
		manager.appendMessage({ role: "user", content: "all content", timestamp: 1 });
		manager.appendMessage(harness);
		manager.appendMessage(tool);
		const entries = manager.getBranch();
		expect(prepareCompaction(entries, DEFAULT_COMPACTION_SETTINGS)!.tokensBefore).toBe(
			buildSessionContext(entries).messages.reduce((sum, message) => sum + estimateTokens(message), 0),
		);
	});
});
