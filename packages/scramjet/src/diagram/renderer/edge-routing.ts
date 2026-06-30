// Based on beautiful-mermaid by Craft Docs, MIT License.

import { getNodeSubgraph } from "./grid.js";
import { getPath, isFreeInGrid, mergePath } from "./pathfinder.js";
import type { AsciiEdge, AsciiGraph, GridCoord, GridDirection } from "./types.js";
import {
	Down,
	dirEquals,
	gridCoordDirection,
	Left,
	LowerLeft,
	LowerRight,
	Middle,
	Right,
	Up,
	UpperLeft,
	UpperRight,
} from "./types.js";

export function getOpposite(d: GridDirection): GridDirection {
	if (d === Up) return Down;
	if (d === Down) return Up;
	if (d === Left) return Right;
	if (d === Right) return Left;
	if (d === UpperRight) return LowerLeft;
	if (d === UpperLeft) return LowerRight;
	if (d === LowerRight) return UpperLeft;
	if (d === LowerLeft) return UpperRight;
	return Middle;
}

export { dirEquals } from "./types.js";

export function determineDirection(from: { x: number; y: number }, to: { x: number; y: number }): GridDirection {
	if (from.x === to.x) {
		return from.y < to.y ? Down : Up;
	} else if (from.y === to.y) {
		return from.x < to.x ? Right : Left;
	} else if (from.x < to.x) {
		return from.y < to.y ? LowerRight : UpperRight;
	} else {
		return from.y < to.y ? LowerLeft : UpperLeft;
	}
}

function selfReferenceDirection(graphDirection: string): [GridDirection, GridDirection, GridDirection, GridDirection] {
	if (graphDirection === "LR") return [Right, Down, Down, Right];
	return [Down, Right, Right, Down];
}

export function determineStartAndEndDir(
	edge: AsciiEdge,
	graphDirection: string,
): [GridDirection, GridDirection, GridDirection, GridDirection] {
	if (edge.from === edge.to) return selfReferenceDirection(graphDirection);

	const d = determineDirection(edge.from.gridCoord!, edge.to.gridCoord!);

	let preferredDir: GridDirection;
	let preferredOppositeDir: GridDirection;
	let alternativeDir: GridDirection;
	let alternativeOppositeDir: GridDirection;

	const isBackwards =
		graphDirection === "LR"
			? dirEquals(d, Left) || dirEquals(d, UpperLeft) || dirEquals(d, LowerLeft)
			: dirEquals(d, Up) || dirEquals(d, UpperLeft) || dirEquals(d, UpperRight);

	if (dirEquals(d, LowerRight)) {
		if (graphDirection === "LR") {
			preferredDir = Down;
			preferredOppositeDir = Left;
			alternativeDir = Right;
			alternativeOppositeDir = Up;
		} else {
			preferredDir = Right;
			preferredOppositeDir = Up;
			alternativeDir = Down;
			alternativeOppositeDir = Left;
		}
	} else if (dirEquals(d, UpperRight)) {
		if (graphDirection === "LR") {
			preferredDir = Up;
			preferredOppositeDir = Left;
			alternativeDir = Right;
			alternativeOppositeDir = Down;
		} else {
			preferredDir = Right;
			preferredOppositeDir = Down;
			alternativeDir = Up;
			alternativeOppositeDir = Left;
		}
	} else if (dirEquals(d, LowerLeft)) {
		if (graphDirection === "LR") {
			preferredDir = Down;
			preferredOppositeDir = Down;
			alternativeDir = Left;
			alternativeOppositeDir = Up;
		} else {
			preferredDir = Left;
			preferredOppositeDir = Up;
			alternativeDir = Down;
			alternativeOppositeDir = Right;
		}
	} else if (dirEquals(d, UpperLeft)) {
		if (graphDirection === "LR") {
			preferredDir = Down;
			preferredOppositeDir = Down;
			alternativeDir = Left;
			alternativeOppositeDir = Down;
		} else {
			preferredDir = Right;
			preferredOppositeDir = Right;
			alternativeDir = Up;
			alternativeOppositeDir = Right;
		}
	} else if (isBackwards) {
		if (graphDirection === "LR" && dirEquals(d, Left)) {
			preferredDir = Down;
			preferredOppositeDir = Down;
			alternativeDir = Left;
			alternativeOppositeDir = Right;
		} else if (graphDirection === "TD" && dirEquals(d, Up)) {
			preferredDir = Right;
			preferredOppositeDir = Right;
			alternativeDir = Up;
			alternativeOppositeDir = Down;
		} else {
			preferredDir = d;
			preferredOppositeDir = getOpposite(d);
			alternativeDir = d;
			alternativeOppositeDir = getOpposite(d);
		}
	} else {
		preferredDir = d;
		preferredOppositeDir = getOpposite(d);
		alternativeDir = d;
		alternativeOppositeDir = getOpposite(d);
	}

	return [preferredDir, preferredOppositeDir, alternativeDir, alternativeOppositeDir];
}

function isSecondaryBidirectional(graph: AsciiGraph, edge: AsciiEdge, effectiveDir: string): boolean {
	const fromGc = edge.from.gridCoord!;
	const toGc = edge.to.gridCoord!;
	const sameLevel = effectiveDir === "LR" ? fromGc.x === toGc.x : fromGc.y === toGc.y;
	if (!sameLevel) return false;
	const hasOpposing = graph.edges.some((e) => e.from === edge.to && e.to === edge.from);
	if (!hasOpposing) return false;
	if (effectiveDir === "LR") return fromGc.y > toGc.y;
	return fromGc.x > toGc.x;
}

export function determinePath(graph: AsciiGraph, edge: AsciiEdge): void {
	const sourceSg = getNodeSubgraph(graph, edge.from);
	const targetSg = getNodeSubgraph(graph, edge.to);
	const effectiveDir =
		sourceSg && sourceSg === targetSg && sourceSg.direction ? sourceSg.direction : graph.config.graphDirection;

	if (edge.from === edge.to) {
		const selfPath = buildSelfLoopPath(graph, edge, effectiveDir);
		if (selfPath) {
			edge.startDir = selfPath.startDir;
			edge.endDir = selfPath.endDir;
			edge.path = selfPath.path;
			return;
		}
	}

	if (isSecondaryBidirectional(graph, edge, effectiveDir)) {
		const fromGc = edge.from.gridCoord!;
		const toGc = edge.to.gridCoord!;
		if (effectiveDir === "TD") {
			const shiftedFrom = { x: fromGc.x, y: fromGc.y };
			const shiftedTo = { x: toGc.x + 2, y: toGc.y };
			if (isFreeInGrid(graph.grid, { x: Math.min(shiftedFrom.x, shiftedTo.x) + 1, y: shiftedFrom.y })) {
				const path = getPath(graph.grid, shiftedFrom, shiftedTo);
				if (path) {
					edge.startDir = Left;
					edge.endDir = Right;
					edge.path = mergePath(path);
					return;
				}
			}
		} else {
			const shiftedFrom = { x: fromGc.x, y: fromGc.y };
			const shiftedTo = { x: toGc.x, y: toGc.y + 2 };
			if (isFreeInGrid(graph.grid, { x: shiftedFrom.x, y: Math.min(shiftedFrom.y, shiftedTo.y) + 1 })) {
				const path = getPath(graph.grid, shiftedFrom, shiftedTo);
				if (path) {
					edge.startDir = Up;
					edge.endDir = Down;
					edge.path = mergePath(path);
					return;
				}
			}
		}
	}

	const [preferredDir, preferredOppositeDir, alternativeDir, alternativeOppositeDir] = determineStartAndEndDir(
		edge,
		effectiveDir,
	);

	const prefFrom = gridCoordDirection(edge.from.gridCoord!, preferredDir);
	const prefTo = gridCoordDirection(edge.to.gridCoord!, preferredOppositeDir);
	let preferredPath = getPath(graph.grid, prefFrom, prefTo);

	const altFrom = gridCoordDirection(edge.from.gridCoord!, alternativeDir);
	const altTo = gridCoordDirection(edge.to.gridCoord!, alternativeOppositeDir);
	let alternativePath = getPath(graph.grid, altFrom, altTo);

	if (preferredPath !== null && alternativePath !== null) {
		preferredPath = mergePath(preferredPath);
		alternativePath = mergePath(alternativePath);

		if (preferredPath.length <= alternativePath.length) {
			edge.startDir = preferredDir;
			edge.endDir = preferredOppositeDir;
			edge.path = preferredPath;
		} else {
			edge.startDir = alternativeDir;
			edge.endDir = alternativeOppositeDir;
			edge.path = alternativePath;
		}
		return;
	}

	if (preferredPath !== null) {
		edge.startDir = preferredDir;
		edge.endDir = preferredOppositeDir;
		edge.path = mergePath(preferredPath);
		return;
	}

	if (alternativePath !== null) {
		edge.startDir = alternativeDir;
		edge.endDir = alternativeOppositeDir;
		edge.path = mergePath(alternativePath);
		return;
	}

	edge.startDir = preferredDir;
	edge.endDir = preferredOppositeDir;
	edge.path = [prefFrom, prefTo];
}

function hasSiblingEdgeOnSide(graph: AsciiGraph, node: typeof graph.nodes[number], side: GridDirection): boolean {
	for (const e of graph.edges) {
		if (e.from === e.to) continue;
		if (e.from === node && e.to.gridCoord!.y === node.gridCoord!.y && dirEquals(side, Right)) return true;
		if (e.from === node && e.to.gridCoord!.x === node.gridCoord!.x && dirEquals(side, Down)) return true;
	}
	return false;
}

function buildSelfLoopPath(
	graph: AsciiGraph,
	edge: AsciiEdge,
	effectiveDir: string,
): { startDir: GridDirection; endDir: GridDirection; path: GridCoord[] } | null {
	const gc = edge.from.gridCoord!;

	if (effectiveDir === "LR") {
		// LR: exit Right, loop right and below, enter from Below
		const right = { x: gc.x + 3, y: gc.y + 1 };
		const rightBelow = { x: gc.x + 3, y: gc.y + 3 };
		const below = { x: gc.x + 1, y: gc.y + 3 };
		if (
			isFreeInGrid(graph.grid, right) &&
			isFreeInGrid(graph.grid, rightBelow) &&
			isFreeInGrid(graph.grid, below)
		) {
			let entryCell = { x: gc.x + 1, y: gc.y + 2 };
			let entryApproach = below;
			if (hasSiblingEdgeOnSide(graph, edge.from, Down)) {
				const shifted = { x: gc.x + 0, y: gc.y + 3 };
				const shiftedEntry = { x: gc.x + 0, y: gc.y + 2 };
				if (isFreeInGrid(graph.grid, shifted)) {
					entryApproach = shifted;
					entryCell = shiftedEntry;
				}
			}
			return {
				startDir: Right,
				endDir: Down,
				path: [{ x: gc.x + 1, y: gc.y + 1 }, right, rightBelow, entryApproach, entryCell],
			};
		}
		return null;
	}

	// TD: exit Down, loop below and to the right, enter from Right
	const below = { x: gc.x + 1, y: gc.y + 3 };
	const belowRight = { x: gc.x + 3, y: gc.y + 3 };
	const right = { x: gc.x + 3, y: gc.y + 1 };
	if (
		isFreeInGrid(graph.grid, below) &&
		isFreeInGrid(graph.grid, belowRight) &&
		isFreeInGrid(graph.grid, right)
	) {
		let entryCell = { x: gc.x + 2, y: gc.y + 1 };
		let entryApproach = right;
		if (hasSiblingEdgeOnSide(graph, edge.from, Right)) {
			const shifted = { x: gc.x + 3, y: gc.y + 0 };
			const shiftedEntry = { x: gc.x + 2, y: gc.y + 0 };
			if (isFreeInGrid(graph.grid, shifted)) {
				entryApproach = shifted;
				entryCell = shiftedEntry;
			}
		}
		return {
			startDir: Down,
			endDir: Right,
			path: [{ x: gc.x + 1, y: gc.y + 2 }, below, belowRight, entryApproach, entryCell],
		};
	}
	return null;
}

export function determineLabelLine(graph: AsciiGraph, edge: AsciiEdge): void {
	if (edge.text.length === 0) return;

	const lenLabel = edge.text.length;
	const pathLen = edge.path.length;

	const segments: {
		line: [GridCoord, GridCoord];
		width: number;
		index: number;
	}[] = [];

	for (let i = 1; i < pathLen; i++) {
		const p1 = edge.path[i - 1]!;
		const p2 = edge.path[i]!;
		const line: [GridCoord, GridCoord] = [p1, p2];
		const width = calculateLineWidth(graph, line);
		segments.push({ line, width, index: i });
	}

	const maxSegmentIndex = edge.from === edge.to ? pathLen - 1 : pathLen;
	const suitableSegments = segments.filter((s) => s.width >= lenLabel && s.index > 1 && s.index < maxSegmentIndex);

	let largestLine: [GridCoord, GridCoord];

	if (suitableSegments.length > 0) {
		const isSelfLoop = edge.from === edge.to;
		suitableSegments.sort((a, b) => isSelfLoop ? a.index - b.index : b.index - a.index);
		largestLine = suitableSegments[0]!.line;
	} else {
		const fallbackSegments = segments.filter((s) => s.width >= lenLabel && s.index < maxSegmentIndex);
		if (fallbackSegments.length > 0) {
			fallbackSegments.sort((a, b) => b.index - a.index);
			largestLine = fallbackSegments[0]!.line;
		} else {
			segments.sort((a, b) => b.width - a.width);
			largestLine = segments[0]?.line ?? [edge.path[0]!, edge.path[1]!];
		}
	}

	const minX = Math.min(largestLine[0].x, largestLine[1].x);
	const maxX = Math.max(largestLine[0].x, largestLine[1].x);
	const middleX = minX + Math.floor((maxX - minX) / 2);

	const current = graph.columnWidth.get(middleX) ?? 0;
	graph.columnWidth.set(middleX, Math.max(current, lenLabel + 2));

	edge.labelLine = [largestLine[0], largestLine[1]];
}

function calculateLineWidth(graph: AsciiGraph, line: [GridCoord, GridCoord]): number {
	let total = 0;
	const startX = Math.min(line[0].x, line[1].x);
	const endX = Math.max(line[0].x, line[1].x);
	for (let x = startX; x <= endX; x++) {
		total += graph.columnWidth.get(x) ?? 0;
	}
	return total;
}
