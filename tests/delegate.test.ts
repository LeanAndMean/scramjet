import { describe, expect, it } from "vitest";
import {
	detectCycle,
	intersectTools,
	parseDelegateArgs,
	registerDelegateTool,
	substituteArguments,
} from "../delegate.ts";
import type { CommandDef, DelegateFrame, ScramjetState } from "../types.ts";
import { freshState, recordingPi } from "./helpers.ts";

function def(name: string, body: string, allowedTools?: string[]): CommandDef {
	const d: CommandDef = { name, filePath: `/fake/${name}.md`, body };
	if (allowedTools !== undefined) d.allowedTools = allowedTools;
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
	it("registers exactly one tool named 'delegate' and one before_agent_start handler", () => {
		const { pi, tools, handlers } = recordingPi();
		registerDelegateTool(pi, freshState());
		expect(tools).toHaveLength(1);
		expect(tools[0].name).toBe("delegate");
		expect(handlers.has("before_agent_start")).toBe(true);
	});
});

describe("registerDelegateTool — execute paths", () => {
	function setupWithRegistry(entries: CommandDef[]): {
		state: ScramjetState;
		execute: (params: { command: string; args: string }) => Promise<any>;
	} {
		const state = freshState({ registry: new Map(entries.map((d) => [d.name, d])) });
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);
		const tool = tools[0];
		return {
			state,
			execute: (params) => tool.execute("call-1", params, undefined, undefined, { cwd: "/" }),
		};
	}

	it("returns an error for an unknown command and does not push a frame", async () => {
		const { state, execute } = setupWithRegistry([]);
		const result = await execute({ command: "mach12:nope", args: "" });
		expect(result.content[0].text).toContain("unknown command");
		expect(result.content[0].text).toContain("mach12:nope");
		expect(result.details.error).toBe("unknown_command");
		expect(state.delegateStack).toHaveLength(0);
	});

	it("returns the substituted body and pushes a frame for a valid call", async () => {
		const { state, execute } = setupWithRegistry([def("mach12:push", "Run with: $ARGUMENTS")]);
		const result = await execute({ command: "mach12:push", args: "ship it" });
		expect(result.content[0].text).toBe("Run with: ship it");
		expect(state.delegateStack).toHaveLength(1);
		expect(state.delegateStack[0].commandName).toBe("mach12:push");
		expect(state.delegateStack[0].depth).toBe(0);
	});

	it("narrows effectiveAllowedTools monotonically across the latched stack on sequential calls", async () => {
		// MVP latched semantics: frames are never popped within a turn, so a
		// second top-level delegate inherits the first frame's tools as its
		// "caller". This is intentional (CLAUDE.md "tool-scoping is advisory
		// in MVP") and distinct from true call-stack push/pop where a sibling
		// call would see the unrestricted top-level scope.
		const state = freshState({
			registry: new Map([
				["a", def("a", "body-a", ["Read", "Bash", "Edit"])],
				["b", def("b", "body-b", ["Bash", "Write"])],
			]),
		});
		const { pi, tools } = recordingPi();
		registerDelegateTool(pi, state);
		const tool = tools[0];

		await tool.execute("call-1", { command: "a", args: "" }, undefined, undefined, { cwd: "/" });
		expect(state.delegateStack[0].effectiveAllowedTools).toEqual(["Read", "Bash", "Edit"]);

		await tool.execute("call-2", { command: "b", args: "" }, undefined, undefined, { cwd: "/" });
		expect(state.delegateStack[1].effectiveAllowedTools).toEqual(["Bash"]);
		expect(state.delegateStack[1].depth).toBe(1);
	});

	it("leaves effectiveAllowedTools undefined when neither caller nor callee restrict", async () => {
		const { state, execute } = setupWithRegistry([def("mach12:push", "body")]);
		await execute({ command: "mach12:push", args: "" });
		expect(state.delegateStack[0].effectiveAllowedTools).toBeUndefined();
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
		expect(result.content[0].text).toBe("first=one two second=three");
	});
});
