// Based on beautiful-mermaid by Craft Docs, MIT License.

import { visibleWidth } from "@leanandmean/tui";
import { splitLines } from "./multiline-utils.js";
import type { DrawingCoord, GridDirection, NodeShape } from "./types.js";
import { Down, dirEquals, Left, LowerLeft, LowerRight, Right, Up, UpperLeft, UpperRight } from "./types.js";

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

export function getCorners(shape: NodeShape, useAscii: boolean): CornerChars {
	const corners = SHAPE_CORNERS[shape] ?? SHAPE_CORNERS.rectangle;
	return useAscii ? corners.ascii : corners.unicode;
}

// ============================================================================
// Direction comparison
// ============================================================================

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
	getAttachmentPoint: getBoxAttachmentPoint,
};

const roundedRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	getAttachmentPoint: getBoxAttachmentPoint,
};

const diamondRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	getAttachmentPoint: getBoxAttachmentPoint,
};

const circleRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	getAttachmentPoint: getBoxAttachmentPoint,
};

const hexagonRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	getAttachmentPoint: getBoxAttachmentPoint,
};

const doublecircleRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	getAttachmentPoint: getBoxAttachmentPoint,
};

const asymmetricRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	getAttachmentPoint: getBoxAttachmentPoint,
};

const trapezoidRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
	getAttachmentPoint: getBoxAttachmentPoint,
};

const trapezoidAltRenderer: ShapeRenderer = {
	getDimensions: getBoxDimensions,
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

function getShapeRenderer(shape: NodeShape): ShapeRenderer {
	return shapeRegistry.get(shape) ?? rectangleRenderer;
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
