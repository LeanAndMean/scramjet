import { describe, expect, it } from "vitest";
import { buildCommandCatalogBlock, registerCommandCatalog } from "../src/command-catalog.js";
import type { CommandDef, CommandRegistry } from "../src/types.js";
import { freshState, recordingPi } from "./helpers.js";

function makeDef(name: string, opts: Partial<CommandDef> = {}): CommandDef {
	return { name, filePath: `/commands/${name}.md`, body: "Body.", ...opts };
}

function makeRegistry(...defs: CommandDef[]): CommandRegistry {
	return new Map(defs.map((d) => [d.name, d]));
}

describe("buildCommandCatalogBlock", () => {
	it("returns empty string for empty registry", () => {
		expect(buildCommandCatalogBlock(new Map())).toBe("");
	});

	it("returns empty string when all commands are delegate-only", () => {
		const registry = makeRegistry(
			makeDef("mach12:push", { delegateOnly: true, description: "Push" }),
			makeDef("mach12:gh-assign", { delegateOnly: true, description: "Assign" }),
		);
		expect(buildCommandCatalogBlock(registry)).toBe("");
	});

	it("formats commands with description and argument-hint", () => {
		const registry = makeRegistry(
			makeDef("mach12:issue-plan", {
				description: "Create a staged implementation plan",
				argumentHint: "<issue-number> [context]",
			}),
		);
		const block = buildCommandCatalogBlock(registry);
		expect(block).toContain("# Available commands");
		expect(block).toContain("- /mach12:issue-plan <issue-number> [context]: Create a staged implementation plan");
	});

	it("formats commands without argument-hint (name only)", () => {
		const registry = makeRegistry(makeDef("mach12:pr-merge", { description: "Merge the PR" }));
		const block = buildCommandCatalogBlock(registry);
		expect(block).toContain("- /mach12:pr-merge: Merge the PR");
	});

	it("formats commands without description (name and hint only)", () => {
		const registry = makeRegistry(makeDef("mach12:bare", { argumentHint: "<arg>" }));
		const block = buildCommandCatalogBlock(registry);
		expect(block).toContain("- /mach12:bare <arg>");
		expect(block).not.toContain("- /mach12:bare <arg>:");
	});

	it("formats commands with neither description nor argument-hint", () => {
		const registry = makeRegistry(makeDef("mach12:minimal"));
		const block = buildCommandCatalogBlock(registry);
		expect(block).toContain("- /mach12:minimal");
		expect(block).not.toContain("- /mach12:minimal:");
	});

	it("filters out delegate-only commands", () => {
		const registry = makeRegistry(
			makeDef("mach12:issue-plan", { description: "Plan" }),
			makeDef("mach12:push", { delegateOnly: true, description: "Push" }),
			makeDef("mach12:pr-create", { description: "Create PR" }),
		);
		const block = buildCommandCatalogBlock(registry);
		expect(block).toContain("mach12:issue-plan");
		expect(block).toContain("mach12:pr-create");
		expect(block).not.toContain("mach12:push");
	});

	it("sorts commands alphabetically", () => {
		const registry = makeRegistry(
			makeDef("mach12:pr-merge", { description: "Merge" }),
			makeDef("mach12:issue-plan", { description: "Plan" }),
			makeDef("infra:rotate", { description: "Rotate keys" }),
		);
		const block = buildCommandCatalogBlock(registry);
		const lines = block.split("\n").filter((l) => l.startsWith("- /"));
		expect(lines[0]).toContain("infra:rotate");
		expect(lines[1]).toContain("mach12:issue-plan");
		expect(lines[2]).toContain("mach12:pr-merge");
	});
});

describe("registerCommandCatalog", () => {
	it("registers exactly one before_agent_start handler", () => {
		const { pi, handlers, tools } = recordingPi();
		const state = freshState();
		registerCommandCatalog(pi, state);
		expect(handlers.get("before_agent_start")).toHaveLength(1);
		expect(tools).toHaveLength(0);
	});

	it("returns catalog as a system prompt section", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState({
			registry: makeRegistry(makeDef("mach12:issue-plan", { description: "Plan issues" })),
		});
		registerCommandCatalog(pi, state);
		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as {
			systemPromptSection: { id: string; text: string };
		};
		expect(result.systemPromptSection.id).toBe("scramjet:command-catalog");
		expect(result.systemPromptSection.text).toMatch(/^\n\n# Available commands/);
		expect(result.systemPromptSection.text).toContain("- /mach12:issue-plan: Plan issues");
	});

	it("returns only systemPromptSection, no systemPrompt or message", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState({
			registry: makeRegistry(makeDef("mach12:issue-plan", { description: "Plan" })),
		});
		registerCommandCatalog(pi, state);
		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as Record<string, unknown>;
		expect(result).toHaveProperty("systemPromptSection");
		expect(result).not.toHaveProperty("systemPrompt");
		expect(result).not.toHaveProperty("message");
	});

	it("returns empty object when registry is empty", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerCommandCatalog(pi, state);
		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as Record<string, unknown>;
		expect(result).toEqual({});
	});

	it("returns empty object when all commands are delegate-only", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState({
			registry: makeRegistry(makeDef("mach12:push", { delegateOnly: true, description: "Push" })),
		});
		registerCommandCatalog(pi, state);
		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as Record<string, unknown>;
		expect(result).toEqual({});
	});

	it("reads registry at call time (not closure-captured)", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerCommandCatalog(pi, state);
		const handler = handlers.get("before_agent_start")![0];

		// Initially empty
		let result = (await handler({ systemPrompt: "BASE" })) as Record<string, unknown>;
		expect(result).toEqual({});

		// Mutate registry (simulating resources_discover reassignment)
		state.registry = makeRegistry(makeDef("mach12:new-cmd", { description: "New" }));
		result = (await handler({ systemPrompt: "BASE" })) as { systemPromptSection: { text: string } };
		expect((result as any).systemPromptSection.text).toContain("mach12:new-cmd");
	});

	it("is flag-independent (works regardless of enabled state)", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState({
			enabled: false,
			registry: makeRegistry(makeDef("mach12:issue-plan", { description: "Plan" })),
		});
		registerCommandCatalog(pi, state);
		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as { systemPromptSection: { id: string } };
		expect(result.systemPromptSection.id).toBe("scramjet:command-catalog");
	});
});
