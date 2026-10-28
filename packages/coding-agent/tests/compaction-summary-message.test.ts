import { stripVTControlCharacters } from "node:util";
import { getRenderedCopy, Markdown, visibleWidth } from "@leanandmean/tui";
import { beforeEach, describe, expect, it } from "vitest";
import { createCompactionSummaryMessage } from "../src/core/messages.js";
import { CompactionSummaryMessageComponent } from "../src/modes/interactive/components/compaction-summary-message.js";
import { getMarkdownTheme, initTheme } from "../src/modes/interactive/theme/theme.js";

function component(summary: string) {
	return new CompactionSummaryMessageComponent(createCompactionSummaryMessage(summary, 250000, "2026-10-27"));
}

function plain(lines: string[]) {
	return lines.map((line) => stripVTControlCharacters(line).trimEnd()).join("\n");
}

describe("compaction summary pane", () => {
	beforeEach(() => initTheme("pi-dark"));

	it("does not emit summary-supplied screen-erasure controls in the collapsed preview", () => {
		const pane = component("BEGIN checkpoint \x1b[2J AFTER");
		const output = pane.render(100).join("\n");
		expect(output).toContain("[compaction]");
		expect(output).not.toContain("\x1b[2J");
	});

	it("shows the persisted approximate count and a complete short beginning without implying omission", () => {
		const pane = component("BEGIN **generated summary** END");
		const output = plain(pane.render(100));
		expect(output).toContain("Estimated pre-compaction context: ~250,000 tokens");
		expect(output).toContain("to expand");
		expect(output).toContain("BEGIN generated summary END");
		expect(output).not.toContain("…");
	});

	it.each([12, 40, 100])("keeps the beginning and at most ten content rows at width %i", (width) => {
		const pane = component(`BEGIN ${"word ".repeat(1000)} END`);
		const preview = pane.children.at(-1)!.render(width - 2);
		expect(preview).toHaveLength(11);
		expect(plain(preview)).toContain("BEGIN");
		expect(plain(preview)).not.toContain("END");
		expect(plain(preview).trimEnd()).toMatch(/…$/);
		for (const row of pane.render(width)) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
	});

	it("clips by rendered rows even for a source shorter than the prefix limit", () => {
		const pane = component(Array.from({ length: 30 }, (_, i) => `ROW-${i}`).join("\n\n"));
		const preview = pane.children.at(-1)!.render(80);
		expect(preview).toHaveLength(11);
		expect(plain(preview)).toContain("ROW-0");
		expect(plain(preview)).not.toContain("ROW-29");
		expect(plain(preview)).toContain("…");
	});

	it("bounds source work to 4096 code units without cutting a surrogate pair", () => {
		const source = `${"a".repeat(4095)}😀TAIL`;
		const pane = component(source);
		const output = plain(pane.children.at(-1)!.render(5000));
		expect(output).toContain("a".repeat(4095));
		expect(output).not.toContain("😀");
		expect(output).not.toMatch(/[\uD800-\uDFFF�]/u);
		expect(output).toContain("…");
		pane.setExpanded(true);
		expect(plain(pane.render(6000))).toContain(source);
	});

	it("does not imply omission exactly at the source limit", () => {
		expect(plain(component("a".repeat(4096)).render(5000))).not.toContain("…");
	});

	it("reflows Unicode after resize/invalidation and preserves the full summary on expansion and recollapse", () => {
		const summary = `BEGIN ${"界😀é ".repeat(110)} END`;
		const pane = component(summary);
		const narrow = plain(pane.render(20));
		expect(narrow).not.toContain("END");
		expect(narrow).toContain("…");
		pane.invalidate();
		const wide = plain(pane.render(200));
		expect(wide).toContain("END");
		expect(wide).not.toContain("…");
		pane.setExpanded(true);
		const expanded = plain(pane.render(2000));
		expect(expanded).toContain(summary);
		expect(expanded).toContain("~250,000 tokens");
		expect(expanded).toContain("otherwise conversation messages only");
		expect(expanded).toContain("not the summarization request size");
		expect(expanded).toContain("to collapse");
		pane.setExpanded(false);
		expect(plain(pane.render(20))).toBe(narrow);
	});

	it("preserves displayed-copy spans and soft-wrap separators when slicing and boxing Markdown", () => {
		const summary = `BEGIN ${"word ".repeat(150)} END`;
		const pane = component(summary);
		const preview = pane.children.at(-1)!.render(28);
		const original = new Markdown(summary, 0, 0, getMarkdownTheme()).render(28);
		expect(preview.slice(0, 10).map(stripVTControlCharacters)).toEqual(
			original.slice(0, 10).map(stripVTControlCharacters),
		);
		expect(getRenderedCopy(preview).slice(0, 10)).toEqual(getRenderedCopy(original).slice(0, 10));
		expect(getRenderedCopy(preview).some((row) => row?.after !== undefined)).toBe(true);
		const boxed = pane.render(30);
		const start = boxed.findIndex((row) => stripVTControlCharacters(row).includes("BEGIN"));
		expect(getRenderedCopy(boxed).slice(start, start + 10)).toEqual(
			getRenderedCopy(preview)
				.slice(0, 10)
				.map((row) => row && { ...row, start: row.start + 1, end: row.end + 1 }),
		);
		expect(getRenderedCopy(boxed)[0]).toBeNull();
	});
});
