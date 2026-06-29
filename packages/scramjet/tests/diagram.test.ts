import { visibleWidth } from "@leanandmean/tui";
import type { AsciiRenderOptions } from "beautiful-mermaid";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mockRenderMermaidASCII = vi.fn<(text: string, options?: AsciiRenderOptions) => string>();

vi.mock("beautiful-mermaid", () => ({
	renderMermaidASCII: (...args: Parameters<typeof mockRenderMermaidASCII>) => mockRenderMermaidASCII(...args),
}));

const { registerDiagramTool } = await import("../src/diagram/diagram-tool.js");

function recordingPi() {
	const tools: any[] = [];
	const pi: any = {
		registerTool(tool: any) {
			tools.push(tool);
		},
	};
	return { pi, tools };
}

function narrowOutput(width: number): string {
	return Array.from({ length: 5 }, () => "x".repeat(width)).join("\n");
}

describe("registerDiagramTool", () => {
	it("registers exactly one tool unconditionally", () => {
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		expect(tools).toHaveLength(1);
		expect(tools[0].name).toBe("draw_diagram");
	});

	it("tool has correct parameter schema — source required, title optional, no format", () => {
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		const params = tools[0].parameters;
		expect(params.properties.source).toBeDefined();
		expect(params.properties.title).toBeDefined();
		expect(params.properties.format).toBeUndefined();
		expect(params.required).toContain("source");
		expect(params.required).not.toContain("title");
	});

	it("description mentions supported diagram types", () => {
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		expect(tools[0].description).toContain("flowchart");
		expect(tools[0].description).toContain("sequenceDiagram");
	});

	it("promptSnippet mentions Mermaid syntax", () => {
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		expect(tools[0].promptSnippet).toMatch(/[Mm]ermaid/);
	});
});

describe("execute", () => {
	let tool: any;

	beforeEach(() => {
		mockRenderMermaidASCII.mockReset();
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		tool = tools[0];
	});

	it("returns text content when diagram fits at spacious tier", async () => {
		mockRenderMermaidASCII.mockReturnValue(narrowOutput(80));
		const result = await tool.execute("call-1", { source: "graph LR; A-->B" }, undefined, undefined, undefined);
		expect(result.content).toHaveLength(1);
		expect(result.content[0].type).toBe("text");
		expect(result.content[0].text).toContain("x".repeat(80));
		expect(mockRenderMermaidASCII).toHaveBeenCalledTimes(1);
		expect(mockRenderMermaidASCII.mock.calls[0][1]).toMatchObject({ colorMode: "none", paddingX: 5 });
	});

	it("compacts to tier 2 when tier 1 exceeds MAX_WIDTH", async () => {
		mockRenderMermaidASCII
			.mockReturnValueOnce(narrowOutput(130)) // tier 1 too wide
			.mockReturnValueOnce(narrowOutput(100)); // tier 2 fits
		const result = await tool.execute("call-1", { source: "graph LR; A-->B" }, undefined, undefined, undefined);
		expect(mockRenderMermaidASCII).toHaveBeenCalledTimes(2);
		expect(mockRenderMermaidASCII.mock.calls[1][1]).toMatchObject({ colorMode: "none", paddingX: 3 });
		expect(result.content[0].text).toContain("x".repeat(100));
	});

	it("compacts to tier 3 when tiers 1-2 exceed MAX_WIDTH", async () => {
		mockRenderMermaidASCII
			.mockReturnValueOnce(narrowOutput(130)) // tier 1
			.mockReturnValueOnce(narrowOutput(125)) // tier 2
			.mockReturnValueOnce(narrowOutput(110)); // tier 3 fits
		const result = await tool.execute("call-1", { source: "graph LR; A-->B" }, undefined, undefined, undefined);
		expect(mockRenderMermaidASCII).toHaveBeenCalledTimes(3);
		expect(mockRenderMermaidASCII.mock.calls[2][1]).toMatchObject({ colorMode: "none", paddingX: 1 });
		expect(result.content[0].text).toContain("x".repeat(110));
	});

	it("succeeds at exactly MAX_WIDTH (120 columns)", async () => {
		mockRenderMermaidASCII.mockReturnValue(narrowOutput(120));
		const result = await tool.execute("call-1", { source: "graph LR; A-->B" }, undefined, undefined, undefined);
		expect(result.content[0].text).toContain("x".repeat(120));
	});

	it("rejects when all tiers exceed MAX_WIDTH", async () => {
		mockRenderMermaidASCII.mockReturnValue(narrowOutput(150));
		await expect(
			tool.execute("call-1", { source: "graph LR; A-->B-->C-->D-->E" }, undefined, undefined, undefined),
		).rejects.toThrow(/too wide.*150 columns.*max 120/i);
	});

	it("rejection error includes simplification guidance", async () => {
		mockRenderMermaidASCII.mockReturnValue(narrowOutput(200));
		await expect(
			tool.execute("call-1", { source: "graph LR; A-->B" }, undefined, undefined, undefined),
		).rejects.toThrow(/simplify/i);
	});

	it("width rejection error is NOT misclassified as syntax error (F1)", async () => {
		mockRenderMermaidASCII.mockReturnValue(narrowOutput(150));
		await expect(
			tool.execute("call-1", { source: "graph LR; A-->B" }, undefined, undefined, undefined),
		).rejects.toThrow(/too wide/i);
		await expect(
			tool.execute("call-1", { source: "graph LR; A-->B" }, undefined, undefined, undefined),
		).rejects.not.toThrow(/invalid mermaid syntax/i);
	});

	it("classifies unsupported diagram type errors", async () => {
		mockRenderMermaidASCII.mockImplementation(() => {
			throw new Error('Invalid mermaid header: "gantt"');
		});
		await expect(
			tool.execute("call-1", { source: "gantt\ntitle A" }, undefined, undefined, undefined),
		).rejects.toThrow(/unsupported diagram type.*gantt.*supported types/i);
	});

	it("classifies parse errors", async () => {
		mockRenderMermaidASCII.mockImplementation(() => {
			throw new Error("Unexpected token at line 3");
		});
		await expect(
			tool.execute("call-1", { source: "graph LR;\n  broken" }, undefined, undefined, undefined),
		).rejects.toThrow(/invalid mermaid syntax/i);
	});

	it("classifies empty diagram errors", async () => {
		mockRenderMermaidASCII.mockImplementation(() => {
			throw new Error("Empty mermaid diagram");
		});
		await expect(tool.execute("call-1", { source: "" }, undefined, undefined, undefined)).rejects.toThrow(
			/empty diagram source/i,
		);
	});

	it("uses colorMode 'none' for model-facing content (no ANSI)", async () => {
		mockRenderMermaidASCII.mockReturnValue("plain text output");
		await tool.execute("call-1", { source: "graph LR; A-->B" }, undefined, undefined, undefined);
		for (const call of mockRenderMermaidASCII.mock.calls) {
			expect(call[1]).toMatchObject({ colorMode: "none" });
		}
	});

	it("includes title in content when provided", async () => {
		mockRenderMermaidASCII.mockReturnValue(narrowOutput(80));
		const result = await tool.execute(
			"call-1",
			{ source: "graph LR; A-->B", title: "My Diagram" },
			undefined,
			undefined,
			undefined,
		);
		expect(result.content[0].text).toMatch(/^My Diagram\n/);
	});

	it("does not include title prefix when title is omitted", async () => {
		mockRenderMermaidASCII.mockReturnValue("rendered");
		const result = await tool.execute("call-1", { source: "graph LR; A-->B" }, undefined, undefined, undefined);
		expect(result.content[0].text).toBe("rendered");
	});

	it("stores source, title, and tier in details for renderResult", async () => {
		mockRenderMermaidASCII
			.mockReturnValueOnce(narrowOutput(130)) // tier 1 too wide
			.mockReturnValueOnce(narrowOutput(100)); // tier 2 fits
		const result = await tool.execute(
			"call-1",
			{ source: "graph LR; A-->B", title: "Flow" },
			undefined,
			undefined,
			undefined,
		);
		expect(result.details).toEqual({ source: "graph LR; A-->B", title: "Flow", tier: 1 });
	});
});

describe("renderResult", () => {
	let tool: any;

	beforeEach(() => {
		mockRenderMermaidASCII.mockReset();
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		tool = tools[0];
	});

	it("returns a DiagramComponent when details.source is present", () => {
		const component = tool.renderResult({ details: { source: "graph LR; A-->B", tier: 0 }, content: [] });
		expect(component).toBeDefined();
		expect(typeof component.render).toBe("function");
		expect(typeof component.invalidate).toBe("function");
	});

	it("returns a PlainTextComponent when no details", () => {
		const component = tool.renderResult({ content: [{ type: "text", text: "fallback text" }] });
		expect(component.render(80)).toEqual(["fallback text"]);
	});

	it("PlainTextComponent shows '[No diagram]' when content is empty", () => {
		const component = tool.renderResult({ content: [] });
		expect(component.render(80)).toEqual(["[No diagram]"]);
	});
});

describe("DiagramComponent", () => {
	let tool: any;

	beforeEach(() => {
		mockRenderMermaidASCII.mockReset();
		const { pi, tools } = recordingPi();
		registerDiagramTool(pi);
		tool = tools[0];
	});

	it("renders with colorMode ansi256", () => {
		mockRenderMermaidASCII.mockReturnValue("colored output");
		const component = tool.renderResult({ details: { source: "graph LR; A-->B", tier: 0 }, content: [] });
		component.render(120);
		expect(mockRenderMermaidASCII).toHaveBeenCalledWith(
			"graph LR; A-->B",
			expect.objectContaining({ colorMode: "ansi256" }),
		);
	});

	it("output lines do not exceed render width (visibleWidth)", () => {
		const longLine = "x".repeat(200);
		mockRenderMermaidASCII.mockReturnValue(`short\n${longLine}\nend`);
		const component = tool.renderResult({ details: { source: "graph LR; A-->B", tier: 0 }, content: [] });
		const lines = component.render(80);
		for (const line of lines) {
			expect(visibleWidth(line)).toBeLessThanOrEqual(80);
		}
	});

	it("selects tighter tier when spacious output exceeds width", () => {
		mockRenderMermaidASCII
			.mockReturnValueOnce(narrowOutput(100)) // spacious too wide for width=90
			.mockReturnValueOnce(narrowOutput(85)); // compact fits
		const component = tool.renderResult({ details: { source: "graph LR; A-->B", tier: 0 }, content: [] });
		component.render(90);
		expect(mockRenderMermaidASCII).toHaveBeenCalledTimes(2);
		expect(mockRenderMermaidASCII.mock.calls[1][1]).toMatchObject({ colorMode: "ansi256", paddingX: 3 });
	});

	it("falls back to tightest tier when all tiers exceed width", () => {
		mockRenderMermaidASCII.mockReturnValue(narrowOutput(200));
		const component = tool.renderResult({ details: { source: "graph LR; A-->B", tier: 0 }, content: [] });
		const lines = component.render(80);
		expect(lines.length).toBeGreaterThan(0);
		for (const line of lines) {
			expect(visibleWidth(line)).toBeLessThanOrEqual(80);
		}
	});

	it("includes title when provided", () => {
		mockRenderMermaidASCII.mockReturnValue("diagram");
		const component = tool.renderResult({
			details: { source: "graph LR; A-->B", title: "Architecture", tier: 0 },
			content: [],
		});
		const lines = component.render(120);
		expect(lines[0]).toBe("Architecture");
		expect(lines[1]).toBe("");
		expect(lines[2]).toBe("diagram");
	});

	it("truncates title that exceeds render width (F2)", () => {
		mockRenderMermaidASCII.mockReturnValue("diagram");
		const longTitle = "A".repeat(200);
		const component = tool.renderResult({
			details: { source: "graph LR; A-->B", title: longTitle, tier: 0 },
			content: [],
		});
		const lines = component.render(80);
		expect(visibleWidth(lines[0])).toBeLessThanOrEqual(80);
		expect(lines[1]).toBe("");
	});

	it("returns error message on render failure without throwing", () => {
		mockRenderMermaidASCII.mockImplementation(() => {
			throw new Error("render broke");
		});
		const component = tool.renderResult({ details: { source: "bad", tier: 0 }, content: [] });
		const lines = component.render(80);
		expect(lines).toHaveLength(1);
		expect(lines[0]).toContain("Diagram render error");
		expect(lines[0]).toContain("render broke");
	});

	it("invalidate is callable without error", () => {
		const component = tool.renderResult({ details: { source: "graph LR; A-->B", tier: 0 }, content: [] });
		expect(() => component.invalidate()).not.toThrow();
	});
});

describe("integration with real library", () => {
	let realRenderMermaidASCII: typeof import("beautiful-mermaid").renderMermaidASCII;

	beforeEach(async () => {
		const lib = await vi.importActual<typeof import("beautiful-mermaid")>("beautiful-mermaid");
		realRenderMermaidASCII = lib.renderMermaidASCII;
	});

	it("renders a basic flowchart without error", () => {
		const result = realRenderMermaidASCII("graph LR\n  A --> B\n  B --> C", {
			colorMode: "none",
			paddingX: 3,
			paddingY: 2,
			boxBorderPadding: 1,
		});
		expect(result).toContain("A");
		expect(result).toContain("B");
		expect(result).toContain("C");
		expect(result.split("\n").length).toBeGreaterThan(1);
	});

	it("renders with ansi256 color mode", () => {
		const result = realRenderMermaidASCII("graph LR\n  A-->B", {
			colorMode: "ansi256",
			paddingX: 3,
			paddingY: 2,
			boxBorderPadding: 1,
		});
		// ANSI escape sequences present
		expect(result).toMatch(/\x1b\[/);
		expect(result).toContain("A");
	});

	it("throws on unsupported diagram type", () => {
		expect(() => realRenderMermaidASCII("gantt\n  title Test", { colorMode: "none" })).toThrow();
	});
});
