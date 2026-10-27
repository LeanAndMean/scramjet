import { Container, Text, type TUI } from "@leanandmean/tui";
import { Type } from "typebox";
import { expect, it, vi } from "vitest";
import type { ToolDefinition, ToolRenderContext } from "../src/core/extensions/types.js";
import {
	ToolExecutionComponent,
	type ToolExecutionOptions,
} from "../src/modes/interactive/components/tool-execution.js";
import { InteractiveMode } from "../src/modes/interactive/interactive-mode.js";
import { initTheme } from "../src/modes/interactive/theme/theme.js";

it.each([undefined, "source", "rendered"] as const)(
	"transports optional Markdown presentation %s to both slots",
	(markdownToolDisplay) => {
		initTheme("pi-dark");
		const contexts: ToolRenderContext[] = [];
		const definition: ToolDefinition = {
			name: "custom",
			label: "custom",
			description: "custom",
			parameters: Type.Object({}),
			execute: async () => ({ content: [] }),
			renderCall: (_args, _theme, context) => {
				contexts.push(context);
				return new Text("call", 0, 0);
			},
			renderResult: (_result, _options, _theme, context) => {
				contexts.push(context);
				return new Text("result", 0, 0);
			},
		};
		const options: ToolExecutionOptions = markdownToolDisplay ? { markdownToolDisplay } : {};
		const card = new ToolExecutionComponent("custom", "id", {}, options, definition, {} as TUI, process.cwd());
		card.updateResult({ content: [], isError: false });
		expect(contexts).toHaveLength(3);
		expect(contexts.every((context) => context.markdownToolDisplay === (markdownToolDisplay ?? "source"))).toBe(true);
	},
);

it("refreshes Markdown presentation without replacing tool state, controls or lifecycle guards", () => {
	initTheme("pi-dark");
	const contexts: ToolRenderContext[] = [];
	const args = { text: "arguments" };
	const result = { content: [{ type: "text", text: "result" }], isError: false };
	const renderResult = vi.fn((_result, _options, _theme, context: ToolRenderContext) => {
		contexts.push(context);
		const text = context.lastComponent instanceof Text ? context.lastComponent : new Text("", 0, 0);
		text.setText(`${context.markdownToolDisplay} ${context.expanded}`);
		return text;
	});
	const definition: ToolDefinition = {
		name: "custom",
		label: "custom",
		description: "custom",
		parameters: Type.Object({}),
		execute: async () => ({ content: [] }),
		renderResult,
	};
	const ui = { rebuild: vi.fn(), requestRender: vi.fn() };
	const card = new ToolExecutionComponent("custom", "id", args, {}, definition, ui as unknown as TUI, process.cwd());
	card.markExecutionStarted();
	card.setArgsComplete();
	card.updateResult(result);
	card.setExpanded(true);
	const controls = Object.assign(new Text("CONTROL", 0, 0), { handleInput: vi.fn() });
	card.attachCommittedContext(controls);
	const before = contexts.at(-1)!;
	card.seal();
	card.markCommitted();
	card.setMarkdownToolDisplay("rendered");
	const after = contexts.at(-1)!;
	expect(after).toMatchObject({
		args,
		expanded: true,
		executionStarted: true,
		argsComplete: true,
		isPartial: false,
		toolCallId: "id",
		markdownToolDisplay: "rendered",
	});
	expect(after.state).toBe(before.state);
	expect(after.lastComponent).toBe(before.lastComponent);
	expect(renderResult.mock.calls.at(-1)![0]).toEqual({ content: result.content, details: undefined });
	expect(card.render(80).join("\n")).toContain("rendered true");
	expect(card.render(80).join("\n")).toContain("CONTROL");
	card.handleInput("input");
	expect(controls.handleInput).toHaveBeenCalledWith("input");
	before.invalidate();
	expect(ui.rebuild).not.toHaveBeenCalled();
	after.invalidate();
	expect(ui.rebuild).toHaveBeenCalledOnce();
	card.detach();
	after.invalidate();
	expect(ui.rebuild).toHaveBeenCalledOnce();
});

it("allows a committed tool renderer to settle details requested by expansion", async () => {
	initTheme("pi-dark");
	let settleExpandedRenderer = () => {};
	const committedChatContainer = new Container();
	const chatContainer = new Container();
	const rebuild = vi.fn();
	const mode = Object.create(InteractiveMode.prototype) as Record<string, unknown>;
	Object.assign(mode, {
		isInitialized: true,
		ui: { requestRender: vi.fn(), commit: vi.fn(), rebuild, getViewportState: () => undefined },
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
