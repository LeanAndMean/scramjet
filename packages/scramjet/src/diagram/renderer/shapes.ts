// Based on beautiful-mermaid by Craft Docs, MIT License.

import { visibleWidth } from "@leanandmean/tui";
import { mkCanvas } from "./canvas.js";
import { splitLines } from "./multiline-utils.js";
import type { Canvas, DrawingCoord, GridDirection, NodeShape } from "./types.js";
import { Down, Left, LowerLeft, LowerRight, Right, Up, UpperLeft, UpperRight } from "./types.js";

// ============================================================================
// Shape dimension and renderer types
// ============================================================================

export interface ShapeDimensions {
	width: number;
	height: number;
	labelArea: { x: number; y: number; width: number; height: number };
	gridColumns: [number, number, number];
	gridRows: [number, number, number];
}

export interface ShapeRenderOptions {
	useAscii: boolean;
	padding: number;
}

export interface ShapeRenderer {
	getDimensions(label: string, options: ShapeRenderOptions): ShapeDimensions;
	render(label: string, dimensions: ShapeDimensions, options: ShapeRenderOptions): Canvas;
	getAttachmentPoint(dir: GridDirection, dimensions: ShapeDimensions, baseCoord: DrawingCoord): DrawingCoord;
}

// ============================================================================
// Corner characters
// ============================================================================

interface CornerChars {
	tl: string;
	tr: string;
	bl: string;
	br: string;
}

interface ShapeCorners {
	unicode: CornerChars;
	ascii: CornerChars;
}

const SHAPE_CORNERS: Record<NodeShape, ShapeCorners> = {
	rectangle: {
		unicode: { tl: "┌", tr: "┐", bl: "└", br: "┘" },
		ascii: { tl: "+", tr: "+", bl: "+", br: "+" },
	},
	rounded: {
		unicode: { tl: "╭", tr: "╮", bl: "╰", br: "╯" },
		ascii: { tl: ".", tr: ".", bl: "'", br: "'" },
	},
	circle: {
		unicode: { tl: "◯", tr: "◯", bl: "◯", br: "◯" },
		ascii: { tl: "o", tr: "o", bl: "o", br: "o" },
	},
	doublecircle: {
		unicode: { tl: "◎", tr: "◎", bl: "◎", br: "◎" },
		ascii: { tl: "@", tr: "@", bl: "@", br: "@" },
	},
	diamond: {
		unicode: { tl: "◇", tr: "◇", bl: "◇", br: "◇" },
		ascii: { tl: "<", tr: ">", bl: "<", br: ">" },
	},
	hexagon: {
		unicode: { tl: "⌜", tr: "⌝", bl: "⌞", br: "⌟" },
		ascii: { tl: "*", tr: "*", bl: "*", br: "*" },
	},
	stadium: {
		unicode: { tl: "(", tr: ")", bl: "(", br: ")" },
		ascii: { tl: "(", tr: ")", bl: "(", br: ")" },
	},
	subroutine: {
		unicode: { tl: "╟", tr: "╢", bl: "╟", br: "╢" },
		ascii: { tl: "|", tr: "|", bl: "|", br: "|" },
	},
	cylinder: {
		unicode: { tl: "╭", tr: "╮", bl: "╰", br: "╯" },
		ascii: { tl: ".", tr: ".", bl: "'", br: "'" },
	},
	asymmetric: {
		unicode: { tl: "▷", tr: "┐", bl: "▷", br: "┘" },
		ascii: { tl: ">", tr: "+", bl: ">", br: "+" },
	},
	trapezoid: {
		unicode: { tl: "/", tr: "\\", bl: "└", br: "┘" },
		ascii: { tl: "/", tr: "\\", bl: "+", br: "+" },
	},
	"trapezoid-alt": {
		unicode: { tl: "┌", tr: "┐", bl: "\\", br: "/" },
		ascii: { tl: "+", tr: "+", bl: "\\", br: "/" },
	},
	"state-start": {
		unicode: { tl: "●", tr: "●", bl: "●", br: "●" },
		ascii: { tl: "*", tr: "*", bl: "*", br: "*" },
	},
	"state-end": {
		unicode: { tl: "◉", tr: "◉", bl: "◉", br: "◉" },
		ascii: { tl: "@", tr: "@", bl: "@", br: "@" },
	},
};

function getCorners(shape: NodeShape, useAscii: boolean): CornerChars {
	const corners = SHAPE_CORNERS[shape] ?? SHAPE_CORNERS.rectangle;
	return useAscii ? corners.ascii : corners.unicode;
}

// ============================================================================
// Direction comparison
// ============================================================================

export function dirEquals(a: GridDirection, b: GridDirection): boolean {
	return a.x === b.x && a.y === b.y;
}

// ============================================================================
// Shared box rendering logic
// ============================================================================

function getBoxDimensions(label: string, options: ShapeRenderOptions): ShapeDimensions {
	const lines = splitLines(label);
	const maxLW = Math.max(...lines.map((l) => visibleWidth(l)), 0);
	const lc = lines.length;

	const innerWidth = 2 * options.padding + maxLW;
	const width = innerWidth + 2;
	const rawInnerHeight = lc + 2 * options.padding;
	const innerHeight = rawInnerHeight % 2 === 0 ? rawInnerHeight + 1 : rawInnerHeight;
	const height = innerHeight + 2;

	return {
		width,
		height,
		labelArea: { x: 1 + options.padding, y: 1 + options.padding, width: maxLW, height: lc },
		gridColumns: [1, innerWidth, 1],
		gridRows: [1, innerHeight, 1],
	};
}

function renderBox(label: string, dimensions: ShapeDimensions, corners: CornerChars, useAscii: boolean): Canvas {
	const { width, height } = dimensions;
	const canvas = mkCanvas(width - 1, height - 1);

	const hLine = useAscii ? "-" : "─";
	const vLine = useAscii ? "|" : "│";

	for (let x = 1; x < width - 1; x++) {
		canvas[x]![0] = hLine;
		canvas[x]![height - 1] = hLine;
	}
	for (let y = 1; y < height - 1; y++) {
		canvas[0]![y] = vLine;
		canvas[width - 1]![y] = vLine;
	}

	canvas[0]![0] = corners.tl;
	canvas[width - 1]![0] = corners.tr;
	canvas[0]![height - 1] = corners.bl;
	canvas[width - 1]![height - 1] = corners.br;

	const lines = splitLines(label);
	const w = width - 1;
	const h = height - 1;
	const centerY = Math.floor(h / 2);
	const startY = centerY - Math.floor((lines.length - 1) / 2);

	for (let i = 0; i < lines.length; i++) {
		const line = lines[i]!;
		const lw = visibleWidth(line);
		const textX = Math.floor(w / 2) - Math.ceil(lw / 2) + 1;
		let col = textX;
		for (const ch of line) {
			const cw = visibleWidth(ch);
			const y = startY + i;
			if (col >= 0 && col < canvas.length && y >= 0 && y < canvas[0]!.length) {
				canvas[col]![y] = ch;
				for (let pw = 1; pw < cw; pw++) {
					if (col + pw < canvas.length) canvas[col + pw]![y] = "";
				}
			}
			col += cw;
		}
	}

	return canvas;
}

function getBoxAttachmentPoint(dir: GridDirection, dimensions: ShapeDimensions, baseCoord: DrawingCoord): DrawingCoord {
	const { width, height } = dimensions;
	const centerX = baseCoord.x + Math.floor(width / 2);
	const centerY = baseCoord.y + Math.floor(height / 2);

	if (dirEquals(dir, Up)) return { x: centerX, y: baseCoord.y };
	if (dirEquals(dir, Down)) return { x: centerX, y: baseCoord.y + height - 1 };
	if (dirEquals(dir, Left)) return { x: baseCoord.x, y: centerY };
	if (dirEquals(dir, Right)) return { x: baseCoord.x + width - 1, y: centerY };
	if (dirEquals(dir, UpperLeft)) return { x: baseCoord.x, y: baseCoord.y };
	if (dirEquals(dir, UpperRight)) return { x: baseCoord.x + width - 1, y: baseCoord.y };
	if (dirEquals(dir, LowerLeft)) return { x: baseCoord.x, y: baseCoord.y + height - 1 };
	if (dirEquals(dir, LowerRight)) return { x: baseCoord.x + width - 1, y: baseCoord.y + height - 1 };
	return { x: centerX, y: centerY };
}

// ============================================================================
// Shape renderers
// ============================================================================

const rectangleRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	render(label, dimensions, options) {
		return renderBox(label, dimensions, getCorners("rectangle", options.useAscii), options.useAscii);
	},
	getAttachmentPoint: getBoxAttachmentPoint,
};

const roundedRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	render(label, dimensions, options) {
		return renderBox(label, dimensions, getCorners("rounded", options.useAscii), options.useAscii);
	},
	getAttachmentPoint: getBoxAttachmentPoint,
};

const diamondRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	render(label, dimensions, options) {
		return renderBox(label, dimensions, getCorners("diamond", options.useAscii), options.useAscii);
	},
	getAttachmentPoint: getBoxAttachmentPoint,
};

const circleRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	render(label, dimensions, options) {
		return renderBox(label, dimensions, getCorners("circle", options.useAscii), options.useAscii);
	},
	getAttachmentPoint: getBoxAttachmentPoint,
};

const hexagonRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	render(label, dimensions, options) {
		return renderBox(label, dimensions, getCorners("hexagon", options.useAscii), options.useAscii);
	},
	getAttachmentPoint: getBoxAttachmentPoint,
};

const doublecircleRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	render(label, dimensions, options) {
		return renderBox(label, dimensions, getCorners("doublecircle", options.useAscii), options.useAscii);
	},
	getAttachmentPoint: getBoxAttachmentPoint,
};

const asymmetricRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	render(label, dimensions, options) {
		return renderBox(label, dimensions, getCorners("asymmetric", options.useAscii), options.useAscii);
	},
	getAttachmentPoint: getBoxAttachmentPoint,
};

const trapezoidRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	render(label, dimensions, options) {
		return renderBox(label, dimensions, getCorners("trapezoid", options.useAscii), options.useAscii);
	},
	getAttachmentPoint: getBoxAttachmentPoint,
};

const trapezoidAltRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	render(label, dimensions, options) {
		return renderBox(label, dimensions, getCorners("trapezoid-alt", options.useAscii), options.useAscii);
	},
	getAttachmentPoint: getBoxAttachmentPoint,
};

// ============================================================================
// Subroutine — custom double-border rendering
// ============================================================================

const subroutineRenderer: ShapeRenderer = {
	getDimensions(label, options) {
		const lines = splitLines(label);
		const maxLW = Math.max(...lines.map((l) => visibleWidth(l)), 0);
		const lc = lines.length;

		const innerWidth = 2 * options.padding + maxLW;
		const width = innerWidth + 4;
		const innerHeight = lc + 2 * options.padding;
		const height = innerHeight + 2;

		return {
			width,
			height,
			labelArea: { x: 2 + options.padding, y: 1 + options.padding, width: maxLW, height: lc },
			gridColumns: [2, innerWidth, 2],
			gridRows: [1, innerHeight, 1],
		};
	},

	render(label, dimensions, options) {
		const { width, height } = dimensions;
		const canvas = mkCanvas(width - 1, height - 1);

		const hChar = options.useAscii ? "-" : "─";
		const vChar = options.useAscii ? "|" : "│";

		canvas[0]![0] = options.useAscii ? "+" : "┌";
		canvas[1]![0] = options.useAscii ? "+" : "┬";
		for (let x = 2; x < width - 2; x++) canvas[x]![0] = hChar;
		canvas[width - 2]![0] = options.useAscii ? "+" : "┬";
		canvas[width - 1]![0] = options.useAscii ? "+" : "┐";

		for (let y = 1; y < height - 1; y++) {
			canvas[0]![y] = vChar;
			canvas[1]![y] = vChar;
			canvas[width - 2]![y] = vChar;
			canvas[width - 1]![y] = vChar;
		}

		canvas[0]![height - 1] = options.useAscii ? "+" : "└";
		canvas[1]![height - 1] = options.useAscii ? "+" : "┴";
		for (let x = 2; x < width - 2; x++) canvas[x]![height - 1] = hChar;
		canvas[width - 2]![height - 1] = options.useAscii ? "+" : "┴";
		canvas[width - 1]![height - 1] = options.useAscii ? "+" : "┘";

		const lines = splitLines(label);
		const centerY = Math.floor(height / 2);
		const startY = centerY - Math.floor((lines.length - 1) / 2);

		for (let i = 0; i < lines.length; i++) {
			const line = lines[i]!;
			const lw = visibleWidth(line);
			const textX = Math.floor(width / 2) - Math.floor(lw / 2);
			let col = textX;
			for (const ch of line) {
				const cw = visibleWidth(ch);
				const y = startY + i;
				if (col > 1 && col < width - 2 && y > 0 && y < height - 1) {
					canvas[col]![y] = ch;
					for (let pw = 1; pw < cw; pw++) {
						if (col + pw < canvas.length) canvas[col + pw]![y] = "";
					}
				}
				col += cw;
			}
		}

		return canvas;
	},

	getAttachmentPoint: getBoxAttachmentPoint,
};

// ============================================================================
// Stadium — special parentheses rendering
// ============================================================================

const stadiumRenderer: ShapeRenderer = {
	getDimensions(label, options) {
		const lines = splitLines(label);
		const maxLW = Math.max(...lines.map((l) => visibleWidth(l)), 0);
		const lc = lines.length;

		const innerWidth = 2 * options.padding + maxLW;
		const width = innerWidth + 4;
		const innerHeight = lc + 2 * options.padding;
		const height = Math.max(innerHeight + 2, 3);

		return {
			width,
			height,
			labelArea: { x: 2 + options.padding, y: 1 + options.padding, width: maxLW, height: lc },
			gridColumns: [2, innerWidth, 2],
			gridRows: [1, innerHeight, 1],
		};
	},

	render(label, dimensions, options) {
		const { width, height } = dimensions;
		const canvas = mkCanvas(width - 1, height - 1);

		const centerY = Math.floor(height / 2);
		const hChar = options.useAscii ? "-" : "─";

		if (height === 3) {
			canvas[0]![centerY] = "(";
			canvas[width - 1]![centerY] = ")";
		} else if (!options.useAscii) {
			canvas[0]![0] = "╭";
			for (let x = 1; x < width - 1; x++) canvas[x]![0] = hChar;
			canvas[width - 1]![0] = "╮";
			for (let y = 1; y < height - 1; y++) {
				canvas[0]![y] = "│";
				canvas[width - 1]![y] = "│";
			}
			canvas[0]![height - 1] = "╰";
			for (let x = 1; x < width - 1; x++) canvas[x]![height - 1] = hChar;
			canvas[width - 1]![height - 1] = "╯";
		} else {
			for (let y = 0; y < height; y++) {
				canvas[0]![y] = "(";
				canvas[width - 1]![y] = ")";
			}
			for (let x = 1; x < width - 1; x++) {
				canvas[x]![0] = hChar;
				canvas[x]![height - 1] = hChar;
			}
		}

		const lines = splitLines(label);
		const startY = centerY - Math.floor((lines.length - 1) / 2);

		for (let i = 0; i < lines.length; i++) {
			const line = lines[i]!;
			const lw = visibleWidth(line);
			const textX = Math.floor(width / 2) - Math.floor(lw / 2);
			let col = textX;
			for (const ch of line) {
				const cw = visibleWidth(ch);
				const y = startY + i;
				if (col > 0 && col < width - 1 && y >= 0 && y < height) {
					canvas[col]![y] = ch;
					for (let pw = 1; pw < cw; pw++) {
						if (col + pw < canvas.length) canvas[col + pw]![y] = "";
					}
				}
				col += cw;
			}
		}

		return canvas;
	},

	getAttachmentPoint: getBoxAttachmentPoint,
};

// ============================================================================
// Cylinder — database shape
// ============================================================================

const cylinderRenderer: ShapeRenderer = {
	getDimensions(label, options) {
		const lines = splitLines(label);
		const maxLW = Math.max(...lines.map((l) => visibleWidth(l)), 0);
		const lc = lines.length;

		const innerWidth = 2 * options.padding + maxLW;
		const width = innerWidth + 2;
		const innerHeight = lc + 2 * options.padding + 2;
		const height = innerHeight + 2;

		return {
			width,
			height,
			labelArea: { x: 1 + options.padding, y: 2 + options.padding, width: maxLW, height: lc },
			gridColumns: [1, innerWidth, 1],
			gridRows: [2, innerHeight - 2, 2],
		};
	},

	render(label, dimensions, options) {
		const { width, height } = dimensions;
		const canvas = mkCanvas(width - 1, height - 1);

		const hChar = options.useAscii ? "-" : "─";
		const vChar = options.useAscii ? "|" : "│";

		canvas[0]![0] = options.useAscii ? "." : "╭";
		for (let x = 1; x < width - 1; x++) canvas[x]![0] = hChar;
		canvas[width - 1]![0] = options.useAscii ? "." : "╮";

		canvas[0]![1] = vChar;
		for (let x = 1; x < width - 1; x++) canvas[x]![1] = hChar;
		canvas[width - 1]![1] = vChar;

		for (let y = 2; y < height - 2; y++) {
			canvas[0]![y] = vChar;
			canvas[width - 1]![y] = vChar;
		}

		canvas[0]![height - 2] = vChar;
		for (let x = 1; x < width - 1; x++) canvas[x]![height - 2] = hChar;
		canvas[width - 1]![height - 2] = vChar;

		canvas[0]![height - 1] = options.useAscii ? "'" : "╰";
		for (let x = 1; x < width - 1; x++) canvas[x]![height - 1] = hChar;
		canvas[width - 1]![height - 1] = options.useAscii ? "'" : "╯";

		const lines = splitLines(label);
		const centerY = Math.floor(height / 2);
		const startY = centerY - Math.floor((lines.length - 1) / 2);

		for (let i = 0; i < lines.length; i++) {
			const line = lines[i]!;
			const lw = visibleWidth(line);
			const textX = Math.floor(width / 2) - Math.floor(lw / 2);
			let col = textX;
			for (const ch of line) {
				const cw = visibleWidth(ch);
				const y = startY + i;
				if (col > 0 && col < width - 1 && y > 1 && y < height - 2) {
					canvas[col]![y] = ch;
					for (let pw = 1; pw < cw; pw++) {
						if (col + pw < canvas.length) canvas[col + pw]![y] = "";
					}
				}
				col += cw;
			}
		}

		return canvas;
	},

	getAttachmentPoint: getBoxAttachmentPoint,
};

// ============================================================================
// State pseudo-states
// ============================================================================

function getSmallAttachmentPoint(
	dir: GridDirection,
	dimensions: ShapeDimensions,
	baseCoord: DrawingCoord,
): DrawingCoord {
	const { width, height } = dimensions;
	const centerX = baseCoord.x + Math.floor(width / 2);
	const centerY = baseCoord.y + Math.floor(height / 2);

	if (dirEquals(dir, Up)) return { x: centerX, y: baseCoord.y };
	if (dirEquals(dir, Down)) return { x: centerX, y: baseCoord.y + height - 1 };
	if (dirEquals(dir, Left)) return { x: baseCoord.x, y: centerY };
	if (dirEquals(dir, Right)) return { x: baseCoord.x + width - 1, y: centerY };
	return { x: centerX, y: centerY };
}

const stateStartRenderer: ShapeRenderer = {
	getDimensions() {
		return {
			width: 5,
			height: 3,
			labelArea: { x: 2, y: 1, width: 1, height: 1 },
			gridColumns: [1, 3, 1],
			gridRows: [1, 1, 1],
		};
	},

	render(_label, dimensions, options) {
		const { width, height } = dimensions;
		const canvas = mkCanvas(width - 1, height - 1);
		const cx = Math.floor(width / 2);

		if (!options.useAscii) {
			canvas[0]![0] = "╭";
			canvas[1]![0] = "─";
			canvas[2]![0] = "─";
			canvas[3]![0] = "─";
			canvas[4]![0] = "╮";
			canvas[0]![1] = "│";
			canvas[cx]![1] = "●";
			canvas[4]![1] = "│";
			canvas[0]![2] = "╰";
			canvas[1]![2] = "─";
			canvas[2]![2] = "─";
			canvas[3]![2] = "─";
			canvas[4]![2] = "╯";
		} else {
			canvas[0]![0] = ".";
			canvas[1]![0] = "-";
			canvas[2]![0] = "-";
			canvas[3]![0] = "-";
			canvas[4]![0] = ".";
			canvas[0]![1] = "|";
			canvas[cx]![1] = "*";
			canvas[4]![1] = "|";
			canvas[0]![2] = "'";
			canvas[1]![2] = "-";
			canvas[2]![2] = "-";
			canvas[3]![2] = "-";
			canvas[4]![2] = "'";
		}

		return canvas;
	},

	getAttachmentPoint: getSmallAttachmentPoint,
};

const stateEndRenderer: ShapeRenderer = {
	getDimensions() {
		return {
			width: 5,
			height: 3,
			labelArea: { x: 2, y: 1, width: 1, height: 1 },
			gridColumns: [1, 3, 1],
			gridRows: [1, 1, 1],
		};
	},

	render(_label, dimensions, options) {
		const { width, height } = dimensions;
		const canvas = mkCanvas(width - 1, height - 1);
		const cx = Math.floor(width / 2);

		if (!options.useAscii) {
			canvas[0]![0] = "╔";
			canvas[1]![0] = "═";
			canvas[2]![0] = "═";
			canvas[3]![0] = "═";
			canvas[4]![0] = "╗";
			canvas[0]![1] = "║";
			canvas[cx]![1] = "◎";
			canvas[4]![1] = "║";
			canvas[0]![2] = "╚";
			canvas[1]![2] = "═";
			canvas[2]![2] = "═";
			canvas[3]![2] = "═";
			canvas[4]![2] = "╝";
		} else {
			canvas[0]![0] = "#";
			canvas[1]![0] = "=";
			canvas[2]![0] = "=";
			canvas[3]![0] = "=";
			canvas[4]![0] = "#";
			canvas[0]![1] = "#";
			canvas[cx]![1] = "*";
			canvas[4]![1] = "#";
			canvas[0]![2] = "#";
			canvas[1]![2] = "=";
			canvas[2]![2] = "=";
			canvas[3]![2] = "=";
			canvas[4]![2] = "#";
		}

		return canvas;
	},

	getAttachmentPoint: getSmallAttachmentPoint,
};

// ============================================================================
// Shape registry
// ============================================================================

const shapeRegistry = new Map<NodeShape, ShapeRenderer>([
	["rectangle", rectangleRenderer],
	["rounded", roundedRenderer],
	["diamond", diamondRenderer],
	["stadium", stadiumRenderer],
	["circle", circleRenderer],
	["subroutine", subroutineRenderer],
	["doublecircle", doublecircleRenderer],
	["hexagon", hexagonRenderer],
	["cylinder", cylinderRenderer],
	["asymmetric", asymmetricRenderer],
	["trapezoid", trapezoidRenderer],
	["trapezoid-alt", trapezoidAltRenderer],
	["state-start", stateStartRenderer],
	["state-end", stateEndRenderer],
]);

export function getShapeRenderer(shape: NodeShape): ShapeRenderer {
	return shapeRegistry.get(shape) ?? rectangleRenderer;
}

export function renderShape(shape: NodeShape, label: string, options: ShapeRenderOptions): Canvas {
	const renderer = getShapeRenderer(shape);
	const dimensions = renderer.getDimensions(label, options);
	return renderer.render(label, dimensions, options);
}

export function getShapeDimensions(shape: NodeShape, label: string, options: ShapeRenderOptions): ShapeDimensions {
	const renderer = getShapeRenderer(shape);
	return renderer.getDimensions(label, options);
}

export function getShapeAttachmentPoint(
	shape: NodeShape,
	dir: GridDirection,
	dimensions: ShapeDimensions,
	baseCoord: DrawingCoord,
): DrawingCoord {
	const renderer = getShapeRenderer(shape);
	return renderer.getAttachmentPoint(dir, dimensions, baseCoord);
}
