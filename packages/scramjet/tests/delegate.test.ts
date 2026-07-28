import { initTheme, keyText, ToolExecutionComponent } from "@leanandmean/coding-agent";
import { getKeybindings, KeybindingsManager, setKeybindings } from "@leanandmean/tui";
import { describe, expect, it } from "vitest";
import { parseDelegateArgs, substituteArguments } from "../src/commands/substitute.js";
import { DELEGATE_TOOL_NAME, detectCycle, intersectTools, registerDelegateTool } from "../src/delegate.js";
import { COMMAND_START_TYPE } from "../src/history.js";
import { activeCommandName } from "../src/lifecycle.js";
import type { CommandDef, DelegateFrame, ScramjetState, SidebarEntry } from "../src/types.js";
import { derivedPhase, freshState, lifecycleFor, recordingPi } from "./helpers.js";

initTheme(undefined, false);

const renderTheme = {
	fg: (_color: string, text: string) => text,
	bold: (text: string) => text,
};

function renderText(component: { render(width: number): string[] }): string {
	return component
		.render(120)
		.map((line) => line.trimEnd())
		.join("\n")
		.replace(/\x1b\[[0-9;]*m/g, "");
}

function delegateTool() {
	const { pi, tools } = recordingPi();
	registerDelegateTool(pi, freshState());
	return tools[0];
}

function successResult(scope?: string[]) {
	const details: Record<string, unknown> = { command: "mach12:push", depth: 1 };
	if (scope !== undefined) details.effectiveAllowedTools = scope;
	return {
		content: [{ type: "text", text: "BODY_START\ndelegated instructions\nBODY_END" }],
		details,
	};
}

function def(name: string, body: string, allowedTools?: string[], delegateOnly = true): CommandDef {
	const d: CommandDef = { name, filePath: `/fake/${name}.md`, body };
	if (allowedTools !== undefined) d.allowedTools = allowedTools;
	if (delegateOnly) d.delegateOnly = true;
	return d;
}

describe("parseDelegateArgs — bash-style splitting", () => {
	it("returns an empty array for an empty string", () => {
		expect(parseDelegateArgs("")).toEqual([]);
	});

	it("splits on spaces and tabs", () => {
		expect(parseDelegateArgs("a b c")).toEqual(["a", "b", "c"]);
		expect(parseDelegateArgs("a\tb\tc")).toEqual(["a", "b", "c"]);
	});

	it("collapses runs of whitespace", () => {
		expect(parseDelegateArgs("a   b\t\tc")).toEqual(["a", "b", "c"]);
	});

	it("respects double-quote grouping", () => {
		expect(parseDelegateArgs('a "b c d" e')).toEqual(["a", "b c d", "e"]);
	});

	it("respects single-quote grouping", () => {
		expect(parseDelegateArgs("a 'b c d' e")).toEqual(["a", "b c d", "e"]);
	});

	it("treats unclosed quotes as continuing to end-of-string", () => {
		expect(parseDelegateArgs('a "b c')).toEqual(["a", "b c"]);
	});
});

describe("substituteArguments — placeholder expansion", () => {
	it("returns the body unchanged when no placeholders are present", () => {
		expect(substituteArguments("plain text", ["a"])).toBe("plain text");
	});

	it("replaces $ARGUMENTS with the joined args", () => {
		expect(substituteArguments("Run: $ARGUMENTS", ["a", "b", "c"])).toBe("Run: a b c");
	});

	it("replaces $@ with the joined args", () => {
		expect(substituteArguments("Run: $@", ["a", "b"])).toBe("Run: a b");
	});

	it("substitutes positional $1, $2, ... by 1-indexed position", () => {
		expect(substituteArguments("first=$1 second=$2 third=$3", ["x", "y", "z"])).toBe("first=x second=y third=z");
	});

	it("replaces missing positional args with empty string", () => {
		expect(substituteArguments("$1-$2-$3", ["only"])).toBe("only--");
	});

	it("substitutes positional BEFORE wildcards so $-digit in arg values is preserved", () => {
		// $1 = "$100" must not re-trigger $1-substitution.
		expect(substituteArguments("price=$1 all=$@", ["$100", "USD"])).toBe("price=$100 all=$100 USD");
	});

	it("supports {@:N} bash-style slicing from N onwards", () => {
		// biome-ignore lint/suspicious/noTemplateCurlyInString: bash slicing syntax, not a JS template literal
		expect(substituteArguments("tail=${@:2}", ["a", "b", "c", "d"])).toBe("tail=b c d");
	});

	it("supports {@:N:L} bash-style slicing for L items from N", () => {
		// biome-ignore lint/suspicious/noTemplateCurlyInString: bash slicing syntax, not a JS template literal
		expect(substituteArguments("slice=${@:2:2}", ["a", "b", "c", "d"])).toBe("slice=b c");
	});

	it("treats {@:0} as starting from the first arg (bash convention)", () => {
		// biome-ignore lint/suspicious/noTemplateCurlyInString: bash slicing syntax, not a JS template literal
		expect(substituteArguments("all=${@:0}", ["a", "b"])).toBe("all=a b");
	});

	it("handles repeated placeholders", () => {
		expect(substituteArguments("$1 then $1 again, $@ overall", ["x", "y"])).toBe("x then x again, x y overall");
	});

	it("substitutes $ARGUMENTS to empty string when no args given", () => {
		expect(substituteArguments("Run: $ARGUMENTS", [])).toBe("Run: ");
	});
});

describe("detectCycle — call-stack membership", () => {
	it("returns false for an empty stack", () => {
		expect(detectCycle([], "mach12:push")).toBe(false);
	});

	it("returns false when the name is absent", () => {
		const stack: DelegateFrame[] = [{ commandName: "mach12:push", depth: 0 }];
		expect(detectCycle(stack, "mach12:other")).toBe(false);
	});

	it("returns true when the name is present at any depth", () => {
		const stack: DelegateFrame[] = [
			{ commandName: "mach12:a", depth: 0 },
			{ commandName: "mach12:b", depth: 1 },
		];
		expect(detectCycle(stack, "mach12:a")).toBe(true);
		expect(detectCycle(stack, "mach12:b")).toBe(true);
	});
});

describe("intersectTools — caller vs callee semantics", () => {
	it("returns undefined when both sides are unrestricted", () => {
		expect(intersectTools(undefined, undefined)).toBeUndefined();
	});

	it("returns the callee's set when the caller is unrestricted", () => {
		expect(intersectTools(undefined, ["Read", "Bash"])).toEqual(["Read", "Bash"]);
	});

	it("returns the caller's set when the callee is unrestricted", () => {
		expect(intersectTools(["Read", "Bash"], undefined)).toEqual(["Read", "Bash"]);
	});

	it("returns the intersection when both sides restrict", () => {
		expect(intersectTools(["Read", "Bash", "Edit"], ["Bash", "Edit", "Write"])).toEqual(["Bash", "Edit"]);
	});

	it("returns an empty array when the intersection is empty (distinct from undefined)", () => {
		expect(intersectTools(["Read"], ["Bash"])).toEqual([]);
	});

	it("preserves callee order in the intersection", () => {
		expect(intersectTools(["Bash", "Read", "Edit"], ["Edit", "Read"])).toEqual(["Edit", "Read"]);
	});
});

describe("registerDelegateTool — registration shape", () => {
	it("registers exactly one selected tool with explicit same-context guidance", () => {
		const { pi, tools, handlers } = recordingPi();
		registerDelegateTool(pi, freshState());
		expect(DELEGATE_TOOL_NAME).toBe("delegate");
		expect(tools).toHaveLength(1);
		expect(tools[0].name).toBe(DELEGATE_TOOL_NAME);
		expect(tools[0].description).toMatch(/current agent.*execute immediately.*same conversation/i);
		expect(tools[0].description).toMatch(/not.*separate-agent.*top-level.*completion routing.*future suggestions/i);
		expect(tools[0].promptSnippet).toMatch(/current agent.*execute now.*same conversation/i);
		expect(tools[0].promptGuidelines).toHaveLength(1);
		expect(tools[0].promptGuidelines[0]).toMatch(/delegate-only.*execute now yourself/i);
		expect(handlers.has("before_agent_start")).toBe(true);
	});
});

describe("registerDelegateTool — compact rendering", () => {
	it("renders a compact command call with normalized, bounded arguments and the configured expansion key", () => {
		const originalKeybindings = getKeybindings();
		setKeybindings(
			new KeybindingsManager(
				{ "app.tools.expand": { defaultKeys: "ctrl+o", description: "Toggle tool output" } },
				{ "app.tools.expand": "alt+e" },
			),
		);
		try {
			const tool = delegateTool();
			const output = renderText(
				tool.renderCall({ command: "mach12:push", args: `first\n${"x".repeat(100)}` }, renderTheme, {
					argsComplete: true,
				}),
			);

			expect(output).toContain("/mach12:push first x");
			expect(output).toContain(`${keyText("app.tools.expand")} to toggle details`);
			expect(output).not.toContain("ctrl+o");
			expect(output).toContain("…");
			expect(output).not.toContain("\n");
			expect(output.length).toBeLessThan(120);
		} finally {
			setKeybindings(originalKeybindings);
		}
	});

	it.each([{}, { command: 42 }, { command: "mach12:push", args: 42 }])(
		"does not throw for partial or malformed call arguments %#",
		(args) => {
			const tool = delegateTool();
			expect(() => tool.renderCall(args, renderTheme, { argsComplete: false })).not.toThrow();
		},
	);

	it("hides a recognized successful body while collapsed and shows it in full while expanded", () => {
		const tool = delegateTool();
		const collapsed = renderText(
			tool.renderResult(successResult(), { expanded: false, isPartial: false }, renderTheme, { isError: false }),
		);
		const expanded = renderText(
			tool.renderResult(successResult(), { expanded: true, isPartial: false }, renderTheme, { isError: false }),
		);

		expect(collapsed).not.toContain("BODY_START");
		expect(expanded).toContain("BODY_START");
		expect(expanded).toContain("BODY_END");
	});

	it("keeps an empty-scope warning visible while hiding the body when collapsed", () => {
		const tool = delegateTool();
		const collapsed = renderText(
			tool.renderResult(successResult([]), { expanded: false, isPartial: false }, renderTheme, { isError: false }),
		);

		expect(collapsed).toContain("allowed-tools scopes do not overlap");
		expect(collapsed).toContain("advisory violations");
		expect(collapsed).toContain("Widen the caller");
		expect(collapsed).toContain("scope or abort delegation");
		expect(collapsed).not.toContain("BODY_START");
	});

	it.each([
		["unknown command", { ...successResult(), details: { command: "missing", error: "unknown_command" } }, false],
		["cycle", { ...successResult(), details: { command: "a", error: "cycle" } }, false],
		["runtime error", successResult(), true],
		["partial result", successResult(), false, true],
		["missing details", { ...successResult(), details: undefined }, false],
		["missing depth", { ...successResult(), details: { command: "a" } }, false],
		["invalid depth", { ...successResult(), details: { command: "a", depth: 0 } }, false],
		["error-only chain", { ...successResult(), details: { command: "a", depth: 1, chain: "a -> b" } }, false],
		["unknown metadata", { ...successResult(), details: { command: "a", depth: 1, diagnostic: true } }, false],
		["invalid scope", { ...successResult(), details: { command: "a", depth: 1, effectiveAllowedTools: [1] } }, false],
		[
			"multiple content",
			{ ...successResult(), content: [...successResult().content, { type: "text", text: "more" }] },
			false,
		],
	])("fails open for %s", (_name, result, isError, isPartial = false) => {
		const tool = delegateTool();
		const output = renderText(tool.renderResult(result, { expanded: false, isPartial }, renderTheme, { isError }));
		const expected = result.content.filter((entry: any) => entry.type === "text").map((entry: any) => entry.text);
		for (const text of expected) expect(output).toContain(text);
	});

	it.each([
		[{ type: "image", data: "x", mimeType: "image/png" }],
		[{ type: "text", text: "" }],
		[{ type: "text", text: "   " }],
	])("shows an explicit warning when content has no visible text", (content) => {
		const tool = delegateTool();
		const result = { ...successResult(), content };
		const output = renderText(
			tool.renderResult(result, { expanded: false, isPartial: false }, renderTheme, { isError: false }),
		);

		expect(output).toContain("WARNING: delegate result contains unsupported or malformed content.");
	});

	it("toggles a completed live row and restores replay results after global expansion state", () => {
		const tool = delegateTool();
		const makeComponent = (expandedBeforeResult = false) => {
			const component = new ToolExecutionComponent(
				"delegate",
				"call-id",
				{ command: "mach12:push", args: "386 stage 1" },
				undefined,
				tool,
				{ requestRender: () => {} } as any,
				process.cwd(),
			);
			component.markExecutionStarted();
			component.setArgsComplete();
			if (expandedBeforeResult) component.setExpanded(true);
			component.updateResult({ ...successResult(), isError: false }, false);
			return component;
		};

		const live = makeComponent();
		expect(renderText(live)).not.toContain("BODY_START");
		live.setExpanded(true);
		expect(renderText(live)).toContain("BODY_START");
		live.setExpanded(false);
		expect(renderText(live)).not.toContain("BODY_START");
		expect(renderText(makeComponent(true))).toContain("BODY_START");
	});
});

describe("registerDelegateTool — execute paths", () => {
	function setupWithRegistry(entries: CommandDef[]): {
		state: ScramjetState;
		execute: (params: { command: string; args: string }) => Promise<any>;
	} {
		const caller = def("test:caller", "caller", undefined, false);
		const allEntries = [caller, ...entries];
		const state = freshState({
			registry: new Map(allEntries.map((d) => [d.name, d])),
			lifecycle: lifecycleFor("dormant", caller.name),
		});
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);
		const tool = tools[0];
		return {
			state,
			execute: (params) => tool.execute("call-1", params, undefined, undefined, { cwd: "/" }),
		};
	}

	it("rejects invalid caller, lifecycle, and target states without mutation", async () => {
		const cases = [
			{ name: "idle", state: freshState({ registry: new Map() }), command: "missing", error: "no_active_command" },
			{
				name: "reported",
				state: freshState({
					registry: new Map([["caller", def("caller", "body", undefined, false)]]),
					lifecycle: lifecycleFor("reported", "caller"),
				}),
				command: "missing",
				error: "report_pending",
			},
			{
				name: "stale caller",
				state: freshState({ registry: new Map(), lifecycle: lifecycleFor("dormant", "stale") }),
				command: "missing",
				error: "unknown_caller",
			},
			{
				name: "unknown target",
				state: freshState({
					registry: new Map([["caller", def("caller", "body", undefined, false)]]),
					lifecycle: lifecycleFor("dormant", "caller"),
				}),
				command: "missing",
				error: "unknown_command",
			},
			{
				name: "top-level target",
				state: freshState({
					registry: new Map([
						["caller", def("caller", "body", undefined, false)],
						["top", def("top", "body", undefined, false)],
					]),
					lifecycle: lifecycleFor("dormant", "caller"),
				}),
				command: "top",
				error: "not_subcommand",
			},
		];

		for (const testCase of cases) {
			const { pi, tools } = recordingPi();
			registerDelegateTool(pi, testCase.state);
			const before = {
				stack: structuredClone(testCase.state.delegateStack),
				sidebar: structuredClone(testCase.state.sidebarLog),
				lifecycle: structuredClone(testCase.state.lifecycle),
				generation: testCase.state.lifecycleGeneration,
			};
			const result = await tools[0].execute(
				"call",
				{ command: testCase.command, args: "" },
				undefined,
				undefined,
				{ cwd: "/" },
			);
			expect(result.details.error, testCase.name).toBe(testCase.error);
			expect(testCase.state.delegateStack).toEqual(before.stack);
			expect(testCase.state.sidebarLog).toEqual(before.sidebar);
			expect(testCase.state.lifecycle).toEqual(before.lifecycle);
			expect(testCase.state.lifecycleGeneration).toBe(before.generation);
			expect(pi.appended).toEqual([]);
		}
	});

	it.each(["running", "probing", "waiting", "dormant"] as const)("allows calls while %s", async (phase) => {
		const caller = def("caller", "caller", undefined, false);
		const target = def("target", "target");
		const state = freshState({
			registry: new Map([
				[caller.name, caller],
				[target.name, target],
			]),
			lifecycle: lifecycleFor(phase, caller.name),
		});
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);
		const result = await tools[0].execute("call", { command: target.name, args: "" }, undefined, undefined, {
			cwd: "/",
		});
		expect(result.details.error).toBeUndefined();
	});

	it("returns the substituted body and pushes a frame for a valid call", async () => {
		const { state, execute } = setupWithRegistry([def("mach12:push", "Run with: $ARGUMENTS")]);
		const result = await execute({ command: "mach12:push", args: "ship it" });
		expect(result.content[0].text).toBe(
			'<scramjet-command name="mach12:push">\nRun with: ship it\n</scramjet-command>',
		);
		expect(state.delegateStack).toHaveLength(1);
		expect(state.delegateStack[0].commandName).toBe("mach12:push");
		expect(state.delegateStack[0].depth).toBe(1);
		expect(state.sidebarLog).toHaveLength(1);
		expect(state.sidebarLog[0]).toMatchObject({ command: "mach12:push", origin: "agent", depth: 1 });
	});

	it("journals delegated command starts without changing activeTopLevelCommand", async () => {
		const state = freshState({
			registry: new Map([
				["mach12:issue-plan", def("mach12:issue-plan", "caller", undefined, false)],
				["mach12:push", def("mach12:push", "body")],
			]),
			lifecycle: lifecycleFor("dormant", "mach12:issue-plan"),
		});
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);

		await tools[0].execute("call-1", { command: "mach12:push", args: "" }, undefined, undefined, { cwd: "/" });

		expect(activeCommandName(state.lifecycle)).toBe("mach12:issue-plan");
		// Lifecycle preserves the top-level command (dormant state since
		// no command-start event fired for the delegate)
		expect(derivedPhase(state.lifecycle)).toBe("dormant");
		expect(state.sidebarLog[0]).toMatchObject({ command: "mach12:push", origin: "agent", depth: 1 });
		expect(pi.appended).toHaveLength(1);
		expect(pi.appended[0].customType).toBe(COMMAND_START_TYPE);
		expect(pi.appended[0].data as SidebarEntry).toMatchObject({
			command: "mach12:push",
			origin: "agent",
			depth: 1,
		});
	});

	it("uses the active top-level command as the first delegate caller scope", async () => {
		const state = freshState({
			registry: new Map([
				["top", def("top", "top-body", ["Read"])],
				["callee", def("callee", "callee-body", ["Read", "Bash"])],
			]),
			lifecycle: lifecycleFor("dormant", "top"),
		});
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);

		const result = await tools[0].execute("call-1", { command: "callee", args: "" }, undefined, undefined, {
			cwd: "/",
		});

		expect(state.delegateStack[0].effectiveAllowedTools).toEqual(["Read"]);
		expect(result.details.effectiveAllowedTools).toEqual(["Read"]);
	});

	it("applies the callee restriction when the active top-level command is unrestricted", async () => {
		const state = freshState({
			registry: new Map([
				["top", def("top", "top-body")],
				["callee", def("callee", "callee-body", ["Bash"])],
			]),
			lifecycle: lifecycleFor("dormant", "top"),
		});
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);

		await tools[0].execute("call-1", { command: "callee", args: "" }, undefined, undefined, { cwd: "/" });

		expect(state.delegateStack[0].effectiveAllowedTools).toEqual(["Bash"]);
	});

	it("applies the top-level restriction when the first callee is unrestricted", async () => {
		const state = freshState({
			registry: new Map([
				["top", def("top", "top-body", ["Read", "Edit"])],
				["callee", def("callee", "callee-body")],
			]),
			lifecycle: lifecycleFor("dormant", "top"),
		});
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);

		await tools[0].execute("call-1", { command: "callee", args: "" }, undefined, undefined, { cwd: "/" });

		expect(state.delegateStack[0].effectiveAllowedTools).toEqual(["Read", "Edit"]);
	});

	it("prepends the empty-scope warning when top-level and first callee scopes are disjoint", async () => {
		const state = freshState({
			registry: new Map([
				["top", def("top", "top-body", ["Read"])],
				["callee", def("callee", "callee-body", ["Bash"])],
			]),
			lifecycle: lifecycleFor("dormant", "top"),
		});
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);

		const result = await tools[0].execute("call-1", { command: "callee", args: "" }, undefined, undefined, {
			cwd: "/",
		});

		expect(state.delegateStack[0].effectiveAllowedTools).toEqual([]);
		expect(result.details.effectiveAllowedTools).toEqual([]);
		expect(result.content[0].text).toMatch(
			/\[scramjet\/delegate\] WARNING: effective allowed-tools scope for 'callee' is empty/,
		);
		expect(result.content[0].text).toContain("Tool calls will trigger advisory warnings rather than be blocked");
		expect(result.content[0].text).toContain("widening the caller's scope or aborting the delegation");
		expect(result.content[0].text).toContain("callee-body");
	});

	it("each delegation intersects independently with the top-level command scope", async () => {
		const state = freshState({
			registry: new Map([
				["top", def("top", "body-top", ["Read", "Bash", "Edit"])],
				["a", def("a", "body-a", ["Read", "Bash"])],
				["b", def("b", "body-b", ["Bash", "Write"])],
			]),
			lifecycle: lifecycleFor("dormant", "top"),
		});
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);
		const tool = tools[0];

		await tool.execute("call-1", { command: "a", args: "" }, undefined, undefined, { cwd: "/" });
		expect(state.delegateStack[0].effectiveAllowedTools).toEqual(["Read", "Bash"]);

		await tool.execute("call-2", { command: "b", args: "" }, undefined, undefined, { cwd: "/" });
		// "b" intersects with top-level [Read,Bash,Edit], not with "a"'s [Read,Bash]
		expect(state.delegateStack[1].effectiveAllowedTools).toEqual(["Bash"]);
		expect(state.delegateStack[1].depth).toBe(2);
		expect(state.sidebarLog.map((entry) => entry.depth)).toEqual([1, 2]);
	});

	it("leaves effectiveAllowedTools undefined when neither caller nor callee restrict", async () => {
		const { state, execute } = setupWithRegistry([def("mach12:push", "body")]);
		await execute({ command: "mach12:push", args: "" });
		expect(state.delegateStack[0].effectiveAllowedTools).toBeUndefined();
	});

	it("rejects the active caller loading itself without mutation", async () => {
		const caller = def("caller", "body", undefined, true);
		const state = freshState({ registry: new Map([[caller.name, caller]]), lifecycle: lifecycleFor("dormant", caller.name) });
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);
		const result = await tools[0].execute("call", { command: caller.name, args: "" }, undefined, undefined, { cwd: "/" });
		expect(result.details.error).toBe("cycle");
		expect(result.details.chain).toBe("caller -> caller");
		expect(state.delegateStack).toEqual([]);
		expect(state.sidebarLog).toEqual([]);
		expect(pi.appended).toEqual([]);
	});

	it("rejects a cycle and does not push a second frame for the same name", async () => {
		const { state, execute } = setupWithRegistry([def("mach12:push", "body-with-$1", ["Read"])]);
		await execute({ command: "mach12:push", args: "" });
		expect(state.delegateStack).toHaveLength(1);

		const result = await execute({ command: "mach12:push", args: "again" });
		expect(result.content[0].text).toContain("cycle");
		expect(result.content[0].text).toContain("mach12:push -> mach12:push");
		expect(result.details.error).toBe("cycle");
		expect(state.delegateStack).toHaveLength(1);
	});

	it("detects a multi-frame cycle a -> b -> a and reports the full chain (F37/S6)", async () => {
		// Single-self-loop is the easy case; the latched-stack design means a
		// non-trivial chain (top -> sub -> top) is the more realistic shape of
		// any future authoring error. The chain string in the error must list
		// every frame so the agent can see *where* the loop closes, not just
		// that one exists.
		const { state, execute } = setupWithRegistry([def("a", "body-a"), def("b", "body-b")]);
		await execute({ command: "a", args: "" });
		await execute({ command: "b", args: "" });
		expect(state.delegateStack.map((f) => f.commandName)).toEqual(["a", "b"]);

		const result = await execute({ command: "a", args: "" });
		expect(result.details.error).toBe("cycle");
		expect(result.details.chain).toBe("test:caller -> a -> b -> a");
		expect(result.content[0].text).toContain("a -> b -> a");
		// No third frame pushed; latched stack stays at depth 2.
		expect(state.delegateStack).toHaveLength(2);
	});

	it("distinct sequential subcommands each get their own scope", async () => {
		const { state, execute } = setupWithRegistry([
			def("a", "body-a", ["Read", "Bash"]),
			def("b", "body-b"),
			def("c", "body-c", ["Bash"]),
		]);

		const r1 = await execute({ command: "a", args: "" });
		expect(r1.details.error).toBeUndefined();
		expect(r1.details.depth).toBe(1);

		const r2 = await execute({ command: "b", args: "" });
		expect(r2.details.error).toBeUndefined();
		expect(r2.details.depth).toBe(2);

		const r3 = await execute({ command: "c", args: "" });
		expect(r3.details.error).toBeUndefined();
		expect(r3.details.depth).toBe(3);

		expect(state.delegateStack.map((f) => f.commandName)).toEqual(["a", "b", "c"]);
		// The setup caller is unrestricted, so each frame keeps its own scope.
		expect(state.delegateStack[0].effectiveAllowedTools).toEqual(["Read", "Bash"]);
		expect(state.delegateStack[1].effectiveAllowedTools).toBeUndefined();
		expect(state.delegateStack[2].effectiveAllowedTools).toEqual(["Bash"]);
		expect(state.sidebarLog.map((entry) => entry.depth)).toEqual([1, 2, 3]);
	});

	it("nested delegation with active command: each intersects top-level scope independently", async () => {
		const state = freshState({
			registry: new Map([
				["top", def("top", "body-top", ["Read", "Bash", "Write"])],
				["a", def("a", "body-a", ["Read", "Bash"])],
				["b", def("b", "body-b")],
				["c", def("c", "body-c", ["Bash"])],
			]),
			lifecycle: lifecycleFor("dormant", "top"),
		});
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);
		const tool = tools[0];

		await tool.execute("call-1", { command: "a", args: "" }, undefined, undefined, { cwd: "/" });
		await tool.execute("call-2", { command: "b", args: "" }, undefined, undefined, { cwd: "/" });
		await tool.execute("call-3", { command: "c", args: "" }, undefined, undefined, { cwd: "/" });

		expect(state.delegateStack.map((f) => f.commandName)).toEqual(["a", "b", "c"]);
		// Each intersects with top-level [Read,Bash,Write] independently.
		expect(state.delegateStack[0].effectiveAllowedTools).toEqual(["Read", "Bash"]);
		// Unrestricted "b" inherits top-level scope.
		expect(state.delegateStack[1].effectiveAllowedTools).toEqual(["Read", "Bash", "Write"]);
		expect(state.delegateStack[2].effectiveAllowedTools).toEqual(["Bash"]);
	});

	it("clears the stack on before_agent_start so each turn starts fresh", async () => {
		const state = freshState({ registry: new Map([["a", def("a", "body-a")]]) });
		state.delegateStack.push({ commandName: "leftover", depth: 0 });
		const { pi, emit } = recordingPi();
		registerDelegateTool(pi, state);
		await emit("before_agent_start");
		expect(state.delegateStack).toHaveLength(0);
	});

	it("parses bash-style args before substituting (quoted strings stay one positional)", async () => {
		const { execute } = setupWithRegistry([def("a", "first=$1 second=$2")]);
		const result = await execute({ command: "a", args: '"one two" three' });
		expect(result.content[0].text).toBe(
			'<scramjet-command name="a">\nfirst=one two second=three\n</scramjet-command>',
		);
	});

	it("sibling delegation does not inherit prior sibling's narrowed scope", async () => {
		const state = freshState({
			registry: new Map([
				["top", def("top", "body-top", ["Read", "Bash", "Edit", "Write"])],
				["narrow", def("narrow", "body-narrow", ["Read"])],
				["wide", def("wide", "body-wide", ["Bash", "Edit", "Write"])],
			]),
			lifecycle: lifecycleFor("dormant", "top"),
		});
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);
		const tool = tools[0];

		await tool.execute("call-1", { command: "narrow", args: "" }, undefined, undefined, { cwd: "/" });
		expect(state.delegateStack[0].effectiveAllowedTools).toEqual(["Read"]);

		await tool.execute("call-2", { command: "wide", args: "" }, undefined, undefined, { cwd: "/" });
		// Old behavior would yield [] (intersect [Read] with [Bash,Edit,Write]).
		// New behavior: intersect top-level [Read,Bash,Edit,Write] with [Bash,Edit,Write].
		expect(state.delegateStack[1].effectiveAllowedTools).toEqual(["Bash", "Edit", "Write"]);
	});

	it("does not prepend the empty-scope warning when allowed-tools is undefined or non-empty", async () => {
		const { execute } = setupWithRegistry([def("unrestricted", "body-u"), def("restricted", "body-r", ["Read"])]);
		const r1 = await execute({ command: "unrestricted", args: "" });
		expect(r1.content[0].text).toBe('<scramjet-command name="unrestricted">\nbody-u\n</scramjet-command>');
		const r2 = await execute({ command: "restricted", args: "" });
		expect(r2.content[0].text).toBe('<scramjet-command name="restricted">\nbody-r\n</scramjet-command>');
	});
});
