import { Container, type TUI } from "@leanandmean/tui";
import { expect, it, vi } from "vitest";
import { InteractiveMode } from "../src/modes/interactive/interactive-mode.js";
import { initTheme } from "../src/modes/interactive/theme/theme.js";

it("commits finalized bash output after a replaceable status", async () => {
	initTheme("pi-dark");
	const committedChatContainer = new Container();
	const mode = Object.create(InteractiveMode.prototype) as Record<string, unknown>;
	Object.assign(mode, {
		ui: { requestRender: vi.fn(), commit: vi.fn() } as unknown as TUI,
		committedChatContainer,
		chatContainer: new Container(),
		pendingMessagesContainer: new Container(),
		pendingBashComponents: [],
		mutableChatComponents: new Set(),
		runtimeHost: {
			session: {
				isStreaming: false,
				extensionRunner: { emitUserBash: async () => undefined },
				sessionManager: { getCwd: () => process.cwd() },
				executeBash: async (_command: string, onOutput: (chunk: string) => void) => {
					onOutput("FINAL-BASH-OUTPUT");
					return { output: "FINAL-BASH-OUTPUT", exitCode: 0, cancelled: false, truncated: false };
				},
				recordBashResult: vi.fn(),
			},
		},
	});

	(mode.showStatus as (message: string) => void).call(mode, "STATUS-BEFORE-BASH");
	await (mode.handleBashCommand as (command: string) => Promise<void>).call(mode, "proof");

	expect(committedChatContainer.render(80).join("\n")).toContain("FINAL-BASH-OUTPUT");
});
