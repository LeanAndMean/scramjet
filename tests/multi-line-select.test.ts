import { describe, expect, it, vi } from "vitest";
import { type MultiLineSelectItem, MultiLineSelectList, type MultiLineSelectTheme } from "../multi-line-select.ts";

const identity = (text: string) => text;
const theme: MultiLineSelectTheme = {
	selectedPrefix: identity,
	selectedText: identity,
	description: (text: string) => `[desc]${text}[/desc]`,
	scrollInfo: (text: string) => `[scroll]${text}[/scroll]`,
};

function items(...specs: Array<{ label: string; description?: string }>): MultiLineSelectItem[] {
	return specs.map((s, i) => ({ value: String(i), label: s.label, description: s.description }));
}

const UP = "\x1b[A";
const DOWN = "\x1b[B";
const ENTER = "\r";
const ESCAPE = "\x1b";

describe("MultiLineSelectList", () => {
	describe("item layout", () => {
		it("renders label and description on separate lines", () => {
			const list = new MultiLineSelectList(
				items({ label: "/mach12:pr-review 55", description: "Review the PR" }),
				8,
				theme,
			);
			const lines = list.render(80);
			expect(lines[0]).toContain("→ /mach12:pr-review 55");
			expect(lines[1]).toContain("Review the PR");
			expect(lines[1]).toMatch(/^\s{5}/);
		});

		it("renders label without description when description is absent", () => {
			const list = new MultiLineSelectList(items({ label: "/some-command" }), 8, theme);
			const lines = list.render(80);
			expect(lines).toHaveLength(1);
			expect(lines[0]).toContain("→ /some-command");
		});

		it("renders empty description as no extra lines", () => {
			const list = new MultiLineSelectList(items({ label: "/cmd", description: "" }), 8, theme);
			const lines = list.render(80);
			expect(lines).toHaveLength(1);
		});

		it("applies selected theme to all lines of selected item", () => {
			const selTheme: MultiLineSelectTheme = {
				...theme,
				selectedText: (text: string) => `[sel]${text}[/sel]`,
			};
			const list = new MultiLineSelectList(items({ label: "/cmd", description: "reason" }), 8, selTheme);
			const lines = list.render(80);
			expect(lines[0]).toMatch(/^\[sel\].*\[\/sel\]$/);
			expect(lines[1]).toMatch(/^\[sel\].*\[\/sel\]$/);
		});

		it("applies description theme to non-selected description lines", () => {
			const list = new MultiLineSelectList(
				items({ label: "/cmd-a", description: "reason A" }, { label: "/cmd-b", description: "reason B" }),
				8,
				theme,
			);
			const lines = list.render(80);
			// Second item (index 1) is not selected
			const descLine = lines.find((l) => l.includes("reason B"));
			expect(descLine).toMatch(/^\[desc\].*\[\/desc\]$/);
		});

		it("prefixes selected item with → and others with spaces", () => {
			const list = new MultiLineSelectList(items({ label: "A" }, { label: "B" }), 8, theme);
			const lines = list.render(80);
			expect(lines[0]).toMatch(/^→ /);
			expect(lines[1]).toMatch(/^ {2}/);
		});
	});

	describe("[recommended] tag", () => {
		it("prepends [recommended] to description of recommended item", () => {
			const list = new MultiLineSelectList(
				items({ label: "/cmd-a", description: "first reason" }, { label: "/cmd-b", description: "second reason" }),
				8,
				theme,
				{ recommendedIndex: 0 },
			);
			const lines = list.render(80);
			const descLine = lines.find((l) => l.includes("[recommended]"));
			expect(descLine).toBeDefined();
			expect(descLine).toContain("[recommended] first reason");
		});

		it("does not add [recommended] when recommendedIndex is not set", () => {
			const list = new MultiLineSelectList(items({ label: "/cmd", description: "reason" }), 8, theme);
			const lines = list.render(80);
			expect(lines.join("\n")).not.toContain("[recommended]");
		});

		it("does not add [recommended] to the label line", () => {
			const list = new MultiLineSelectList(items({ label: "/cmd", description: "reason" }), 8, theme, {
				recommendedIndex: 0,
			});
			const lines = list.render(80);
			expect(lines[0]).not.toContain("[recommended]");
		});
	});

	describe("per-field line cap", () => {
		it("truncates label exceeding maxLinesPerField", () => {
			const longLabel = Array(10).fill("word").join(" ");
			const list = new MultiLineSelectList(items({ label: longLabel }), 8, theme, { maxLinesPerField: 2 });
			// Narrow width to force wrapping
			const lines = list.render(15);
			// Should have at most 2 lines for the label
			expect(lines.length).toBeLessThanOrEqual(2);
			expect(lines[lines.length - 1]).toContain("…");
		});

		it("truncates description exceeding maxLinesPerField", () => {
			const longDesc = Array(20).fill("description").join(" ");
			const list = new MultiLineSelectList(items({ label: "/cmd", description: longDesc }), 8, theme, {
				maxLinesPerField: 2,
			});
			const lines = list.render(30);
			// 1 label line + at most 2 description lines
			const descLines = lines.filter((l) => l.startsWith("     ") || l.match(/\[desc\]/));
			expect(descLines.length).toBeLessThanOrEqual(2);
			const lastDesc = descLines[descLines.length - 1];
			expect(lastDesc).toContain("…");
		});

		it("does not truncate fields within the cap", () => {
			const list = new MultiLineSelectList(items({ label: "/cmd", description: "short" }), 8, theme, {
				maxLinesPerField: 4,
			});
			const lines = list.render(80);
			expect(lines.join("\n")).not.toContain("…");
		});
	});

	describe("scroll behavior with variable-height items", () => {
		it("shows all items when count <= maxVisible", () => {
			const list = new MultiLineSelectList(
				items({ label: "A", description: "desc A" }, { label: "B", description: "desc B" }),
				8,
				theme,
			);
			const lines = list.render(80);
			expect(lines.join("\n")).toContain("A");
			expect(lines.join("\n")).toContain("B");
			// No scroll indicator
			expect(lines.join("\n")).not.toContain("[scroll]");
		});

		it("shows scroll indicator when items exceed maxVisible", () => {
			const list = new MultiLineSelectList(
				items(
					{ label: "A", description: "desc" },
					{ label: "B", description: "desc" },
					{ label: "C", description: "desc" },
				),
				2,
				theme,
			);
			const lines = list.render(80);
			expect(lines.join("\n")).toContain("[scroll]");
			expect(lines.join("\n")).toContain("(1/3)");
		});

		it("scrolls correctly past variable-height items", () => {
			const list = new MultiLineSelectList(
				items(
					{ label: "A", description: "desc A" },
					{ label: "B", description: "desc B" },
					{ label: "C", description: "desc C" },
				),
				2,
				theme,
			);
			// Navigate down twice to select C
			list.handleInput(DOWN);
			list.handleInput(DOWN);
			const lines = list.render(80);
			expect(lines.join("\n")).toContain("→ C");
			expect(lines.join("\n")).toContain("(3/3)");
		});
	});

	describe("keyboard navigation", () => {
		it("down arrow moves selection", () => {
			const list = new MultiLineSelectList(items({ label: "A" }, { label: "B" }), 8, theme);
			list.handleInput(DOWN);
			const lines = list.render(80);
			expect(lines[0]).toMatch(/^ {2}A/);
			expect(lines[1]).toMatch(/^→ B/);
		});

		it("up arrow moves selection", () => {
			const list = new MultiLineSelectList(items({ label: "A" }, { label: "B" }), 8, theme);
			list.handleInput(DOWN);
			list.handleInput(UP);
			const lines = list.render(80);
			expect(lines[0]).toMatch(/^→ A/);
		});

		it("down wraps from last to first", () => {
			const list = new MultiLineSelectList(items({ label: "A" }, { label: "B" }), 8, theme);
			list.handleInput(DOWN);
			list.handleInput(DOWN);
			const lines = list.render(80);
			expect(lines[0]).toMatch(/^→ A/);
		});

		it("up wraps from first to last", () => {
			const list = new MultiLineSelectList(items({ label: "A" }, { label: "B" }), 8, theme);
			list.handleInput(UP);
			const lines = list.render(80);
			expect(lines[1]).toMatch(/^→ B/);
		});

		it("enter fires onSelect with selected item", () => {
			const onSelect = vi.fn();
			const list = new MultiLineSelectList(items({ label: "A" }, { label: "B" }), 8, theme);
			list.onSelect = onSelect;
			list.handleInput(DOWN);
			list.handleInput(ENTER);
			expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ value: "1", label: "B" }));
		});

		it("escape fires onCancel", () => {
			const onCancel = vi.fn();
			const list = new MultiLineSelectList(items({ label: "A" }), 8, theme);
			list.onCancel = onCancel;
			list.handleInput(ESCAPE);
			expect(onCancel).toHaveBeenCalled();
		});

		it("fires onSelectionChange on navigation", () => {
			const onChange = vi.fn();
			const list = new MultiLineSelectList(items({ label: "A" }, { label: "B" }), 8, theme);
			list.onSelectionChange = onChange;
			list.handleInput(DOWN);
			expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ value: "1" }));
		});
	});

	describe("edge cases", () => {
		it("handles single item", () => {
			const list = new MultiLineSelectList(items({ label: "Only one", description: "reason" }), 8, theme);
			const lines = list.render(80);
			expect(lines[0]).toContain("→ Only one");
			expect(lines[1]).toContain("reason");
			expect(lines).toHaveLength(2);
		});

		it("handles empty items list", () => {
			const list = new MultiLineSelectList([], 8, theme);
			const lines = list.render(80);
			expect(lines).toHaveLength(0);
		});

		it("setSelectedIndex clamps to valid range", () => {
			const list = new MultiLineSelectList(items({ label: "A" }, { label: "B" }), 8, theme);
			list.setSelectedIndex(99);
			const lines = list.render(80);
			expect(lines[1]).toMatch(/^→ B/);

			list.setSelectedIndex(-5);
			const lines2 = list.render(80);
			expect(lines2[0]).toMatch(/^→ A/);
		});

		it("getSelectedItem returns current selection", () => {
			const list = new MultiLineSelectList(items({ label: "A" }, { label: "B" }), 8, theme);
			expect(list.getSelectedItem()?.label).toBe("A");
			list.handleInput(DOWN);
			expect(list.getSelectedItem()?.label).toBe("B");
		});

		it("wraps label at narrow widths", () => {
			const list = new MultiLineSelectList(
				items({ label: "/mach12:pr-review-fix 94 --review-comment 4662883802" }),
				8,
				theme,
			);
			const lines = list.render(30);
			// Should wrap, producing more than one line
			expect(lines.length).toBeGreaterThan(1);
			// All content should be present across lines
			const all = lines.join(" ");
			expect(all).toContain("4662883802");
		});
	});
});
