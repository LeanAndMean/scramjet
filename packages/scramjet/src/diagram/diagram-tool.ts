/**
 * draw_diagram tool: renders Mermaid, Graphviz, or PlantUML source
 * and returns an inline image via Pi's terminal image support.
 */

import type { ExtensionAPI } from "@scramjet/coding-agent";
import { Image, Text } from "@scramjet/tui";
import { Type } from "typebox";
import { type DiagramFormat, detectRenderers, renderDiagram } from "./renderers.js";

export function registerDiagramTool(pi: ExtensionAPI) {
	const renderers = detectRenderers();
	const available = (Object.entries(renderers) as [DiagramFormat, { available: boolean }][])
		.filter(([, info]) => info.available)
		.map(([name]) => name);

	if (available.length === 0) return;

	pi.registerTool({
		name: "draw_diagram",
		label: "Draw Diagram",
		description: `Render a diagram and display it inline. Supported formats: ${available.join(", ")}. Use this instead of ASCII art for flowcharts, architecture diagrams, sequence diagrams, etc.`,
		promptSnippet: `Render diagrams inline with draw_diagram (${available.join(", ")})`,
		parameters: Type.Object({
			source: Type.String({ description: "The diagram source code" }),
			format: Type.Union(
				available.map((f) => Type.Literal(f)),
				{ description: "Diagram language" },
			),
			title: Type.Optional(Type.String({ description: "Title for the diagram" })),
		}),
		async execute(_toolCallId, params, signal) {
			const png = renderDiagram(params.source, params.format as DiagramFormat, signal ?? undefined);
			const base64 = png.toString("base64");

			return {
				content: [
					{ type: "image" as const, data: base64, mimeType: "image/png" as const },
					{
						type: "text" as const,
						text: `Rendered ${params.format} diagram${params.title ? `: ${params.title}` : ""}`,
					},
				],
				details: { source: params.source, format: params.format, title: params.title },
			};
		},
		renderResult(result, _options, theme) {
			const img = result.content?.find((c: { type: string }) => c.type === "image") as
				| { type: "image"; data: string; mimeType: string }
				| undefined;
			if (img) {
				return new Image(
					img.data,
					img.mimeType,
					{ fallbackColor: theme.fg.bind(theme, "dim") },
					{
						maxWidthCells: 80,
						maxHeightCells: 40,
					},
				);
			}
			const text = result.content?.find((c: { type: string }) => c.type === "text") as
				| { type: "text"; text: string }
				| undefined;
			return new Text(text?.text || "No diagram rendered", 0, 0);
		},
	});
}
