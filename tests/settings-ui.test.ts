import { describe, expect, it } from "vitest";
import { buildCommandItems, buildEdgeItems, buildTopLevelItems } from "../settings-ui.ts";
import type { AutonomyConfig, CommandDef, NextStepPolicy } from "../types.ts";
import { freshState } from "./helpers.ts";

const noopTheme = {
	label: (text: string) => text,
	value: (text: string) => text,
	description: (text: string) => text,
	cursor: "> ",
	hint: (text: string) => text,
};

function makeCommandDef(name: string, next?: NextStepPolicy): CommandDef {
	return { name, filePath: `/fake/${name}.md`, body: "", description: `${name} desc`, next };
}

describe("buildEdgeItems", () => {
	it("returns a forced edge as non-cycleable with annotation", () => {
		const policy: NextStepPolicy = { mode: "forced", target: "mach12:push" };
		const items = buildEdgeItems("mach12:pr-create", policy, null);

		expect(items).toHaveLength(1);
		expect(items[0].label).toBe("mach12:push");
		expect(items[0].currentValue).toBe("default");
		expect(items[0].values).toBeUndefined();
		expect(items[0].description).toContain("Forced");
	});

	it("returns closed policy edges as cycleable with current config value", () => {
		const policy: NextStepPolicy = {
			mode: "closed",
			candidates: [{ name: "mach12:pr-create" }, { name: "mach12:issue-implement" }],
		};
		const config: AutonomyConfig = { edges: { "mach12:plan": { "mach12:pr-create": "chain" } } };
		const items = buildEdgeItems("mach12:plan", policy, config);

		expect(items).toHaveLength(2);
		expect(items[0].label).toBe("mach12:pr-create");
		expect(items[0].currentValue).toBe("chain");
		expect(items[0].values).toEqual(["default", "chain", "pause"]);
		expect(items[1].label).toBe("mach12:issue-implement");
		expect(items[1].currentValue).toBe("default");
		expect(items[1].values).toEqual(["default", "chain", "pause"]);
	});

	it("returns open policy edges as cycleable", () => {
		const policy: NextStepPolicy = {
			mode: "open",
			candidates: [{ name: "mach12:pr-review" }],
		};
		const items = buildEdgeItems("mach12:implement", policy, null);

		expect(items).toHaveLength(1);
		expect(items[0].currentValue).toBe("default");
		expect(items[0].values).toEqual(["default", "chain", "pause"]);
	});

	it("returns empty array for ask policy (no targets)", () => {
		const policy: NextStepPolicy = { mode: "ask" };
		const items = buildEdgeItems("mach12:review", policy, null);
		expect(items).toEqual([]);
	});

	it("uses null config gracefully (all defaults)", () => {
		const policy: NextStepPolicy = { mode: "closed", candidates: [{ name: "mach12:x" }] };
		const items = buildEdgeItems("mach12:cmd", policy, null);
		expect(items[0].currentValue).toBe("default");
	});
});

describe("buildCommandItems", () => {
	it("returns sorted items for commands with next policies", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:z-last", makeCommandDef("mach12:z-last", { mode: "forced", target: "mach12:push" })],
			["mach12:a-first", makeCommandDef("mach12:a-first", { mode: "closed", candidates: [{ name: "x" }] })],
			["mach12:no-next", makeCommandDef("mach12:no-next")],
		]);

		const items = buildCommandItems(state, null, noopTheme, () => {});

		expect(items).toHaveLength(2);
		expect(items[0].label).toBe("mach12:a-first");
		expect(items[1].label).toBe("mach12:z-last");
	});

	it("includes description from the command def", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "forced", target: "mach12:push" })],
		]);

		const items = buildCommandItems(state, null, noopTheme, () => {});
		expect(items[0].description).toBe("mach12:cmd desc");
	});

	it("shows override count in currentValue when config has overrides", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }, { name: "b" }] })],
		]);
		const config: AutonomyConfig = { edges: { "mach12:cmd": { a: "chain" } } };

		const items = buildCommandItems(state, config, noopTheme, () => {});
		expect(items[0].currentValue).toContain("1/2 overridden");
	});

	it("returns empty array when no commands have next policies", () => {
		const state = freshState();
		state.registry = new Map([["mach12:cmd", makeCommandDef("mach12:cmd")]]);

		const items = buildCommandItems(state, null, noopTheme, () => {});
		expect(items).toEqual([]);
	});

	it("each item has a submenu function", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "forced", target: "mach12:push" })],
		]);

		const items = buildCommandItems(state, null, noopTheme, () => {});
		expect(items[0].submenu).toBeTypeOf("function");
	});
});

describe("buildTopLevelItems", () => {
	it("always includes auto-continuation toggle reflecting state.enabled", () => {
		const stateOn = freshState({ enabled: true });
		const stateOff = freshState({ enabled: false });

		const itemsOn = buildTopLevelItems(stateOn, null, noopTheme, () => {});
		const itemsOff = buildTopLevelItems(stateOff, null, noopTheme, () => {});

		const toggleOn = itemsOn.find((i) => i.id === "auto-continuation");
		const toggleOff = itemsOff.find((i) => i.id === "auto-continuation");

		expect(toggleOn?.currentValue).toBe("on");
		expect(toggleOff?.currentValue).toBe("off");
		expect(toggleOn?.values).toEqual(["on", "off"]);
	});

	it("includes command autonomy submenu when registry has commands with next policies", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "forced", target: "mach12:push" })],
		]);

		const items = buildTopLevelItems(state, null, noopTheme, () => {});
		const autonomy = items.find((i) => i.id === "command-autonomy");
		expect(autonomy).toBeDefined();
		expect(autonomy?.submenu).toBeTypeOf("function");
	});

	it("shows informational item when no commands have next policies", () => {
		const state = freshState();
		state.registry = new Map([["mach12:cmd", makeCommandDef("mach12:cmd")]]);

		const items = buildTopLevelItems(state, null, noopTheme, () => {});
		const autonomy = items.find((i) => i.id === "command-autonomy");
		expect(autonomy).toBeDefined();
		expect(autonomy?.currentValue).toBe("no edges");
		expect(autonomy?.description).toContain("No commands");
		expect(autonomy?.submenu).toBeUndefined();
	});

	it("shows override count in command autonomy summary", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }] })],
		]);
		const config: AutonomyConfig = { edges: { "mach12:cmd": { a: "pause" } } };

		const items = buildTopLevelItems(state, config, noopTheme, () => {});
		const autonomy = items.find((i) => i.id === "command-autonomy");
		expect(autonomy?.currentValue).toContain("1 override");
	});

	it("shows 'all defaults' when no overrides configured", () => {
		const state = freshState();
		state.registry = new Map([["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "forced", target: "x" })]]);

		const items = buildTopLevelItems(state, null, noopTheme, () => {});
		const autonomy = items.find((i) => i.id === "command-autonomy");
		expect(autonomy?.currentValue).toBe("all defaults");
	});
});
