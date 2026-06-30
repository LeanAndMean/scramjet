import type { Theme } from "@leanandmean/coding-agent";
import { visibleWidth } from "@leanandmean/tui";
import { beforeEach, describe, expect, it } from "vitest";
import { registerDiagramTool } from "../src/diagram/diagram-tool.js";

function recordingPi() {
	const tools: any[] = [];
	const pi: any = {
		registerTool(tool: any) {
			tools.push(tool);
		},
	};
	return { pi, tools };
}

function mockTheme(): Theme {
	return {
		fg(_color: string, text: string) {
			return `\x1b[38;5;1m${text}\x1b[39m`;
		},
	} as unknown as Theme;
}

function mockContext(lastComponent?: any) {
	return {
		lastComponent,
		args: {},
		toolCallId: "tc-1",
		invalidate() {},
		cwd: "/",
		executionStarted: true,
		argsComplete: true,
		isPartial: false,
		expanded: true,
		showImages: false,
		isError: false,
	};
}

describe("registerDiagramTool", () => {
	it("registers exactly one tool", () => {
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		expect(tools).toHaveLength(1);
		expect(tools[0].name).toBe("draw_diagram");
	});

	it("tool has correct parameter schema — source required, title optional", () => {
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		const params = tools[0].parameters;
		expect(params.properties.source).toBeDefined();
		expect(params.properties.title).toBeDefined();
		expect(params.required).toContain("source");
		expect(params.required).not.toContain("title");
	});

	it("description mentions supported diagram types", () => {
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		expect(tools[0].description).toContain("flowchart");
		expect(tools[0].description).toContain("stateDiagram-v2");
	});

	it("promptSnippet mentions Mermaid syntax and supported types", () => {
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		expect(tools[0].promptSnippet).toMatch(/[Mm]ermaid/);
		expect(tools[0].promptSnippet).toContain("flowchart");
		expect(tools[0].promptSnippet).toContain("stateDiagram-v2");
	});
});

describe("execute", () => {
	let tool: any;

	beforeEach(() => {
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		tool = tools[0];
	});

	it("returns text content for valid flowchart, no ANSI", async () => {
		const result = await tool.execute("call-1", { source: "graph LR\n  A --> B" });
		expect(result.content).toHaveLength(1);
		expect(result.content[0].type).toBe("text");
		expect(result.content[0].text).toContain("A");
		expect(result.content[0].text).toContain("B");
		expect(result.content[0].text).not.toMatch(/\x1b\[/);
	});

	it("output width does not exceed MAX_WIDTH (120)", async () => {
		const result = await tool.execute("call-1", { source: "graph TD\n  A --> B\n  B --> C\n  C --> D" });
		for (const line of result.content[0].text.split("\n")) {
			expect(visibleWidth(line)).toBeLessThanOrEqual(120);
		}
	});

	it("unsupported type sequenceDiagram → error", async () => {
		await expect(tool.execute("call-1", { source: "sequenceDiagram\n  A->>B: hi" })).rejects.toThrow(
			/unsupported diagram type/i,
		);
	});

	it("unsupported type classDiagram → error", async () => {
		await expect(tool.execute("call-1", { source: "classDiagram\n  class Animal" })).rejects.toThrow(
			/unsupported diagram type/i,
		);
	});

	it("unsupported type erDiagram → error", async () => {
		await expect(tool.execute("call-1", { source: "erDiagram\n  CUSTOMER ||--o{ ORDER : places" })).rejects.toThrow(
			/unsupported diagram type/i,
		);
	});

	it("unsupported type xychart-beta → error", async () => {
		await expect(tool.execute("call-1", { source: "xychart-beta\n  title Sales" })).rejects.toThrow(
			/unsupported diagram type/i,
		);
	});

	it("unsupported type error lists supported types", async () => {
		await expect(tool.execute("call-1", { source: "gantt\n  title A" })).rejects.toThrow(
			/supported types.*flowchart.*graph.*stateDiagram-v2/i,
		);
	});

	it("invalid header → classified as unsupported type", async () => {
		await expect(tool.execute("call-1", { source: "pie\n  title Budget" })).rejects.toThrow(
			/unsupported diagram type.*pie/i,
		);
	});

	it("empty source → classified error", async () => {
		await expect(tool.execute("call-1", { source: "" })).rejects.toThrow(/empty diagram source/i);
	});

	it("whitespace-only source → classified error", async () => {
		await expect(tool.execute("call-1", { source: "   \n\n  " })).rejects.toThrow(/empty diagram source/i);
	});

	it("includes title in content when provided", async () => {
		const result = await tool.execute("call-1", { source: "graph LR\n  A --> B", title: "My Diagram" });
		expect(result.content[0].text).toMatch(/^My Diagram\n/);
	});

	it("does not include title prefix when title is omitted", async () => {
		const result = await tool.execute("call-1", { source: "graph LR\n  A --> B" });
		expect(result.content[0].text).not.toMatch(/^My Diagram/);
	});

	it("stores source, title, and tier in details", async () => {
		const source = "graph LR\n  A --> B";
		const result = await tool.execute("call-1", { source, title: "Flow" });
		expect(result.details).toMatchObject({ source, title: "Flow" });
		expect(typeof result.details.tier).toBe("number");
	});

	it("too-wide diagram → width error with guidance", async () => {
		const nodes = Array.from({ length: 8 }, (_, i) => `N${i}[ThisIsAVeryLongNodeLabelThatForcesWidth${i}]`).join(
			"\n  ",
		);
		const edges = Array.from({ length: 7 }, (_, i) => `N${i} --> N${i + 1}`).join("\n  ");
		const source = `graph LR\n  ${nodes}\n  ${edges}`;
		await expect(tool.execute("call-1", { source })).rejects.toThrow(/too wide.*max 120.*simplify/i);
	});
});

describe("renderResult", () => {
	let tool: any;

	beforeEach(() => {
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		tool = tools[0];
	});

	it("returns a DiagramComponent when details.source is present", () => {
		const component = tool.renderResult(
			{ details: { source: "graph LR\n  A --> B", tier: 0 }, content: [] },
			{ expanded: true, isPartial: false },
			mockTheme(),
			mockContext(),
		);
		expect(component).toBeDefined();
		expect(typeof component.render).toBe("function");
		expect(typeof component.invalidate).toBe("function");
	});

	it("returns a PlainTextComponent when no details", () => {
		const component = tool.renderResult(
			{ content: [{ type: "text", text: "fallback text" }] },
			{ expanded: true, isPartial: false },
			undefined,
			mockContext(),
		);
		expect(component.render(80)).toEqual(["fallback text"]);
	});

	it("PlainTextComponent shows '[No diagram]' when content is empty", () => {
		const component = tool.renderResult(
			{ content: [] },
			{ expanded: true, isPartial: false },
			undefined,
			mockContext(),
		);
		expect(component.render(80)).toEqual(["[No diagram]"]);
	});

	it("reuses context.lastComponent when source matches", () => {
		const source = "graph LR\n  A --> B";
		const first = tool.renderResult(
			{ details: { source, tier: 0 }, content: [] },
			{ expanded: true, isPartial: false },
			mockTheme(),
			mockContext(),
		);
		const second = tool.renderResult(
			{ details: { source, tier: 0 }, content: [] },
			{ expanded: true, isPartial: false },
			mockTheme(),
			mockContext(first),
		);
		expect(second).toBe(first);
	});
});

describe("DiagramComponent", () => {
	let tool: any;

	beforeEach(() => {
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		tool = tools[0];
	});

	it("renders with theme coloring (ANSI in output)", () => {
		const component = tool.renderResult(
			{ details: { source: "graph LR\n  A --> B", tier: 0 }, content: [] },
			{ expanded: true, isPartial: false },
			mockTheme(),
			mockContext(),
		);
		const lines = component.render(120);
		const text = lines.join("\n");
		expect(text).toMatch(/\x1b\[/);
	});

	it("renders without ANSI when theme not provided", () => {
		const component = tool.renderResult(
			{ details: { source: "graph LR\n  A --> B", tier: 0 }, content: [] },
			{ expanded: true, isPartial: false },
			undefined,
			mockContext(),
		);
		const lines = component.render(120);
		const text = lines.join("\n");
		expect(text).not.toMatch(/\x1b\[/);
		expect(text).toContain("A");
		expect(text).toContain("B");
	});

	it("output lines do not exceed render width (visibleWidth)", () => {
		const component = tool.renderResult(
			{ details: { source: "graph TD\n  A --> B\n  B --> C", tier: 0 }, content: [] },
			{ expanded: true, isPartial: false },
			mockTheme(),
			mockContext(),
		);
		const lines = component.render(80);
		for (const line of lines) {
			expect(visibleWidth(line)).toBeLessThanOrEqual(80);
		}
	});

	it("includes title when provided", () => {
		const component = tool.renderResult(
			{ details: { source: "graph LR\n  A --> B", title: "Architecture", tier: 0 }, content: [] },
			{ expanded: true, isPartial: false },
			undefined,
			mockContext(),
		);
		const lines = component.render(120);
		expect(lines[0]).toBe("Architecture");
		expect(lines[1]).toBe("");
	});

	it("truncates title that exceeds render width", () => {
		const longTitle = "A".repeat(200);
		const component = tool.renderResult(
			{ details: { source: "graph LR\n  A --> B", title: longTitle, tier: 0 }, content: [] },
			{ expanded: true, isPartial: false },
			undefined,
			mockContext(),
		);
		const lines = component.render(80);
		expect(visibleWidth(lines[0])).toBeLessThanOrEqual(80);
	});

	it("returns error message on render failure without throwing", () => {
		const component = tool.renderResult(
			{ details: { source: "totally broken {{{{", tier: 0 }, content: [] },
			{ expanded: true, isPartial: false },
			undefined,
			mockContext(),
		);
		const lines = component.render(80);
		expect(lines).toHaveLength(1);
		expect(lines[0]).toContain("Diagram render error");
	});

	it("cache hit: same (source, width) does not re-render", () => {
		const component = tool.renderResult(
			{ details: { source: "graph LR\n  A --> B", tier: 0 }, content: [] },
			{ expanded: true, isPartial: false },
			undefined,
			mockContext(),
		);
		const lines1 = component.render(120);
		const lines2 = component.render(120);
		expect(lines1).toBe(lines2);
	});

	it("cache invalidated on invalidate()", () => {
		const component = tool.renderResult(
			{ details: { source: "graph LR\n  A --> B", tier: 0 }, content: [] },
			{ expanded: true, isPartial: false },
			undefined,
			mockContext(),
		);
		const lines1 = component.render(120);
		component.invalidate();
		const lines2 = component.render(120);
		expect(lines1).not.toBe(lines2);
		expect(lines1).toEqual(lines2);
	});
});

describe("integration", () => {
	let tool: any;

	beforeEach(() => {
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		tool = tools[0];
	});

	it("10-node flowchart renders with all labels readable", async () => {
		const source = [
			"flowchart TD",
			"  Start[Start] --> Auth[Authentication]",
			"  Auth --> Valid{Valid?}",
			"  Valid -->|yes| Process[Process]",
			"  Valid -->|no| Error[Error]",
			"  Process --> Cache[Cache]",
			"  Cache --> Store[Store]",
			"  Store --> Notify[Notify]",
			"  Notify --> Log[Log]",
			"  Log --> End[End]",
		].join("\n");
		const result = await tool.execute("call-1", { source });
		const text = result.content[0].text;
		for (const label of [
			"Start",
			"Authentication",
			"Valid?",
			"Process",
			"Error",
			"Cache",
			"Store",
			"Notify",
			"Log",
			"End",
		]) {
			expect(text).toContain(label);
		}
		for (const line of text.split("\n")) {
			expect(visibleWidth(line)).toBeLessThanOrEqual(120);
		}
	});

	it("openn junction bug is fixed — no 'openn' in output", async () => {
		const source = [
			"flowchart TD",
			"    A[NodeA]",
			"    B[NodeB]",
			"    C[NodeC]",
			"    A -->|open| B",
			"    A -->|open| C",
			"    B -->|open| B",
			"    B -->|open| C",
		].join("\n");
		const result = await tool.execute("call-1", { source });
		for (const line of result.content[0].text.split("\n")) {
			expect(line).not.toContain("openn");
		}
	});

	it("stateDiagram-v2 renders successfully", async () => {
		const source = [
			"stateDiagram-v2",
			"  [*] --> Idle",
			"  Idle --> Running",
			"  Running --> Idle",
			"  Running --> [*]",
		].join("\n");
		const result = await tool.execute("call-1", { source });
		expect(result.content[0].text).toContain("Idle");
		expect(result.content[0].text).toContain("Running");
	});

	it("self-loop renders without crash", async () => {
		const source = "graph TD\n  A[Loop] -->|repeat| A";
		const result = await tool.execute("call-1", { source });
		expect(result.content[0].text).toContain("Loop");
	});
});
