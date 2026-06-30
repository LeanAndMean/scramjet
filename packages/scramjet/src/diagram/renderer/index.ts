// Based on beautiful-mermaid by Craft Docs, MIT License.

import { parseMermaid } from "../parser.js";
import { flipCanvasVertically, flipRoleCanvasVertically } from "./canvas.js";
import { convertToAsciiGraph } from "./converter.js";
import { drawGraph } from "./draw.js";
import { createMapping } from "./grid.js";
import type { AsciiConfig, Canvas, RoleCanvas } from "./types.js";

export type { Canvas, CharRole, RoleCanvas } from "./types.js";

export interface RenderOptions {
	useAscii?: boolean;
	paddingX?: number;
	paddingY?: number;
	boxBorderPadding?: number;
}

export interface RenderedDiagram {
	chars: Canvas;
	roles: RoleCanvas;
}

export function renderDiagram(source: string, options: RenderOptions = {}): RenderedDiagram {
	const parsed = parseMermaid(source);

	const config: AsciiConfig = {
		useAscii: options.useAscii ?? false,
		paddingX: options.paddingX ?? 5,
		paddingY: options.paddingY ?? 5,
		boxBorderPadding: options.boxBorderPadding ?? 1,
		graphDirection: "TD",
	};

	if (parsed.direction === "LR" || parsed.direction === "RL") {
		config.graphDirection = "LR";
	} else {
		config.graphDirection = "TD";
	}

	const graph = convertToAsciiGraph(parsed, config);
	createMapping(graph);
	drawGraph(graph);

	if (parsed.direction === "BT") {
		flipCanvasVertically(graph.canvas);
		flipRoleCanvasVertically(graph.roleCanvas);
	}

	return {
		chars: graph.canvas,
		roles: graph.roleCanvas,
	};
}
