import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import type { Component } from "@leanandmean/tui";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { loadAutonomyConfig, resetCache, saveAutonomyConfig } from "../src/autonomy-settings.js";
import { loadPreferences, resetCache as resetPrefsCache, savePreferences } from "../src/preferences.js";
import {
	buildApplyRecommendationsItem,
	buildCommandItems,
	buildEdgeItems,
	buildTopLevelItems,
	classifyRecommendationEdges,
	showSettingsPage,
} from "../src/settings-ui.js";
import type {
	AutonomyConfig,
	AutonomyRecommendations,
	CommandDef,
	NextStepPolicy,
	RecommendationSetting,
} from "../src/types.js";
import { freshState } from "./helpers.js";

const noopTheme = {
	label: (text: string) => text,
	value: (text: string) => text,
	description: (text: string) => text,
	cursor: "> ",
	hint: (text: string) => text,
};

const tuiTheme = {
	fg: (_color: string, text: string) => text,
	bold: (text: string) => text,
};

let tmpDir: string | null = null;

beforeEach(() => {
	tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-settings-ui-"));
});

afterEach(() => {
	resetCache();
	resetPrefsCache();
	if (tmpDir) fs.rmSync(tmpDir, { recursive: true, force: true });
	tmpDir = null;
});

function makeCommandDef(name: string, next?: NextStepPolicy): CommandDef {
	return { name, filePath: `/fake/${name}.md`, body: "", description: `${name} desc`, next };
}

function renderText(component: Component | undefined): string {
	return component?.render(120).join("\n") ?? "";
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

	it("shows wildcard-inherited closed policy settings distinctly", () => {
		const policy: NextStepPolicy = {
			mode: "closed",
			candidates: [{ name: "mach12:pr-create" }, { name: "mach12:issue-implement" }],
		};
		const config: AutonomyConfig = { edges: { "mach12:plan": { "*": "pause", "mach12:pr-create": "chain" } } };
		const items = buildEdgeItems("mach12:plan", policy, config);

		expect(items[0].currentValue).toBe("chain");
		expect(items[0].description).toContain("Override:");
		expect(items[1].currentValue).toBe("pause");
		expect(items[1].description).toContain("Inherited wildcard override: pause");
		expect(items[1].values).toEqual(["chain", "pause"]);
	});

	it("shows wildcard-inherited open policy settings distinctly", () => {
		const policy: NextStepPolicy = {
			mode: "open",
			candidates: [{ name: "mach12:pr-review" }],
		};
		const config: AutonomyConfig = { edges: { "mach12:implement": { "*": "chain" } } };
		const items = buildEdgeItems("mach12:implement", policy, config);

		expect(items[0].currentValue).toBe("chain");
		expect(items[0].description).toContain("Inherited wildcard override: chain");
		expect(items[0].values).toEqual(["chain", "pause"]);
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

		const items = buildCommandItems(
			state,
			() => null,
			noopTheme,
			() => {},
		);

		expect(items).toHaveLength(2);
		expect(items[0].label).toBe("mach12:a-first");
		expect(items[1].label).toBe("mach12:z-last");
	});

	it("includes description from the command def", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "forced", target: "mach12:push" })],
		]);

		const items = buildCommandItems(
			state,
			() => null,
			noopTheme,
			() => {},
		);
		expect(items[0].description).toBe("mach12:cmd desc");
	});

	it("shows override count in currentValue when config has overrides", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }, { name: "b" }] })],
		]);
		const config: AutonomyConfig = { edges: { "mach12:cmd": { a: "chain" } } };

		const items = buildCommandItems(
			state,
			() => config,
			noopTheme,
			() => {},
		);
		expect(items[0].currentValue).toContain("1/2 overridden");
	});

	it("includes wildcard-inherited edges in currentValue summaries", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }, { name: "b" }] })],
		]);
		const config: AutonomyConfig = { edges: { "mach12:cmd": { "*": "pause", a: "chain" } } };

		const items = buildCommandItems(
			state,
			() => config,
			noopTheme,
			() => {},
		);
		expect(items[0].currentValue).toBe("closed · 1/2 overridden, 1/2 wildcard");
	});

	it("returns empty array when no commands have next policies", () => {
		const state = freshState();
		state.registry = new Map([["mach12:cmd", makeCommandDef("mach12:cmd")]]);

		const items = buildCommandItems(
			state,
			() => null,
			noopTheme,
			() => {},
		);
		expect(items).toEqual([]);
	});

	it("each item has a submenu function", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "forced", target: "mach12:push" })],
		]);

		const items = buildCommandItems(
			state,
			() => null,
			noopTheme,
			() => {},
		);
		expect(items[0].submenu).toBeTypeOf("function");
	});

	it("reads fresh config when opening an edge submenu", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }] })],
		]);
		let config: AutonomyConfig | null = null;
		const items = buildCommandItems(
			state,
			() => config,
			noopTheme,
			() => {},
		);

		config = { edges: { "mach12:cmd": { a: "chain" } } };
		const submenu = items[0].submenu?.("closed · 1 edge", () => {});

		expect(renderText(submenu)).toContain("a  chain");
	});

	it("passes a refreshed command summary when closing an edge submenu", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }, { name: "b" }] })],
		]);
		let config: AutonomyConfig | null = null;
		const items = buildCommandItems(
			state,
			() => config,
			noopTheme,
			() => {},
		);
		let selectedValue: string | undefined;
		const submenu = items[0].submenu?.("closed · 2 edges", (value) => {
			selectedValue = value;
		});

		config = { edges: { "mach12:cmd": { a: "pause" } } };
		submenu?.handleInput?.("\x1b");

		expect(selectedValue).toBe("closed · 1/2 overridden");
	});
});

describe("buildTopLevelItems", () => {
	it("always includes auto-continuation toggle reflecting state.enabled", () => {
		const stateOn = freshState({ enabled: true });
		const stateOff = freshState({ enabled: false });

		const itemsOn = buildTopLevelItems(
			stateOn,
			() => null,
			noopTheme,
			() => {},
		);
		const itemsOff = buildTopLevelItems(
			stateOff,
			() => null,
			noopTheme,
			() => {},
		);

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

		const items = buildTopLevelItems(
			state,
			() => null,
			noopTheme,
			() => {},
		);
		const autonomy = items.find((i) => i.id === "command-autonomy");
		expect(autonomy).toBeDefined();
		expect(autonomy?.submenu).toBeTypeOf("function");
	});

	it("shows informational item when no commands have next policies", () => {
		const state = freshState();
		state.registry = new Map([["mach12:cmd", makeCommandDef("mach12:cmd")]]);

		const items = buildTopLevelItems(
			state,
			() => null,
			noopTheme,
			() => {},
		);
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

		const items = buildTopLevelItems(
			state,
			() => config,
			noopTheme,
			() => {},
		);
		const autonomy = items.find((i) => i.id === "command-autonomy");
		expect(autonomy?.currentValue).toContain("1 override");
	});

	it("shows 'all defaults' when no overrides configured", () => {
		const state = freshState();
		state.registry = new Map([["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "forced", target: "x" })]]);

		const items = buildTopLevelItems(
			state,
			() => null,
			noopTheme,
			() => {},
		);
		const autonomy = items.find((i) => i.id === "command-autonomy");
		expect(autonomy?.currentValue).toBe("all defaults");
	});

	it("reads fresh config when opening the command autonomy submenu", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }] })],
		]);
		let config: AutonomyConfig | null = null;
		const items = buildTopLevelItems(
			state,
			() => config,
			noopTheme,
			() => {},
		);
		const autonomy = items.find((i) => i.id === "command-autonomy");

		config = { edges: { "mach12:cmd": { a: "chain" } } };
		const submenu = autonomy?.submenu?.("all defaults", () => {});

		expect(renderText(submenu)).toContain("mach12:cmd  closed · 1/1 overridden");
	});

	it("passes a refreshed registry summary when closing the command autonomy submenu", () => {
		const state = freshState();
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }] })],
		]);
		let config: AutonomyConfig | null = null;
		const items = buildTopLevelItems(
			state,
			() => config,
			noopTheme,
			() => {},
		);
		const autonomy = items.find((i) => i.id === "command-autonomy");
		let selectedValue: string | undefined;
		const submenu = autonomy?.submenu?.("all defaults", (value) => {
			selectedValue = value;
		});

		config = { edges: { "mach12:cmd": { a: "pause" } } };
		submenu?.handleInput?.("\x1b");

		expect(selectedValue).toBe("1 override");
	});
});

describe("buildTopLevelItems — notification preferences", () => {
	it("includes title-indicator toggle with correct default (on)", () => {
		const state = freshState({ preferencesPath: path.join(tmpDir!, "preferences.yaml") });
		const items = buildTopLevelItems(
			state,
			() => null,
			noopTheme,
			() => {},
		);
		const item = items.find((i) => i.id === "title-indicator");
		expect(item).toBeDefined();
		expect(item?.currentValue).toBe("on");
		expect(item?.values).toEqual(["on", "off"]);
		expect(item?.description).toContain("title");
	});

	it("includes terminal-bell toggle with correct default (off)", () => {
		const state = freshState({ preferencesPath: path.join(tmpDir!, "preferences.yaml") });
		const items = buildTopLevelItems(
			state,
			() => null,
			noopTheme,
			() => {},
		);
		const item = items.find((i) => i.id === "terminal-bell");
		expect(item).toBeDefined();
		expect(item?.currentValue).toBe("off");
		expect(item?.values).toEqual(["on", "off"]);
		expect(item?.description).toContain("bell");
	});

	it("reflects saved preferences", () => {
		const prefsPath = path.join(tmpDir!, "preferences.yaml");
		savePreferences(prefsPath, { title_indicator: false, bell: true });
		const state = freshState({ preferencesPath: prefsPath });
		const items = buildTopLevelItems(
			state,
			() => null,
			noopTheme,
			() => {},
		);
		expect(items.find((i) => i.id === "title-indicator")?.currentValue).toBe("off");
		expect(items.find((i) => i.id === "terminal-bell")?.currentValue).toBe("on");
	});
});

describe("showSettingsPage", () => {
	it("toggling title-indicator persists to preferences file", async () => {
		const prefsPath = path.join(tmpDir!, "preferences.yaml");
		const state = freshState({ preferencesPath: prefsPath });
		state.registry = new Map();

		const pi = { appendEntry: vi.fn() };
		const ctx = {
			ui: {
				notify: vi.fn(),
				custom: async (
					factory: (tui: unknown, theme: typeof tuiTheme, keybindings: unknown, done: () => void) => Component,
				) => {
					const component = factory({ requestRender: vi.fn() }, tuiTheme, {}, vi.fn());
					// Navigate to title-indicator (index 1) and toggle
					component.handleInput?.("\x1b[B"); // down to title-indicator
					component.handleInput?.("\r"); // cycle: on → off
				},
			},
		};

		await showSettingsPage(pi as never, ctx as never, state);

		const prefs = loadPreferences(prefsPath);
		expect(prefs.title_indicator).toBe(false);
		expect(prefs.bell).toBe(false);
	});

	it("toggling terminal-bell persists to preferences file", async () => {
		const prefsPath = path.join(tmpDir!, "preferences.yaml");
		const state = freshState({ preferencesPath: prefsPath });
		state.registry = new Map();

		const pi = { appendEntry: vi.fn() };
		const ctx = {
			ui: {
				notify: vi.fn(),
				custom: async (
					factory: (tui: unknown, theme: typeof tuiTheme, keybindings: unknown, done: () => void) => Component,
				) => {
					const component = factory({ requestRender: vi.fn() }, tuiTheme, {}, vi.fn());
					// Navigate to terminal-bell (index 2) and toggle
					component.handleInput?.("\x1b[B"); // down to title-indicator
					component.handleInput?.("\x1b[B"); // down to terminal-bell
					component.handleInput?.("\r"); // cycle: off → on
				},
			},
		};

		await showSettingsPage(pi as never, ctx as never, state);

		const prefs = loadPreferences(prefsPath);
		expect(prefs.bell).toBe(true);
		expect(prefs.title_indicator).toBe(true);
	});

	it("preserves fresh disk overrides when saving an autonomy toggle", async () => {
		const configPath = path.join(tmpDir!, "autonomy.yaml");
		const state = freshState({ autonomyConfigPath: configPath });
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }, { name: "b" }] })],
		]);
		saveAutonomyConfig(configPath, { edges: { "mach12:cmd": { a: "pause" } } });

		const pi = { appendEntry: vi.fn() };
		const ctx = {
			ui: {
				notify: vi.fn(),
				custom: async (
					factory: (tui: unknown, theme: typeof tuiTheme, keybindings: unknown, done: () => void) => Component,
				) => {
					const component = factory({ requestRender: vi.fn() }, tuiTheme, {}, vi.fn());
					saveAutonomyConfig(configPath, { edges: { "mach12:cmd": { a: "chain", b: "pause" } } });

					component.handleInput?.("\x1b[B"); // title-indicator
					component.handleInput?.("\x1b[B"); // terminal-bell
					component.handleInput?.("\x1b[B"); // command-autonomy
					component.handleInput?.("\r"); // open command submenu
					component.handleInput?.("\r"); // open edge submenu
					expect(renderText(component)).toContain("a  chain");
					component.handleInput?.("\r");
				},
			},
		};

		await showSettingsPage(pi as never, ctx as never, state);

		expect(loadAutonomyConfig(configPath)).toEqual({ edges: { "mach12:cmd": { a: "pause", b: "pause" } } });
	});
});

describe("buildApplyRecommendationsItem", () => {
	it("returns null when no recommendations exist", () => {
		const item = buildApplyRecommendationsItem(
			new Map(),
			() => null,
			"/tmp/test/autonomy.yaml",
			() => {},
			() => {},
			noopTheme,
		);
		expect(item).toBeNull();
	});

	it("returns non-null with 'all applied' label when all recommended edges are already configured", () => {
		const recs: Map<string, AutonomyRecommendations> = new Map([
			["mach12", { edges: { "mach12:impl": { "mach12:pr": "chain" } } }],
		]);
		const config: AutonomyConfig = { edges: { "mach12:impl": { "mach12:pr": "pause" } } };
		const item = buildApplyRecommendationsItem(
			recs,
			() => config,
			"/tmp/test/autonomy.yaml",
			() => {},
			() => {},
			noopTheme,
		);
		expect(item).not.toBeNull();
		expect(item!.label).toContain("all applied");
	});

	it("returns null when all recommendations are default (no-ops)", () => {
		const recs: Map<string, AutonomyRecommendations> = new Map([
			["mach12", { edges: { "mach12:impl": { "mach12:pr": "default" } } }],
		]);
		const item = buildApplyRecommendationsItem(
			recs,
			() => null,
			"/tmp/test/autonomy.yaml",
			() => {},
			() => {},
			noopTheme,
		);
		expect(item).toBeNull();
	});

	it("shows correct count of unapplied edges in label", () => {
		const recs: Map<string, AutonomyRecommendations> = new Map([
			[
				"mach12",
				{
					edges: {
						"mach12:impl": { "mach12:impl": "chain", "mach12:pr": "chain" },
						"mach12:pr": { "mach12:review": "chain" },
					},
				},
			],
		]);
		const item = buildApplyRecommendationsItem(
			recs,
			() => null,
			"/tmp/test/autonomy.yaml",
			() => {},
			() => {},
			noopTheme,
		);
		expect(item).not.toBeNull();
		expect(item!.label).toBe("Apply recommended settings (3 pending)");
	});

	it("shows singular when only one unapplied edge", () => {
		const recs: Map<string, AutonomyRecommendations> = new Map([
			["mach12", { edges: { "mach12:impl": { "mach12:pr": "chain" } } }],
		]);
		const item = buildApplyRecommendationsItem(
			recs,
			() => null,
			"/tmp/test/autonomy.yaml",
			() => {},
			() => {},
			noopTheme,
		);
		expect(item!.label).toBe("Apply recommended settings (1 pending)");
	});

	it("excludes already-configured edges from count", () => {
		const recs: Map<string, AutonomyRecommendations> = new Map([
			[
				"mach12",
				{
					edges: {
						"mach12:impl": { "mach12:impl": "chain", "mach12:pr": "chain" },
					},
				},
			],
		]);
		const config: AutonomyConfig = { edges: { "mach12:impl": { "mach12:impl": "pause" } } };
		const item = buildApplyRecommendationsItem(
			recs,
			() => config,
			"/tmp/test/autonomy.yaml",
			() => {},
			() => {},
			noopTheme,
		);
		expect(item!.label).toBe("Apply recommended settings (1 pending)");
	});

	it("opening submenu does not write config", () => {
		const configPath = path.join(tmpDir!, "autonomy.yaml");
		const recs: Map<string, AutonomyRecommendations> = new Map([
			["mach12", { edges: { "mach12:impl": { "mach12:pr": "chain" } } }],
		]);
		let exitCalled = false;
		const notifications: { msg: string; type?: string }[] = [];
		const item = buildApplyRecommendationsItem(
			recs,
			() => null,
			configPath,
			() => {
				exitCalled = true;
			},
			(msg, type) => {
				notifications.push({ msg, type });
			},
			noopTheme,
		);

		item!.submenu!("", () => {});

		expect(exitCalled).toBe(false);
		expect(notifications).toHaveLength(0);
		expect(fs.existsSync(configPath)).toBe(false);
	});

	it("confirm action writes config, notifies, and exits submenu", () => {
		const configPath = path.join(tmpDir!, "autonomy.yaml");
		const recs: Map<string, AutonomyRecommendations> = new Map([
			["mach12", { edges: { "mach12:impl": { "mach12:pr": "chain" } } }],
		]);
		let exitCalled = false;
		const notifications: { msg: string; type?: string }[] = [];
		const item = buildApplyRecommendationsItem(
			recs,
			() => null,
			configPath,
			() => {
				exitCalled = true;
			},
			(msg, type) => {
				notifications.push({ msg, type });
			},
			noopTheme,
		);

		const submenu = item!.submenu!("", () => {});
		// Navigate to the "Apply" item (last item) and activate
		submenu.handleInput?.("\x1b[B"); // down to Apply item
		submenu.handleInput?.("\r"); // confirm

		expect(exitCalled).toBe(true);
		expect(notifications).toHaveLength(1);
		expect(notifications[0].msg).toContain("Applied 1 recommended setting");
		expect(notifications[0].type).toBe("info");

		const config = loadAutonomyConfig(configPath);
		expect(config).toEqual({ edges: { "mach12:impl": { "mach12:pr": "chain" } } });
	});

	it("notifies error and exits when apply fails", () => {
		// Create a file where the config directory should be, making saveAutonomyConfig throw
		const blocker = path.join(tmpDir!, "blocker");
		fs.writeFileSync(blocker, "block");
		const configPath = path.join(blocker, "autonomy.yaml");

		const recs: Map<string, AutonomyRecommendations> = new Map([
			["mach12", { edges: { "mach12:impl": { "mach12:pr": "chain" } } }],
		]);
		let exitCalled = false;
		const notifications: { msg: string; type?: string }[] = [];
		const item = buildApplyRecommendationsItem(
			recs,
			() => null,
			configPath,
			() => {
				exitCalled = true;
			},
			(msg, type) => {
				notifications.push({ msg, type });
			},
			noopTheme,
		);

		const submenu = item!.submenu!("", () => {});
		submenu.handleInput?.("\x1b[B"); // down to Apply item
		submenu.handleInput?.("\r"); // confirm

		expect(notifications).toHaveLength(1);
		expect(notifications[0].type).toBe("error");
		expect(notifications[0].msg).toContain("Failed to apply");
		expect(exitCalled).toBe(true);
	});

	it("preserves user overrides after apply confirmation", () => {
		const configPath = path.join(tmpDir!, "autonomy.yaml");
		saveAutonomyConfig(configPath, { edges: { "mach12:impl": { "mach12:impl": "pause" } } });

		const recs: Map<string, AutonomyRecommendations> = new Map([
			[
				"mach12",
				{
					edges: {
						"mach12:impl": { "mach12:impl": "chain", "mach12:pr": "chain" },
					},
				},
			],
		]);
		const item = buildApplyRecommendationsItem(
			recs,
			() => loadAutonomyConfig(configPath),
			configPath,
			() => {},
			() => {},
			noopTheme,
		);
		expect(item!.label).toBe("Apply recommended settings (1 pending)");

		const submenu = item!.submenu!("", () => {});
		// Navigate past the edge items to the Apply item and confirm
		submenu.handleInput?.("\x1b[B"); // down past configured edge
		submenu.handleInput?.("\x1b[B"); // down to Apply item
		submenu.handleInput?.("\r"); // confirm

		const config = loadAutonomyConfig(configPath);
		expect(config?.edges["mach12:impl"]).toEqual({
			"mach12:impl": "pause", // preserved user override
			"mach12:pr": "chain", // applied recommendation
		});
	});

	it("submenu renders edge items with pending/configured status text", () => {
		const recs: Map<string, AutonomyRecommendations> = new Map([
			[
				"mach12",
				{
					edges: {
						"mach12:impl": { "mach12:pr": "chain", "mach12:review": "pause" },
					},
				},
			],
		]);
		const config: AutonomyConfig = { edges: { "mach12:impl": { "mach12:pr": "chain" } } };
		const item = buildApplyRecommendationsItem(
			recs,
			() => config,
			"/tmp/test/autonomy.yaml",
			() => {},
			() => {},
			noopTheme,
		);

		const submenu = item!.submenu!("", () => {});
		const rendered = renderText(submenu);
		expect(rendered).toContain("mach12:impl \u2192 mach12:pr");
		expect(rendered).toContain("chain (configured)");
		expect(rendered).toContain("mach12:impl \u2192 mach12:review");
		expect(rendered).toContain("pause (pending)");
	});

	it("no Apply item when all edges are configured", () => {
		const recs: Map<string, AutonomyRecommendations> = new Map([
			["mach12", { edges: { "mach12:impl": { "mach12:pr": "chain" } } }],
		]);
		const config: AutonomyConfig = { edges: { "mach12:impl": { "mach12:pr": "pause" } } };
		const item = buildApplyRecommendationsItem(
			recs,
			() => config,
			"/tmp/test/autonomy.yaml",
			() => {},
			() => {},
			noopTheme,
		);

		const submenu = item!.submenu!("", () => {});
		const rendered = renderText(submenu);
		expect(rendered).not.toContain("Apply");
		expect(rendered).toContain("chain (configured)");
	});

	it("submenu re-reads configGetter at activation time", () => {
		const recs: Map<string, AutonomyRecommendations> = new Map([
			["mach12", { edges: { "mach12:impl": { "mach12:pr": "chain" } } }],
		]);
		let config: AutonomyConfig | null = null;
		const item = buildApplyRecommendationsItem(
			recs,
			() => config,
			"/tmp/test/autonomy.yaml",
			() => {},
			() => {},
			noopTheme,
		);
		expect(item!.label).toContain("1 pending");

		// Config changes between construction and submenu open
		config = { edges: { "mach12:impl": { "mach12:pr": "pause" } } };
		const submenu = item!.submenu!("", () => {});
		const rendered = renderText(submenu);
		expect(rendered).toContain("configured");
		expect(rendered).not.toContain("Apply");
	});

	it("Esc on confirmation submenu calls done to return to command list", () => {
		const recs: Map<string, AutonomyRecommendations> = new Map([
			["mach12", { edges: { "mach12:impl": { "mach12:pr": "chain" } } }],
		]);
		let doneCalled = false;
		let exitCalled = false;
		const item = buildApplyRecommendationsItem(
			recs,
			() => null,
			"/tmp/test/autonomy.yaml",
			() => {
				exitCalled = true;
			},
			() => {},
			noopTheme,
		);

		const submenu = item!.submenu!("", () => {
			doneCalled = true;
		});
		submenu.handleInput?.("\x1b"); // Esc

		expect(doneCalled).toBe(true);
		expect(exitCalled).toBe(false);
	});
});

describe("classifyRecommendationEdges", () => {
	it("returns empty array for empty merged edges", () => {
		const merged: AutonomyRecommendations = { edges: {} };
		const result = classifyRecommendationEdges(merged, null);
		expect(result).toEqual([]);
	});

	it("classifies all as pending when no config exists", () => {
		const merged: AutonomyRecommendations = {
			edges: {
				"mach12:impl": { "mach12:pr": "chain" },
				"mach12:pr": { "mach12:review": "pause" },
			},
		};
		const result = classifyRecommendationEdges(merged, null);
		expect(result).toEqual([
			{ source: "mach12:impl", target: "mach12:pr", setting: "chain", status: "pending" },
			{ source: "mach12:pr", target: "mach12:review", setting: "pause", status: "pending" },
		]);
	});

	it("classifies all as configured when every edge has a config override", () => {
		const merged: AutonomyRecommendations = {
			edges: {
				"mach12:impl": { "mach12:pr": "chain" },
			},
		};
		const config: AutonomyConfig = { edges: { "mach12:impl": { "mach12:pr": "pause" } } };
		const result = classifyRecommendationEdges(merged, config);
		expect(result).toEqual([{ source: "mach12:impl", target: "mach12:pr", setting: "chain", status: "configured" }]);
	});

	it("filters out default settings and classifies mixed correctly", () => {
		const merged: AutonomyRecommendations = {
			edges: {
				"mach12:impl": {
					"mach12:pr": "chain",
					"mach12:review": "default" as RecommendationSetting,
					"mach12:merge": "pause",
				},
			},
		};
		const config: AutonomyConfig = { edges: { "mach12:impl": { "mach12:pr": "chain" } } };
		const result = classifyRecommendationEdges(merged, config);
		expect(result).toHaveLength(2);
		expect(result).toEqual([
			{ source: "mach12:impl", target: "mach12:pr", setting: "chain", status: "configured" },
			{ source: "mach12:impl", target: "mach12:merge", setting: "pause", status: "pending" },
		]);
	});
});
