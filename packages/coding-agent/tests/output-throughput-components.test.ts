import { describe, expect, it } from "vitest";
import { OutputThroughputHistory } from "../src/core/output-throughput.js";
import { FooterComponent } from "../src/modes/interactive/components/footer.js";
import { ModelSelectorComponent } from "../src/modes/interactive/components/model-selector.js";
import { ScopedModelsSelectorComponent } from "../src/modes/interactive/components/scoped-models-selector.js";
import { initTheme } from "../src/modes/interactive/theme/theme.js";
import { stripAnsi } from "../src/utils/ansi.js";

initTheme("pi-dark");

const model = {
	provider: "test-provider",
	id: "test-model",
	name: "Test Model",
	api: "openai-completions",
	baseUrl: "https://example.invalid",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 1000,
	maxTokens: 100,
} as any;

function historyWithSample(): OutputThroughputHistory {
	const history = new OutputThroughputHistory();
	history.add({ provider: model.provider, model: model.id, outputTokens: 42, durationMs: 1000, observedAt: 1 });
	return history;
}

function footerSession(liveOutputRate: number | undefined, medianOutputRate?: number): any {
	return {
		state: { model, thinkingLevel: "off" },
		liveOutputRate,
		medianOutputRate,
		sessionManager: {
			getEntries: () => [
				{
					type: "message",
					message: {
						role: "assistant",
						usage: {
							input: 0,
							output: 10,
							cacheRead: 0,
							cacheWrite: 0,
							cost: { total: 0 },
						},
					},
				},
			],
			getCwd: () => "/tmp/project",
			getSessionName: () => undefined,
		},
		getContextUsage: () => ({ tokens: 10, percent: 1, contextWindow: 1000, contextWindowBudget: 1000 }),
		modelRegistry: { isUsingOAuth: () => false },
	};
}

const footerData = {
	getGitBranch: () => null,
	getAvailableProviderCount: () => 1,
	getExtensionStatuses: () => new Map(),
} as any;

describe("output throughput components", () => {
	it("shows only median output rate and respects narrow widths", () => {
		const active = stripAnsi(new FooterComponent(footerSession(42, 30), footerData).render(80).join("\n"));
		const completed = stripAnsi(new FooterComponent(footerSession(undefined, 30), footerData).render(80).join("\n"));
		const empty = stripAnsi(new FooterComponent(footerSession(undefined), footerData).render(80).join("\n"));
		const narrow = new FooterComponent(footerSession(42), footerData).render(8);

		expect(active).not.toContain("42tok/s");
		expect(active).toContain("↓10 (30tok/s)");
		expect(completed).toContain("30tok/s");
		expect(empty).not.toContain("tok/s");
		expect(narrow.every((line) => stripAnsi(line).length <= 8)).toBe(true);
	});

	it("keeps scoped-selector rows unchanged without history and puts status rightmost with history", () => {
		const config = { allModels: [model], enabledModelIds: [] };
		const callbacks = { onChange: () => {}, onPersist: () => {}, onCancel: () => {} };
		const withoutHistory = stripAnsi(new ScopedModelsSelectorComponent(config, callbacks).render(80).join("\n"));
		const withHistory = stripAnsi(
			new ScopedModelsSelectorComponent(config, callbacks, historyWithSample()).render(80).join("\n"),
		);

		expect(withoutHistory).not.toContain("tok/s");
		expect(withHistory).toContain("test-model [test-provider] 42tok/s ✗");
	});

	it("adds requested-model history to the model selector without changing the old constructor", async () => {
		const registry = {
			refresh: () => {},
			getError: () => undefined,
			getAvailable: async () => [model],
			find: () => model,
		};
		const settings = { setDefaultModelAndProvider: () => {} };
		const tui = { requestRender: () => {} };
		const oldForm = new ModelSelectorComponent(
			tui as any,
			model,
			settings as any,
			registry as any,
			[],
			() => {},
			() => {},
		);
		const sampled = new ModelSelectorComponent(
			tui as any,
			model,
			settings as any,
			registry as any,
			[],
			() => {},
			() => {},
			undefined,
			historyWithSample(),
		);
		await new Promise((resolve) => setTimeout(resolve, 0));

		expect(stripAnsi(oldForm.render(80).join("\n"))).not.toContain("tok/s");
		expect(stripAnsi(sampled.render(80).join("\n"))).toContain("test-model [test-provider] 42tok/s ✓");
	});
});
