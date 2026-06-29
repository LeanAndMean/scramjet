import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { type Component, truncateToWidth, visibleWidth } from "@leanandmean/tui";
import type { AsciiRenderOptions } from "beautiful-mermaid";
import { renderMermaidASCII } from "beautiful-mermaid";
import { Type } from "typebox";

const SUPPORTED_TYPES = [
	"flowchart",
	"graph",
	"sequenceDiagram",
	"classDiagram",
	"stateDiagram-v2",
	"erDiagram",
	"xychart-beta",
] as const;

const PADDING_TIERS: AsciiRenderOptions[] = [
	{ paddingX: 5, paddingY: 3, boxBorderPadding: 2 },
	{ paddingX: 3, paddingY: 2, boxBorderPadding: 1 },
	{ paddingX: 1, paddingY: 1, boxBorderPadding: 1 },
];

const MAX_WIDTH = 120;

interface DiagramDetails {
	source: string;
	title?: string;
	tier: number;
}

function classifyError(error: unknown): string {
	const msg = error instanceof Error ? error.message : String(error);
	if (msg.includes("Invalid mermaid header")) {
		const match = msg.match(/Invalid mermaid header: "([^"]+)"/);
		const header = match?.[1] ?? "unknown";
		return `Unsupported diagram type: "${header}". Supported types: ${SUPPORTED_TYPES.join(", ")}`;
	}
	if (msg.includes("Empty mermaid diagram")) {
		return "Empty diagram source. Provide valid Mermaid syntax.";
	}
	return `Invalid Mermaid syntax: ${msg}`;
}

function maxLineWidth(text: string): number {
	let max = 0;
	for (const line of text.split("\n")) {
		const w = visibleWidth(line);
		if (w > max) max = w;
	}
	return max;
}

class DiagramComponent implements Component {
	constructor(
		private source: string,
		private title?: string,
	) {}

	invalidate(): void {}

	render(width: number): string[] {
		try {
			let rendered: string | undefined;
			let lastResult: string | undefined;
			for (const tier of PADDING_TIERS) {
				const result = renderMermaidASCII(this.source, { colorMode: "none", ...tier });
				lastResult = result;
				if (maxLineWidth(result) <= width) {
					rendered = result;
					break;
				}
			}
			rendered ??= lastResult!;
			const lines: string[] = [];
			if (this.title) {
				lines.push(visibleWidth(this.title) > width ? truncateToWidth(this.title, width) : this.title, "");
			}
			for (const line of rendered.split("\n")) {
				lines.push(visibleWidth(line) > width ? truncateToWidth(line, width) : line);
			}
			return lines;
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
		promptSnippet: "Render diagrams inline with draw_diagram (Mermaid syntax: flowchart, sequence, class, state, ER)",
		parameters: Type.Object({
			source: Type.String({ description: "The diagram source code in Mermaid syntax" }),
			title: Type.Optional(Type.String({ description: "Title for the diagram" })),
		}),
		async execute(_toolCallId, params, _signal, _onUpdate, _ctx) {
			const source = params.source as string;
			const title = params.title as string | undefined;

			let rendered: string | undefined;
			let lastResult: string | undefined;
			let usedTier = PADDING_TIERS.length - 1;
			try {
				for (let i = 0; i < PADDING_TIERS.length; i++) {
					const result = renderMermaidASCII(source, { colorMode: "none", ...PADDING_TIERS[i] });
					lastResult = result;
					if (maxLineWidth(result) <= MAX_WIDTH) {
						rendered = result;
						usedTier = i;
						break;
					}
				}
			} catch (e) {
				throw new Error(classifyError(e));
			}
			if (!rendered) {
				const actualWidth = maxLineWidth(lastResult!);
				throw new Error(
					`Diagram too wide for terminal display (${actualWidth} columns needed, max ${MAX_WIDTH}). Simplify the diagram: reduce nodes, shorten labels, or split into multiple diagrams.`,
				);
			}

			const text = title ? `${title}\n\n${rendered}` : rendered;
			return {
				content: [{ type: "text" as const, text }],
				details: { source, title, tier: usedTier } as DiagramDetails | undefined,
			};
		},
		renderResult(result) {
			const details = result.details as DiagramDetails | undefined;
			if (details?.source) {
				return new DiagramComponent(details.source, details.title);
			}
			const text = result.content?.find((c: { type: string }) => c.type === "text") as
				| { type: "text"; text: string }
				| undefined;
			return new PlainTextComponent(text?.text ?? "[No diagram]");
		},
	});
}
