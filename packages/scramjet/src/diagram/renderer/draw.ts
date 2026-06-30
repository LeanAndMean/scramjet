// Based on beautiful-mermaid by Craft Docs, MIT License.

import { copyCanvas, drawText, mergeCanvases, mkCanvas, setRole, visibleWidth } from "./canvas.js";
import { determineDirection, dirEquals, getOpposite } from "./edge-routing.js";
import { gridToDrawingCoord, lineToDrawing } from "./grid.js";
import { splitLines } from "./multiline-utils.js";
import { getCorners, getShapeAttachmentPoint } from "./shapes.js";
import type {
	AsciiEdge,
	AsciiGraph,
	AsciiNode,
	AsciiSubgraph,
	Canvas,
	CharRole,
	DrawingCoord,
	EdgeBundle,
	EdgeStyle,
	GridCoord,
	GridDirection,
	RoleCanvas,
} from "./types.js";
import {
	Down,
	drawingCoordEquals,
	Left,
	LowerLeft,
	LowerRight,
	Middle,
	Right,
	Up,
	UpperLeft,
	UpperRight,
} from "./types.js";

// ============================================================================
// Node drawing
// ============================================================================

export function drawBox(node: AsciiNode, graph: AsciiGraph): Canvas {
	const gc = node.gridCoord!;
	const useAscii = graph.config.useAscii;

	let w = 0;
	for (let i = 0; i < 2; i++) {
		w += graph.columnWidth.get(gc.x + i) ?? 0;
	}
	let h = 0;
	for (let i = 0; i < 2; i++) {
		h += graph.rowHeight.get(gc.y + i) ?? 0;
	}

	const from: DrawingCoord = { x: 0, y: 0 };
	const to: DrawingCoord = { x: w, y: h };
	const box = mkCanvas(Math.max(from.x, to.x), Math.max(from.y, to.y));

	const corners = getCorners(node.shape, useAscii);

	const isDoubleBox = node.shape === "state-end";
	const hChar = useAscii ? (isDoubleBox ? "=" : "-") : isDoubleBox ? "═" : "─";
	const vChar = useAscii ? (isDoubleBox ? "‖" : "|") : isDoubleBox ? "║" : "│";

	const doubleCorners = useAscii ? { tl: "#", tr: "#", bl: "#", br: "#" } : { tl: "╔", tr: "╗", bl: "╚", br: "╝" };
	const effectiveCorners = isDoubleBox ? doubleCorners : corners;

	for (let x = from.x + 1; x < to.x; x++) box[x]![from.y] = hChar;
	for (let x = from.x + 1; x < to.x; x++) box[x]![to.y] = hChar;
	for (let y = from.y + 1; y < to.y; y++) box[from.x]![y] = vChar;
	for (let y = from.y + 1; y < to.y; y++) box[to.x]![y] = vChar;
	box[from.x]![from.y] = effectiveCorners.tl;
	box[to.x]![from.y] = effectiveCorners.tr;
	box[from.x]![to.y] = effectiveCorners.bl;
	box[to.x]![to.y] = effectiveCorners.br;

	const label = node.displayLabel;
	const lines = splitLines(label);
	const textCenterY = from.y + Math.floor(h / 2);
	const startY = textCenterY - Math.floor((lines.length - 1) / 2);

	for (let i = 0; i < lines.length; i++) {
		const line = lines[i]!;
		const lineVW = visibleWidth(line);
		const textX = from.x + Math.floor(w / 2) - Math.ceil(lineVW / 2) + 1;
		let col = textX;
		for (const char of line) {
			const charWidth = visibleWidth(char);
			if (col >= 0 && col < box.length && startY + i >= 0 && startY + i < box[0]!.length) {
				box[col]![startY + i] = char;
				for (let cw = 1; cw < charWidth; cw++) {
					if (col + cw < box.length) box[col + cw]![startY + i] = "";
				}
			}
			col += charWidth;
		}
	}

	return box;
}

// ============================================================================
// Shared direction-to-character helpers
// ============================================================================

function getCornerChar(prevDir: GridDirection, nextDir: GridDirection, useAscii: boolean): string {
	if (useAscii) return "+";
	if (
		(dirEquals(prevDir, Right) && dirEquals(nextDir, Down)) ||
		(dirEquals(prevDir, Up) && dirEquals(nextDir, Left))
	) {
		return "┐";
	}
	if (
		(dirEquals(prevDir, Right) && dirEquals(nextDir, Up)) ||
		(dirEquals(prevDir, Down) && dirEquals(nextDir, Left))
	) {
		return "┘";
	}
	if (
		(dirEquals(prevDir, Left) && dirEquals(nextDir, Down)) ||
		(dirEquals(prevDir, Up) && dirEquals(nextDir, Right))
	) {
		return "┌";
	}
	if (
		(dirEquals(prevDir, Left) && dirEquals(nextDir, Up)) ||
		(dirEquals(prevDir, Down) && dirEquals(nextDir, Right))
	) {
		return "└";
	}
	return "+";
}

function arrowChar(dir: GridDirection, useAscii: boolean, fallback?: string): string {
	if (!useAscii) {
		if (dirEquals(dir, Up)) return "▲";
		if (dirEquals(dir, Down)) return "▼";
		if (dirEquals(dir, Left)) return "◄";
		if (dirEquals(dir, Right)) return "►";
		if (dirEquals(dir, UpperRight)) return "◥";
		if (dirEquals(dir, UpperLeft)) return "◤";
		if (dirEquals(dir, LowerRight)) return "◢";
		if (dirEquals(dir, LowerLeft)) return "◣";
		return fallback ?? "▼";
	}
	if (dirEquals(dir, Up)) return "^";
	if (dirEquals(dir, Down)) return "v";
	if (dirEquals(dir, Left)) return "<";
	if (dirEquals(dir, Right)) return ">";
	return fallback ?? "v";
}

// ============================================================================
// Line drawing
// ============================================================================

const LINE_CHARS = {
	solid: { h: { unicode: "─", ascii: "-" }, v: { unicode: "│", ascii: "|" } },
	dotted: { h: { unicode: "┄", ascii: "." }, v: { unicode: "┆", ascii: ":" } },
	thick: { h: { unicode: "━", ascii: "=" }, v: { unicode: "┃", ascii: "‖" } },
} as const;

export function drawLine(
	canvas: Canvas,
	from: DrawingCoord,
	to: DrawingCoord,
	offsetFrom: number,
	offsetTo: number,
	useAscii: boolean,
	style: EdgeStyle = "solid",
): DrawingCoord[] {
	const dir = determineDirection(from, to);
	const drawnCoords: DrawingCoord[] = [];

	const chars = LINE_CHARS[style];
	const hChar = useAscii ? chars.h.ascii : chars.h.unicode;
	const vChar = useAscii ? chars.v.ascii : chars.v.unicode;

	if (dirEquals(dir, Up)) {
		for (let y = from.y - offsetFrom; y >= to.y - offsetTo; y--) {
			drawnCoords.push({ x: from.x, y });
			canvas[from.x]![y] = vChar;
		}
	} else if (dirEquals(dir, Down)) {
		for (let y = from.y + offsetFrom; y <= to.y + offsetTo; y++) {
			drawnCoords.push({ x: from.x, y });
			canvas[from.x]![y] = vChar;
		}
	} else if (dirEquals(dir, Left)) {
		for (let x = from.x - offsetFrom; x >= to.x - offsetTo; x--) {
			drawnCoords.push({ x, y: from.y });
			canvas[x]![from.y] = hChar;
		}
	} else if (dirEquals(dir, Right)) {
		for (let x = from.x + offsetFrom; x <= to.x + offsetTo; x++) {
			drawnCoords.push({ x, y: from.y });
			canvas[x]![from.y] = hChar;
		}
	} else if (dirEquals(dir, UpperLeft)) {
		for (let x = from.x - offsetFrom; x >= to.x; x--) {
			drawnCoords.push({ x, y: from.y });
			canvas[x]![from.y] = hChar;
		}
		for (let y = from.y - 1; y >= to.y - offsetTo; y--) {
			drawnCoords.push({ x: to.x, y });
			canvas[to.x]![y] = vChar;
		}
	} else if (dirEquals(dir, UpperRight)) {
		for (let x = from.x + offsetFrom; x <= to.x; x++) {
			drawnCoords.push({ x, y: from.y });
			canvas[x]![from.y] = hChar;
		}
		for (let y = from.y - 1; y >= to.y - offsetTo; y--) {
			drawnCoords.push({ x: to.x, y });
			canvas[to.x]![y] = vChar;
		}
	} else if (dirEquals(dir, LowerLeft)) {
		for (let x = from.x - offsetFrom; x >= to.x; x--) {
			drawnCoords.push({ x, y: from.y });
			canvas[x]![from.y] = hChar;
		}
		for (let y = from.y + 1; y <= to.y + offsetTo; y++) {
			drawnCoords.push({ x: to.x, y });
			canvas[to.x]![y] = vChar;
		}
	} else if (dirEquals(dir, LowerRight)) {
		const dx = to.x - from.x;
		if (dx <= 1) {
			for (let y = from.y + offsetFrom; y <= to.y + offsetTo; y++) {
				drawnCoords.push({ x: from.x, y });
				canvas[from.x]![y] = vChar;
			}
		} else {
			for (let x = from.x + offsetFrom; x <= to.x; x++) {
				drawnCoords.push({ x, y: from.y });
				canvas[x]![from.y] = hChar;
			}
			for (let y = from.y + 1; y <= to.y + offsetTo; y++) {
				drawnCoords.push({ x: to.x, y });
				canvas[to.x]![y] = vChar;
			}
		}
	}

	return drawnCoords;
}

// ============================================================================
// Arrow drawing
// ============================================================================

export function drawArrow(
	graph: AsciiGraph,
	edge: AsciiEdge,
	sharedLabelCanvas?: Canvas,
	labelRegions?: LabelRegion[],
): [Canvas, Canvas, Canvas, Canvas, Canvas] {
	if (edge.path.length === 0) {
		const empty = copyCanvas(graph.canvas);
		return [empty, empty, empty, empty, empty];
	}

	drawArrowLabel(graph, edge, sharedLabelCanvas, labelRegions);
	const sourceAttach = getNodeAttachmentPoint(graph, edge.from, edge.startDir);
	const targetAttach = getNodeAttachmentPoint(graph, edge.to, edge.endDir);
	adjustAttachForOffsetPath(graph, edge, sourceAttach, targetAttach);
	const [pathCanvas, linesDrawn, lineDirs] = drawPath(graph, edge.path, edge.style, sourceAttach, targetAttach);
	const boxStartCanvas = drawBoxStart(graph, edge.path, linesDrawn[0]!, edge.from.shape);

	let arrowHeadEndCanvas: Canvas;
	if (edge.hasArrowEnd) {
		arrowHeadEndCanvas = drawArrowHead(graph, linesDrawn[linesDrawn.length - 1]!, lineDirs[lineDirs.length - 1]!);
	} else {
		arrowHeadEndCanvas = copyCanvas(graph.canvas);
	}

	let arrowHeadStartCanvas: Canvas;
	if (edge.hasArrowStart && linesDrawn.length > 0) {
		const firstLine = linesDrawn[0]!;
		const firstPoint = firstLine[0]!;
		const startDir = getOpposite(lineDirs[0]!);

		const arrowPos: DrawingCoord = { x: firstPoint.x, y: firstPoint.y };
		if (dirEquals(lineDirs[0]!, Right)) arrowPos.x = firstPoint.x - 1;
		else if (dirEquals(lineDirs[0]!, Left)) arrowPos.x = firstPoint.x + 1;
		else if (dirEquals(lineDirs[0]!, Down)) arrowPos.y = firstPoint.y - 1;
		else if (dirEquals(lineDirs[0]!, Up)) arrowPos.y = firstPoint.y + 1;

		const syntheticLine: DrawingCoord[] = [firstPoint, arrowPos];
		arrowHeadStartCanvas = drawArrowHead(graph, syntheticLine, startDir);
	} else {
		arrowHeadStartCanvas = copyCanvas(graph.canvas);
	}

	const cornersCanvas = drawCorners(graph, edge.path);

	return [pathCanvas, boxStartCanvas, arrowHeadEndCanvas, arrowHeadStartCanvas, cornersCanvas];
}

function drawPath(
	graph: AsciiGraph,
	path: GridCoord[],
	style: EdgeStyle = "solid",
	sourceAttach?: DrawingCoord | null,
	targetAttach?: DrawingCoord | null,
): [Canvas, DrawingCoord[][], GridDirection[]] {
	const canvas = copyCanvas(graph.canvas);
	let previousCoord = path[0]!;
	const linesDrawn: DrawingCoord[][] = [];
	const lineDirs: GridDirection[] = [];

	for (let i = 1; i < path.length; i++) {
		const nextCoord = path[i]!;
		let prevDC = gridToDrawingCoord(graph, previousCoord);
		let nextDC = gridToDrawingCoord(graph, nextCoord);

		if (i === 1 && sourceAttach) prevDC = sourceAttach;
		if (i === path.length - 1 && targetAttach) nextDC = targetAttach;

		if (drawingCoordEquals(prevDC, nextDC)) {
			previousCoord = nextCoord;
			continue;
		}

		const dir = determineDirection(previousCoord, nextCoord);
		const segment = drawLine(canvas, prevDC, nextDC, 1, -1, graph.config.useAscii, style);
		if (segment.length === 0) segment.push(prevDC);
		linesDrawn.push(segment);
		lineDirs.push(dir);
		previousCoord = nextCoord;
	}

	return [canvas, linesDrawn, lineDirs];
}

function drawBoxStart(graph: AsciiGraph, path: GridCoord[], firstLine: DrawingCoord[], sourceShape: string): Canvas {
	const canvas = copyCanvas(graph.canvas);
	if (graph.config.useAscii) return canvas;

	if (sourceShape === "state-start" || sourceShape === "state-end") {
		return canvas;
	}

	const from = firstLine[0]!;
	const dir = determineDirection(path[0]!, path[1]!);

	if (dirEquals(dir, Up)) canvas[from.x]![from.y + 1] = "┴";
	else if (dirEquals(dir, Down)) canvas[from.x]![from.y - 1] = "┬";
	else if (dirEquals(dir, Left)) canvas[from.x + 1]![from.y] = "┤";
	else if (dirEquals(dir, Right)) canvas[from.x - 1]![from.y] = "├";

	return canvas;
}

function drawArrowHead(graph: AsciiGraph, lastLine: DrawingCoord[], fallbackDir: GridDirection): Canvas {
	const canvas = copyCanvas(graph.canvas);
	if (lastLine.length === 0) return canvas;

	const from = lastLine[0]!;
	const lastPos = lastLine[lastLine.length - 1]!;
	let dir = determineDirection(from, lastPos);
	if (lastLine.length === 1 || dirEquals(dir, Middle)) dir = fallbackDir;

	const useAscii = graph.config.useAscii;
	const char = arrowChar(dir, useAscii, arrowChar(fallbackDir, useAscii, useAscii ? "*" : "●"));

	canvas[lastPos.x]![lastPos.y] = char;
	return canvas;
}

function drawCorners(graph: AsciiGraph, path: GridCoord[]): Canvas {
	const canvas = copyCanvas(graph.canvas);

	for (let idx = 1; idx < path.length - 1; idx++) {
		const coord = path[idx]!;
		const dc = gridToDrawingCoord(graph, coord);
		const prevDir = determineDirection(path[idx - 1]!, coord);
		const nextDir = determineDirection(coord, path[idx + 1]!);
		canvas[dc.x]![dc.y] = getCornerChar(prevDir, nextDir, graph.config.useAscii);
	}

	return canvas;
}

interface LabelRegion {
	x: number;
	y: number;
	width: number;
}

function drawArrowLabel(
	graph: AsciiGraph,
	edge: AsciiEdge,
	sharedCanvas?: Canvas,
	labelRegions?: LabelRegion[],
): Canvas {
	const canvas = sharedCanvas ?? copyCanvas(graph.canvas);
	if (edge.text.length === 0) return canvas;

	const drawingLine = lineToDrawing(graph, edge.labelLine);

	let isUpwardEdge: boolean | undefined;
	if (edge.path.length >= 2) {
		const startY = edge.path[0]!.y;
		const endY = edge.path[edge.path.length - 1]!.y;
		if (endY < startY) {
			isUpwardEdge = true;
		} else if (endY > startY) {
			isUpwardEdge = false;
		}
	}

	drawTextOnLine(canvas, drawingLine, edge.text, isUpwardEdge, labelRegions);
	return canvas;
}

function drawTextOnLine(
	canvas: Canvas,
	line: DrawingCoord[],
	label: string,
	isUpwardEdge?: boolean,
	labelRegions?: LabelRegion[],
): void {
	if (line.length < 2) return;
	const minX = Math.min(line[0]!.x, line[1]!.x);
	const maxX = Math.max(line[0]!.x, line[1]!.x);
	const minY = Math.min(line[0]!.y, line[1]!.y);
	const maxY = Math.max(line[0]!.y, line[1]!.y);
	const middleX = minX + Math.floor((maxX - minX) / 2);
	let middleY = minY + Math.floor((maxY - minY) / 2);

	if (isUpwardEdge !== undefined && minX === maxX) {
		const segmentHeight = maxY - minY;
		const offset = Math.max(1, Math.floor(segmentHeight / 4));
		if (isUpwardEdge) {
			middleY = middleY - offset;
		} else {
			middleY = middleY + offset;
		}
	}

	const lines = splitLines(label);
	const startY = middleY - Math.floor((lines.length - 1) / 2);

	for (let i = 0; i < lines.length; i++) {
		const lineText = lines[i]!;
		const startX = middleX - Math.floor(lineText.length / 2);
		drawText(canvas, { x: startX, y: startY + i }, lineText);
		if (labelRegions) {
			labelRegions.push({ x: startX, y: startY + i, width: lineText.length });
		}
	}
}

// ============================================================================
// Node attachment point
// ============================================================================

function adjustAttachForOffsetPath(
	graph: AsciiGraph,
	edge: AsciiEdge,
	sourceAttach: DrawingCoord,
	targetAttach: DrawingCoord,
): void {
	if (edge.path.length < 2) return;
	const firstPoint = edge.path[0]!;
	const lastPoint = edge.path[edge.path.length - 1]!;
	const fromGc = edge.from.gridCoord!;
	const toGc = edge.to.gridCoord!;

	const expectedStartY = fromGc.y + edge.startDir.y;
	if (firstPoint.y !== expectedStartY) {
		const expectedDC = gridToDrawingCoord(graph, { x: firstPoint.x, y: expectedStartY });
		const actualDC = gridToDrawingCoord(graph, firstPoint);
		sourceAttach.y += actualDC.y - expectedDC.y;
	}

	const expectedEndY = toGc.y + edge.endDir.y;
	if (lastPoint.y !== expectedEndY) {
		const expectedDC = gridToDrawingCoord(graph, { x: lastPoint.x, y: expectedEndY });
		const actualDC = gridToDrawingCoord(graph, lastPoint);
		targetAttach.y += actualDC.y - expectedDC.y;
	}
}

function getNodeAttachmentPoint(graph: AsciiGraph, node: AsciiNode, dir: GridDirection): DrawingCoord {
	const gc = node.gridCoord!;

	let w = 0;
	for (let i = 0; i < 2; i++) {
		w += graph.columnWidth.get(gc.x + i) ?? 0;
	}
	let h = 0;
	for (let i = 0; i < 2; i++) {
		h += graph.rowHeight.get(gc.y + i) ?? 0;
	}

	const gridDimensions = {
		width: w + 1,
		height: h + 1,
		labelArea: { x: 0, y: 0, width: 0, height: 0 },
		gridColumns: [0, 0, 0] as [number, number, number],
		gridRows: [0, 0, 0] as [number, number, number],
	};

	const baseCoord = node.drawingCoord!;
	return getShapeAttachmentPoint(node.shape, dir, gridDimensions, baseCoord);
}

// ============================================================================
// Bundled edge drawing
// ============================================================================

function drawBundledEdgeSegment(
	graph: AsciiGraph,
	edge: AsciiEdge,
	bundle: EdgeBundle,
): [Canvas, Canvas, Canvas, Canvas, Canvas] {
	const empty = copyCanvas(graph.canvas);

	if (!edge.pathToJunction || edge.pathToJunction.length === 0) {
		return [empty, empty, empty, empty, empty];
	}

	const pathCanvas = copyCanvas(graph.canvas);
	const useAscii = graph.config.useAscii;

	const drawingPath = edge.pathToJunction.map((gc, idx) => {
		if (bundle.type === "fan-in" && idx === 0) {
			return getNodeAttachmentPoint(graph, edge.from, edge.startDir);
		}
		if (bundle.type === "fan-out" && idx === edge.pathToJunction!.length - 1) {
			return getNodeAttachmentPoint(graph, edge.to, edge.endDir);
		}
		return gridToDrawingCoord(graph, gc);
	});

	for (let i = 1; i < drawingPath.length; i++) {
		const from = drawingPath[i - 1]!;
		const to = drawingPath[i]!;
		if (!drawingCoordEquals(from, to)) {
			drawLine(pathCanvas, from, to, 1, -1, useAscii, edge.style);
		}
	}

	const cornersCanvas = copyCanvas(graph.canvas);
	for (let idx = 1; idx < edge.pathToJunction.length - 1; idx++) {
		const coord = edge.pathToJunction[idx]!;
		const dc = gridToDrawingCoord(graph, coord);
		const prevDir = determineDirection(edge.pathToJunction[idx - 1]!, coord);
		const nextDir = determineDirection(coord, edge.pathToJunction[idx + 1]!);
		cornersCanvas[dc.x]![dc.y] = getCornerChar(prevDir, nextDir, useAscii);
	}

	const boxStartCanvas = copyCanvas(graph.canvas);
	if (bundle.type === "fan-in" && edge.pathToJunction.length >= 2) {
		const firstPoint = drawingPath[0]!;
		const dir = determineDirection(edge.pathToJunction[0]!, edge.pathToJunction[1]!);

		if (!useAscii) {
			if (dirEquals(dir, Up)) boxStartCanvas[firstPoint.x]![firstPoint.y] = "┴";
			else if (dirEquals(dir, Down)) boxStartCanvas[firstPoint.x]![firstPoint.y] = "┬";
			else if (dirEquals(dir, Left)) boxStartCanvas[firstPoint.x]![firstPoint.y] = "┤";
			else if (dirEquals(dir, Right)) boxStartCanvas[firstPoint.x]![firstPoint.y] = "├";
		}
	}

	return [pathCanvas, boxStartCanvas, empty, empty, cornersCanvas];
}

function drawBundleSharedPath(graph: AsciiGraph, bundle: EdgeBundle): [Canvas, Canvas] {
	const pathCanvas = copyCanvas(graph.canvas);
	const cornersCanvas = copyCanvas(graph.canvas);

	if (bundle.sharedPath.length < 2) {
		return [pathCanvas, cornersCanvas];
	}

	const useAscii = graph.config.useAscii;
	const style = bundle.edges[0]?.style ?? "solid";
	const graphDir = graph.config.graphDirection;

	const drawingPath = bundle.sharedPath.map((gc, idx) => {
		if (bundle.type === "fan-in" && idx === bundle.sharedPath.length - 1) {
			const entryDir = graphDir === "TD" ? Up : Left;
			return getNodeAttachmentPoint(graph, bundle.sharedNode, entryDir);
		}
		if (bundle.type === "fan-out" && idx === 0) {
			const exitDir = graphDir === "TD" ? Down : Right;
			return getNodeAttachmentPoint(graph, bundle.sharedNode, exitDir);
		}
		return gridToDrawingCoord(graph, gc);
	});

	for (let i = 1; i < drawingPath.length; i++) {
		const from = drawingPath[i - 1]!;
		const to = drawingPath[i]!;
		if (!drawingCoordEquals(from, to)) {
			drawLine(pathCanvas, from, to, 1, -1, useAscii, style);
		}
	}

	for (let idx = 1; idx < bundle.sharedPath.length - 1; idx++) {
		const coord = bundle.sharedPath[idx]!;
		const dc = gridToDrawingCoord(graph, coord);
		const prevDir = determineDirection(bundle.sharedPath[idx - 1]!, coord);
		const nextDir = determineDirection(coord, bundle.sharedPath[idx + 1]!);
		cornersCanvas[dc.x]![dc.y] = getCornerChar(prevDir, nextDir, useAscii);
	}

	return [pathCanvas, cornersCanvas];
}

function drawBundleArrowhead(graph: AsciiGraph, bundle: EdgeBundle): Canvas {
	const canvas = copyCanvas(graph.canvas);

	if (bundle.sharedPath.length < 2) return canvas;

	const lastIdx = bundle.sharedPath.length - 1;
	const secondLast = bundle.sharedPath[lastIdx - 1]!;
	const last = bundle.sharedPath[lastIdx]!;
	const dir = determineDirection(secondLast, last);

	const graphDir = graph.config.graphDirection;
	const entryDir = graphDir === "TD" ? Up : Left;
	const dc = getNodeAttachmentPoint(graph, bundle.sharedNode, entryDir);
	if (graphDir === "TD") dc.y -= 1;
	else dc.x -= 1;

	canvas[dc.x]![dc.y] = arrowChar(dir, graph.config.useAscii);
	return canvas;
}

function drawBundledEdgeArrowhead(graph: AsciiGraph, edge: AsciiEdge): Canvas {
	const canvas = copyCanvas(graph.canvas);

	if (!edge.pathToJunction || edge.pathToJunction.length < 2) return canvas;

	const lastIdx = edge.pathToJunction.length - 1;
	const secondLast = edge.pathToJunction[lastIdx - 1]!;
	const last = edge.pathToJunction[lastIdx]!;
	const dir = determineDirection(secondLast, last);

	const graphDir = graph.config.graphDirection;
	const entryDir = graphDir === "TD" ? Up : Left;
	const dc = getNodeAttachmentPoint(graph, edge.to, entryDir);
	if (graphDir === "TD") dc.y -= 1;
	else dc.x -= 1;

	canvas[dc.x]![dc.y] = arrowChar(dir, graph.config.useAscii);
	return canvas;
}

function drawJunctionCharacter(graph: AsciiGraph, bundle: EdgeBundle): Canvas {
	const canvas = copyCanvas(graph.canvas);

	if (!bundle.junctionPoint) return canvas;

	const dc = gridToDrawingCoord(graph, bundle.junctionPoint);
	const useAscii = graph.config.useAscii;

	let hasUp = false;
	let hasDown = false;
	let hasLeft = false;
	let hasRight = false;

	if (bundle.sharedPath.length >= 2) {
		const junctionIdx = bundle.type === "fan-in" ? 0 : bundle.sharedPath.length - 1;
		const adjacentIdx = bundle.type === "fan-in" ? 1 : bundle.sharedPath.length - 2;
		const sharedDir = determineDirection(bundle.sharedPath[junctionIdx]!, bundle.sharedPath[adjacentIdx]!);
		if (dirEquals(sharedDir, Down)) hasDown = true;
		else if (dirEquals(sharedDir, Up)) hasUp = true;
		else if (dirEquals(sharedDir, Right)) hasRight = true;
		else if (dirEquals(sharedDir, Left)) hasLeft = true;
	}

	for (const edge of bundle.edges) {
		if (edge.pathToJunction && edge.pathToJunction.length >= 2) {
			const junctionIdx = bundle.type === "fan-in" ? edge.pathToJunction.length - 1 : 0;
			const adjacentIdx = bundle.type === "fan-in" ? edge.pathToJunction.length - 2 : 1;

			const arrivalDir = determineDirection(edge.pathToJunction[adjacentIdx]!, edge.pathToJunction[junctionIdx]!);
			if (dirEquals(arrivalDir, Down)) hasUp = true;
			else if (dirEquals(arrivalDir, Up)) hasDown = true;
			else if (dirEquals(arrivalDir, Right)) hasLeft = true;
			else if (dirEquals(arrivalDir, Left)) hasRight = true;
		}
	}

	let char: string;
	if (!useAscii) {
		if (hasUp && hasDown && hasLeft && hasRight) char = "┼";
		else if (hasDown && hasLeft && hasRight && !hasUp) char = "┬";
		else if (hasUp && hasLeft && hasRight && !hasDown) char = "┴";
		else if (hasUp && hasDown && hasRight && !hasLeft) char = "├";
		else if (hasUp && hasDown && hasLeft && !hasRight) char = "┤";
		else if (hasLeft && hasRight) char = "─";
		else if (hasUp && hasDown) char = "│";
		else if (hasDown && hasRight) char = "┌";
		else if (hasDown && hasLeft) char = "┐";
		else if (hasUp && hasRight) char = "└";
		else if (hasUp && hasLeft) char = "┘";
		else char = "┼";
	} else {
		char = "+";
	}

	canvas[dc.x]![dc.y] = char;
	return canvas;
}

// ============================================================================
// Subgraph drawing
// ============================================================================

export function drawSubgraphBox(sg: AsciiSubgraph, graph: AsciiGraph): Canvas {
	const width = sg.maxX - sg.minX;
	const height = sg.maxY - sg.minY;
	if (width <= 0 || height <= 0) return mkCanvas(0, 0);

	const from: DrawingCoord = { x: 0, y: 0 };
	const to: DrawingCoord = { x: width, y: height };
	const canvas = mkCanvas(width, height);

	if (!graph.config.useAscii) {
		for (let x = from.x + 1; x < to.x; x++) canvas[x]![from.y] = "─";
		for (let x = from.x + 1; x < to.x; x++) canvas[x]![to.y] = "─";
		for (let y = from.y + 1; y < to.y; y++) canvas[from.x]![y] = "│";
		for (let y = from.y + 1; y < to.y; y++) canvas[to.x]![y] = "│";
		canvas[from.x]![from.y] = "┌";
		canvas[to.x]![from.y] = "┐";
		canvas[from.x]![to.y] = "└";
		canvas[to.x]![to.y] = "┘";
	} else {
		for (let x = from.x + 1; x < to.x; x++) canvas[x]![from.y] = "-";
		for (let x = from.x + 1; x < to.x; x++) canvas[x]![to.y] = "-";
		for (let y = from.y + 1; y < to.y; y++) canvas[from.x]![y] = "|";
		for (let y = from.y + 1; y < to.y; y++) canvas[to.x]![y] = "|";
		canvas[from.x]![from.y] = "+";
		canvas[to.x]![from.y] = "+";
		canvas[from.x]![to.y] = "+";
		canvas[to.x]![to.y] = "+";
	}

	return canvas;
}

export function drawSubgraphLabel(sg: AsciiSubgraph, _graph: AsciiGraph): [Canvas, DrawingCoord] {
	const width = sg.maxX - sg.minX;
	const height = sg.maxY - sg.minY;
	if (width <= 0 || height <= 0) return [mkCanvas(0, 0), { x: 0, y: 0 }];

	const canvas = mkCanvas(width, height);

	const lines = splitLines(sg.name);

	for (let i = 0; i < lines.length; i++) {
		const line = lines[i]!;
		const labelY = 1 + i;
		const lineVW = visibleWidth(line);
		let labelX = Math.floor(width / 2) - Math.floor(lineVW / 2);
		if (labelX < 1) labelX = 1;

		let col = labelX;
		for (const char of line) {
			const charWidth = visibleWidth(char);
			if (col < width && labelY < height) {
				canvas[col]![labelY] = char;
				for (let cw = 1; cw < charWidth; cw++) {
					if (col + cw < width) canvas[col + cw]![labelY] = "";
				}
			}
			col += charWidth;
		}
	}

	return [canvas, { x: sg.minX, y: sg.minY }];
}

// ============================================================================
// Role tracking helpers
// ============================================================================

function fillRolesFromCanvas(roleCanvas: RoleCanvas, canvas: Canvas, offset: DrawingCoord, role: CharRole): void {
	for (let x = 0; x < canvas.length; x++) {
		for (let y = 0; y < (canvas[0]?.length ?? 0); y++) {
			const char = canvas[x]?.[y];
			if (char && char !== " ") {
				const rx = x + offset.x;
				const ry = y + offset.y;
				if (rx >= 0 && ry >= 0) {
					setRole(roleCanvas, rx, ry, role);
				}
			}
		}
	}
}

function fillRolesFromCanvases(roleCanvas: RoleCanvas, canvases: Canvas[], offset: DrawingCoord, role: CharRole): void {
	for (const canvas of canvases) {
		fillRolesFromCanvas(roleCanvas, canvas, offset, role);
	}
}

function fillRolesForNodeBox(roleCanvas: RoleCanvas, canvas: Canvas, offset: DrawingCoord): void {
	const lastX = canvas.length - 1;
	const lastY = (canvas[0]?.length ?? 1) - 1;

	for (let x = 0; x <= lastX; x++) {
		for (let y = 0; y <= lastY; y++) {
			const char = canvas[x]?.[y];
			if (!char || char === " ") continue;
			const rx = x + offset.x;
			const ry = y + offset.y;
			if (rx < 0 || ry < 0) continue;
			const isPerimeter = x === 0 || x === lastX || y === 0 || y === lastY;
			setRole(roleCanvas, rx, ry, isPerimeter ? "border" : "text");
		}
	}

	for (let y = 1; y < lastY; y++) {
		let first = -1;
		let last = -1;
		for (let x = 1; x < lastX; x++) {
			const c = canvas[x]?.[y];
			if (c && c !== " ") {
				if (first === -1) first = x;
				last = x;
			}
		}
		if (first !== -1) {
			for (let x = first; x <= last; x++) {
				if (canvas[x]?.[y] === " ") {
					const rx = x + offset.x;
					const ry = y + offset.y;
					if (rx >= 0 && ry >= 0) setRole(roleCanvas, rx, ry, "text");
				}
			}
		}
	}
}

// ============================================================================
// Top-level draw orchestrator
// ============================================================================

function sortSubgraphsByDepth(subgraphs: AsciiSubgraph[]): AsciiSubgraph[] {
	function getDepth(sg: AsciiSubgraph): number {
		return sg.parent === null ? 0 : 1 + getDepth(sg.parent);
	}
	const sorted = [...subgraphs];
	sorted.sort((a, b) => getDepth(a) - getDepth(b));
	return sorted;
}

export function drawGraph(graph: AsciiGraph): Canvas {
	const useAscii = graph.config.useAscii;
	const zero: DrawingCoord = { x: 0, y: 0 };

	const sortedSgs = sortSubgraphsByDepth(graph.subgraphs);
	for (const sg of sortedSgs) {
		const sgCanvas = drawSubgraphBox(sg, graph);
		const offset: DrawingCoord = { x: sg.minX, y: sg.minY };
		graph.canvas = mergeCanvases(graph.canvas, offset, useAscii, sgCanvas);
		fillRolesFromCanvas(graph.roleCanvas, sgCanvas, offset, "border");
	}

	for (const node of graph.nodes) {
		if (!node.drawn && node.drawingCoord && node.drawing) {
			graph.canvas = mergeCanvases(graph.canvas, node.drawingCoord, useAscii, node.drawing);
			fillRolesForNodeBox(graph.roleCanvas, node.drawing, node.drawingCoord);
			node.drawn = true;
		}
	}

	const lineCanvases: Canvas[] = [];
	const cornerCanvases: Canvas[] = [];
	const arrowHeadEndCanvases: Canvas[] = [];
	const arrowHeadStartCanvases: Canvas[] = [];
	const boxStartCanvases: Canvas[] = [];
	const sharedLabelCanvas = copyCanvas(graph.canvas);
	const labelRegions: LabelRegion[] = [];
	const junctionCanvases: Canvas[] = [];

	const processedBundles = new Set<EdgeBundle>();

	for (const edge of graph.edges) {
		if (edge.bundle && edge.pathToJunction) {
			const bundle = edge.bundle;

			const [pathC, boxStartC, , , cornersC] = drawBundledEdgeSegment(graph, edge, bundle);
			lineCanvases.push(pathC);
			cornerCanvases.push(cornersC);
			boxStartCanvases.push(boxStartC);

			if (!processedBundles.has(bundle)) {
				processedBundles.add(bundle);

				const [sharedPathC, sharedCornersC] = drawBundleSharedPath(graph, bundle);
				lineCanvases.push(sharedPathC);
				cornerCanvases.push(sharedCornersC);

				if (bundle.type === "fan-in") {
					const arrowHeadC = drawBundleArrowhead(graph, bundle);
					arrowHeadEndCanvases.push(arrowHeadC);
				}

				const junctionC = drawJunctionCharacter(graph, bundle);
				junctionCanvases.push(junctionC);
			}

			if (bundle.type === "fan-out" && edge.hasArrowEnd) {
				const arrowHeadC = drawBundledEdgeArrowhead(graph, edge);
				arrowHeadEndCanvases.push(arrowHeadC);
			}
		} else {
			const [pathC, boxStartC, arrowHeadEndC, arrowHeadStartC, cornersC] = drawArrow(
				graph,
				edge,
				sharedLabelCanvas,
				labelRegions,
			);
			lineCanvases.push(pathC);
			cornerCanvases.push(cornersC);
			arrowHeadEndCanvases.push(arrowHeadEndC);
			arrowHeadStartCanvases.push(arrowHeadStartC);
			boxStartCanvases.push(boxStartC);
		}
	}

	graph.canvas = mergeCanvases(graph.canvas, zero, useAscii, ...lineCanvases);
	fillRolesFromCanvases(graph.roleCanvas, lineCanvases, zero, "line");

	graph.canvas = mergeCanvases(graph.canvas, zero, useAscii, ...cornerCanvases);
	fillRolesFromCanvases(graph.roleCanvas, cornerCanvases, zero, "corner");

	graph.canvas = mergeCanvases(graph.canvas, zero, useAscii, ...junctionCanvases);
	fillRolesFromCanvases(graph.roleCanvas, junctionCanvases, zero, "junction");

	graph.canvas = mergeCanvases(graph.canvas, zero, useAscii, ...arrowHeadEndCanvases);
	fillRolesFromCanvases(graph.roleCanvas, arrowHeadEndCanvases, zero, "arrow");

	graph.canvas = mergeCanvases(graph.canvas, zero, useAscii, ...boxStartCanvases);
	fillRolesFromCanvases(graph.roleCanvas, boxStartCanvases, zero, "junction");

	graph.canvas = mergeCanvases(graph.canvas, zero, useAscii, ...arrowHeadStartCanvases);
	fillRolesFromCanvases(graph.roleCanvas, arrowHeadStartCanvases, zero, "arrow");

	for (const region of labelRegions) {
		for (let x = region.x; x < region.x + region.width; x++) {
			if (x >= 0 && x < graph.canvas.length) {
				graph.canvas[x]![region.y] = " ";
			}
		}
	}

	graph.canvas = mergeCanvases(graph.canvas, zero, useAscii, sharedLabelCanvas);
	fillRolesFromCanvas(graph.roleCanvas, sharedLabelCanvas, zero, "text");

	for (const sg of graph.subgraphs) {
		if (sg.nodes.length === 0) continue;
		const [labelCanvas, offset] = drawSubgraphLabel(sg, graph);
		graph.canvas = mergeCanvases(graph.canvas, offset, useAscii, labelCanvas);
		fillRolesFromCanvas(graph.roleCanvas, labelCanvas, offset, "text");
	}

	return graph.canvas;
}
