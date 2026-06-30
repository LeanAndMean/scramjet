// Based on beautiful-mermaid by Craft Docs, MIT License.

// ============================================================================
// Parser output types — logical structure extracted from Mermaid text
// ============================================================================

export interface MermaidGraph {
	direction: GraphDirection;
	nodes: Map<string, MermaidNode>;
	edges: MermaidEdge[];
	subgraphs: MermaidSubgraph[];
	classDefs: Map<string, Record<string, string>>;
	classAssignments: Map<string, string>;
	nodeStyles: Map<string, Record<string, string>>;
	linkStyles: Map<number | "default", Record<string, string>>;
}

export type GraphDirection = "TD" | "TB" | "LR" | "BT" | "RL";

export interface MermaidNode {
	id: string;
	label: string;
	shape: NodeShape;
}

export type NodeShape =
	| "rectangle"
	| "rounded"
	| "diamond"
	| "stadium"
	| "circle"
	| "subroutine"
	| "doublecircle"
	| "hexagon"
	| "cylinder"
	| "asymmetric"
	| "trapezoid"
	| "trapezoid-alt"
	| "state-start"
	| "state-end";

export interface MermaidEdge {
	source: string;
	target: string;
	label?: string;
	style: EdgeStyle;
	hasArrowStart: boolean;
	hasArrowEnd: boolean;
}

export type EdgeStyle = "solid" | "dotted" | "thick";

export interface MermaidSubgraph {
	id: string;
	label: string;
	nodeIds: string[];
	children: MermaidSubgraph[];
	direction?: GraphDirection;
}

// ============================================================================
// Renderer internal types — grid-based coordinate system and canvas
// ============================================================================

export interface GridCoord {
	x: number;
	y: number;
}

export interface DrawingCoord {
	x: number;
	y: number;
}

/**
 * Grid direction — positions on a node's 3x3 grid block.
 *
 *   (0,0) UL   (1,0) Up   (2,0) UR
 *   (0,1) Left (1,1) Mid  (2,1) Right
 *   (0,2) LL   (1,2) Down (2,2) LR
 */
export interface GridDirection {
	readonly x: number;
	readonly y: number;
}

export const Up: GridDirection = { x: 1, y: 0 };
export const Down: GridDirection = { x: 1, y: 2 };
export const Left: GridDirection = { x: 0, y: 1 };
export const Right: GridDirection = { x: 2, y: 1 };
export const UpperRight: GridDirection = { x: 2, y: 0 };
export const UpperLeft: GridDirection = { x: 0, y: 0 };
export const LowerRight: GridDirection = { x: 2, y: 2 };
export const LowerLeft: GridDirection = { x: 0, y: 2 };
export const Middle: GridDirection = { x: 1, y: 1 };

export const ALL_DIRECTIONS: readonly GridDirection[] = [
	Up,
	Down,
	Left,
	Right,
	UpperRight,
	UpperLeft,
	LowerRight,
	LowerLeft,
	Middle,
];

/** 2D text canvas — column-major (canvas[x][y]). */
export type Canvas = string[][];

export interface AsciiNode {
	name: string;
	displayLabel: string;
	shape: NodeShape;
	index: number;
	gridCoord: GridCoord | null;
	drawingCoord: DrawingCoord | null;
	drawing: Canvas | null;
	drawn: boolean;
	styleClassName: string;
	styleClass: AsciiStyleClass;
}

export interface AsciiStyleClass {
	name: string;
	styles: Record<string, string>;
}

export type AsciiEdgeStyle = "solid" | "dotted" | "thick";

export interface AsciiEdge {
	from: AsciiNode;
	to: AsciiNode;
	text: string;
	path: GridCoord[];
	labelLine: GridCoord[];
	startDir: GridDirection;
	endDir: GridDirection;
	style: AsciiEdgeStyle;
	hasArrowStart: boolean;
	hasArrowEnd: boolean;
	bundle?: EdgeBundle;
	pathToJunction?: GridCoord[];
}

export interface AsciiSubgraph {
	name: string;
	nodes: AsciiNode[];
	parent: AsciiSubgraph | null;
	children: AsciiSubgraph[];
	minX: number;
	minY: number;
	maxX: number;
	maxY: number;
	direction?: "LR" | "TD";
}

export interface AsciiConfig {
	useAscii: boolean;
	paddingX: number;
	paddingY: number;
	boxBorderPadding: number;
	graphDirection: "LR" | "TD";
}

export interface AsciiGraph {
	nodes: AsciiNode[];
	edges: AsciiEdge[];
	canvas: Canvas;
	roleCanvas: RoleCanvas;
	grid: Map<string, AsciiNode>;
	columnWidth: Map<number, number>;
	rowHeight: Map<number, number>;
	subgraphs: AsciiSubgraph[];
	config: AsciiConfig;
	offsetX: number;
	offsetY: number;
	bundles: EdgeBundle[];
}

// ============================================================================
// Coordinate helpers
// ============================================================================

export function gridCoordEquals(a: GridCoord, b: GridCoord): boolean {
	return a.x === b.x && a.y === b.y;
}

export function drawingCoordEquals(a: DrawingCoord, b: DrawingCoord): boolean {
	return a.x === b.x && a.y === b.y;
}

export function gridCoordDirection(c: GridCoord, dir: GridDirection): GridCoord {
	return { x: c.x + dir.x, y: c.y + dir.y };
}

export function gridKey(c: GridCoord): string {
	return `${c.x},${c.y}`;
}

export const EMPTY_STYLE: AsciiStyleClass = { name: "", styles: {} };

// ============================================================================
// Character role types
// ============================================================================

export type CharRole = "text" | "border" | "line" | "arrow" | "corner" | "junction";

export type RoleCanvas = (CharRole | null)[][];

// ============================================================================
// Edge bundling types
// ============================================================================

export interface EdgeBundle {
	type: "fan-in" | "fan-out";
	edges: AsciiEdge[];
	sharedNode: AsciiNode;
	otherNodes: AsciiNode[];
	junctionPoint: GridCoord | null;
	sharedPath: GridCoord[];
	junctionDir: GridDirection;
	sharedNodeDir: GridDirection;
}
