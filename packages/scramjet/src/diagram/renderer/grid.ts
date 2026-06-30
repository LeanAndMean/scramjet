// Based on beautiful-mermaid by Craft Docs, MIT License.

import {
	getCanvasSize,
	increaseRoleCanvasSize,
	increaseSize,
	setCanvasSizeToGrid,
	setRoleCanvasSizeToGrid,
} from "./canvas.js";
import { isAncestorOrSelf } from "./converter.js";
import { drawBox } from "./draw.js";
import { analyzeEdgeBundles, processBundles } from "./edge-bundling.js";
import { determineLabelLine, determinePath } from "./edge-routing.js";
import { getShapeDimensions } from "./shapes.js";
import type { AsciiGraph, AsciiNode, AsciiSubgraph, DrawingCoord, GridCoord, GridDirection } from "./types.js";
import { gridKey } from "./types.js";

export function gridToDrawingCoord(graph: AsciiGraph, c: GridCoord, dir?: GridDirection): DrawingCoord {
	const target: GridCoord = dir ? { x: c.x + dir.x, y: c.y + dir.y } : c;

	let x = 0;
	for (let col = 0; col < target.x; col++) {
		x += graph.columnWidth.get(col) ?? 0;
	}

	let y = 0;
	for (let row = 0; row < target.y; row++) {
		y += graph.rowHeight.get(row) ?? 0;
	}

	const colW = graph.columnWidth.get(target.x) ?? 0;
	const rowH = graph.rowHeight.get(target.y) ?? 0;
	return {
		x: x + Math.floor(colW / 2) + graph.offsetX,
		y: y + Math.floor(rowH / 2) + graph.offsetY,
	};
}

export function lineToDrawing(graph: AsciiGraph, line: GridCoord[]): DrawingCoord[] {
	return line.map((c) => gridToDrawingCoord(graph, c));
}

const MAX_GRID_SEARCH = 100;

export function reserveSpotInGrid(
	graph: AsciiGraph,
	node: AsciiNode,
	requested: GridCoord,
	effectiveDir?: "LR" | "TD",
): GridCoord {
	const dir = effectiveDir ?? getEffectiveDirection(graph, node);

	let pos = requested;
	for (let i = 0; i < MAX_GRID_SEARCH; i++) {
		if (!graph.grid.has(gridKey(pos))) {
			for (let dx = 0; dx < 3; dx++) {
				for (let dy = 0; dy < 3; dy++) {
					graph.grid.set(gridKey({ x: pos.x + dx, y: pos.y + dy }), node);
				}
			}
			node.gridCoord = pos;
			return pos;
		}
		pos = dir === "LR" ? { x: pos.x, y: pos.y + 4 } : { x: pos.x + 4, y: pos.y };
	}

	throw new Error("Grid is too dense to place node — exceeded search limit");
}

export function setColumnWidth(graph: AsciiGraph, node: AsciiNode): void {
	const gc = node.gridCoord!;
	const padding = graph.config.boxBorderPadding;

	const shapeDims = getShapeDimensions(node.shape, node.displayLabel, {
		useAscii: graph.config.useAscii,
		padding,
	});

	const colWidths = shapeDims.gridColumns;
	const rowHeights = shapeDims.gridRows;

	for (let idx = 0; idx < colWidths.length; idx++) {
		const xCoord = gc.x + idx;
		const current = graph.columnWidth.get(xCoord) ?? 0;
		graph.columnWidth.set(xCoord, Math.max(current, colWidths[idx]!));
	}

	for (let idx = 0; idx < rowHeights.length; idx++) {
		const yCoord = gc.y + idx;
		const current = graph.rowHeight.get(yCoord) ?? 0;
		graph.rowHeight.set(yCoord, Math.max(current, rowHeights[idx]!));
	}

	if (gc.x > 0) {
		const current = graph.columnWidth.get(gc.x - 1) ?? 0;
		graph.columnWidth.set(gc.x - 1, Math.max(current, graph.config.paddingX));
	}

	if (gc.y > 0) {
		let basePadding = graph.config.paddingY;
		if (hasIncomingEdgeFromOutsideSubgraph(graph, node)) {
			basePadding += 4;
		}
		const current = graph.rowHeight.get(gc.y - 1) ?? 0;
		graph.rowHeight.set(gc.y - 1, Math.max(current, basePadding));
	}
}

export function increaseGridSizeForPath(graph: AsciiGraph, path: GridCoord[]): void {
	for (const c of path) {
		if (!graph.columnWidth.has(c.x)) {
			graph.columnWidth.set(c.x, Math.max(1, Math.floor(graph.config.paddingX / 2)));
		}
		if (!graph.rowHeight.has(c.y)) {
			graph.rowHeight.set(c.y, Math.max(1, Math.floor(graph.config.paddingY / 2)));
		}
	}
}

function isNodeInAnySubgraph(graph: AsciiGraph, node: AsciiNode): boolean {
	return graph.subgraphs.some((sg) => sg.nodes.includes(node));
}

export function getNodeSubgraph(graph: AsciiGraph, node: AsciiNode): AsciiSubgraph | null {
	let innermost: AsciiSubgraph | null = null;
	for (const sg of graph.subgraphs) {
		if (sg.nodes.includes(node)) {
			if (!innermost || isAncestorOrSelf(innermost, sg)) {
				innermost = sg;
			}
		}
	}
	return innermost;
}

export function getEffectiveDirection(graph: AsciiGraph, node: AsciiNode): "LR" | "TD" {
	const sg = getNodeSubgraph(graph, node);
	if (sg?.direction) {
		return sg.direction;
	}
	return graph.config.graphDirection;
}

function hasIncomingEdgeFromOutsideSubgraph(graph: AsciiGraph, node: AsciiNode): boolean {
	const nodeSg = getNodeSubgraph(graph, node);
	if (!nodeSg) return false;

	let hasExternalEdge = false;
	for (const edge of graph.edges) {
		if (edge.to === node) {
			const sourceSg = getNodeSubgraph(graph, edge.from);
			if (sourceSg !== nodeSg) {
				hasExternalEdge = true;
				break;
			}
		}
	}

	if (!hasExternalEdge) return false;

	for (const otherNode of nodeSg.nodes) {
		if (otherNode === node || !otherNode.gridCoord) continue;
		let otherHasExternal = false;
		for (const edge of graph.edges) {
			if (edge.to === otherNode) {
				const sourceSg = getNodeSubgraph(graph, edge.from);
				if (sourceSg !== nodeSg) {
					otherHasExternal = true;
					break;
				}
			}
		}
		if (otherHasExternal && otherNode.gridCoord.y < node.gridCoord!.y) {
			return false;
		}
	}

	return true;
}

function calculateSubgraphBoundingBox(graph: AsciiGraph, sg: AsciiSubgraph): void {
	if (sg.nodes.length === 0) return;

	let minX = 1_000_000;
	let minY = 1_000_000;
	let maxX = -1_000_000;
	let maxY = -1_000_000;

	for (const child of sg.children) {
		calculateSubgraphBoundingBox(graph, child);
		if (child.nodes.length > 0) {
			minX = Math.min(minX, child.minX);
			minY = Math.min(minY, child.minY);
			maxX = Math.max(maxX, child.maxX);
			maxY = Math.max(maxY, child.maxY);
		}
	}

	for (const node of sg.nodes) {
		if (!node.drawingCoord || !node.drawing) continue;
		const nodeMinX = node.drawingCoord.x;
		const nodeMinY = node.drawingCoord.y;
		const nodeMaxX = nodeMinX + node.drawing.length - 1;
		const nodeMaxY = nodeMinY + node.drawing[0]!.length - 1;
		minX = Math.min(minX, nodeMinX);
		minY = Math.min(minY, nodeMinY);
		maxX = Math.max(maxX, nodeMaxX);
		maxY = Math.max(maxY, nodeMaxY);
	}

	const subgraphPadding = 2;
	const subgraphLabelSpace = 2;
	sg.minX = minX - subgraphPadding;
	sg.minY = minY - subgraphPadding - subgraphLabelSpace;
	sg.maxX = maxX + subgraphPadding;
	sg.maxY = maxY + subgraphPadding;
}

function ensureSubgraphSpacing(graph: AsciiGraph): void {
	const minSpacing = 1;
	const rootSubgraphs = graph.subgraphs.filter((sg) => sg.parent === null && sg.nodes.length > 0);

	for (let i = 0; i < rootSubgraphs.length; i++) {
		for (let j = i + 1; j < rootSubgraphs.length; j++) {
			const sg1 = rootSubgraphs[i]!;
			const sg2 = rootSubgraphs[j]!;

			if (sg1.minX < sg2.maxX && sg1.maxX > sg2.minX) {
				if (sg1.maxY >= sg2.minY - minSpacing && sg1.minY < sg2.minY) {
					sg2.minY = sg1.maxY + minSpacing + 1;
				} else if (sg2.maxY >= sg1.minY - minSpacing && sg2.minY < sg1.minY) {
					sg1.minY = sg2.maxY + minSpacing + 1;
				}
			}
			if (sg1.minY < sg2.maxY && sg1.maxY > sg2.minY) {
				if (sg1.maxX >= sg2.minX - minSpacing && sg1.minX < sg2.minX) {
					sg2.minX = sg1.maxX + minSpacing + 1;
				} else if (sg2.maxX >= sg1.minX - minSpacing && sg2.minX < sg1.minX) {
					sg1.minX = sg2.maxX + minSpacing + 1;
				}
			}
		}
	}
}

export function calculateSubgraphBoundingBoxes(graph: AsciiGraph): void {
	for (const sg of graph.subgraphs) {
		calculateSubgraphBoundingBox(graph, sg);
	}
	ensureSubgraphSpacing(graph);
}

export function offsetDrawingForSubgraphs(graph: AsciiGraph): void {
	if (graph.subgraphs.length === 0) return;

	let minX = 0;
	let minY = 0;
	for (const sg of graph.subgraphs) {
		minX = Math.min(minX, sg.minX);
		minY = Math.min(minY, sg.minY);
	}

	const offsetX = -minX;
	const offsetY = -minY;
	if (offsetX === 0 && offsetY === 0) return;

	graph.offsetX = offsetX;
	graph.offsetY = offsetY;

	for (const sg of graph.subgraphs) {
		sg.minX += offsetX;
		sg.minY += offsetY;
		sg.maxX += offsetX;
		sg.maxY += offsetY;
	}

	for (const node of graph.nodes) {
		if (node.drawingCoord) {
			node.drawingCoord.x += offsetX;
			node.drawingCoord.y += offsetY;
		}
	}
}

export function createMapping(graph: AsciiGraph): void {
	const dir = graph.config.graphDirection;
	const highestPositionPerLevel: number[] = new Array(100).fill(0);

	const nodesFound = new Set<string>();
	const initialRoots: AsciiNode[] = [];

	for (const node of graph.nodes) {
		if (!nodesFound.has(node.name)) {
			initialRoots.push(node);
		}
		nodesFound.add(node.name);
		for (const child of getChildren(graph, node)) {
			nodesFound.add(child.name);
		}
	}

	const filteredRoots = initialRoots.filter((node) => {
		const nodeSg = getNodeSubgraph(graph, node);
		if (!nodeSg) return true;

		for (const edge of graph.edges) {
			if (edge.to === node) {
				const sourceSg = getNodeSubgraph(graph, edge.from);
				if (sourceSg !== nodeSg) {
					return false;
				}
			}
		}
		return true;
	});

	// Fall back to initialRoots when all candidates are filtered out (e.g., cross-subgraph cycles)
	const rootNodes = filteredRoots.length > 0 ? filteredRoots : initialRoots;

	let hasExternalRoots = false;
	let hasSubgraphRootsWithEdges = false;
	for (const node of rootNodes) {
		if (isNodeInAnySubgraph(graph, node)) {
			if (getChildren(graph, node).length > 0) hasSubgraphRootsWithEdges = true;
		} else {
			hasExternalRoots = true;
		}
	}
	const shouldSeparate = dir === "LR" && hasExternalRoots && hasSubgraphRootsWithEdges;

	let externalRootNodes: AsciiNode[];
	let subgraphRootNodes: AsciiNode[] = [];

	if (shouldSeparate) {
		externalRootNodes = rootNodes.filter((n) => !isNodeInAnySubgraph(graph, n));
		subgraphRootNodes = rootNodes.filter((n) => isNodeInAnySubgraph(graph, n));
	} else {
		externalRootNodes = rootNodes;
	}

	for (const node of externalRootNodes) {
		const requested: GridCoord =
			dir === "LR" ? { x: 0, y: highestPositionPerLevel[0]! } : { x: highestPositionPerLevel[0]!, y: 0 };
		reserveSpotInGrid(graph, graph.nodes[node.index]!, requested);
		highestPositionPerLevel[0] = highestPositionPerLevel[0]! + 4;
	}

	if (shouldSeparate && subgraphRootNodes.length > 0) {
		const subgraphLevel = 4;
		for (const node of subgraphRootNodes) {
			const requested: GridCoord =
				dir === "LR"
					? { x: subgraphLevel, y: highestPositionPerLevel[subgraphLevel]! }
					: { x: highestPositionPerLevel[subgraphLevel]!, y: subgraphLevel };
			reserveSpotInGrid(graph, graph.nodes[node.index]!, requested);
			highestPositionPerLevel[subgraphLevel] = highestPositionPerLevel[subgraphLevel]! + 4;
		}
	}

	let placedCount = externalRootNodes.length + subgraphRootNodes.length;
	while (placedCount < graph.nodes.length) {
		const prevCount = placedCount;
		for (const node of graph.nodes) {
			if (node.gridCoord === null) continue;
			const gc = node.gridCoord;

			for (const child of getChildren(graph, node)) {
				if (child.gridCoord !== null) continue;

				const parentSg = getNodeSubgraph(graph, node);
				const childSg = getNodeSubgraph(graph, child);
				const edgeDir =
					parentSg && parentSg === childSg && parentSg.direction
						? parentSg.direction
						: graph.config.graphDirection;

				const childLevel = edgeDir === "LR" ? gc.x + 4 : gc.y + 4;

				let highestPosition: number;
				if (edgeDir !== graph.config.graphDirection) {
					highestPosition = edgeDir === "LR" ? gc.y : gc.x;
				} else {
					highestPosition = highestPositionPerLevel[childLevel]!;
				}

				const requested: GridCoord =
					edgeDir === "LR" ? { x: childLevel, y: highestPosition } : { x: highestPosition, y: childLevel };
				reserveSpotInGrid(graph, graph.nodes[child.index]!, requested, edgeDir);

				if (edgeDir === graph.config.graphDirection) {
					highestPositionPerLevel[childLevel] = highestPosition + 4;
				}
				placedCount++;
			}
		}
		if (placedCount === prevCount) break;
	}

	for (const node of graph.nodes) {
		setColumnWidth(graph, node);
	}

	graph.bundles = analyzeEdgeBundles(graph);
	processBundles(graph);

	for (const edge of graph.edges) {
		if (edge.bundle && edge.path.length > 0) {
			increaseGridSizeForPath(graph, edge.path);
			determineLabelLine(graph, edge);
			continue;
		}

		determinePath(graph, edge);
		increaseGridSizeForPath(graph, edge.path);
		determineLabelLine(graph, edge);
	}

	for (const node of graph.nodes) {
		node.drawingCoord = gridToDrawingCoord(graph, node.gridCoord!);
		node.drawing = drawBox(node, graph);
	}

	setCanvasSizeToGrid(graph.canvas, graph.columnWidth, graph.rowHeight);
	setRoleCanvasSizeToGrid(graph.roleCanvas, graph.columnWidth, graph.rowHeight);
	calculateSubgraphBoundingBoxes(graph);
	offsetDrawingForSubgraphs(graph);

	if (graph.offsetX > 0 || graph.offsetY > 0) {
		const [curX, curY] = getCanvasSize(graph.canvas);
		increaseSize(graph.canvas, curX + graph.offsetX, curY + graph.offsetY);
		increaseRoleCanvasSize(graph.roleCanvas, curX + graph.offsetX, curY + graph.offsetY);
	}
}

function getEdgesFromNode(graph: AsciiGraph, node: AsciiNode): AsciiGraph["edges"] {
	return graph.edges.filter((e) => e.from.name === node.name);
}

function getChildren(graph: AsciiGraph, node: AsciiNode): AsciiNode[] {
	return getEdgesFromNode(graph, node).map((e) => e.to);
}
