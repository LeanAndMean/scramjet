import { initTheme, ToolExecutionComponent } from "@leanandmean/coding-agent";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
	COMMAND_START_TYPE,
	registerHistory,
	STRUCTURED_INPUT_CANCELLATION_TYPE,
	USER_INPUT_PARKED_TYPE,
} from "../src/history.js";
import { isDormant, isParkedForInput, isProbeDue, isProbeInFlight } from "../src/lifecycle.js";
import { registerUserInputTool, USER_INPUT_TYPE } from "../src/user-input.js";
import { freshState, lifecycleFor, recordingPi } from "./helpers.js";

initTheme(undefined, false);

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
			{ executionStarted: false, isPartial: false },
		);

		expect(renderText(component)).toContain("What release title should I use?");
	});

	it("hides message from renderCall after result arrives", () => {
		const { tool } = toolFor();
		const component = tool.renderCall({ type: "confirm", message: "Deploy now?" }, renderTheme, {
			executionStarted: true,
			isPartial: false,
		});

		const output = renderText(component);
		expect(output).toContain("get_scramjet_user_input");
		expect(output).not.toContain("Deploy now?");
	});

	it("shows message in renderCall during execution", () => {
		const { tool } = toolFor();
		const component = tool.renderCall({ type: "confirm", message: "Deploy now?" }, renderTheme, {
			executionStarted: true,
			isPartial: true,
		});

		const output = renderText(component);
		expect(output).toContain("get_scramjet_user_input");
		expect(output).toContain("Deploy now?");
	});

	it("falls back to the tool name while args stream without a message", () => {
		const { tool } = toolFor();
		const component = tool.renderCall({}, renderTheme, {});

		expect(renderText(component)).toContain("get_scramjet_user_input");
	});

	it("renders a structured prompt summary in Pi's tool row after execution", () => {
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
		component.markExecutionStarted();
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
		const occurrences = output.split("Which bump level?").length - 1;
		expect(occurrences).toBe(1);
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

describe("registerUserInputTool — gate", () => {
	it.each(["idle", "dormant", "waiting"] as const)("accepts calls when lifecycle is %s", async (phase) => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor(phase) }));
		const ctx = mockUICtx("yes");
		const result = await execute({ type: "confirm", message: "Proceed?" }, ctx);

		expect(result.details.error).toBeUndefined();
		expect(result.details.confirmed).toBe(true);
	});

	it("rejects with report-pending error when lifecycle is reported", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("reported") }));
		const ctx = mockUICtx("yes");
		const result = await execute({ type: "confirm", message: "Proceed?" }, ctx);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("report-pending");
		expect(String(result.content[0].text)).toContain("pending dispatch");
	});

	it("reported guard does not clear lastReport", async () => {
		const state = freshState({ lifecycle: lifecycleFor("reported") });
		const { execute } = toolFor(state);
		const ctx = mockUICtx("yes");
		await execute({ type: "confirm", message: "Proceed?" }, ctx);

		expect(state.lifecycle.lastReport).not.toBeNull();
	});

	it("accepts calls when probe is armed (running)", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const ctx = mockUICtx("yes");
		const result = await execute({ type: "confirm", message: "Proceed?" }, ctx);

		expect(result.details.error).toBeUndefined();
	});

	it("accepts calls when probe is in flight (probing)", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));
		const ctx = mockUICtx("yes");
		const result = await execute({ type: "confirm", message: "Proceed?" }, ctx);

		expect(result.details.error).toBeUndefined();
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

	it("returns non-TUI error from idle phase", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("idle") }));
		const result = await execute({ type: "confirm", message: "Proceed?" }, {});

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

	it("widget render does not contain the prompt message", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const { ctx, getFactory } = mockUICtxWithFactory();

		const promise = execute({ type: "confirm", message: "Deploy?" }, ctx);

		await new Promise((r) => setTimeout(r, 0));
		const factory = getFactory();
		const rendered = factory.render(80).join("\n");
		expect(rendered).not.toContain("Deploy?");
		expect(rendered).toContain("Yes");
		expect(rendered).toContain("cancel");

		factory.handleInput("\r");
		await promise;
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

		await new Promise((r) => setTimeout(r, 0));
		const factory = getFactory();
		expect(factory).toBeDefined();
		expect(factory.render(80)).toBeDefined();

		factory.handleInput("\r");
		const result = await promise;
		const parsed = JSON.parse(result.content[0].text);
		expect(parsed.selected).toBe("minor");
	});

	it("widget render does not contain the prompt message", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const { ctx, getFactory } = mockUICtxWithFactory();

		const promise = execute({ ...selectParams }, ctx);

		await new Promise((r) => setTimeout(r, 0));
		const factory = getFactory();
		const rendered = factory.render(80).join("\n");
		expect(rendered).not.toContain("Which bump?");
		expect(rendered).toContain("Patch");
		expect(rendered).toContain("navigate");

		factory.handleInput("\r");
		await promise;
	});
});

describe("registerUserInputTool — freetext interaction", () => {
	it.each(["running", "probing"] as const)("terminates and parks from %s", async (phase) => {
		const state = freshState({ lifecycle: lifecycleFor(phase) });
		const { execute } = toolFor(state);
		const result = await execute({ type: "freetext", message: "Release title?", placeholder: "v1.2.3" });

		const parsed = JSON.parse(result.content[0].text);
		expect(parsed).toEqual({ parked: true });
		expect(result.details).toEqual({ type: "freetext", parked: true });
		expect(result.terminate).toBe(true);
		expect(isParkedForInput(state.lifecycle)).toBe(true);
	});

	it("works when ctx.ui is absent", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ type: "freetext", message: "Release title?" }, {});

		expect(result.terminate).toBe(true);
		expect(result.details.error).toBeUndefined();
	});
});

describe("registerUserInputTool — idle phase behavior", () => {
	it("freetext when idle: returns parked:false, lifecycle unmutated, no parked entry", async () => {
		const state = freshState({ lifecycle: lifecycleFor("idle") });
		const { execute, pi } = toolFor(state);
		const result = await execute({ type: "freetext", message: "Title?" });

		const parsed = JSON.parse(result.content[0].text);
		expect(parsed).toEqual({ parked: false });
		expect(result.terminate).toBe(true);
		expect(state.lifecycle.activeCommand).toBeNull();
		expect(state.lifecycle.probeArmed).toBe(false);
		expect(state.lifecycle.parkedForInput).toBe(false);
		expect(state.freetextAwaitingReply).toBe(false);
		expect(pi.appended.filter((e: any) => e.customType === USER_INPUT_PARKED_TYPE)).toHaveLength(0);
	});

	it("freetext when idle: journals USER_INPUT_TYPE", async () => {
		const { execute, pi } = toolFor(freshState({ lifecycle: lifecycleFor("idle") }));
		await execute({ type: "freetext", message: "Title?" });

		const entry = pi.appended.find((e: any) => e.customType === USER_INPUT_TYPE);
		expect(entry).toBeDefined();
		expect(entry.data).toMatchObject({ interactionType: "freetext", message: "Title?" });
	});

	it("confirm success when idle: returns answer, no terminate, no lifecycle mutation", async () => {
		const state = freshState({ lifecycle: lifecycleFor("idle") });
		const { execute } = toolFor(state);
		const result = await execute({ type: "confirm", message: "Proceed?" }, mockUICtx("yes"));

		const parsed = JSON.parse(result.content[0].text);
		expect(parsed).toEqual({ confirmed: true });
		expect(result.terminate).toBeUndefined();
		expect(state.lifecycle.activeCommand).toBeNull();
		expect(state.lifecycle.probeArmed).toBe(false);
	});

	it("confirm cancel when idle: terminate true, lifecycle stays idle", async () => {
		const state = freshState({ lifecycle: lifecycleFor("idle") });
		const { execute } = toolFor(state);
		const result = await execute({ type: "confirm", message: "Proceed?" }, mockUICtx(null));

		const parsed = JSON.parse(result.content[0].text);
		expect(parsed).toEqual({ cancelled: true });
		expect(result.terminate).toBe(true);
		expect(state.lifecycle.activeCommand).toBeNull();
		expect(state.lifecycle.probeArmed).toBe(false);
	});

	it("select cancel when idle: terminate true, lifecycle stays idle", async () => {
		const state = freshState({ lifecycle: lifecycleFor("idle") });
		const { execute } = toolFor(state);
		const result = await execute(
			{ type: "select", message: "Pick", options: [{ value: "a", label: "A" }] },
			mockUICtx(null),
		);

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.activeCommand).toBeNull();
		expect(state.lifecycle.probeArmed).toBe(false);
	});
});

describe("registerUserInputTool — dormant phase behavior", () => {
	it("confirm cancel when dormant: transitions to dormant idempotently", async () => {
		const state = freshState({ lifecycle: lifecycleFor("dormant", "mach12:test") });
		const { execute } = toolFor(state);
		const result = await execute({ type: "confirm", message: "Proceed?" }, mockUICtx(null));

		expect(result.terminate).toBe(true);
		expect(isDormant(state.lifecycle)).toBe(true);
		expect(state.lifecycle.activeCommand).toBe("mach12:test");
	});

	it("select cancel when dormant: transitions to dormant idempotently", async () => {
		const state = freshState({ lifecycle: lifecycleFor("dormant", "mach12:test") });
		const { execute } = toolFor(state);
		const result = await execute(
			{ type: "select", message: "Pick", options: [{ value: "a", label: "A" }] },
			mockUICtx(null),
		);

		expect(result.terminate).toBe(true);
		expect(isDormant(state.lifecycle)).toBe(true);
		expect(state.lifecycle.activeCommand).toBe("mach12:test");
	});

	it("freetext when dormant: parks command and journals parked entry", async () => {
		const state = freshState({ lifecycle: lifecycleFor("dormant", "mach12:test") });
		const { execute, pi } = toolFor(state);
		const result = await execute({ type: "freetext", message: "Release title?" });

		const parsed = JSON.parse(result.content[0].text);
		expect(parsed).toEqual({ parked: true });
		expect(result.terminate).toBe(true);
		expect(isParkedForInput(state.lifecycle)).toBe(true);
		expect(state.lifecycle.activeCommand).toBe("mach12:test");
		expect(pi.appended.filter((e: any) => e.customType === USER_INPUT_PARKED_TYPE)).toHaveLength(1);
	});
});

describe("registerUserInputTool — cancellation behavior", () => {
	it("transitions running to dormant on cancellation (not waiting)", async () => {
		const state = freshState({ lifecycle: lifecycleFor("running", "mach12:test") });
		const { execute } = toolFor(state);
		const result = await execute({ type: "confirm", message: "Continue?" }, mockUICtx(null));

		expect(result.terminate).toBe(true);
		expect(isDormant(state.lifecycle)).toBe(true);
		expect(state.lifecycle.activeCommand).toBe("mach12:test");
		expect(state.lifecycle.cancellationResumeEligible).toBe(true);
	});

	it("transitions probing to dormant on cancellation", async () => {
		const state = freshState({ lifecycle: lifecycleFor("probing", "mach12:test") });
		const { execute } = toolFor(state);
		const result = await execute({ type: "confirm", message: "Continue?" }, mockUICtx(null));

		expect(result.terminate).toBe(true);
		expect(isDormant(state.lifecycle)).toBe(true);
		expect(state.lifecycle.activeCommand).toBe("mach12:test");
		expect(state.lifecycle.cancellationResumeEligible).toBe(true);
	});

	it("does NOT journal user-input-parked on cancellation", async () => {
		const state = freshState({ lifecycle: lifecycleFor("running", "mach12:test") });
		const { execute, pi } = toolFor(state);
		await execute({ type: "confirm", message: "Continue?" }, mockUICtx(null));

		expect(pi.appended.filter((e: any) => e.customType === USER_INPUT_PARKED_TYPE)).toHaveLength(0);
	});

	it("journals user-input entry on cancellation", async () => {
		const state = freshState({ lifecycle: lifecycleFor("running", "mach12:test") });
		const { execute, pi } = toolFor(state);
		await execute({ type: "confirm", message: "Continue?" }, mockUICtx(null));

		expect(pi.appended.filter((e: any) => e.customType === USER_INPUT_TYPE)).toHaveLength(1);
		expect(pi.appended.filter((e: any) => e.customType === STRUCTURED_INPUT_CANCELLATION_TYPE)).toEqual([
			{
				customType: STRUCTURED_INPUT_CANCELLATION_TYPE,
				data: { commandName: "mach12:test", resumable: true },
			},
		]);
	});

	it("falls back to generic dormant when cancellation grant persistence fails", async () => {
		const logger = { warn: vi.fn(), debug: vi.fn(), lifecycle: vi.fn() };
		const state = freshState({ lifecycle: lifecycleFor("running", "mach12:test"), logger: logger as any });
		const { execute, pi } = toolFor(state);
		const appendEntry = pi.appendEntry;
		pi.appendEntry = (type: string, data: unknown) => {
			if (type === STRUCTURED_INPUT_CANCELLATION_TYPE) throw new Error("disk full");
			appendEntry(type, data);
		};

		const result = await execute({ type: "confirm", message: "Continue?" }, mockUICtx(null));

		expect(result.terminate).toBe(true);
		expect(isDormant(state.lifecycle)).toBe(true);
		expect(state.lifecycle.cancellationResumeEligible).toBe(false);
		expect(logger.warn).toHaveBeenCalledWith(
			"input",
			"failed to persist structured input cancellation; resumability disabled",
			expect.objectContaining({ command: "mach12:test", error: "disk full" }),
		);
	});

	it.each([
		["confirm", { type: "confirm", message: "Continue?" }, null],
		[
			"select",
			{
				type: "select",
				message: "Pick one",
				options: [{ value: "a", label: "A" }],
			},
			"a",
		],
	] as const)("ignores stale %s results after the lifecycle changes", async (_type, params, uiResult) => {
		let resolveInput: (result: unknown) => void = () => {};
		const logger = { warn: vi.fn(), debug: vi.fn(), lifecycle: vi.fn() };
		const state = freshState({
			lifecycle: lifecycleFor("running", "mach12:test"),
			logger: logger as any,
		});
		const { execute, pi } = toolFor(state);
		const promise = execute(params, {
			ui: {
				custom: () =>
					new Promise((resolve) => {
						resolveInput = resolve;
					}),
			},
		});
		await Promise.resolve();

		state.lifecycle = lifecycleFor("running", "mach12:other");
		state.lifecycleGeneration++;
		resolveInput(uiResult);
		const result = await promise;

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("stale-result");
		expect(state.lifecycle.activeCommand).toBe("mach12:other");
		expect(isProbeDue(state.lifecycle)).toBe(true);
		expect(pi.appended.filter((e: any) => e.customType === USER_INPUT_TYPE)).toHaveLength(0);
		expect(logger.warn).toHaveBeenCalledWith(
			"input",
			"stale get_scramjet_user_input result ignored",
			expect.objectContaining({ expectedCommand: "mach12:test", currentCommand: "mach12:other" }),
		);
	});

	it.each(["session_start", "session_tree"])(
		"ignores a pending confirm after same-name %s reconstruction",
		async (eventName) => {
			let resolveInput: (result: unknown) => void = () => {};
			const state = freshState({ lifecycle: lifecycleFor("running", "mach12:test") });
			const { pi, handlers, execute } = toolFor(state);
			registerHistory(pi, state);
			const promise = execute(
				{ type: "confirm", message: "Continue?" },
				{
					ui: {
						custom: () =>
							new Promise((resolve) => {
								resolveInput = resolve;
							}),
					},
				},
			);
			await Promise.resolve();

			const branch = [
				{
					type: "custom",
					customType: COMMAND_START_TYPE,
					data: { command: "mach12:test", origin: "user", depth: 0, timestamp: 1 },
				},
			];
			for (const handler of handlers.get(eventName) ?? []) {
				await handler({}, { sessionManager: { getBranch: () => branch } });
			}
			resolveInput("yes");
			const result = await promise;

			expect(result.details.error).toBe("stale-result");
			expect(isDormant(state.lifecycle)).toBe(true);
			expect(state.lifecycle.activeCommand).toBe("mach12:test");
		},
	);

	it.each(["session_start", "session_tree"])(
		"ignores a pending select after same-name %s reconstruction",
		async (eventName) => {
			let resolveInput: (result: unknown) => void = () => {};
			const state = freshState({ lifecycle: lifecycleFor("running", "mach12:test") });
			const { pi, handlers, execute } = toolFor(state);
			registerHistory(pi, state);
			const promise = execute(
				{ type: "select", message: "Pick", options: [{ value: "a", label: "A" }] },
				{
					ui: {
						custom: () =>
							new Promise((resolve) => {
								resolveInput = resolve;
							}),
					},
				},
			);
			await Promise.resolve();

			const branch = [
				{
					type: "custom",
					customType: COMMAND_START_TYPE,
					data: { command: "mach12:test", origin: "user", depth: 0, timestamp: 1 },
				},
			];
			for (const handler of handlers.get(eventName) ?? []) {
				await handler({}, { sessionManager: { getBranch: () => branch } });
			}
			resolveInput("a");
			const result = await promise;

			expect(result.details.error).toBe("stale-result");
			expect(isDormant(state.lifecycle)).toBe(true);
			expect(state.lifecycle.activeCommand).toBe("mach12:test");
		},
	);

	it("ignores stale probing results without resuming the replacement probe", async () => {
		let resolveInput: (result: unknown) => void = () => {};
		const logger = { warn: vi.fn(), debug: vi.fn(), lifecycle: vi.fn() };
		const state = freshState({
			lifecycle: lifecycleFor("probing", "mach12:test"),
			logger: logger as any,
			suspendProbeWatchdog: vi.fn(),
			rearmProbeWatchdog: vi.fn(),
		});
		const { execute, pi } = toolFor(state);
		const promise = execute(
			{ type: "confirm", message: "Continue?" },
			{
				ui: {
					custom: () =>
						new Promise((resolve) => {
							resolveInput = resolve;
						}),
				},
			},
		);
		await Promise.resolve();

		state.lifecycle = lifecycleFor("probing", "mach12:other");
		state.lifecycleGeneration++;
		resolveInput("yes");
		const result = await promise;

		expect(result.details.error).toBe("stale-result");
		expect(state.lifecycle.activeCommand).toBe("mach12:other");
		expect(isProbeInFlight(state.lifecycle)).toBe(true);
		expect(state.suspendProbeWatchdog).toHaveBeenCalledTimes(1);
		expect(state.rearmProbeWatchdog).not.toHaveBeenCalled();
		expect(pi.appended.filter((e: any) => e.customType === USER_INPUT_TYPE)).toHaveLength(0);
	});
});

describe("registerUserInputTool — UI interaction errors", () => {
	it("returns a structured non-terminating error when UI fails during running", async () => {
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
		expect(isProbeDue(state.lifecycle)).toBe(true);
	});

	it("keeps probe in flight and returns a structured error if UI throws", async () => {
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
		expect(isProbeInFlight(state.lifecycle)).toBe(true);
	});

	it("does not suspend watchdog when probe is armed (running)", async () => {
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
		expect(isProbeDue(state.lifecycle)).toBe(true);
	});

	it("preserves continueCount when successful probe-time input resumes", async () => {
		const state = freshState({ lifecycle: lifecycleFor("probing", "test:cmd", { continueCount: 2 }) });
		const { execute } = toolFor(state);
		const ctx = mockUICtx("yes");
		await execute({ type: "confirm", message: "Continue?" }, ctx);

		expect(state.lifecycle.probeArmed).toBe(true);
		expect(state.lifecycle.continueCount).toBe(2);
		expect(state.lifecycle.activeCommand).toBe("test:cmd");
	});

	it("lifecycle stays probing past an armed watchdog timeout while UI is pending", async () => {
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
		expect(isProbeInFlight(state.lifecycle)).toBe(true);

		resolveUI!("yes");
		const result = await resultPromise;
		expect(result.terminate).toBeUndefined();
		expect(isProbeDue(state.lifecycle)).toBe(true);
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
		expect(parkedEntry.data).toEqual({ commandName: "mach12:test", parked: true });
	});

	it("does not journal parked marker on cancellation (new behavior)", async () => {
		const { execute, pi } = toolFor(freshState({ lifecycle: lifecycleFor("running", "mach12:test") }));
		const ctx = mockUICtx(null);
		await execute({ type: "confirm", message: "Deploy?" }, ctx);

		const parkedEntries = pi.appended.filter((e: any) => e.customType === USER_INPUT_PARKED_TYPE);
		expect(parkedEntries).toHaveLength(0);
	});

	it("does not journal when report-pending guard rejects", async () => {
		const { execute, pi } = toolFor(freshState({ lifecycle: lifecycleFor("reported") }));
		await execute({ type: "confirm", message: "Deploy?" }, mockUICtx("yes"));

		expect(pi.appended.filter((e: any) => e.customType === USER_INPUT_TYPE)).toHaveLength(0);
	});

	it("does not journal when validation rejects", async () => {
		const { execute, pi } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		await execute({ type: "select", message: "Pick", options: [] }, { ui: {} });

		expect(pi.appended.filter((e: any) => e.customType === USER_INPUT_TYPE)).toHaveLength(0);
	});
});

describe("registerUserInputTool — rebuild path deduplication (issue 257)", () => {
	function buildFreetextComponent(tool: any) {
		return new ToolExecutionComponent(
			"get_scramjet_user_input",
			"call-rebuild",
			{ type: "freetext", message: "What release title should I use?" },
			undefined,
			tool,
			{ requestRender: () => {} } as any,
			process.cwd(),
		);
	}

	const freetextResult = {
		content: [{ type: "text" as const, text: JSON.stringify({ parked: true }) }],
		details: { type: "freetext", parked: true },
		isError: false,
	};

	it("rebuild with markExecutionStarted + setArgsComplete: message appears exactly once", () => {
		const { tool } = toolFor();
		const component = buildFreetextComponent(tool);

		component.markExecutionStarted();
		component.setArgsComplete();
		component.updateResult(freetextResult, false);

		const output = visibleText(component.render(120).join("\n"));
		const occurrences = output.split("What release title should I use?").length - 1;
		expect(occurrences).toBe(1);
		expect(output).toContain("Parked for reply");
	});

	it("rebuild without markExecutionStarted: message appears twice (documents pre-fix bug)", () => {
		const { tool } = toolFor();
		const component = buildFreetextComponent(tool);

		// Simulate the old rebuild path: only updateResult, no markExecutionStarted/setArgsComplete
		component.updateResult(freetextResult, false);

		const output = visibleText(component.render(120).join("\n"));
		const occurrences = output.split("What release title should I use?").length - 1;
		expect(occurrences).toBe(2);
	});
});
