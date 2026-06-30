import { describe, expect, it } from "vitest";
import {
	drawText,
	flipCanvasVertically,
	flipRoleCanvasVertically,
	mergeCanvases,
	mkCanvas,
	mkRoleCanvas,
} from "../src/diagram/renderer/canvas.js";
import { renderDiagram } from "../src/diagram/renderer/index.js";
import type { CharRole } from "../src/diagram/renderer/types.js";

function canvasToString(chars: string[][]): string {
	const maxY = chars[0]?.length ?? 0;
	const lines: string[] = [];
	for (let y = 0; y < maxY; y++) {
		let line = "";
		for (let x = 0; x < chars.length; x++) {
			line += chars[x]![y] || "";
		}
		lines.push(line.trimEnd());
	}
	while (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
	return lines.join("\n");
}

function collectRoles(roles: (CharRole | null)[][]): Set<CharRole> {
	const found = new Set<CharRole>();
	for (const col of roles) {
		for (const r of col) {
			if (r !== null) found.add(r);
		}
	}
	return found;
}

describe("canvas", () => {
	describe("mergeCanvases junction protection", () => {
		it("does not overwrite junction char with alphanumeric", () => {
			const base = mkCanvas(2, 0);
			base[0]![0] = "┬";
			base[1]![0] = "─";
			base[2]![0] = " ";

			const overlay = mkCanvas(2, 0);
			overlay[0]![0] = "n";
			overlay[1]![0] = " ";
			overlay[2]![0] = " ";

			const merged = mergeCanvases(base, { x: 0, y: 0 }, false, overlay);
			expect(merged[0]![0]).toBe("┬");
		});

		it("merges junction with junction correctly", () => {
			const base = mkCanvas(0, 0);
			base[0]![0] = "─";

			const overlay = mkCanvas(0, 0);
			overlay[0]![0] = "│";

			const merged = mergeCanvases(base, { x: 0, y: 0 }, false, overlay);
			expect(merged[0]![0]).toBe("┼");
		});

		it("allows non-structural overwrites (arrow over space)", () => {
			const base = mkCanvas(0, 0);
			base[0]![0] = " ";

			const overlay = mkCanvas(0, 0);
			overlay[0]![0] = "▼";

			const merged = mergeCanvases(base, { x: 0, y: 0 }, false, overlay);
			expect(merged[0]![0]).toBe("▼");
		});

		it("allows label text to overwrite plain line chars", () => {
			const base = mkCanvas(0, 0);
			base[0]![0] = "│";

			const overlay = mkCanvas(0, 0);
			overlay[0]![0] = "e";

			const merged = mergeCanvases(base, { x: 0, y: 0 }, false, overlay);
			expect(merged[0]![0]).toBe("e");
		});

		it("allows label text to overwrite arrow chars", () => {
			const base = mkCanvas(0, 0);
			base[0]![0] = "▲";

			const overlay = mkCanvas(0, 0);
			overlay[0]![0] = "x";

			const merged = mergeCanvases(base, { x: 0, y: 0 }, false, overlay);
			expect(merged[0]![0]).toBe("x");
		});

		it("does not overwrite structural char with CJK label text", () => {
			const base = mkCanvas(0, 0);
			base[0]![0] = "┴";

			const overlay = mkCanvas(0, 0);
			overlay[0]![0] = "你";

			const merged = mergeCanvases(base, { x: 0, y: 0 }, false, overlay);
			expect(merged[0]![0]).toBe("┴");
		});
	});

	describe("drawText", () => {
		it("skips entire write when any target cell is occupied", () => {
			const canvas = mkCanvas(4, 0);
			canvas[0]![0] = "H";
			canvas[1]![0] = "i";

			drawText(canvas, { x: 0, y: 0 }, "Bye!!");
			expect(canvas[0]![0]).toBe("H");
			expect(canvas[1]![0]).toBe("i");
			expect(canvas[2]![0]).toBe(" ");
			expect(canvas[3]![0]).toBe(" ");
			expect(canvas[4]![0]).toBe(" ");
		});

		it("writes when all target cells are spaces", () => {
			const canvas = mkCanvas(4, 0);

			drawText(canvas, { x: 1, y: 0 }, "Hi!");
			expect(canvas[0]![0]).toBe(" ");
			expect(canvas[1]![0]).toBe("H");
			expect(canvas[2]![0]).toBe("i");
			expect(canvas[3]![0]).toBe("!");
		});

		it("overwrites when forceOverwrite is true", () => {
			const canvas = mkCanvas(2, 0);
			canvas[0]![0] = "A";
			canvas[1]![0] = "B";

			drawText(canvas, { x: 0, y: 0 }, "XYZ", true);
			expect(canvas[0]![0]).toBe("X");
			expect(canvas[1]![0]).toBe("Y");
			expect(canvas[2]![0]).toBe("Z");
		});

		it("handles CJK double-width characters", () => {
			const canvas = mkCanvas(5, 0);
			drawText(canvas, { x: 0, y: 0 }, "你好x");
			expect(canvas[0]![0]).toBe("你");
			expect(canvas[1]![0]).toBe("");
			expect(canvas[2]![0]).toBe("好");
			expect(canvas[3]![0]).toBe("");
			expect(canvas[4]![0]).toBe("x");
		});
	});

	describe("flipCanvasVertically", () => {
		it("reverses rows and remaps directional chars", () => {
			const canvas = mkCanvas(0, 2);
			canvas[0]![0] = "▲";
			canvas[0]![1] = "│";
			canvas[0]![2] = "▼";

			flipCanvasVertically(canvas);
			expect(canvas[0]![0]).toBe("▲");
			expect(canvas[0]![1]).toBe("│");
			expect(canvas[0]![2]).toBe("▼");
		});

		it("flips corner characters", () => {
			const canvas = mkCanvas(1, 1);
			canvas[0]![0] = "┌";
			canvas[1]![0] = "┐";
			canvas[0]![1] = "└";
			canvas[1]![1] = "┘";

			flipCanvasVertically(canvas);
			expect(canvas[0]![0]).toBe("┌");
			expect(canvas[1]![0]).toBe("┐");
			expect(canvas[0]![1]).toBe("└");
			expect(canvas[1]![1]).toBe("┘");
		});
	});

	describe("flipRoleCanvasVertically", () => {
		it("reverses role rows", () => {
			const rc = mkRoleCanvas(0, 2);
			rc[0]![0] = "border";
			rc[0]![1] = "text";
			rc[0]![2] = "arrow";

			flipRoleCanvasVertically(rc);
			expect(rc[0]![0]).toBe("arrow");
			expect(rc[0]![1]).toBe("text");
			expect(rc[0]![2]).toBe("border");
		});
	});
});

describe("renderDiagram end-to-end", () => {
	it("renders a simple 3-node graph with expected labels", () => {
		const { chars } = renderDiagram("graph TD\n  A[Start] --> B[Middle] --> C[End]");
		const text = canvasToString(chars);
		expect(text).toContain("Start");
		expect(text).toContain("Middle");
		expect(text).toContain("End");
	});

	it("produces edge and arrow roles in roleCanvas", () => {
		const { roles } = renderDiagram("graph TD\n  A --> B");
		const found = collectRoles(roles);
		expect(found.has("line")).toBe(true);
		expect(found.has("arrow")).toBe(true);
		expect(found.has("border")).toBe(true);
		expect(found.has("text")).toBe(true);
	});

	it("openn regression: 4-edge 'open' label produces no 'openn'", () => {
		const source = [
			"graph TD",
			"  A[Router] -->|open| B[Handler]",
			"  A -->|close| C[Closer]",
			"  A -->|send| D[Sender]",
			"  A -->|recv| E[Receiver]",
		].join("\n");
		const { chars } = renderDiagram(source);
		const text = canvasToString(chars);
		expect(text).toContain("open");
		expect(text).not.toContain("openn");
	});

	it("self-loop renders without crash", () => {
		const { chars } = renderDiagram("graph TD\n  A[Loop] --> A");
		const text = canvasToString(chars);
		expect(text).toContain("Loop");
	});

	it("BT direction: arrow characters point upward after flip", () => {
		const { chars } = renderDiagram("graph BT\n  A --> B");
		const text = canvasToString(chars);
		// BT: arrows should point upward (▲) since flow goes bottom-to-top
		expect(text).toContain("▲");
	});

	it("cross-subgraph cycle does not crash", () => {
		const source = [
			"flowchart TD",
			"    subgraph S1",
			"        A[Node A]",
			"    end",
			"    subgraph S2",
			"        B[Node B]",
			"    end",
			"    A --> B",
			"    B --> A",
		].join("\n");
		const { chars } = renderDiagram(source);
		const text = canvasToString(chars);
		expect(text).toContain("Node A");
		expect(text).toContain("Node B");
	});

	it("overlapping edge labels do not produce corruption", () => {
		const source = [
			"flowchart TD",
			'    IC["issue-create"] -->|open| IP["issue-plan"]',
			'    IP -->|open| IR["issue-review"]',
			'    IP -->|open| II["issue-implement"]',
			"    IR -->|open| IR",
			"    IR -->|open| II",
			"    II -->|open| II",
			'    II -->|open| PC["pr-create"]',
			'    PC -->|open| PRV["pr-review"]',
			'    PRV -->|forced| PRA["pr-review-assessment"]',
			'    PRA -->|closed| PRF["pr-review-fix"]',
			'    PRA -->|closed| PPM["pr-pre-merge"]',
			"    PRF -->|open| PRF",
			"    PRF -->|open| PRV",
			"    PRF -->|open| PPM",
			'    PPM -->|open| PM["pr-merge"]',
			"    PPM -->|open| PRF",
		].join("\n");
		const { chars } = renderDiagram(source, { paddingX: 5, paddingY: 5, boxBorderPadding: 2 });
		const text = canvasToString(chars);
		expect(text).not.toContain("oopen");
		expect(text).not.toContain("openn");
		expect(text).toContain("open");
		expect(text).toContain("issue-review");
		expect(text).toContain("pr-review-fix");
	});

	it("self-loop with long label at tight padding does not crash", () => {
		const source = 'flowchart TD\n    A[Node A] -->|"some long label text here"| A';
		expect(() => renderDiagram(source, { paddingX: 1, paddingY: 1, boxBorderPadding: 1 })).not.toThrow();
	});

	it("self-loop inside subgraph does not crash", () => {
		const source = [
			"flowchart TD",
			"    subgraph S[Group]",
			'      A[Node] -->|"long label for self loop"| A',
			"    end",
		].join("\n");
		expect(() => renderDiagram(source, { paddingX: 5, paddingY: 5, boxBorderPadding: 2 })).not.toThrow();
	});
});
