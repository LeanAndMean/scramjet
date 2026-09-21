import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { Container, TUI } from "@leanandmean/tui";
import { expect, vi } from "vitest";
import { HeadlessTerminal } from "../../../tui/tests/helpers/headless-terminal.js";
import { AgentSession, type AgentSessionEvent } from "../../src/core/agent-session.js";
import { createAgentSessionRuntime } from "../../src/core/agent-session-runtime.js";
import { createAgentSessionServices } from "../../src/core/agent-session-services.js";
import { AuthStorage } from "../../src/core/auth-storage.js";
import type { ExtensionFactory, ExtensionUIContext } from "../../src/core/extensions/index.js";
import { KeybindingsManager } from "../../src/core/keybindings.js";
import { ModelRegistry } from "../../src/core/model-registry.js";
import { SessionManager } from "../../src/core/session-manager.js";
import { SettingsManager } from "../../src/core/settings-manager.js";
import { InteractiveMode } from "../../src/modes/interactive/interactive-mode.js";
import { onThemeChange, stopThemeWatcher } from "../../src/modes/interactive/theme/theme.js";
import { ensureTool } from "../../src/utils/tools-manager.js";

interface InteractiveInternals {
	ui: TUI;
	headerContainer: Container;
	committedChatContainer: Container;
	chatContainer: Container;
	pendingMessagesContainer: Container;
	statusContainer: Container;
	widgetContainerAbove: Container;
	editorContainer: Container;
	widgetContainerBelow: Container;
	footer: Container;
	handleEvent(event: AgentSessionEvent): Promise<void>;
	configureRetainedViewport(): void;
	handleCtrlZ(): Promise<void>;
	clearTranscript(): void;
	handleReloadCommand(): Promise<void>;
	handleExtensionNewSession(): Promise<{ cancelled: boolean }>;
	renderCurrentSessionState(): void;
	setToolsExpanded(expanded: boolean): void;
	toggleThinkingBlockVisibility(): void;
	updatePendingMessagesDisplay(): void;
}

export async function createProductionInteractiveHarness(
	columns = 60,
	rows = 24,
	extension?: ExtensionFactory,
	viewport = true,
	settings?: SettingsManager,
) {
	const directory = mkdtempSync(join(tmpdir(), "scramjet-interactive-test-"));
	const terminal = new HeadlessTerminal(columns, rows);
	const authStorage = AuthStorage.inMemory();
	const settingsManager =
		settings ??
		SettingsManager.inMemory({
			theme: "pi-dark",
			quietStartup: true,
			compaction: { enabled: false },
			retry: { enabled: false },
		});
	let extensionUI: ExtensionUIContext | undefined;
	const modelRegistry = ModelRegistry.inMemory(authStorage);
	vi.spyOn(modelRegistry, "getAvailable").mockReturnValue([]);
	const services = await createAgentSessionServices({
		cwd: directory,
		agentDir: directory,
		authStorage,
		settingsManager,
		modelRegistry,
		resourceLoaderOptions: {
			noExtensions: true,
			noSkills: true,
			noPromptTemplates: true,
			noThemes: true,
			noContextFiles: true,
			builtinInit: async (pi) => {
				pi.on("session_start", (_event, ctx) => {
					extensionUI = ctx.ui;
				});
				await extension?.(pi);
			},
		},
	});
	const runtime = await createAgentSessionRuntime(
		async ({ sessionManager }) => ({
			services,
			diagnostics: services.diagnostics,
			session: new AgentSession({
				...services,
				sessionManager,
				initialActiveToolNames: [],
				agent: new Agent({
					streamFn: () => {
						throw new Error("Production layout tests must not invoke a model");
					},
				}),
			}),
		}),
		{ cwd: directory, agentDir: directory, sessionManager: SessionManager.inMemory(directory) },
	);
	const keybindings = vi.spyOn(KeybindingsManager, "create").mockImplementation(() => new KeybindingsManager());
	let mode: InteractiveMode;
	try {
		mode = new InteractiveMode(runtime, { terminal });
	} finally {
		keybindings.mockRestore();
	}
	const internals = mode as unknown as InteractiveInternals;
	const legacy = viewport
		? undefined
		: vi
				.spyOn(internals, "configureRetainedViewport")
				.mockImplementation(() => internals.ui.setLiveRegionStart(internals.chatContainer));
	expect(vi.isMockFunction(ensureTool)).toBe(true);
	const provisioningCalls = vi.mocked(ensureTool).mock.calls.length;
	try {
		await mode.init();
		expect(
			vi
				.mocked(ensureTool)
				.mock.calls.slice(provisioningCalls)
				.map(([name]) => name),
		).toEqual(["fd", "rg"]);
	} finally {
		legacy?.mockRestore();
	}
	if (!extensionUI) throw new Error("Production extension UI was not bound");
	extensionUI.setWorkingIndicator({ frames: ["⠋"] });
	return {
		terminal,
		mode,
		internals,
		extensionUI,
		session: runtime.session,
		// Await the real UI consumer; session subscriptions do not await async listeners.
		emit: (event: AgentSessionEvent) => internals.handleEvent(event),
		async frame() {
			await internals.ui.renderNow({ requireFlush: true });
			return terminal.visibleLines();
		},
		async dispose() {
			mode.stop();
			await runtime.dispose();
			stopThemeWatcher();
			onThemeChange(() => {});
			await terminal.flush();
			rmSync(directory, { recursive: true, force: true });
		},
	};
}
