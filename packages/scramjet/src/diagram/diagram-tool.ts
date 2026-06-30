import type { AgentToolResult, ExtensionAPI, Theme, ThemeColor } from "@leanandmean/coding-agent";
import { type Component, truncateToWidth, visibleWidth } from "@leanandmean/tui";
import { Type } from "typebox";
import { type RenderOptions, renderDiagram } from "./renderer/index.js";
import type { Canvas, CharRole, RoleCanvas } from "./renderer/types.js";

const SUPPORTED_TYPES = ["flowchart", "graph", "stateDiagram-v2"] as const;

const PADDING_TIERS: RenderOptions[] = [
	{ paddingX: 5, paddingY: 5, boxBorderPadding: 2 },
	{ paddingX: 3, paddingY: 3, boxBorderPadding: 1 },
	{ paddingX: 1, paddingY: 1, boxBorderPadding: 1 },
];

const MAX_WIDTH = 120;

const ROLE_THEME_MAP: Record<CharRole, ThemeColor> = {
	text: "text",
	border: "border",
	line: "muted",
	arrow: "accent",
	corner: "muted",
	junction: "border",
} satisfies Record<CharRole, ThemeColor>;

interface DiagramDetails {
	source: string;
	title?: string;
	tier: number;
}

function classifyError(error: unknown): string {
	const msg = error instanceof Error ? error.message : String(error);
	if (msg.includes("Unsupported diagram type") || msg.includes("Invalid mermaid header")) {
		const match = msg.match(/Unsupported diagram type: "([^"]+)"|Invalid mermaid header: "([^"]+)"/);
		const header = match?.[1] ?? match?.[2] ?? "unknown";
		return `Unsupported diagram type: "${header}". Supported types: ${SUPPORTED_TYPES.join(", ")}`;
	}
	if (msg.includes("Empty mermaid diagram") || msg.includes("Empty diagram")) {
		return "Empty diagram source. Provide valid Mermaid syntax.";
	}
	if (msg.includes("Grid is too dense")) {
		return `Diagram too complex: ${msg}`;
	}
	if (error instanceof TypeError || error instanceof RangeError) {
		return `Internal diagram renderer error: ${msg}. This is a bug — please report it with the diagram source.`;
	}
	return `Diagram rendering failed: ${msg}`;
}

function canvasToLines(chars: Canvas): string[] {
	if (chars.length === 0) return [];
	const height = chars[0]!.length;
	const width = chars.length;
	const lines: string[] = [];
	for (let y = 0; y < height; y++) {
		let line = "";
		for (let x = 0; x < width; x++) {
			const ch = chars[x]![y]!;
			if (ch !== "") line += ch;
		}
		lines.push(line.trimEnd());
	}
	while (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
	return lines;
}

function canvasToColoredLines(chars: Canvas, roles: RoleCanvas, theme: Theme): string[] {
	if (chars.length === 0) return [];
	const height = chars[0]!.length;
	const width = chars.length;
	const lines: string[] = [];
	for (let y = 0; y < height; y++) {
		let line = "";
		for (let x = 0; x < width; x++) {
			const ch = chars[x]![y]!;
			if (ch === "") continue;
			const role = roles[x]?.[y];
			if (role && ch !== " ") {
				line += theme.fg(ROLE_THEME_MAP[role], ch);
			} else {
				line += ch;
			}
		}
		lines.push(line.trimEnd());
	}
	while (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
	return lines;
}

function maxLineWidth(lines: string[]): number {
	let max = 0;
	for (const line of lines) {
		const w = visibleWidth(line);
		if (w > max) max = w;
	}
	return max;
}

function renderAtTier(source: string, tier: RenderOptions): { lines: string[]; width: number } {
	const { chars } = renderDiagram(source, tier);
	const lines = canvasToLines(chars);
	return { lines, width: maxLineWidth(lines) };
}

class DiagramComponent implements Component {
	private cachedSource: string | undefined;
	private cachedWidth: number | undefined;
	private cachedLines: string[] | undefined;
	private cachedTheme: Theme | undefined;

	readonly diagramSource: string;
	private theme: Theme | undefined;

	constructor(
		source: string,
		private title: string | undefined,
		theme: Theme | undefined,
	) {
		this.diagramSource = source;
		this.theme = theme;
	}

	updateTheme(theme: Theme | undefined): void {
		this.theme = theme;
	}

	invalidate(): void {
		this.cachedSource = undefined;
		this.cachedWidth = undefined;
		this.cachedLines = undefined;
	}

	render(width: number): string[] {
		if (
			this.cachedSource === this.diagramSource &&
			this.cachedWidth === width &&
			this.cachedTheme === this.theme &&
			this.cachedLines
		) {
			return this.cachedLines;
		}

		try {
			let resultLines: string[] | undefined;
			let lastLines: string[] | undefined;
			for (const tier of PADDING_TIERS) {
				const rendered = renderDiagram(this.diagramSource, tier);
				const lines = this.theme
					? canvasToColoredLines(rendered.chars, rendered.roles, this.theme)
					: canvasToLines(rendered.chars);
				lastLines = lines;
				if (maxLineWidth(lines) <= width) {
					resultLines = lines;
					break;
				}
			}
			resultLines ??= lastLines!;

			const output: string[] = [];
			if (this.title) {
				output.push(visibleWidth(this.title) > width ? truncateToWidth(this.title, width) : this.title, "");
			}
			for (const line of resultLines) {
				output.push(visibleWidth(line) > width ? truncateToWidth(line, width) : line);
			}

			this.cachedSource = this.diagramSource;
			this.cachedWidth = width;
			this.cachedTheme = this.theme;
			this.cachedLines = output;
			return output;
		} catch (e) {
			return [`[Diagram render error: ${e instanceof Error ? e.message : String(e)}]`];
		}
	}
}

class PlainTextComponent implements Component {
	constructor(private text: string) {}
	invalidate(): void {}
	render(): string[] {
		return this.text.split("\n");
	}
}

export function registerDiagramTool(pi: ExtensionAPI) {
	pi.registerTool({
		name: "draw_diagram",
		label: "Draw Diagram",
		description: `Render a diagram as text (Unicode box-drawing) and display it inline. Supported diagram types: ${SUPPORTED_TYPES.join(", ")}. Keep diagrams to ~15 nodes or fewer for terminal legibility. Use this instead of ASCII art for flowcharts, architecture diagrams, sequence diagrams, etc.`,
		promptSnippet: "Render diagrams inline with draw_diagram (Mermaid syntax: flowchart, graph, stateDiagram-v2)",
		parameters: Type.Object({
			source: Type.String({ description: "The diagram source code in Mermaid syntax" }),
			title: Type.Optional(Type.String({ description: "Title for the diagram" })),
		}),
		async execute(_toolCallId, params) {
			const source = params.source as string;
			const title = params.title as string | undefined;

			let rendered: { lines: string[]; width: number } | undefined;
			let lastResult: { lines: string[]; width: number } | undefined;
			let usedTier = PADDING_TIERS.length - 1;
			try {
				for (let i = 0; i < PADDING_TIERS.length; i++) {
					const result = renderAtTier(source, PADDING_TIERS[i]!);
					lastResult = result;
					if (result.width <= MAX_WIDTH) {
						rendered = result;
						usedTier = i;
						break;
					}
				}
			} catch (e) {
				throw new Error(classifyError(e));
			}
			if (!rendered) {
				throw new Error(
					`Diagram too wide for terminal display (${lastResult!.width} columns needed, max ${MAX_WIDTH}). Simplify the diagram: reduce nodes, shorten labels, or split into multiple diagrams.`,
				);
			}

			const text = title ? `${title}\n\n${rendered.lines.join("\n")}` : rendered.lines.join("\n");
			return {
				content: [{ type: "text" as const, text }],
				details: { source, title, tier: usedTier },
			};
		},
		renderResult(result: AgentToolResult<DiagramDetails>, _options, theme: Theme | undefined, context) {
			const details = result.details as DiagramDetails | undefined;
			if (details?.source) {
				const prev = context?.lastComponent as DiagramComponent | undefined;
				if (prev && prev instanceof DiagramComponent && prev.diagramSource === details.source) {
					prev.updateTheme(theme);
					return prev;
				}
				return new DiagramComponent(details.source, details.title, theme);
			}
			const textContent = result.content?.find((c: { type: string }) => c.type === "text") as
				| { type: "text"; text: string }
				| undefined;
			return new PlainTextComponent(textContent?.text ?? "[No diagram]");
		},
	});
}
