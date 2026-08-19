import { describe, expect, it, vi } from "vitest";
import { InteractiveMode } from "../src/modes/interactive/interactive-mode.js";

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
