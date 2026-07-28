import { Container, Text } from "@leanandmean/tui";
import { expect, it, vi } from "vitest";
import type { ToolRenderContext } from "../src/core/extensions/types.js";
import { InteractiveMode } from "../src/modes/interactive/interactive-mode.js";
import { initTheme } from "../src/modes/interactive/theme/theme.js";

it("allows a committed tool renderer to settle details requested by expansion", async () => {
	initTheme("pi-dark");
	let settleExpandedRenderer = () => {};
	const committedChatContainer = new Container();
	const chatContainer = new Container();
	const rebuild = vi.fn();
	const mode = Object.create(InteractiveMode.prototype) as Record<string, unknown>;
	Object.assign(mode, {
		isInitialized: true,
		ui: { requestRender: vi.fn(), commit: vi.fn(), rebuild },
		footer: { invalidate: vi.fn() },
		committedChatContainer,
		chatContainer,
		mutableChatComponents: new Set(),
		pendingTools: new Map(),
		pendingToolFinalizations: new Set(),
		runtimeHost: {
			session: {
				settingsManager: { getShowImages: () => true, getImageWidthCells: () => 60 },
				sessionManager: { getCwd: () => process.cwd() },
			},
		},
		toolOutputExpanded: false,
		getRegisteredToolDefinition: () => ({
			renderCall: () => new Text("tool", 0, 0),
			renderResult: (
				_result: unknown,
				_options: unknown,
				_theme: unknown,
				context: ToolRenderContext<{ scheduled?: boolean; settled?: boolean }>,
			) => {
				if (!context.expanded) return new Text("COLLAPSED", 0, 0);
				if (!context.state.scheduled) {
					context.state.scheduled = true;
					settleExpandedRenderer = () => {
						context.state.settled = true;
						context.invalidate();
					};
				}
				return new Text(context.state.settled ? "EXPANDED-DETAIL" : "LOADING-EXPANDED", 0, 0);
			},
		}),
	});
	const emit = (event: unknown) =>
		(mode as unknown as { handleEvent(event: unknown): Promise<void> }).handleEvent(event);

	await emit({ type: "tool_execution_start", toolCallId: "deferred", toolName: "deferred", args: {} });
	await emit({
		type: "tool_execution_end",
		toolCallId: "deferred",
		result: { content: [] },
		isError: false,
	});
	(mode.setToolsExpanded as (expanded: boolean) => void).call(mode, true);
	rebuild.mockClear();
	settleExpandedRenderer();

	expect(rebuild).toHaveBeenCalledOnce();
	expect(committedChatContainer.render(80).join("\n")).toContain("EXPANDED-DETAIL");
});
