import type { Theme } from "@leanandmean/coding-agent";
import { getRenderedCopy, visibleWidth } from "@leanandmean/tui";
import { describe, expect, it, vi } from "vitest";
import { MultiLineSelectList } from "../src/multi-line-select.js";
import { renderSelectorFrame } from "../src/selector-presentation.js";

const identity = (text: string) => text;
const theme: Pick<Theme, "fg" | "bold"> = { fg: (_color, text) => text, bold: identity };
const listTheme = { selectedText: identity, description: identity, scrollInfo: identity };
const choices = Array.from({ length: 12 }, (_, index) => ({
	value: String(index),
	label: `Choice ${index}`,
	description: `Details for ${index}`,
}));

describe("selector presentation", () => {
	it.each([18, 60])("accounts for actual wrapped chrome at width %s and excludes borders from copying", (width) => {
		const list = new MultiLineSelectList(choices, 8, listTheme);
		const height = vi.spyOn(list, "setMaxHeight");
		const rendered = vi.spyOn(list, "render");
		const options = {
			width,
			maximumRows: 18,
			title: "Choose the next action",
			list,
			theme,
			tailRows: ["effort: high", "↑↓ navigate • enter select • esc cancel"],
		};
		const lines = renderSelectorFrame(options);
		const content = rendered.mock.results[0].value as string[];
		expect(height).toHaveBeenCalledWith(18 - (lines.length - content.length));
		expect(lines.length).toBeLessThanOrEqual(18);
		expect(lines.every((line) => visibleWidth(line) <= width)).toBe(true);
		expect(lines[0]).toBe("─".repeat(width));
		expect(lines.at(-1)).toBe("─".repeat(width));
		const copy = getRenderedCopy(lines);
		expect(copy[0]).toBeNull();
		expect(copy.at(-1)).toBeNull();
		expect(copy.slice(1, -1).every((row) => row !== null)).toBe(true);
		expect(renderSelectorFrame(options)).not.toBe(lines);
	});

	it("keeps the selected representation when required chrome cannot fit", () => {
		const list = new MultiLineSelectList(choices, 8, listTheme);
		const lines = renderSelectorFrame({
			width: 30,
			maximumRows: 2,
			title: "Confirm",
			list,
			theme,
			tailRows: ["enter select • esc cancel"],
		});
		expect(lines.length).toBeGreaterThan(2);
		expect(lines).toContain("→ Choice 0");
		expect(lines).toContain("     Details for 0");
		expect(lines.join("\n")).toContain("enter select");
	});

	it("preserves visible fragments without recovering hidden neighboring text", () => {
		const list = new MultiLineSelectList(
			[
				{ value: "a", label: "A1\nA2\nA3" },
				{ value: "b", label: "B1\nB2" },
				{ value: "c", label: "C1\nC2\nC3\nC4" },
			],
			8,
			listTheme,
		);
		list.setMaxHeight(8);
		list.setSelectedIndex(2);
		list.render(30);
		list.setSelectedIndex(1);
		const lines = renderSelectorFrame({ width: 30, maximumRows: 10, title: "Choose", list, theme, tailRows: [] });
		expect(lines).toContain("  A3");
		expect(lines).toContain("  C3");
		expect(lines).not.toContain("  A2");
		expect(lines).not.toContain("  C4");
		const copy = getRenderedCopy(lines);
		for (const text of ["  A3", "  C3"]) {
			const row = lines.indexOf(text);
			expect(copy[row]).toEqual({ start: 0, end: text.length });
		}
	});
});
