import { describe, expect, it, vi } from "vitest";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { ModelSelectorComponent } from "../src/modes/interactive/components/model-selector.js";
import { InteractiveMode } from "../src/modes/interactive/interactive-mode.js";
import { initTheme } from "../src/modes/interactive/theme/theme.js";

initTheme("pi-dark");

interface Deferred {
	promise: Promise<void>;
	resolve: () => void;
	reject: (error: Error) => void;
}

function deferred(): Deferred {
	let resolve!: () => void;
	let reject!: (error: Error) => void;
	const promise = new Promise<void>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

function session(refresh: Promise<void>, history: object = {}) {
	return {
		refreshOutputThroughputHistory: vi.fn(() => refresh),
		outputThroughputHistory: history,
		settingsManager: { getShowTerminalProgress: () => false, getEnabledModels: () => undefined },
		modelRegistry: { refresh: vi.fn(), getAvailable: () => [{}] },
		scopedModels: [],
	};
}

function harness(initialSession: ReturnType<typeof session>) {
	const defaultEditor: Record<string, unknown> = { onAction: vi.fn() };
	const mode = Object.create(InteractiveMode.prototype) as Record<string, any>;
	Object.assign(mode, {
		isInitialized: true,
		runtimeHost: { session: initialSession },
		selectorOpenGeneration: 0,
		pendingSelectorOpenGeneration: undefined,
		showSelector: vi.fn(),
		defaultEditor,
		ui: { onDebug: undefined, stop: vi.fn(), terminal: { setProgress: vi.fn() } },
		unregisterSignalHandlers: vi.fn(),
		clearExtensionTerminalInputListeners: vi.fn(),
		footer: { dispose: vi.fn() },
		footerDataProvider: { dispose: vi.fn() },
	});
	return { mode, defaultEditor };
}

async function showModelSelector(mode: Record<string, any>): Promise<void> {
	await mode.showModelSelector();
}

async function showModelsSelector(mode: Record<string, any>): Promise<void> {
	await mode.showModelsSelector();
}

describe("GitHub Copilot selector visibility", () => {
	it("loads new and retained IDs into the unscoped selector with configured auth", async () => {
		const registry = ModelRegistry.create(
			AuthStorage.inMemory({ "github-copilot": { type: "api_key", key: "synthetic-test-key" } }),
		);
		const current = registry.find("github-copilot", "gpt-6-astra")!;
		const selector = new ModelSelectorComponent(
			{ requestRender: vi.fn() } as any,
			current,
			{ setDefaultModelAndProvider: vi.fn() } as any,
			registry,
			[],
			vi.fn(),
			vi.fn(),
		);
		await new Promise((resolve) => setTimeout(resolve, 0));
		const ids = (selector as any).allModels.map((item: { id: string }) => item.id);
		for (const id of [
			"claude-fable-5.1",
			"claude-opus-5",
			"claude-opus-5.5",
			"gpt-6-sol",
			"gpt-6-luna",
			"kimi-k3",
			"grok-4.6",
			"mai-code-1.1-flash",
			"gpt-6-astra",
		]) {
			expect(ids).toContain(id);
		}
	});
});

describe("deferred model-selector preparation", () => {
	it("lets Escape cancel a pending selector without opening it", async () => {
		const refresh = deferred();
		const { mode, defaultEditor } = harness(session(refresh.promise));
		mode.setupKeyHandlers();
		const opening = showModelSelector(mode);

		(defaultEditor.onEscape as () => void)();
		refresh.resolve();
		await opening;

		expect(mode.pendingSelectorOpenGeneration).toBeUndefined();
		expect(mode.showSelector).not.toHaveBeenCalled();
	});

	it("opens only the newest of two pending requests", async () => {
		const first = deferred();
		const initialSession = session(first.promise);
		const { mode } = harness(initialSession);
		const firstOpening = showModelSelector(mode);
		const second = deferred();
		initialSession.refreshOutputThroughputHistory.mockImplementationOnce(() => second.promise);
		const secondOpening = showModelSelector(mode);

		second.resolve();
		await secondOpening;
		first.resolve();
		await firstOpening;

		expect(mode.showSelector).toHaveBeenCalledTimes(1);
		expect(mode.pendingSelectorOpenGeneration).toBeUndefined();
	});

	it("clears pending state when the session is replaced", async () => {
		const refresh = deferred();
		const { mode } = harness(session(refresh.promise));
		const opening = showModelSelector(mode);
		mode.runtimeHost.session = session(Promise.resolve());

		refresh.resolve();
		await opening;

		expect(mode.showSelector).not.toHaveBeenCalled();
		expect(mode.pendingSelectorOpenGeneration).toBeUndefined();
	});

	it("invalidates pending preparation during shutdown", async () => {
		const refresh = deferred();
		const { mode } = harness(session(refresh.promise));
		const opening = showModelSelector(mode);

		mode.stop();
		refresh.resolve();
		await opening;

		expect(mode.showSelector).not.toHaveBeenCalled();
		expect(mode.pendingSelectorOpenGeneration).toBeUndefined();
		expect(mode.ui.stop).toHaveBeenCalledOnce();
	});

	it("opens from the last-valid snapshot when refresh rejects", async () => {
		const refresh = deferred();
		const { mode } = harness(session(refresh.promise));
		const opening = showModelSelector(mode);

		refresh.reject(new Error("refresh failed"));
		await expect(opening).resolves.toBeUndefined();

		expect(mode.showSelector).toHaveBeenCalledOnce();
		expect(mode.pendingSelectorOpenGeneration).toBeUndefined();
	});

	it("opens the scoped-model selector from the last-valid snapshot when refresh rejects", async () => {
		const refresh = deferred();
		const { mode } = harness(session(refresh.promise));
		const opening = showModelsSelector(mode);

		refresh.reject(new Error("refresh failed"));
		await expect(opening).resolves.toBeUndefined();

		expect(mode.showSelector).toHaveBeenCalledOnce();
		expect(mode.pendingSelectorOpenGeneration).toBeUndefined();
	});

	it("cancels scoped-model selector preparation after session replacement", async () => {
		const refresh = deferred();
		const { mode } = harness(session(refresh.promise));
		const opening = showModelsSelector(mode);
		mode.runtimeHost.session = session(Promise.resolve());

		refresh.resolve();
		await opening;

		expect(mode.showSelector).not.toHaveBeenCalled();
		expect(mode.pendingSelectorOpenGeneration).toBeUndefined();
	});
});
