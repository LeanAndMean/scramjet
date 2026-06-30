import { describe, expect, it } from "vitest";
import {
	drawText,
	flipCanvasVertically,
	flipRoleCanvasVertically,
	mergeCanvases,
	mkCanvas,
	mkRoleCanvas,
} from "../src/diagram/renderer/canvas.js";

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

		it("does not overwrite line char with alphanumeric", () => {
			const base = mkCanvas(0, 0);
			base[0]![0] = "│";

			const overlay = mkCanvas(0, 0);
			overlay[0]![0] = "e";

			const merged = mergeCanvases(base, { x: 0, y: 0 }, false, overlay);
			expect(merged[0]![0]).toBe("│");
		});

		it("does not overwrite arrow char with alphanumeric", () => {
			const base = mkCanvas(0, 0);
			base[0]![0] = "▲";

			const overlay = mkCanvas(0, 0);
			overlay[0]![0] = "x";

			const merged = mergeCanvases(base, { x: 0, y: 0 }, false, overlay);
			expect(merged[0]![0]).toBe("▲");
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
		it("only writes to empty cells (preserves existing content)", () => {
			const canvas = mkCanvas(4, 0);
			canvas[0]![0] = "H";
			canvas[1]![0] = "i";

			drawText(canvas, { x: 0, y: 0 }, "Bye!!");
			expect(canvas[0]![0]).toBe("H");
			expect(canvas[1]![0]).toBe("i");
			expect(canvas[2]![0]).toBe("e");
			expect(canvas[3]![0]).toBe("!");
			expect(canvas[4]![0]).toBe("!");
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
