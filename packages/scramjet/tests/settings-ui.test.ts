import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { type Component, CURSOR_MARKER } from "@leanandmean/tui";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
	loadAutonomyConfig,
	loadScopedAutonomyConfig,
	projectConfigPath,
	resetCache,
	saveAutonomyConfig,
} from "../src/autonomy-settings.js";
import { loadPreferences, resetCache as resetPrefsCache, savePreferences } from "../src/preferences.js";
import {
	buildApplyRecommendationsItem,
	buildCommandItems,
	buildEdgeItems,
	buildPublicationCommandItems,
	buildTopLevelItems,
	classifyRecommendationEdges,
	showSettingsPage,
	type TransitionEditor,
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

function editorFor(config: () => AutonomyConfig | null): TransitionEditor {
	return {
		scope: "global",
		snapshot: () => {
			const raw = config();
			return {
				status: "valid",
				global: {
					scope: "global",
					path: "/fixture/global.yaml",
					...(raw ? ({ status: "valid", config: raw } as const) : ({ status: "missing" } as const)),
				},
				project: { scope: "project", path: "/fixture/.scramjet/autonomy.yaml", status: "missing" },
			};
		},
	};
}

describe("buildEdgeItems", () => {
	it("returns a forced edge as non-cycleable with annotation", () => {
		const policy: NextStepPolicy = { mode: "forced", target: "mach12:push" };
		const items = buildEdgeItems(
			"mach12:pr-create",
			policy,
			editorFor(() => null),
		);

		expect(items).toHaveLength(1);
		expect(items[0].label).toBe("mach12:push");
		expect(items[0].currentValue).toBe("inherit");
		expect(items[0].values).toBeUndefined();
		expect(items[0].description).toContain("Forced");
	});

	it("returns closed policy edges as cycleable with current config value", () => {
		const policy: NextStepPolicy = {
			mode: "closed",
			candidates: [{ name: "mach12:pr-create" }, { name: "mach12:issue-implement" }],
		};
		const config: AutonomyConfig = { edges: { "mach12:plan": { "mach12:pr-create": "chain" } } };
		const items = buildEdgeItems(
			"mach12:plan",
			policy,
			editorFor(() => config),
		);

		expect(items).toHaveLength(2);
		expect(items[0].label).toBe("mach12:pr-create");
		expect(items[0].currentValue).toBe("chain");
		expect(items[0].values).toEqual(["inherit", "chain", "pause"]);
		expect(items[1].label).toBe("mach12:issue-implement");
		expect(items[1].currentValue).toBe("inherit");
		expect(items[1].values).toEqual(["inherit", "chain", "pause"]);
	});

	it("returns open policy edges as cycleable", () => {
		const policy: NextStepPolicy = {
			mode: "open",
			candidates: [{ name: "mach12:pr-review" }],
		};
		const items = buildEdgeItems(
			"mach12:implement",
			policy,
			editorFor(() => null),
		);

		expect(items).toHaveLength(1);
		expect(items[0].currentValue).toBe("inherit");
		expect(items[0].values).toEqual(["inherit", "chain", "pause"]);
	});

	it("shows wildcard-inherited closed policy settings distinctly", () => {
		const policy: NextStepPolicy = {
			mode: "closed",
			candidates: [{ name: "mach12:pr-create" }, { name: "mach12:issue-implement" }],
		};
		const config: AutonomyConfig = { edges: { "mach12:plan": { "*": "pause", "mach12:pr-create": "chain" } } };
		const items = buildEdgeItems(
			"mach12:plan",
			policy,
			editorFor(() => config),
		);

		expect(items[0].currentValue).toBe("chain");
		expect(items[0].description).toContain("Stored global exact:");
		expect(items[1].currentValue).toBe("inherit");
		expect(items[1].description).toContain("global wildcard");
		expect(items[1].values).toEqual(["inherit", "chain", "pause"]);
	});

	it("shows wildcard-inherited open policy settings distinctly", () => {
		const policy: NextStepPolicy = {
			mode: "open",
			candidates: [{ name: "mach12:pr-review" }],
		};
		const config: AutonomyConfig = { edges: { "mach12:implement": { "*": "chain" } } };
		const items = buildEdgeItems(
			"mach12:implement",
			policy,
			editorFor(() => config),
		);

		expect(items[0].currentValue).toBe("inherit");
		expect(items[0].description).toContain("global wildcard");
		expect(items[0].values).toEqual(["inherit", "chain", "pause"]);
	});

	it("returns empty array for ask policy (no targets)", () => {
		const policy: NextStepPolicy = { mode: "ask" };
		const items = buildEdgeItems(
			"mach12:review",
			policy,
			editorFor(() => null),
		);
		expect(items).toEqual([]);
	});

	it("uses null config gracefully (all defaults)", () => {
		const policy: NextStepPolicy = { mode: "closed", candidates: [{ name: "mach12:x" }] };
		const items = buildEdgeItems(
			"mach12:cmd",
			policy,
			editorFor(() => null),
		);
		expect(items[0].currentValue).toBe("inherit");
	});
});

describe("buildPublicationCommandItems", () => {
	it("shows only eligible commands and tools with friendly effective policies", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:review", { ...makeCommandDef("mach12:review"), allowedTools: ["create_issue", "add_pr_comment"] }],
			["mach12:none", makeCommandDef("mach12:none")],
			["mach12:delegate", { ...makeCommandDef("mach12:delegate"), delegateOnly: true, allowedTools: ["create_pr"] }],
		]);
		state.autonomyRecommendations = new Map([
			[
				"mach12",
				{
					edges: {},
					publications: {
						"mach12:review": { create_issue: "require-approval", add_pr_comment: "auto-approve" },
					},
				},
			],
		]);
		const config: AutonomyConfig = {
			edges: {},
			publications: { "mach12:review": { create_issue: "always-ask", create_pr: "auto-approve" } },
		};
		const changes: unknown[] = [];
		const commands = buildPublicationCommandItems(
			state,
			() => config,
			noopTheme,
			(...change) => changes.push(change),
		);
		expect(commands.map((item) => item.label)).toEqual(["mach12:review"]);
		expect(commands[0].currentValue).toBe("1 Always ask · 1 Auto-approve");
		const submenu = commands[0].submenu?.(commands[0].currentValue, () => {});
		const askText = renderText(submenu);
		expect(askText).toContain("create_issue");
		expect(askText).toContain("Always ask");
		expect(askText).toContain("Follow command (Always ask)");
		expect(askText).not.toContain("create_pr");
		expect(askText).not.toContain("add_issue_comment");
		submenu?.handleInput?.("\x1b[B");
		const autoApproveText = renderText(submenu);
		expect(autoApproveText).toContain("add_pr_comment");
		expect(autoApproveText).toContain("Follow command (Auto-approve)");
		expect(`${askText}\n${autoApproveText}`).not.toMatch(/follow-command|require-approval|always-ask/);
	});

	it("uses Always ask for a missing command default and translates friendly choices", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["sample:publish", { ...makeCommandDef("sample:publish"), allowedTools: ["create_issue"] }],
		]);
		const changes: unknown[] = [];
		const commands = buildPublicationCommandItems(
			state,
			() => null,
			noopTheme,
			(...change) => changes.push(change),
		);
		expect(commands[0].currentValue).toBe("Always ask");
		const submenu = commands[0].submenu?.(commands[0].currentValue, () => {});
		expect(renderText(submenu)).toContain("Follow command (Always ask)");
		submenu?.handleInput?.("\r");
		expect(changes).toEqual([]);
		expect(renderText(submenu)).toContain("Keep current policy");
		submenu?.handleInput?.("\x1b[B");
		submenu?.handleInput?.("\r");
		expect(changes).toEqual([["sample:publish", "create_issue", "auto-approve"]]);
	});

	it("restores the persisted policy when saving a changed value fails", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["sample:publish", { ...makeCommandDef("sample:publish"), allowedTools: ["create_issue"] }],
		]);
		const config = { edges: {}, publications: { "sample:publish": { create_issue: "always-ask" as const } } };
		const command = buildPublicationCommandItems(
			state,
			() => config,
			noopTheme,
			() => false,
		)[0]!;
		const submenu = command.submenu?.(command.currentValue, vi.fn());
		submenu?.handleInput?.("\r");
		expect(renderText(submenu).split("\n")[0]).toContain("Always ask");
	});

	it("confirms Follow command when its effective default is Auto-approve", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["sample:publish", { ...makeCommandDef("sample:publish"), allowedTools: ["create_issue"] }],
		]);
		state.autonomyRecommendations = new Map([
			["sample", { edges: {}, publications: { "sample:publish": { create_issue: "auto-approve" } } }],
		]);
		const changes: unknown[] = [];
		const command = buildPublicationCommandItems(
			state,
			() => ({ edges: {}, publications: { "sample:publish": { create_issue: "always-ask" } } }),
			noopTheme,
			(...change) => changes.push(change),
		)[0]!;
		const submenu = command.submenu?.(command.currentValue, vi.fn());
		submenu?.handleInput?.("\r");
		expect(changes).toEqual([]);
		expect(renderText(submenu)).toContain("Keep current policy");
		submenu?.handleInput?.("\x1b[B");
		submenu?.handleInput?.("\r");
		expect(changes).toEqual([["sample:publish", "create_issue", "follow-command"]]);
	});

	it("shows corrupt policy as read-only Always ask instead of an author auto-approve default", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["sample:publish", { ...makeCommandDef("sample:publish"), allowedTools: ["create_issue"] }],
		]);
		state.autonomyRecommendations = new Map([
			["sample", { edges: {}, publications: { "sample:publish": { create_issue: "auto-approve" } } }],
		]);
		const command = buildPublicationCommandItems(
			state,
			() => null,
			noopTheme,
			() => undefined,
			() => true,
		)[0]!;
		expect(command.currentValue).toBe("Always ask (config error)");
		const submenu = command.submenu?.(command.currentValue, vi.fn());
		expect(renderText(submenu)).toContain("Always ask (config error)");
		expect(renderText(submenu)).toContain("safely requires approval");
	});
});

describe("buildCommandItems", () => {
	it("returns sorted items for commands with next policies", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:z-last", makeCommandDef("mach12:z-last", { mode: "forced", target: "mach12:push" })],
			["mach12:a-first", makeCommandDef("mach12:a-first", { mode: "closed", candidates: [{ name: "x" }] })],
			["mach12:no-next", makeCommandDef("mach12:no-next")],
		]);

		const items = buildCommandItems(
			state,
			editorFor(() => null),
			noopTheme,
			() => {},
		);

		expect(items).toHaveLength(2);
		expect(items[0].label).toBe("mach12:a-first");
		expect(items[1].label).toBe("mach12:z-last");
	});

	it("includes description from the command def", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "forced", target: "mach12:push" })],
		]);

		const items = buildCommandItems(
			state,
			editorFor(() => null),
			noopTheme,
			() => {},
		);
		expect(items[0].description).toBe("mach12:cmd desc");
	});

	it("shows override count in currentValue when config has overrides", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }, { name: "b" }] })],
		]);
		const config: AutonomyConfig = { edges: { "mach12:cmd": { a: "chain" } } };

		const items = buildCommandItems(
			state,
			editorFor(() => config),
			noopTheme,
			() => {},
		);
		expect(items[0].currentValue).toContain("1/2 overridden");
	});

	it("includes wildcard-inherited edges in currentValue summaries", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }, { name: "b" }] })],
		]);
		const config: AutonomyConfig = { edges: { "mach12:cmd": { "*": "pause", a: "chain" } } };

		const items = buildCommandItems(
			state,
			editorFor(() => config),
			noopTheme,
			() => {},
		);
		expect(items[0].currentValue).toBe("closed · 1/2 overridden, 1/2 wildcard");
	});

	it("returns empty array when no commands have next policies", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([["mach12:cmd", makeCommandDef("mach12:cmd")]]);

		const items = buildCommandItems(
			state,
			editorFor(() => null),
			noopTheme,
			() => {},
		);
		expect(items).toEqual([]);
	});

	it("each item has a submenu function", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "forced", target: "mach12:push" })],
		]);

		const items = buildCommandItems(
			state,
			editorFor(() => null),
			noopTheme,
			() => {},
		);
		expect(items[0].submenu).toBeTypeOf("function");
	});

	it("reads fresh config when opening an edge submenu", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }] })],
		]);
		let config: AutonomyConfig | null = null;
		const items = buildCommandItems(
			state,
			editorFor(() => config),
			noopTheme,
			() => {},
		);

		config = { edges: { "mach12:cmd": { a: "chain" } } };
		const submenu = items[0].submenu?.("closed · 1 edge", () => {});

		expect(renderText(submenu)).toContain("a  chain");
	});

	it("passes a refreshed command summary when closing an edge submenu", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }, { name: "b" }] })],
		]);
		let config: AutonomyConfig | null = null;
		const items = buildCommandItems(
			state,
			editorFor(() => config),
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
			editorFor(() => null),
			noopTheme,
			() => {},
		);
		const itemsOff = buildTopLevelItems(
			stateOff,
			editorFor(() => null),
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
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "forced", target: "mach12:push" })],
		]);

		const items = buildTopLevelItems(
			state,
			editorFor(() => null),
			noopTheme,
			() => {},
		);
		const autonomy = items.find((i) => i.id === "command-autonomy");
		expect(autonomy).toBeDefined();
		expect(autonomy?.submenu).toBeTypeOf("function");
	});

	it("shows informational item when no commands have next policies", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([["mach12:cmd", makeCommandDef("mach12:cmd")]]);

		const items = buildTopLevelItems(
			state,
			editorFor(() => null),
			noopTheme,
			() => {},
		);
		const autonomy = items.find((i) => i.id === "command-autonomy");
		expect(autonomy).toBeDefined();
		expect(autonomy?.currentValue).toBe("no edges");
		expect(autonomy?.description).toContain("No commands");
		expect(autonomy?.submenu).toBeUndefined();
	});

	it("allows entering command autonomy to choose a scope", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:a", makeCommandDef("mach12:a", { mode: "closed", candidates: [{ name: "mach12:b" }] })],
		]);
		const item = buildTopLevelItems(
			state,
			editorFor(() => null),
			noopTheme,
			() => undefined,
			undefined,
			undefined,
		).find((candidate) => candidate.id === "command-autonomy")!;
		expect(item.currentValue).toBe("all defaults");
		expect(item.submenu).toBeTypeOf("function");
	});

	it("shows override count in command autonomy summary", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }] })],
		]);
		const config: AutonomyConfig = { edges: { "mach12:cmd": { a: "pause" } } };

		const items = buildTopLevelItems(
			state,
			editorFor(() => config),
			noopTheme,
			() => {},
		);
		const autonomy = items.find((i) => i.id === "command-autonomy");
		expect(autonomy?.currentValue).toContain("1 override");
	});

	it("shows 'all defaults' when no overrides configured", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "forced", target: "x" })]]);

		const items = buildTopLevelItems(
			state,
			editorFor(() => null),
			noopTheme,
			() => {},
		);
		const autonomy = items.find((i) => i.id === "command-autonomy");
		expect(autonomy?.currentValue).toBe("all defaults");
	});

	it("reads fresh config when opening the command autonomy submenu", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }] })],
		]);
		let config: AutonomyConfig | null = null;
		const items = buildTopLevelItems(
			state,
			editorFor(() => config),
			noopTheme,
			() => {},
		);
		const autonomy = items.find((i) => i.id === "command-autonomy");

		config = { edges: { "mach12:cmd": { a: "chain" } } };
		const submenu = autonomy?.submenu?.("all defaults", () => {});

		expect(renderText(submenu)).toContain("mach12:cmd  closed · 1/1 overridden");
	});

	it("passes a refreshed registry summary when closing the command autonomy submenu", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:cmd", makeCommandDef("mach12:cmd", { mode: "closed", candidates: [{ name: "a" }] })],
		]);
		let config: AutonomyConfig | null = null;
		const items = buildTopLevelItems(
			state,
			editorFor(() => config),
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

	it("omits publication approval when no top-level command allows a publication tool", () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map([
			["mach12:none", makeCommandDef("mach12:none")],
			[
				"mach12:delegate",
				{ ...makeCommandDef("mach12:delegate"), delegateOnly: true, allowedTools: ["create_issue"] },
			],
		]);
		const items = buildTopLevelItems(
			state,
			editorFor(() => null),
			noopTheme,
			() => {},
			undefined,
			() => {},
		);
		expect(items.some((item) => item.id === "publication-approval")).toBe(false);
	});

	it("enables search only for long publication command lists", () => {
		const build = (count: number) => {
			const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
			state.registry = new Map(
				Array.from({ length: count }, (_, index) => [
					`mach12:publish-${index}`,
					{ ...makeCommandDef(`mach12:publish-${index}`), allowedTools: ["add_pr_comment"] },
				]),
			);
			const items = buildTopLevelItems(
				state,
				editorFor(() => null),
				noopTheme,
				() => {},
				undefined,
				() => {},
			);
			const publication = items.find((item) => item.id === "publication-approval");
			return renderText(publication?.submenu?.("command defaults", () => {}));
		};
		expect(build(10)).not.toContain("Type to search");
		expect(build(11)).toContain("Type to search");
	});
});

describe("buildTopLevelItems — notification preferences", () => {
	it("includes title-indicator toggle with correct default (on)", () => {
		const state = freshState({ preferencesPath: path.join(tmpDir!, "preferences.yaml") });
		const items = buildTopLevelItems(
			state,
			editorFor(() => null),
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
			editorFor(() => null),
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
			editorFor(() => null),
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
			cwd: tmpDir!,
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
			cwd: tmpDir!,
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

	it("friendly Follow command removes only the selected publication override", async () => {
		const configPath = path.join(tmpDir!, "autonomy.yaml");
		saveAutonomyConfig(configPath, {
			edges: {},
			publications: {
				"mach12:publish": { create_issue: "always-ask", add_pr_comment: "auto-approve" },
			},
		});
		const state = freshState({ autonomyConfigPath: configPath });
		state.registry = new Map([
			["mach12:publish", { ...makeCommandDef("mach12:publish"), allowedTools: ["create_issue", "add_pr_comment"] }],
		]);
		state.autonomyRecommendations = new Map([
			[
				"mach12",
				{
					edges: {},
					publications: { "mach12:publish": { create_issue: "require-approval" } },
				},
			],
		]);
		const ctx = {
			cwd: tmpDir!,
			ui: {
				notify: vi.fn(),
				custom: async (
					factory: (tui: unknown, theme: typeof tuiTheme, keybindings: unknown, done: () => void) => Component,
				) => {
					const component = factory({ requestRender: vi.fn() }, tuiTheme, {}, vi.fn());
					for (let index = 0; index < 4; index++) component.handleInput?.("\x1b[B");
					component.handleInput?.("\r");
					component.handleInput?.("\r");
					component.handleInput?.("\r");
				},
			},
		};

		await showSettingsPage({ appendEntry: vi.fn() } as never, ctx as never, state);

		expect(loadAutonomyConfig(configPath)).toEqual({
			edges: {},
			publications: { "mach12:publish": { add_pr_comment: "auto-approve" } },
		});
	});

	it("persists an explicit friendly policy without disturbing sibling settings", async () => {
		const configPath = path.join(tmpDir!, "autonomy.yaml");
		saveAutonomyConfig(configPath, {
			edges: { "mach12:publish": { next: "pause" } },
			publications: { "mach12:publish": { add_pr_comment: "always-ask" } },
		});
		const state = freshState({ autonomyConfigPath: configPath });
		state.registry = new Map([
			["mach12:publish", { ...makeCommandDef("mach12:publish"), allowedTools: ["create_issue", "add_pr_comment"] }],
		]);
		const ctx = {
			cwd: tmpDir!,
			ui: {
				notify: vi.fn(),
				custom: async (
					factory: (tui: unknown, theme: typeof tuiTheme, keybindings: unknown, done: () => void) => Component,
				) => {
					const component = factory({ requestRender: vi.fn() }, tuiTheme, {}, vi.fn());
					for (let index = 0; index < 4; index++) component.handleInput?.("\x1b[B");
					component.handleInput?.("\r");
					component.handleInput?.("\r");
					component.handleInput?.("\r");
					component.handleInput?.("\x1b[B");
					component.handleInput?.("\r");
				},
			},
		};

		await showSettingsPage({ appendEntry: vi.fn() } as never, ctx as never, state);

		expect(loadAutonomyConfig(configPath)).toEqual({
			edges: { "mach12:publish": { next: "pause" } },
			publications: {
				"mach12:publish": { create_issue: "auto-approve", add_pr_comment: "always-ask" },
			},
		});
	});

	it("forwards settings-page focus to the searchable publication list", async () => {
		const state = freshState({ autonomyConfigPath: path.join(tmpDir!, "autonomy.yaml") });
		state.registry = new Map(
			Array.from({ length: 11 }, (_, index) => [
				`mach12:publish-${index}`,
				{ ...makeCommandDef(`mach12:publish-${index}`), allowedTools: ["add_pr_comment"] },
			]),
		);
		let rendered = "";
		const ctx = {
			cwd: tmpDir!,
			ui: {
				notify: vi.fn(),
				custom: async (
					factory: (tui: unknown, theme: typeof tuiTheme, keybindings: unknown, done: () => void) => Component,
				) => {
					const component = factory({ requestRender: vi.fn() }, tuiTheme, {}, vi.fn()) as Component & {
						focused: boolean;
					};
					for (let index = 0; index < 4; index++) component.handleInput?.("\x1b[B");
					component.handleInput?.("\r");
					component.focused = true;
					rendered = component.render(80).join("\n");
				},
			},
		};

		await showSettingsPage({ appendEntry: vi.fn() } as never, ctx as never, state);

		expect(rendered).toContain(CURSOR_MARKER);
	});

	it("does not overwrite corrupt autonomy config from publication settings", async () => {
		const configPath = path.join(tmpDir!, "autonomy.yaml");
		const original = "{ invalid: [";
		fs.writeFileSync(configPath, original);
		resetCache();
		const state = freshState({ autonomyConfigPath: configPath });
		state.registry = new Map([["mach12:cmd", { ...makeCommandDef("mach12:cmd"), allowedTools: ["create_issue"] }]]);
		const notify = vi.fn();
		const ctx = {
			cwd: tmpDir!,
			ui: {
				notify,
				custom: async (
					factory: (tui: unknown, theme: typeof tuiTheme, keybindings: unknown, done: () => void) => Component,
				) => {
					const component = factory({ requestRender: vi.fn() }, tuiTheme, {}, vi.fn());
					for (let index = 0; index < 4; index++) component.handleInput?.("\x1b[B");
					component.handleInput?.("\r");
					component.handleInput?.("\r");
					component.handleInput?.("\r");
				},
			},
		};

		await showSettingsPage({ appendEntry: vi.fn() } as never, ctx as never, state);

		expect(fs.readFileSync(configPath, "utf-8")).toBe(original);
		expect(notify.mock.calls.some(([message]) => String(message).includes("approval is required"))).toBe(true);
		expect(fs.existsSync(`${configPath}.tmp`)).toBe(false);
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
			cwd: tmpDir!,
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
					component.handleInput?.("\r"); // select Global
					component.handleInput?.("\x1b[B");
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
			"global",
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
			"global",
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
			"global",
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
			"global",
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
			"global",
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
			"global",
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
			"global",
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
			"global",
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
			"global",
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
			"global",
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
			"global",
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
			"global",
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
			"global",
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
			"global",
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

describe("scoped transition editing", () => {
	const policy: NextStepPolicy = { mode: "closed", candidates: [{ name: "next" }] };
	function setup() {
		const globalPath = path.join(tmpDir!, "global.yaml");
		const projectPath = projectConfigPath(tmpDir!);
		const state = freshState({
			autonomyConfigPath: globalPath,
			preferencesPath: path.join(tmpDir!, "preferences.yaml"),
		});
		state.registry = new Map([
			["sample:work", { ...makeCommandDef("sample:work", policy), allowedTools: ["create_issue"] }],
		]);
		const editor: TransitionEditor = {
			scope: "project",
			snapshot: () => loadScopedAutonomyConfig(globalPath, tmpDir!),
		};
		return { globalPath, projectPath, state, editor };
	}
	async function exercise(state: ReturnType<typeof freshState>, action: (component: Component) => void) {
		const notify = vi.fn();
		await showSettingsPage(
			{ appendEntry: vi.fn() } as never,
			{
				cwd: tmpDir!,
				ui: {
					notify,
					custom: async (factory: any) => {
						const component = factory({ requestRender: vi.fn() }, tuiTheme, {}, vi.fn());
						action(component);
					},
				},
			} as never,
			state,
		);
		return notify;
	}
	function keys(component: Component, ...input: string[]) {
		for (const key of input) {
			component.handleInput?.(key);
			component.render(120);
		}
	}
	function openAutonomy(component: Component) {
		keys(component, "\x1b[B", "\x1b[B", "\x1b[B", "\r");
	}
	it("displays raw inherited selection separately from scope-first provenance and masked Global edits", () => {
		const { globalPath, projectPath, editor } = setup();
		saveAutonomyConfig(globalPath, { edges: { "sample:work": { next: "chain" } } });
		saveAutonomyConfig(projectPath, { edges: { "sample:work": { "*": "pause" } } }, "project");
		const inherited = buildEdgeItems("sample:work", policy, editor)[0];
		expect(inherited.currentValue).toBe("inherit");
		expect(inherited.values).toEqual(["inherit", "chain", "pause"]);
		expect(inherited.description).toContain("none (inherited)");
		expect(inherited.description).toContain(`pause from project wildcard (${projectPath})`);
		editor.scope = "global";
		const masked = buildEdgeItems("sample:work", policy, editor)[0];
		expect(masked.currentValue).toBe("chain");
		expect(masked.description).toContain("project setting masks Global edits");
	});
	it("defaults to Project, saves and resets only its exact key, then deliberately edits Global", async () => {
		const { globalPath, projectPath, state } = setup();
		saveAutonomyConfig(globalPath, { edges: { "sample:work": { next: "chain" } } });
		saveAutonomyConfig(projectPath, { edges: { "sample:work": { "*": "pause" } } }, "project");
		const originalGlobal = fs.readFileSync(globalPath, "utf8");
		await exercise(state, (component) => {
			openAutonomy(component);
			expect(renderText(component)).toContain("Edit scope");
			expect(renderText(component)).toContain("Project");
			expect(renderText(component)).toContain(projectPath);
			keys(component, "\x1b[B", "\r");
			expect(renderText(component)).toContain("none (inherited)");
			keys(component, "\r");
			expect(loadAutonomyConfig(projectPath, true, "project")?.edges["sample:work"]).toEqual({
				"*": "pause",
				next: "chain",
			});
			expect(renderText(component)).toContain("project exact");
			keys(component, "\r", "\r");
			expect(loadAutonomyConfig(projectPath, true, "project")?.edges["sample:work"]).toEqual({ "*": "pause" });
			expect(renderText(component)).toContain("project wildcard");
			expect(fs.readFileSync(globalPath, "utf8")).toBe(originalGlobal);
			keys(component, "\x1b", "\x1b[A", "\r");
			expect(renderText(component)).toContain(globalPath);
			keys(component, "\x1b[B", "\r", "\r");
			expect(renderText(component)).toContain("project setting masks Global edits");
		});
		expect(loadAutonomyConfig(globalPath)?.edges["sample:work"].next).toBe("pause");
		expect(loadAutonomyConfig(projectPath, true, "project")?.edges["sample:work"]).toEqual({ "*": "pause" });
	});
	it("reset reveals global fallback, then ordinary autopilot when Global exact is removed", () => {
		const { globalPath, editor } = setup();
		saveAutonomyConfig(globalPath, { edges: { "sample:work": { next: "pause" } } });
		expect(buildEdgeItems("sample:work", policy, editor)[0].description).toContain("global exact");
		fs.unlinkSync(globalPath);
		expect(buildEdgeItems("sample:work", policy, editor)[0].description).toContain("ordinary autopilot behavior");
	});
	it("restores persisted raw selection and reports failed writes without overwriting corruption", async () => {
		const { projectPath, state } = setup();
		saveAutonomyConfig(projectPath, { edges: { "sample:work": { next: "pause" } } }, "project");
		const notify = await exercise(state, (component) => {
			openAutonomy(component);
			keys(component, "\x1b[B", "\r");
			fs.writeFileSync(projectPath, "edges: [broken]");
			keys(component, "\r");
			expect(renderText(component)).toContain("inherit");
			expect(renderText(component)).toContain("Cannot edit");
			expect(renderText(component)).toContain("all overrides disabled");
		});
		expect(notify.mock.calls.some(([message]) => message.includes("Failed to save"))).toBe(true);
		expect(fs.readFileSync(projectPath, "utf8")).toBe("edges: [broken]");
	});
	it("preserves a valid raw selection on lock failure", async () => {
		const { projectPath, state } = setup();
		saveAutonomyConfig(projectPath, { edges: { "sample:work": { next: "pause" } } }, "project");
		const notify = await exercise(state, (component) => {
			openAutonomy(component);
			keys(component, "\x1b[B", "\r");
			fs.mkdirSync(`${projectPath}.lock`);
			keys(component, "\r");
			expect(renderText(component).split("\n")[0]).toContain("pause");
		});
		expect(notify.mock.calls.some(([message]) => message.includes("Failed to save"))).toBe(true);
		expect(loadAutonomyConfig(projectPath, true, "project")?.edges["sample:work"].next).toBe("pause");
	});
	it("edits valid Project raw settings while corrupt Global disables effective overrides and publication editing", async () => {
		const { projectPath, globalPath, state } = setup();
		fs.writeFileSync(globalPath, "{ broken: [");
		await exercise(state, (component) => {
			openAutonomy(component);
			keys(component, "\x1b[B", "\r", "\r");
			expect(renderText(component)).toContain("Stored project exact: chain");
			expect(renderText(component)).toContain("all overrides disabled");
			keys(component, "\x1b", "\x1b", "\x1b[B", "\r", "\r");
			expect(renderText(component)).toContain("Always ask (config error)");
		});
		expect(loadAutonomyConfig(projectPath, true, "project")?.edges["sample:work"].next).toBe("chain");
		expect(fs.readFileSync(globalPath, "utf8")).toBe("{ broken: [");
	});

	it("keeps valid Global publication editing available with a malformed Project file", async () => {
		const { projectPath, globalPath, state } = setup();
		saveAutonomyConfig(globalPath, { edges: {}, publications: { "sample:work": { create_issue: "always-ask" } } });
		fs.mkdirSync(path.dirname(projectPath), { recursive: true });
		fs.writeFileSync(projectPath, "publications: {}");
		await exercise(state, (component) => {
			openAutonomy(component);
			keys(component, "\x1b[B", "\r");
			expect(renderText(component)).toContain("all overrides disabled");
			expect(renderText(component)).toContain("Cannot edit");
			keys(component, "\x1b", "\x1b", "\x1b[B", "\r", "\r");
			expect(renderText(component)).toContain("Always ask");
			expect(renderText(component)).not.toContain("config error");
			keys(component, "\r");
		});
		expect(loadAutonomyConfig(globalPath)?.publications).toBeUndefined();
		expect(fs.readFileSync(projectPath, "utf8")).toBe("publications: {}");
	});
	it.each(["project", "global"] as const)(
		"previews and gap-fills recommendations only in selected %s file with fresh exact protection",
		async (scope) => {
			const { globalPath, projectPath, state } = setup();
			state.autonomyRecommendations = new Map([
				[
					"sample",
					{
						edges: { "sample:work": { next: "chain", other: "pause" } },
						publications: { "sample:work": { create_issue: "auto-approve" } },
					},
				],
			]);
			saveAutonomyConfig(globalPath, {
				edges: { "sample:work": { "*": "pause" } },
				publications: { "sample:work": { create_issue: "always-ask" } },
			});
			const destination = scope === "project" ? projectPath : globalPath;
			const otherPath = scope === "project" ? globalPath : projectPath;
			const otherOriginal = fs.existsSync(otherPath) ? fs.readFileSync(otherPath, "utf8") : null;
			await exercise(state, (component) => {
				openAutonomy(component);
				if (scope === "global") keys(component, "\r");
				keys(component, "\x1b[B");
				expect(renderText(component)).toContain(destination);
				keys(component, "\r");
				expect(renderText(component)).toContain("override inherited");
				if (scope === "project") expect(fs.existsSync(destination)).toBe(false);
				const current = loadAutonomyConfig(destination, true, scope) ?? { edges: {} };
				current.edges["sample:work"] ??= {};
				current.edges["sample:work"].next = "pause";
				saveAutonomyConfig(destination, current, scope);
				keys(component, "\x1b[B", "\x1b[B");
				expect(renderText(component)).toContain(`to ${scope}`);
				expect(renderText(component)).toContain(destination);
				keys(component, "\r");
				expect(renderText(component)).toContain("all applied");
			});
			const saved = loadAutonomyConfig(destination, true, scope)!;
			expect(saved.edges["sample:work"]).toMatchObject({ next: "pause", other: "pause" });
			expect(saved.publications).toEqual(
				scope === "global" ? { "sample:work": { create_issue: "always-ask" } } : undefined,
			);
			expect(fs.existsSync(otherPath) ? fs.readFileSync(otherPath, "utf8") : null).toBe(otherOriginal);
		},
	);
	it("Escape from recommendation preview and scope switching saves nothing at narrow width", async () => {
		const { state, globalPath, projectPath } = setup();
		state.autonomyRecommendations = new Map([["sample", { edges: { "sample:work": { next: "chain" } } }]]);
		await exercise(state, (component) => {
			openAutonomy(component);
			expect(component.render(32).join("\n")).toContain("Project");
			keys(component, "\r");
			expect(component.render(32).join("\n")).toContain("Global");
			keys(component, "\x1b[B", "\r", "\x1b", "\x1b", "\x1b");
		});
		expect(fs.existsSync(globalPath)).toBe(false);
		expect(fs.existsSync(projectPath)).toBe(false);
	});
});
