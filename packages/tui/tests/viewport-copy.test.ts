import { stripVTControlCharacters } from "node:util";
import { describe, expect, it, vi } from "vitest";
import { Box } from "../src/components/box.js";
import { Markdown, type MarkdownTheme } from "../src/components/markdown.js";
import { Text } from "../src/components/text.js";
import { getRenderedCopy, setRenderedCopy } from "../src/render-copy.js";
import { type Component, Container } from "../src/tui.js";
import * as utils from "../src/utils.js";
import { RetainedViewport } from "../src/viewport.js";

async function copyAll(component: Component, width: number) {
	const rows = component.render(width).length;
	const copy = vi.fn(async (_text: string) => {});
	const viewport = new RetainedViewport({ getBlocks: () => [{ component }], copy });
	viewport.update(width, rows + 1);
	viewport.markPainted();
	for (const event of ["\x1b[<0;1;1M", `\x1b[<32;${width + 1};${rows}M`, `\x1b[<0;${width + 1};${rows}m`, "\x03"])
		viewport.handleInput(event, false, false);
	await Promise.resolve();
	viewport.cancelInteraction();
	return copy.mock.calls[0]?.[0];
}

const identity = (text: string) => text;
const theme: MarkdownTheme = {
	heading: identity,
	link: identity,
	linkUrl: identity,
	code: identity,
	codeBlock: identity,
	codeBlockBorder: identity,
	quote: identity,
	quoteBorder: identity,
	hr: identity,
	listBullet: identity,
	bold: identity,
	italic: identity,
	strikethrough: identity,
	underline: identity,
};

describe("retained copy provenance", () => {
	it.each(["container", "box", "markdown"])("renders 150,000 rows without argument-count limits in %s", (kind) => {
		const text = Array(150_000).fill("x").join("\n");
		let component: Component;
		if (kind === "markdown") component = new Markdown(`\`\`\`\n${text}\n\`\`\``, 0, 0, theme);
		else {
			const parent = kind === "box" ? new Box(0, 0) : new Container();
			parent.addChild(new Text(text, 0, 0));
			component = parent;
		}
		expect(component.render(20)).toHaveLength(150_000 + (kind === "markdown" ? 2 : 0));
	});

	it("does not remeasure unchanged live rows or republish unchanged copy metadata", () => {
		const box = new Box(1, 1);
		box.addChild(
			new Text(Array.from({ length: 900 }, (_, i) => `\x1b[31mrow-${i} café 界 e\u0301\x1b[0m`).join("\n"), 0, 0),
		);
		const container = new Container();
		container.addChild(box);
		const viewport = new RetainedViewport({ getBlocks: () => [{ component: container }] });
		viewport.update(79, 24);
		const copyRows = getRenderedCopy(container.render(79));
		const measure = vi.spyOn(utils, "visibleWidth");
		try {
			viewport.update(79, 24);
			expect(measure).not.toHaveBeenCalled();
			expect(getRenderedCopy(container.render(79))).toBe(copyRows);
		} finally {
			measure.mockRestore();
		}
	});

	it.each(["box", "container"])("does not trust a %s cache whose output was mutated and reannotated", async (kind) => {
		const component = kind === "box" ? new Box(0, 0) : new Container();
		component.addChild(new Text("alpha beta", 0, 0));
		const lines = component.render(5);
		lines[0] = "other";
		setRenderedCopy(lines, [
			{ start: 0, end: 5, after: " " },
			{ start: 0, end: 4 },
		]);
		expect(await copyAll(component, 5)).toBe("alpha beta");
	});

	it("refreshes a live normalization cache for same-array byte and metadata changes", async () => {
		const lines = ["alpha", "beta"];
		const component = { render: () => lines, invalidate() {} };
		const copy = vi.fn(async (_text: string) => {});
		const viewport = new RetainedViewport({ getBlocks: () => [{ component }], copy });
		viewport.update(5, 5);
		lines[0] = "other";
		viewport.update(5, 5);
		expect(viewport.slice(5, false).lines[0]).toBe("other");
		setRenderedCopy(lines, [
			{ start: 0, end: 5, after: " " },
			{ start: 0, end: 4 },
		]);
		viewport.update(5, 5);
		setRenderedCopy(lines, [
			{ start: 0, end: 5 },
			{ start: 0, end: 4 },
		]);
		viewport.update(5, 5);
		viewport.markPainted();
		for (const event of ["\x1b[<0;1;1M", "\x1b[<32;6;2M", "\x1b[<0;6;2m", "\x03"])
			viewport.handleInput(event, false, false);
		await Promise.resolve();
		expect(copy).toHaveBeenCalledExactlyOnceWith("other\nbeta");
		viewport.cancelInteraction();
	});

	it.each([14, 40])("preserves code indentation inside lists at width %s", async (width) => {
		const text = '- Example:\n\n  ```python\n  if True:\n      print("ok")\n  ```';
		expect(await copyAll(new Markdown(text, 0, 0, theme), width)).toBe(text);
	});

	it.each(["\u{1f469}\x1b[31m\u200d\u{1f4bb}", "\u{1f1fa}\x1b[31m\u{1f1f8}"])(
		"keeps ANSI-separated graphemes whole through wrapping and copying",
		async (grapheme) => {
			const text = `123456789${grapheme}Zabcd`;
			const component = new Markdown(text, 0, 0, theme);
			expect(component.render(11).map((line) => stripVTControlCharacters(line).trimEnd())).toEqual([
				`123456789${stripVTControlCharacters(grapheme)}`,
				"Zabcd",
			]);
			expect(await copyAll(component, 11)).toBe(stripVTControlCharacters(text));
		},
	);

	it.each(["alpha beta", "alpha\nbeta"])("distinguishes wrapping from source breaks in %j", async (text) => {
		expect(await copyAll(new Text(text, 0, 0), 5)).toBe(text);
	});

	it("removes nested UI padding without deleting indentation or blank source lines", async () => {
		const text = "  alpha beta gamma\n    indented\n\nlast";
		const box = new Box(2, 1);
		box.addChild(new Text(text, 1, 1));
		expect(await copyAll(box, 18)).toBe(text);
	});

	it("does not insert separators inside wrapped long words", async () => {
		expect(await copyAll(new Text("abcdefghij", 1, 1), 6)).toBe("abcdefghij");
	});

	it("unwraps prose and code without deleting code indentation", async () => {
		const text = "alpha beta gamma delta\n\n```ts\n    const x = 1;\n    x++;\n```";
		expect(await copyAll(new Markdown(text, 1, 1, theme), 20)).toBe(text);
	});

	it.each(["", "    "])("excludes code gutters even when a long token wraps after %j indentation", async (indent) => {
		const text = `\`\`\`\n${indent}verylongidentifier\n\`\`\``;
		expect(await copyAll(new Markdown(text, 0, 0, theme), 10)).toBe(text);
	});

	it("rebuilds a Box cache if a wrapper mutated its annotated output", async () => {
		const box = new Box(0, 0);
		box.addChild(new Text("alpha beta", 0, 0));
		box.render(5)[0] = "other";
		expect(await copyAll(box, 5)).toBe("alpha beta");
	});

	it.each([
		{ line: "e\x1b[31m\u0301Z", end: 2 },
		{ line: "\u{1f469}\x1b[31m\u200d\u{1f4bb}Z", end: 3 },
	])("does not duplicate ANSI-separated grapheme suffixes when highlighting", ({ line, end }) => {
		const viewport = new RetainedViewport({
			getBlocks: () => [{ component: { render: () => [line], invalidate() {} } }],
		});
		viewport.update(12, 5);
		viewport.markPainted();
		viewport.handleInput("\x1b[<0;1;1M", false, false);
		viewport.handleInput(`\x1b[<32;${end};1M`, false, false);
		expect(stripVTControlCharacters(viewport.slice(12, false).lines[0])).toBe(stripVTControlCharacters(line));
		viewport.cancelInteraction();
	});

	it("removes soft list continuation padding while keeping item boundaries", async () => {
		const text = "- alpha beta gamma delta\n- second item";
		expect(await copyAll(new Markdown(text, 1, 0, theme), 14)).toBe(text);
	});

	it("keeps a hard boundary between components", async () => {
		const container = new Container();
		container.addChild(new Text("alpha beta", 1, 1));
		container.addChild(new Text("gamma delta", 1, 1));
		expect(await copyAll(container, 8)).toBe("alpha beta\ngamma delta");
	});

	it("refreshes cached copy metadata when the same painted rows change their source breaks", async () => {
		const text = new Text("alpha beta", 0, 0);
		const box = new Box(0, 0);
		box.addChild(text);
		expect(await copyAll(box, 5)).toBe("alpha beta");
		text.setText("alpha\nbeta");
		expect(await copyAll(box, 5)).toBe("alpha\nbeta");
	});

	it("holds copy provenance with the selected frame rather than newer component content", async () => {
		const text = new Text("alpha beta", 0, 0);
		const copy = vi.fn(async (_text: string) => {});
		const viewport = new RetainedViewport({ getBlocks: () => [{ component: text }], copy });
		viewport.update(5, 5);
		viewport.markPainted();
		for (const event of ["\x1b[<0;1;1M", "\x1b[<32;6;2M", "\x1b[<0;6;2m"]) viewport.handleInput(event, false, false);
		text.setText("alpha\nbeta");
		viewport.update(5, 5);
		viewport.handleInput("\x03", false, false);
		await Promise.resolve();
		expect(copy).toHaveBeenCalledExactlyOnceWith("alpha beta");
		viewport.cancelInteraction();
	});

	it("falls back to displayed text when a custom wrapper mutates annotated output", async () => {
		const lines = new Text("alpha beta", 0, 0).render(5);
		lines[0] = "other";
		expect(await copyAll({ render: () => lines, invalidate() {} }, 5)).toBe("other\nbeta ");
	});

	it("copies partial wrapped endpoints without adding the unselected prefix or suffix", async () => {
		const text = new Text("alpha beta gamma", 0, 0);
		const copy = vi.fn(async (_text: string) => {});
		const viewport = new RetainedViewport({ getBlocks: () => [{ component: text }], copy });
		viewport.update(6, 6);
		viewport.markPainted();
		for (const event of ["\x1b[<0;3;1M", "\x1b[<32;4;3M", "\x1b[<0;4;3m", "\x03"])
			viewport.handleInput(event, false, false);
		await Promise.resolve();
		expect(copy).toHaveBeenCalledExactlyOnceWith("pha beta gam");
		viewport.cancelInteraction();
	});

	it("preserves the crossed hard break when selection ends at the next line start", async () => {
		const component = new Text("alpha\nbeta", 0, 0);
		const copy = vi.fn(async (_text: string) => {});
		const viewport = new RetainedViewport({ getBlocks: () => [{ component }], copy });
		try {
			viewport.update(12, 5);
			viewport.markPainted();
			expect(
				viewport
					.slice(12, false)
					.lines.slice(0, 2)
					.map((line) => line.trimEnd()),
			).toEqual(["alpha", "beta"]);
			for (const event of ["\x1b[<0;1;1M", "\x1b[<32;1;2M", "\x1b[<0;1;2m", "\x03"])
				viewport.handleInput(event, false, false);
			await Promise.resolve();
			expect(copy).toHaveBeenCalledExactlyOnceWith("alpha\n");
		} finally {
			viewport.cancelInteraction();
		}
	});

	it("does not let a padding-only copy fall through to editor clearing", async () => {
		const copy = vi.fn(async (_text: string) => {});
		const viewport = new RetainedViewport({ getBlocks: () => [{ component: new Text("body", 2, 0) }], copy });
		viewport.update(12, 5);
		viewport.markPainted();
		for (const event of ["\x1b[<0;1;1M", "\x1b[<32;2;1M", "\x1b[<0;2;1m"]) viewport.handleInput(event, false, false);
		expect(viewport.handleInput("\x03", false, false)).toBe(true);
		await Promise.resolve();
		expect(copy).not.toHaveBeenCalled();
		viewport.cancelInteraction();
	});

	it("clips provenance to displayed cells without recovering hidden text", async () => {
		const lines = setRenderedCopy(["kept hidden"], [{ start: 0, end: 11 }]);
		expect(await copyAll({ render: () => lines, invalidate() {} }, 5)).toBe("kept ");
	});

	it("keeps combining characters separated by ANSI in the same selected grapheme", async () => {
		const copy = vi.fn(async (_text: string) => {});
		const viewport = new RetainedViewport({
			getBlocks: () => [{ component: { render: () => ["e\x1b[31m\u0301界Z\x1b[0m"], invalidate() {} } }],
			copy,
		});
		viewport.update(10, 5);
		viewport.markPainted();
		for (const event of ["\x1b[<0;1;1M", "\x1b[<32;4;1M", "\x1b[<0;4;1m", "\x03"])
			viewport.handleInput(event, false, false);
		await Promise.resolve();
		expect(copy).toHaveBeenCalledExactlyOnceWith("e\u0301界");
		viewport.cancelInteraction();
	});

	it("rejects alternate text and out-of-range provenance", () => {
		expect(() => setRenderedCopy(["text"], [{ start: 0, end: 5 }])).toThrow(/Copy metadata/);
		expect(() => setRenderedCopy(["text"], [{ start: 0, end: 4, after: "hidden" }])).toThrow(/Copy metadata/);
		expect(() => setRenderedCopy(["text"], new Array(1))).toThrow(/Copy metadata/);
		expect(() => setRenderedCopy(["text"], [{ start: 0, end: 4, after: [" "] as unknown as string }])).toThrow(
			/Copy metadata/,
		);
		expect(getRenderedCopy(["  text"])).toEqual([{ start: 0, end: 6 }]);
	});

	it("validates and publishes one snapshot of caller-provided metadata", () => {
		let reads = 0;
		const lines = ["text"];
		setRenderedCopy(lines, [
			{
				start: 0,
				end: 4,
				get after() {
					return ++reads === 1 ? " " : "hidden";
				},
			},
		]);
		expect(reads).toBe(1);
		expect(getRenderedCopy(lines)[0]?.after).toBe(" ");
	});

	it("keeps physical-line copying for unannotated custom renderers", async () => {
		expect(await copyAll({ render: () => ["  custom", "    text"], invalidate() {} }, 20)).toBe("  custom\n    text");
	});
});
