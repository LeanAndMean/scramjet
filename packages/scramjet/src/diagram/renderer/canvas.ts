// Based on beautiful-mermaid by Craft Docs, MIT License.

import { visibleWidth } from "@leanandmean/tui";
import type { Canvas, CharRole, DrawingCoord, RoleCanvas } from "./types.js";

export function mkCanvas(x: number, y: number): Canvas {
	const canvas: Canvas = [];
	for (let i = 0; i <= x; i++) {
		const col: string[] = [];
		for (let j = 0; j <= y; j++) {
			col.push(" ");
		}
		canvas.push(col);
	}
	return canvas;
}

export function copyCanvas(source: Canvas): Canvas {
	const [maxX, maxY] = getCanvasSize(source);
	return mkCanvas(maxX, maxY);
}

export function mkRoleCanvas(x: number, y: number): RoleCanvas {
	const roleCanvas: RoleCanvas = [];
	for (let i = 0; i <= x; i++) {
		const col: (CharRole | null)[] = [];
		for (let j = 0; j <= y; j++) {
			col.push(null);
		}
		roleCanvas.push(col);
	}
	return roleCanvas;
}

export function copyRoleCanvas(source: RoleCanvas): RoleCanvas {
	const maxX = source.length - 1;
	const maxY = (source[0]?.length ?? 1) - 1;
	return mkRoleCanvas(maxX, maxY);
}

export function increaseRoleCanvasSize(roleCanvas: RoleCanvas, newX: number, newY: number): RoleCanvas {
	const currX = roleCanvas.length - 1;
	const currY = (roleCanvas[0]?.length ?? 1) - 1;
	const targetX = Math.max(newX, currX);
	const targetY = Math.max(newY, currY);
	const grown = mkRoleCanvas(targetX, targetY);
	for (let x = 0; x < grown.length; x++) {
		for (let y = 0; y < grown[0]!.length; y++) {
			if (x < roleCanvas.length && y < roleCanvas[0]!.length) {
				grown[x]![y] = roleCanvas[x]![y]!;
			}
		}
	}
	roleCanvas.length = 0;
	roleCanvas.push(...grown);
	return roleCanvas;
}

export function setRole(roleCanvas: RoleCanvas, x: number, y: number, role: CharRole): void {
	if (x >= roleCanvas.length || y >= (roleCanvas[0]?.length ?? 0)) {
		increaseRoleCanvasSize(roleCanvas, x, y);
	}
	roleCanvas[x]![y] = role;
}

export function mergeRoleCanvases(base: RoleCanvas, offset: DrawingCoord, ...overlays: RoleCanvas[]): RoleCanvas {
	let maxX = base.length - 1;
	let maxY = (base[0]?.length ?? 1) - 1;

	for (const overlay of overlays) {
		const oX = overlay.length - 1;
		const oY = (overlay[0]?.length ?? 1) - 1;
		maxX = Math.max(maxX, oX + offset.x);
		maxY = Math.max(maxY, oY + offset.y);
	}

	const merged = mkRoleCanvas(maxX, maxY);

	for (let x = 0; x <= maxX; x++) {
		for (let y = 0; y <= maxY; y++) {
			if (x < base.length && y < base[0]!.length) {
				merged[x]![y] = base[x]![y]!;
			}
		}
	}

	for (const overlay of overlays) {
		for (let x = 0; x < overlay.length; x++) {
			for (let y = 0; y < overlay[0]!.length; y++) {
				const role = overlay[x]?.[y];
				if (role !== null && role !== undefined) {
					const mx = x + offset.x;
					const my = y + offset.y;
					merged[mx]![my] = role;
				}
			}
		}
	}

	return merged;
}

export function getCanvasSize(canvas: Canvas): [number, number] {
	return [canvas.length - 1, (canvas[0]?.length ?? 1) - 1];
}

export function increaseSize(canvas: Canvas, newX: number, newY: number): Canvas {
	const [currX, currY] = getCanvasSize(canvas);
	const targetX = Math.max(newX, currX);
	const targetY = Math.max(newY, currY);
	const grown = mkCanvas(targetX, targetY);
	for (let x = 0; x < grown.length; x++) {
		for (let y = 0; y < grown[0]!.length; y++) {
			if (x < canvas.length && y < canvas[0]!.length) {
				grown[x]![y] = canvas[x]![y]!;
			}
		}
	}
	canvas.length = 0;
	canvas.push(...grown);
	return canvas;
}

// ============================================================================
// Junction merging
// ============================================================================

const JUNCTION_CHARS = new Set(["─", "│", "┌", "┐", "└", "┘", "├", "┤", "┬", "┴", "┼", "╴", "╵", "╶", "╷"]);

export function isJunctionChar(c: string): boolean {
	return JUNCTION_CHARS.has(c);
}

const ARROW_CHARS = new Set(["▲", "▼", "◄", "►", "△", "▽", "◁", "▷"]);

function isArrowChar(c: string): boolean {
	return ARROW_CHARS.has(c);
}

const COMPOUND_JUNCTION_CHARS = new Set(["┌", "┐", "└", "┘", "├", "┤", "┬", "┴", "┼"]);

function isCompoundJunctionChar(c: string): boolean {
	return COMPOUND_JUNCTION_CHARS.has(c);
}

const JUNCTION_MAP: Record<string, Record<string, string>> = {
	"─": { "│": "┼", "┌": "┬", "┐": "┬", "└": "┴", "┘": "┴", "├": "┼", "┤": "┼", "┬": "┬", "┴": "┴" },
	"│": { "─": "┼", "┌": "├", "┐": "┤", "└": "├", "┘": "┤", "├": "├", "┤": "┤", "┬": "┼", "┴": "┼" },
	"┌": {
		"─": "┬",
		"│": "├",
		"┐": "┬",
		"└": "├",
		"┘": "┼",
		"├": "├",
		"┤": "┼",
		"┬": "┬",
		"┴": "┼",
	},
	"┐": {
		"─": "┬",
		"│": "┤",
		"┌": "┬",
		"└": "┼",
		"┘": "┤",
		"├": "┼",
		"┤": "┤",
		"┬": "┬",
		"┴": "┼",
	},
	"└": {
		"─": "┴",
		"│": "├",
		"┌": "├",
		"┐": "┼",
		"┘": "┴",
		"├": "├",
		"┤": "┼",
		"┬": "┼",
		"┴": "┴",
	},
	"┘": {
		"─": "┴",
		"│": "┤",
		"┌": "┼",
		"┐": "┤",
		"└": "┴",
		"├": "┼",
		"┤": "┤",
		"┬": "┼",
		"┴": "┴",
	},
	"├": {
		"─": "┼",
		"│": "├",
		"┌": "├",
		"┐": "┼",
		"└": "├",
		"┘": "┼",
		"┤": "┼",
		"┬": "┼",
		"┴": "┼",
	},
	"┤": {
		"─": "┼",
		"│": "┤",
		"┌": "┼",
		"┐": "┤",
		"└": "┼",
		"┘": "┤",
		"├": "┼",
		"┬": "┼",
		"┴": "┼",
	},
	"┬": {
		"─": "┬",
		"│": "┼",
		"┌": "┬",
		"┐": "┬",
		"└": "┼",
		"┘": "┼",
		"├": "┼",
		"┤": "┼",
		"┴": "┼",
	},
	"┴": {
		"─": "┴",
		"│": "┼",
		"┌": "┼",
		"┐": "┼",
		"└": "┴",
		"┘": "┴",
		"├": "┼",
		"┤": "┼",
		"┬": "┼",
	},
};

export function mergeJunctions(c1: string, c2: string): string {
	return JUNCTION_MAP[c1]?.[c2] ?? c1;
}

// ============================================================================
// Canvas merging — with openn fix (structural char protection)
// ============================================================================

export function mergeCanvases(base: Canvas, offset: DrawingCoord, useAscii: boolean, ...overlays: Canvas[]): Canvas {
	let [maxX, maxY] = getCanvasSize(base);
	for (const overlay of overlays) {
		const [oX, oY] = getCanvasSize(overlay);
		maxX = Math.max(maxX, oX + offset.x);
		maxY = Math.max(maxY, oY + offset.y);
	}

	const merged = mkCanvas(maxX, maxY);

	for (let x = 0; x <= maxX; x++) {
		for (let y = 0; y <= maxY; y++) {
			if (x < base.length && y < base[0]!.length) {
				merged[x]![y] = base[x]![y]!;
			}
		}
	}

	for (const overlay of overlays) {
		for (let x = 0; x < overlay.length; x++) {
			for (let y = 0; y < overlay[0]!.length; y++) {
				const c = overlay[x]![y]!;
				if (c !== " ") {
					const mx = x + offset.x;
					const my = y + offset.y;
					const current = merged[mx]![my]!;
					if (!useAscii && isJunctionChar(c) && isJunctionChar(current)) {
						merged[mx]![my] = mergeJunctions(current, c);
					} else if (isCompoundJunctionChar(current) && !isJunctionChar(c) && !isArrowChar(c)) {
						// openn fix: protect compound junction chars (┬┤├┼ etc.) from label text
					} else {
						merged[mx]![my] = c;
					}
				}
			}
		}
	}

	return merged;
}

// ============================================================================
// Canvas vertical flip
// ============================================================================

const VERTICAL_FLIP_MAP: Record<string, string> = {
	"▲": "▼",
	"▼": "▲",
	"◤": "◣",
	"◣": "◤",
	"◥": "◢",
	"◢": "◥",
	"^": "v",
	v: "^",
	"┌": "└",
	"└": "┌",
	"┐": "┘",
	"┘": "┐",
	"┬": "┴",
	"┴": "┬",
	"╵": "╷",
	"╷": "╵",
};

export function flipCanvasVertically(canvas: Canvas): Canvas {
	for (const col of canvas) {
		col.reverse();
	}
	for (const col of canvas) {
		for (let y = 0; y < col.length; y++) {
			const flipped = VERTICAL_FLIP_MAP[col[y]!];
			if (flipped) col[y] = flipped;
		}
	}
	return canvas;
}

export function flipRoleCanvasVertically(roleCanvas: RoleCanvas): RoleCanvas {
	for (const col of roleCanvas) {
		col.reverse();
	}
	return roleCanvas;
}

// ============================================================================
// Text drawing — uses visibleWidth for CJK support
// ============================================================================

export function drawText(canvas: Canvas, start: DrawingCoord, text: string, forceOverwrite = false): void {
	const vw = visibleWidth(text);
	increaseSize(canvas, start.x + vw - 1, start.y);

	let col = start.x;
	for (const char of text) {
		const charWidth = visibleWidth(char);
		const current = canvas[col]![start.y]!;
		if (forceOverwrite || current === " ") {
			canvas[col]![start.y] = char;
			// Fill phantom cells for double-width chars
			for (let w = 1; w < charWidth; w++) {
				if (col + w < canvas.length) {
					canvas[col + w]![start.y] = "";
				}
			}
		}
		col += charWidth;
	}
}

export function setCanvasSizeToGrid(
	canvas: Canvas,
	columnWidth: Map<number, number>,
	rowHeight: Map<number, number>,
): void {
	let maxX = 0;
	let maxY = 0;
	for (const w of columnWidth.values()) maxX += w;
	for (const h of rowHeight.values()) maxY += h;
	increaseSize(canvas, maxX - 1, maxY - 1);
}

export function setRoleCanvasSizeToGrid(
	roleCanvas: RoleCanvas,
	columnWidth: Map<number, number>,
	rowHeight: Map<number, number>,
): void {
	let maxX = 0;
	let maxY = 0;
	for (const w of columnWidth.values()) maxX += w;
	for (const h of rowHeight.values()) maxY += h;
	increaseRoleCanvasSize(roleCanvas, maxX - 1, maxY - 1);
}
