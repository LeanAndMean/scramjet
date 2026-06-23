import { initTheme, ToolExecutionComponent } from "@scramjet/coding-agent";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { USER_INPUT_PARKED_TYPE } from "../src/history.js";
import { registerUserInputTool, USER_INPUT_TYPE } from "../src/user-input.js";
import { freshState, lifecycleFor, recordingPi } from "./helpers.js";

type UserInputParams = {
	type: "confirm" | "select" | "freetext";
	message: string;
	options?: { value: string; label: string; description?: string }[];
	recommended?: number;
	placeholder?: string;
};

function toolFor(state = freshState()) {
	const { pi, tools, handlers } = recordingPi();
	registerUserInputTool(pi, state);
	const tool = tools.find((t: any) => t.name === "get_scramjet_user_input");
	if (!tool) throw new Error("get_scramjet_user_input tool not registered");
	const execute = (params: UserInputParams, ctx?: unknown) =>
		tool.execute("call-id", params, undefined, undefined, ctx) as Promise<any>;
	return { state, pi, tools, handlers, tool, execute };
}

function mockUICtx(customResult: unknown = null, inputResult: string | undefined = undefined) {
	return {
		ui: {
			custom: (_factory: any) => Promise.resolve(customResult),
			input: (_title: string, _placeholder?: string) => Promise.resolve(inputResult),
		},
	};
}

function mockUICtxWithFactory() {
	let capturedFactory: any = null;
	return {
		ctx: {
			ui: {
				custom: (factory: any) => {
					return new Promise((resolve) => {
						const tui = { requestRender: () => {} };
						const theme = {
							fg: (_color: string, text: string) => text,
							bold: (text: string) => text,
						};
						capturedFactory = factory(tui, theme, {}, resolve);
					});
				},
				input: (_title: string, _placeholder?: string) => Promise.resolve(undefined),
			},
		},
		getFactory: () => capturedFactory,
	};
}

const renderTheme = {
	fg: (_color: string, text: string) => text,
	bold: (text: string) => text,
};

function renderText(component: { render(width: number): string[] } | undefined): string {
	return component?.render(120).join("\n") ?? "";
}

function visibleText(text: string): string {
	return text.replace(/\x1b\[[0-9;]*m/g, "");
}

describe("registerUserInputTool — registration", () => {
	it("registers exactly the get_scramjet_user_input tool", () => {
		const { tools } = toolFor();
		expect(tools).toHaveLength(1);
		expect(tools[0].name).toBe("get_scramjet_user_input");
	});

	it("has a promptSnippet for system prompt visibility", () => {
		const { tool } = toolFor();
		expect(tool.promptSnippet).toBeDefined();
		expect(tool.promptSnippet).toContain("get_scramjet_user_input");
	});

	it("has a custom call renderer", () => {
		const { tool } = toolFor();
		expect(tool.renderCall).toBeTypeOf("function");
	});

	it("renders the supplied message in the tool call row", () => {
		const { tool } = toolFor();
		const component = tool.renderCall(
			{ type: "freetext", message: "What release title should I use?" },
			renderTheme,
			{},
		);

		expect(renderText(component)).toContain("What release title should I use?");
	});

	it("falls back to the tool name while args stream without a message", () => {
		const { tool } = toolFor();
		const component = tool.renderCall({}, renderTheme, {});

		expect(renderText(component)).toContain("get_scramjet_user_input");
	});

	it("renders a structured prompt summary in Pi's tool row after execution", () => {
		initTheme(undefined, false);
		const { tool } = toolFor();
		const component = new ToolExecutionComponent(
			"get_scramjet_user_input",
			"call-id",
			{
				type: "select",
				message: "Which bump level?",
				options: [{ value: "patch", label: "Patch", description: "Bug fixes only" }],
			},
			undefined,
			tool,
			{ requestRender: () => {} } as any,
			process.cwd(),
		);
		component.updateResult(
			{
				content: [{ type: "text", text: JSON.stringify({ selected: "patch" }) }],
				details: {
					type: "select",
					selected: "patch",
					options: [{ value: "patch", label: "Patch", description: "Bug fixes only" }],
				},
				isError: false,
			},
			false,
		);

		const output = visibleText(component.render(120).join("\n"));
		expect(output).toContain("Which bump level?");
		expect(output).toContain("Patch");
		expect(output).toContain("Bug fixes only");
		expect(output).toContain("→ Patch");
		expect(output.trim()).not.toBe(JSON.stringify({ selected: "patch" }));
	});

	it("has the expected schema shape with type enum and flat optional fields", () => {
		const { tool } = toolFor();
		const params = tool.parameters;
		expect(params.required).toContain("type");
		expect(params.required).toContain("message");
		expect(params.properties.type.anyOf).toBeDefined();
		expect(params.properties.message.type).toBe("string");
		expect(params.properties.options).toBeDefined();
		expect(params.properties.recommended).toBeDefined();
		expect(params.properties.placeholder).toBeDefined();
	});
});

function renderResultText(tool: any, result: any, args: Partial<UserInputParams> = {}): string {
	const component = tool.renderResult(result, { expanded: false, isPartial: false }, renderTheme, {
		args,
		toolCallId: "call-id",
		invalidate: () => {},
		lastComponent: undefined,
		state: {},
		cwd: process.cwd(),
		executionStarted: true,
		argsComplete: true,
		isPartial: false,
		expanded: false,
		showImages: true,
		isError: false,
	});
	return visibleText(renderText(component));
}

describe("registerUserInputTool — renderResult", () => {
	it("renders confirm yes with the prompt message", () => {
		const { tool } = toolFor();
		const output = renderResultText(
			tool,
			{
				content: [{ type: "text", text: JSON.stringify({ confirmed: true }) }],
				details: { type: "confirm", confirmed: true },
			},
			{ type: "confirm", message: "Deploy now?" },
		);

		expect(output).toContain("Deploy now?");
		expect(output).toContain("Yes");
	});

	it("renders confirm no with the prompt message", () => {
		const { tool } = toolFor();
		const output = renderResultText(
			tool,
			{
				content: [{ type: "text", text: JSON.stringify({ confirmed: false }) }],
				details: { type: "confirm", confirmed: false },
			},
			{ type: "confirm", message: "Deploy now?" },
		);

		expect(output).toContain("Deploy now?");
		expect(output).toContain("No");
	});

	it("renders cancelled confirm with the prompt message", () => {
		const { tool } = toolFor();
		const output = renderResultText(
			tool,
			{
				content: [{ type: "text", text: JSON.stringify({ cancelled: true }) }],
				details: { type: "confirm", cancelled: true },
			},
			{ type: "confirm", message: "Deploy now?" },
		);

		expect(output).toContain("Deploy now?");
		expect(output).toContain("Cancelled");
	});

	it("renders select with all options and the selected value", () => {
		const { tool } = toolFor();
		const options = [
			{ value: "patch", label: "Patch", description: "Bug fixes only" },
			{ value: "minor", label: "Minor", description: "New features" },
		];
		const output = renderResultText(
			tool,
			{
				content: [{ type: "text", text: JSON.stringify({ selected: "minor" }) }],
				details: { type: "select", selected: "minor", options },
			},
			{ type: "select", message: "Which bump?", options },
		);

		expect(output).toContain("Which bump?");
		expect(output).toContain("Patch");
		expect(output).toContain("Bug fixes only");
		expect(output).toContain("Minor");
		expect(output).toContain("New features");
		expect(output).toContain("→ Minor");
		expect(output).toContain("  Patch");
		expect(output).not.toContain("Selected:");
	});

	it("renders cancelled select with all options", () => {
		const { tool } = toolFor();
		const options = [
			{ value: "patch", label: "Patch", description: "Bug fixes only" },
			{ value: "minor", label: "Minor", description: "New features" },
		];
		const output = renderResultText(
			tool,
			{
				content: [{ type: "text", text: JSON.stringify({ cancelled: true }) }],
				details: { type: "select", cancelled: true, options },
			},
			{ type: "select", message: "Which bump?", options },
		);

		expect(output).toContain("Which bump?");
		expect(output).toContain("Patch");
		expect(output).toContain("Bug fixes only");
		expect(output).toContain("Minor");
		expect(output).toContain("New features");
		expect(output).toContain("Cancelled");
	});

	it("renders freetext parked status with the prompt message", () => {
		const { tool } = toolFor();
		const output = renderResultText(
			tool,
			{
				content: [{ type: "text", text: JSON.stringify({ parked: true }) }],
				details: { type: "freetext", parked: true },
			},
			{ type: "freetext", message: "What should the title be?" },
		);

		expect(output).toContain("What should the title be?");
		expect(output).toContain("Parked for reply");
	});

	it("renders the tool text for error details", () => {
		const { tool } = toolFor();
		const output = renderResultText(
			tool,
			{
				content: [{ type: "text", text: "Validation error: message is required" }],
				details: { error: "validation" },
			},
			{ type: "confirm", message: "" },
		);

		expect(output.trimEnd()).toBe("Validation error: message is required");
	});

	it("falls back to empty output for missing details", () => {
		const { tool } = toolFor();
		const output = renderResultText(
			tool,
			{ content: [{ type: "text", text: JSON.stringify({ selected: "patch" }) }] },
			{ type: "select", message: "Which bump?" },
		);

		expect(output).toBe("");
	});

	it("falls back to empty output for unrecognized details", () => {
		const { tool } = toolFor();
		const output = renderResultText(
			tool,
			{ content: [{ type: "text", text: JSON.stringify({ ok: true }) }], details: { type: "other" } },
			{ type: "confirm", message: "Proceed?" },
		);

		expect(output).toBe("");
	});
});

describe("registerUserInputTool — phase gate", () => {
	it.each(["idle", "dormant", "reported", "waiting"] as const)(
		"rejects with a helpful error and no terminate when phase is %s",
		async (phase) => {
			const { execute } = toolFor(freshState({ lifecycle: lifecycleFor(phase) }));
			const ctx = { ui: {} };
			const result = await execute({ type: "confirm", message: "Proceed?" }, ctx);

			expect(result.terminate).toBeUndefined();
			expect(result.details.error).toBe("out-of-phase");
			expect(result.details.phase).toBe(phase);
			expect(String(result.content[0].text)).toContain("not available right now");
		},
	);

	it.each(["running", "probing"] as const)("accepts calls when phase is %s", async (phase) => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor(phase) }));
		const ctx = mockUICtx("yes");
		const result = await execute({ type: "confirm", message: "Proceed?" }, ctx);

		expect(result.details.error).not.toBe("out-of-phase");
	});
});

describe("registerUserInputTool — non-TUI guard", () => {
	it("returns a helpful error when ctx.ui is absent", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ type: "confirm", message: "Proceed?" }, {});

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("non-tui");
		expect(String(result.content[0].text)).toContain("TUI environment");
	});

	it("returns a helpful error when ctx is undefined", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ type: "confirm", message: "Proceed?" }, undefined);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("non-tui");
	});
});

describe("registerUserInputTool — runtime validation", () => {
	const ctx = { ui: {} };

	it("rejects select without options", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ type: "select", message: "Pick one" }, ctx);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("validation");
		expect(String(result.content[0].text)).toContain("options");
		expect(String(result.content[0].text)).toContain("non-empty array");
	});

	it("rejects select with empty options array", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ type: "select", message: "Pick one", options: [] }, ctx);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("validation");
		expect(String(result.content[0].text)).toContain("non-empty array");
	});

	it.each([
		["missing value", [{ label: "A" }]],
		["empty value", [{ value: "", label: "A" }]],
		["missing label", [{ value: "a" }]],
		["empty label", [{ value: "a", label: "   " }]],
		["null option", [null]],
	])("rejects select option with %s", async (_name, options) => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ type: "select", message: "Pick one", options } as any, ctx);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("validation");
		expect(String(result.content[0].text)).toContain("options[0]");
		expect(String(result.content[0].text)).toContain("value");
		expect(String(result.content[0].text)).toContain("label");
	});

	it("rejects select with recommended out of range", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute(
			{
				type: "select",
				message: "Pick one",
				options: [{ value: "a", label: "A" }],
				recommended: 5,
			},
			ctx,
		);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("validation");
		expect(String(result.content[0].text)).toContain("out of range");
	});

	it("accepts select with valid recommended index", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const uiCtx = mockUICtx("b");
		const result = await execute(
			{
				type: "select",
				message: "Pick one",
				options: [
					{ value: "a", label: "A" },
					{ value: "b", label: "B" },
				],
				recommended: 1,
			},
			uiCtx,
		);

		expect(result.details.error).not.toBe("validation");
	});

	it("rejects empty message", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ type: "confirm", message: "" }, ctx);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("validation");
		expect(String(result.content[0].text)).toContain("message");
	});

	it("rejects whitespace-only message", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ type: "confirm", message: "   " }, ctx);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("validation");
	});
});

describe("registerUserInputTool — non-terminating results", () => {
	it("never returns terminate: true on any error path", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const ctx = { ui: {} };

		const validationResult = await execute({ type: "select", message: "Pick", options: [] }, ctx);
		expect(validationResult.terminate).toBeUndefined();

		const nonTuiResult = await execute({ type: "confirm", message: "Ok?" });
		expect(nonTuiResult.terminate).toBeUndefined();

		const uiErrorResult = await execute(
			{ type: "confirm", message: "Ok?" },
			{ ui: { custom: () => Promise.reject(new Error("UI crashed")) } },
		);
		expect(uiErrorResult.terminate).toBeUndefined();
	});

	it("never returns terminate: true on success paths", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const ctx = mockUICtx("yes");
		const result = await execute({ type: "confirm", message: "Proceed?" }, ctx);
		expect(result.terminate).toBeUndefined();
	});
});

describe("registerUserInputTool — confirm interaction", () => {
	it("returns confirmed: true when user selects Yes", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const ctx = mockUICtx("yes");
		const result = await execute({ type: "confirm", message: "Deploy?" }, ctx);

		const parsed = JSON.parse(result.content[0].text);
		expect(parsed).toEqual({ confirmed: true });
		expect(result.details.type).toBe("confirm");
		expect(result.details.confirmed).toBe(true);
	});

	it("returns confirmed: false when user selects No", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const ctx = mockUICtx("no");
		const result = await execute({ type: "confirm", message: "Deploy?" }, ctx);

		const parsed = JSON.parse(result.content[0].text);
		expect(parsed).toEqual({ confirmed: false });
		expect(result.details.confirmed).toBe(false);
	});

	it("returns cancelled: true and terminates when user presses Escape", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const ctx = mockUICtx(null);
		const result = await execute({ type: "confirm", message: "Deploy?" }, ctx);

		const parsed = JSON.parse(result.content[0].text);
		expect(parsed).toEqual({ cancelled: true });
		expect(result.details.cancelled).toBe(true);
		expect(result.terminate).toBe(true);
	});
});

describe("registerUserInputTool — select interaction", () => {
	const selectParams: UserInputParams = {
		type: "select",
		message: "Which bump?",
		options: [
			{ value: "patch", label: "Patch", description: "Bug fixes" },
			{ value: "minor", label: "Minor", description: "Features" },
			{ value: "major", label: "Major", description: "Breaking" },
		],
	};

	it("returns selected value when user picks an option", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const ctx = mockUICtx("minor");
		const result = await execute(selectParams, ctx);

		const parsed = JSON.parse(result.content[0].text);
		expect(parsed).toEqual({ selected: "minor" });
		expect(result.details.type).toBe("select");
		expect(result.details.selected).toBe("minor");
		expect(result.details.options).toEqual(selectParams.options);
	});

	it("returns cancelled: true and terminates when user presses Escape", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const ctx = mockUICtx(null);
		const result = await execute(selectParams, ctx);

		const parsed = JSON.parse(result.content[0].text);
		expect(parsed).toEqual({ cancelled: true });
		expect(result.details.cancelled).toBe(true);
		expect(result.details.options).toEqual(selectParams.options);
		expect(result.terminate).toBe(true);
	});

	it("passes recommended index to the select widget", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const { ctx, getFactory } = mockUICtxWithFactory();

		const promise = execute({ ...selectParams, recommended: 1 }, ctx);

		// Allow the factory to be captured, then simulate selection
		await new Promise((r) => setTimeout(r, 0));
		const factory = getFactory();
		// The factory was created; we can't easily assert the internal selectedIndex
		// but we verify it doesn't throw and the widget renders
		expect(factory).toBeDefined();
		expect(factory.render(80)).toBeDefined();

		// Simulate a select to resolve the promise
		factory.handleInput("\r");
		const result = await promise;
		// With recommended=1, pressing enter on default selection gives "minor"
		const parsed = JSON.parse(result.content[0].text);
		expect(parsed.selected).toBe("minor");
	});
});

describe("registerUserInputTool — freetext interaction", () => {
	it.each(["running", "probing"] as const)("terminates and parks at waiting from %s", async (phase) => {
		const state = freshState({ lifecycle: lifecycleFor(phase) });
		const { execute } = toolFor(state);
		const result = await execute({ type: "freetext", message: "Release title?", placeholder: "v1.2.3" });

		const parsed = JSON.parse(result.content[0].text);
		expect(parsed).toEqual({ parked: true });
		expect(result.details).toEqual({ type: "freetext", parked: true });
		expect(result.terminate).toBe(true);
		expect(state.lifecycle.phase).toBe("waiting");
	});

	it("works when ctx.ui is absent", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ type: "freetext", message: "Release title?" }, {});

		expect(result.terminate).toBe(true);
		expect(result.details.error).toBeUndefined();
	});
});

describe("registerUserInputTool — cancellation phase handling", () => {
	it("transitions running to waiting on cancellation", async () => {
		const state = freshState({ lifecycle: lifecycleFor("running") });
		const { execute } = toolFor(state);
		const result = await execute({ type: "confirm", message: "Continue?" }, mockUICtx(null));

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.phase).toBe("waiting");
	});

	it("transitions probing to waiting on cancellation", async () => {
		const state = freshState({ lifecycle: lifecycleFor("probing") });
		const { execute } = toolFor(state);
		const result = await execute({ type: "confirm", message: "Continue?" }, mockUICtx(null));

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.phase).toBe("waiting");
	});

	it("journals user-input-parked on cancellation with active command", async () => {
		const state = freshState({ lifecycle: lifecycleFor("running", "mach12:test") });
		const { execute, pi } = toolFor(state);
		const result = await execute({ type: "confirm", message: "Continue?" }, mockUICtx(null));

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.phase).toBe("waiting");
		expect(pi.appended.filter((e: any) => e.customType === USER_INPUT_PARKED_TYPE)).toHaveLength(1);
		expect(pi.appended.find((e: any) => e.customType === USER_INPUT_PARKED_TYPE).data).toEqual({
			commandName: "mach12:test",
		});
	});
});

describe("registerUserInputTool — UI interaction errors", () => {
	it("returns a structured non-terminating error when UI fails during running phase", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const ctx = { ui: { custom: () => Promise.reject(new Error("UI crashed")) } };
		const result = await execute({ type: "confirm", message: "Continue?" }, ctx);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("ui-error");
		expect(result.details.message).toBe("UI crashed");
		expect(String(result.content[0].text)).toContain("UI interaction failed");
	});
});

describe("registerUserInputTool — probing phase compatibility", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("suspends probe watchdog before UI and resumes running after", async () => {
		const state = freshState({ lifecycle: lifecycleFor("probing") });
		const suspended: string[] = [];
		state.suspendProbeWatchdog = () => suspended.push("suspended");
		state.rearmProbeWatchdog = () => suspended.push("rearmed");
		const { execute } = toolFor(state);
		const ctx = mockUICtx("yes");
		await execute({ type: "confirm", message: "Continue?" }, ctx);

		expect(suspended).toEqual(["suspended"]);
		expect(state.lifecycle.phase).toBe("running");
	});

	it("keeps probing reportable and returns a structured error if UI throws", async () => {
		const state = freshState({ lifecycle: lifecycleFor("probing") });
		const suspended: string[] = [];
		state.suspendProbeWatchdog = () => suspended.push("suspended");
		state.rearmProbeWatchdog = () => suspended.push("rearmed");
		const { execute } = toolFor(state);
		const ctx = {
			ui: {
				custom: () => Promise.reject(new Error("UI crashed")),
				input: () => Promise.reject(new Error("UI crashed")),
			},
		};
		const result = await execute({ type: "confirm", message: "Continue?" }, ctx);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("ui-error");
		expect(result.details.message).toBe("UI crashed");
		expect(suspended).toEqual(["suspended", "rearmed"]);
		expect(state.lifecycle.phase).toBe("probing");
	});

	it("does not suspend watchdog when phase is running", async () => {
		const state = freshState({ lifecycle: lifecycleFor("running") });
		const suspended: string[] = [];
		state.suspendProbeWatchdog = () => suspended.push("suspended");
		state.rearmProbeWatchdog = () => suspended.push("rearmed");
		const { execute } = toolFor(state);
		const ctx = mockUICtx("yes");
		await execute({ type: "confirm", message: "Continue?" }, ctx);

		expect(suspended).toEqual([]);
	});

	it("does not terminate so command work can continue", async () => {
		const state = freshState({ lifecycle: lifecycleFor("probing") });
		const { execute } = toolFor(state);
		const ctx = mockUICtx("yes");
		const result = await execute({ type: "confirm", message: "Continue?" }, ctx);

		expect(result.terminate).toBeUndefined();
		expect(state.lifecycle.phase).toBe("running");
	});

	it("preserves continueCount when successful probe-time input resumes running", async () => {
		const state = freshState({ lifecycle: lifecycleFor("probing", "test:cmd", { continueCount: 2 }) });
		const { execute } = toolFor(state);
		const ctx = mockUICtx("yes");
		await execute({ type: "confirm", message: "Continue?" }, ctx);

		expect(state.lifecycle).toEqual({ phase: "running", command: "test:cmd", continueCount: 2 });
	});

	it("phase stays probing past an armed watchdog timeout while UI is pending", async () => {
		const { registerAutoContinue } = await import("../src/auto-continue.js");
		const state = freshState({
			lifecycle: lifecycleFor("running", "mach12:test"),
			registry: new Map([
				[
					"mach12:test",
					{ name: "mach12:test", filePath: "", body: "", next: { mode: "forced", target: "mach12:next" } },
				],
			]),
		});
		const { pi, emit } = recordingPi();
		registerAutoContinue(pi, state);

		await emit("agent_end", {}, { ui: { notify: () => {} } });
		await vi.advanceTimersByTimeAsync(0);
		expect(pi.sent).toHaveLength(1);

		let resolveUI: (v: string) => void;
		const uiPromise = new Promise<string>((r) => {
			resolveUI = r;
		});
		const { execute } = toolFor(state);
		const ctx = { ui: { custom: () => uiPromise, input: () => uiPromise } };

		const resultPromise = execute({ type: "confirm", message: "Continue?" }, ctx);

		await vi.advanceTimersByTimeAsync(35_000);
		expect(state.lifecycle.phase).toBe("probing");

		resolveUI!("yes");
		const result = await resultPromise;
		expect(result.terminate).toBeUndefined();
		expect(state.lifecycle.phase).toBe("running");
	});
});

describe("registerUserInputTool — journaling", () => {
	it("journals a confirm interaction", async () => {
		const { execute, pi } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const ctx = mockUICtx("yes");
		await execute({ type: "confirm", message: "Deploy?" }, ctx);

		const entry = pi.appended.find((e: any) => e.customType === USER_INPUT_TYPE);
		expect(entry).toBeDefined();
		expect(entry.data).toMatchObject({
			interactionType: "confirm",
			message: "Deploy?",
			type: "confirm",
			confirmed: true,
		});
	});

	it("journals a select interaction", async () => {
		const { execute, pi } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const ctx = mockUICtx("patch");
		await execute(
			{
				type: "select",
				message: "Bump level?",
				options: [{ value: "patch", label: "Patch" }],
			},
			ctx,
		);

		const entry = pi.appended.find((e: any) => e.customType === USER_INPUT_TYPE);
		expect(entry).toBeDefined();
		expect(entry.data).toMatchObject({
			interactionType: "select",
			message: "Bump level?",
			type: "select",
			selected: "patch",
			options: [{ value: "patch", label: "Patch" }],
		});
	});

	it("journals options for a cancelled select interaction", async () => {
		const { execute, pi } = toolFor(freshState({ lifecycle: lifecycleFor("running", "mach12:test") }));
		const ctx = mockUICtx(null);
		await execute(
			{
				type: "select",
				message: "Bump level?",
				options: [{ value: "patch", label: "Patch", description: "Bug fixes" }],
			},
			ctx,
		);

		const entry = pi.appended.find((e: any) => e.customType === USER_INPUT_TYPE);
		expect(entry).toBeDefined();
		expect(entry.data).toMatchObject({
			interactionType: "select",
			message: "Bump level?",
			type: "select",
			cancelled: true,
			options: [{ value: "patch", label: "Patch", description: "Bug fixes" }],
		});
	});

	it("journals a prompt-only freetext interaction and user-input-parked entry", async () => {
		const { execute, pi } = toolFor(freshState({ lifecycle: lifecycleFor("running", "mach12:test") }));
		await execute({ type: "freetext", message: "Title?" });

		const entry = pi.appended.find((e: any) => e.customType === USER_INPUT_TYPE);
		expect(entry).toBeDefined();
		expect(entry.data).toEqual({
			interactionType: "freetext",
			message: "Title?",
		});

		const parkedEntry = pi.appended.find((e: any) => e.customType === USER_INPUT_PARKED_TYPE);
		expect(parkedEntry).toBeDefined();
		expect(parkedEntry.data).toEqual({ commandName: "mach12:test" });
	});

	it("journals a cancelled interaction and user-input-parked entry", async () => {
		const { execute, pi } = toolFor(freshState({ lifecycle: lifecycleFor("running", "mach12:test") }));
		const ctx = mockUICtx(null);
		await execute({ type: "confirm", message: "Deploy?" }, ctx);

		const entry = pi.appended.find((e: any) => e.customType === USER_INPUT_TYPE);
		expect(entry).toBeDefined();
		expect(entry.data).toMatchObject({
			interactionType: "confirm",
			message: "Deploy?",
			cancelled: true,
		});

		const parkedEntry = pi.appended.find((e: any) => e.customType === USER_INPUT_PARKED_TYPE);
		expect(parkedEntry).toBeDefined();
		expect(parkedEntry.data).toEqual({ commandName: "mach12:test" });
	});

	it("does not journal when phase gate rejects", async () => {
		const { execute, pi } = toolFor(freshState({ lifecycle: lifecycleFor("idle") }));
		await execute({ type: "confirm", message: "Deploy?" }, { ui: {} });

		expect(pi.appended.filter((e: any) => e.customType === USER_INPUT_TYPE)).toHaveLength(0);
	});

	it("does not journal when validation rejects", async () => {
		const { execute, pi } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		await execute({ type: "select", message: "Pick", options: [] }, { ui: {} });

		expect(pi.appended.filter((e: any) => e.customType === USER_INPUT_TYPE)).toHaveLength(0);
	});
});
