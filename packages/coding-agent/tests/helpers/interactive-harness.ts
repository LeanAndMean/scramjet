import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { Container, TUI } from "@leanandmean/tui";
import { vi } from "vitest";
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

vi.mock("../../src/utils/tools-manager.js", () => ({ ensureTool: async () => undefined }));

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
}

export async function createProductionInteractiveHarness(columns = 60, rows = 24, extension?: ExtensionFactory) {
	const directory = mkdtempSync(join(tmpdir(), "scramjet-interactive-test-"));
	const terminal = new HeadlessTerminal(columns, rows);
	const authStorage = AuthStorage.inMemory();
	const settingsManager = SettingsManager.inMemory({
		theme: "pi-dark",
		quietStartup: true,
		compaction: { enabled: false },
		retry: { enabled: false },
	});
	let extensionUI: ExtensionUIContext | undefined;
	const services = await createAgentSessionServices({
		cwd: directory,
		agentDir: directory,
		authStorage,
		settingsManager,
		modelRegistry: ModelRegistry.inMemory(authStorage),
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
	vi.useFakeTimers();
	const keybindings = vi.spyOn(KeybindingsManager, "create").mockImplementation(() => new KeybindingsManager());
	let mode: InteractiveMode;
	try {
		mode = new InteractiveMode(runtime, { terminal });
	} finally {
		keybindings.mockRestore();
	}
	await mode.init();
	const internals = mode as unknown as InteractiveInternals;
	if (!extensionUI) throw new Error("Production extension UI was not bound");
	return {
		terminal,
		mode,
		internals,
		extensionUI,
		session: runtime.session,
		// Await the real UI consumer; session subscriptions do not await async listeners.
		emit: (event: AgentSessionEvent) => internals.handleEvent(event),
		async frame() {
			await vi.advanceTimersByTimeAsync(20);
			await terminal.flush();
			return terminal.visibleLines();
		},
		async dispose() {
			mode.stop();
			await vi.advanceTimersByTimeAsync(20);
			vi.useRealTimers();
			await runtime.dispose();
			stopThemeWatcher();
			onThemeChange(() => {});
			await terminal.flush();
			rmSync(directory, { recursive: true, force: true });
		},
	};
}
