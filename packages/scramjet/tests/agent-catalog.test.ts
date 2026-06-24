import { describe, expect, it } from "vitest";
import { buildAgentCatalogBlock, registerAgentCatalog } from "../src/agent-catalog.js";
import type { AgentRegistry } from "../src/types.js";
import { freshState, recordingPi } from "./helpers.js";

function makeRegistry(...entries: [string, string?][]): AgentRegistry {
	return new Map(entries.map(([name, description]) => [name, { name, filePath: `/agents/${name}.md`, description }]));
}

describe("buildAgentCatalogBlock", () => {
	it("returns empty string for empty registry", () => {
		expect(buildAgentCatalogBlock(new Map())).toBe("");
	});

	it("returns formatted block with names and descriptions", () => {
		const registry = makeRegistry(["mach12:code-explorer", "Explores code"], ["mach12:code-architect", "Designs"]);
		const block = buildAgentCatalogBlock(registry);
		expect(block).toContain("# Available subagents");
		expect(block).toContain("- mach12:code-architect: Designs");
		expect(block).toContain("- mach12:code-explorer: Explores code");
	});

	it("includes name only when description is missing", () => {
		const registry = makeRegistry(["my-agent"]);
		const block = buildAgentCatalogBlock(registry);
		expect(block).toContain("- my-agent");
		expect(block).not.toContain("- my-agent:");
	});

	it("sorts agents alphabetically", () => {
		const registry = makeRegistry(["zebra", "desc z"], ["alpha", "desc a"], ["middle", "desc m"]);
		const block = buildAgentCatalogBlock(registry);
		const lines = block.split("\n").filter((l) => l.startsWith("- "));
		expect(lines[0]).toContain("alpha");
		expect(lines[1]).toContain("middle");
		expect(lines[2]).toContain("zebra");
	});
});

describe("registerAgentCatalog", () => {
	it("registers exactly one before_agent_start handler", () => {
		const { pi, handlers, tools } = recordingPi();
		const state = freshState();
		registerAgentCatalog(pi, state);
		expect(handlers.get("before_agent_start")).toHaveLength(1);
		expect(tools).toHaveLength(0);
	});

	it("returns catalog as a cache-aware system prompt section", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState({
			agentRegistry: makeRegistry(["test-agent", "Does testing"]),
		});
		registerAgentCatalog(pi, state);
		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as {
			systemPromptSection: { id: string; text: string };
			systemPrompt?: unknown;
		};
		expect(result.systemPromptSection.id).toBe("scramjet:agent-catalog");
		expect(result.systemPromptSection.text).toMatch(/^\n\n# Available subagents/);
		expect(result.systemPromptSection.text).toContain("- test-agent: Does testing");
		expect(result.systemPromptSection.text).not.toContain("BASE");
		expect(result.systemPrompt).toBeUndefined();
	});

	it("returns only systemPromptSection, no systemPrompt or message", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState({
			agentRegistry: makeRegistry(["a", "desc"]),
		});
		registerAgentCatalog(pi, state);
		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as Record<string, unknown>;
		expect(result).toHaveProperty("systemPromptSection");
		expect(result).not.toHaveProperty("systemPrompt");
		expect(result).not.toHaveProperty("message");
	});

	it("returns empty object when registry is empty", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerAgentCatalog(pi, state);
		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({ systemPrompt: "BASE" })) as Record<string, unknown>;
		expect(result).toEqual({});
	});
});
