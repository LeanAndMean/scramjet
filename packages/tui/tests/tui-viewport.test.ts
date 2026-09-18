import { afterEach, describe, expect, it, vi } from "vitest";
import { Image } from "../src/components/image.js";
import { Text } from "../src/components/text.js";
import { resetCapabilitiesCache, setCapabilities } from "../src/terminal-image.js";
import { type Component, CURSOR_MARKER, TUI } from "../src/tui.js";
import { sliceByColumn } from "../src/utils.js";
import type { ViewportBlock } from "../src/viewport.js";
import { HeadlessTerminal } from "./helpers/headless-terminal.js";

class Rows implements Component {
	readonly render = vi.fn((_width: number) => [...this.lines]);
	readonly invalidate = vi.fn();
	constructor(public lines: string[]) {}
}

const running: TUI[] = [];
afterEach(() => {
	for (const tui of running.splice(0)) tui.stop();
	resetCapabilitiesCache();
});

async function setup(blocks: ViewportBlock[], width = 21, height = 4) {
	const terminal = new HeadlessTerminal(width, height);
	terminal.write("shell sentinel\r\n\x1b[?1049h");
	const tui = new TUI(terminal, true);
	tui.configureViewport({ getBlocks: () => blocks });
	running.push(tui);
	tui.start();
	await tui.renderNow({ requireFlush: true });
	const frame = async () => tui.renderNow({ requireFlush: true });
	const text = () =>
		terminal.visibleLines().map((line) => sliceByColumn(line, 0, terminal.columns - 1, true).trimEnd());
	return { tui, terminal, frame, text };
}

describe("retained viewport", () => {
	it("retains tall mutable output, paints exact slices, and follows only an explicit return to the tail", async () => {
		const card = new Rows(Array.from({ length: 12 }, (_, i) => `card-${i}`));
		const { tui, terminal, frame, text } = await setup([{ component: card }]);
		expect(text()).toEqual(["card-8", "card-9", "card-10", "card-11"]);
		tui.scrollViewport(-7);
		await frame();
		expect(text()).toEqual(["card-1", "card-2", "card-3", "card-4"]);
		card.lines.push("card-12");
		card.lines.unshift("new first");
		await frame();
		expect(text()).toEqual(["card-1", "card-2", "card-3", "card-4"]);
		expect(tui.getViewportState()).toMatchObject({ offset: 2, totalRows: 14, followingTail: false });
		tui.scrollViewportTo(100);
		await frame();
		card.lines.push("card-13");
		await frame();
		expect(text()).toEqual(["card-10", "card-11", "card-12", "card-13"]);
		expect(tui.getViewportState()?.followingTail).toBe(true);
		expect(terminal.bufferLines()).toHaveLength(4);
		expect(terminal.writes.join("")).not.toContain("\x1b[3J");
	});

	it("reserves a scrollbar column even when content is short and clears stale rows and cells", async () => {
		const card = new Rows(["a", "long old row", "c", "d", "e"]);
		const { frame, text, terminal } = await setup([{ component: card }], 21, 4);
		expect(card.render).toHaveBeenLastCalledWith(20);
		card.lines = ["tiny"];
		await frame();
		expect(text()).toEqual(["tiny", "", "", ""]);
		expect(terminal.visibleLines()).toEqual([`tiny${" ".repeat(17)}`, ...Array(3).fill(" ".repeat(21))]);
		card.lines = [];
		await frame();
		expect(terminal.visibleLines()).toEqual(Array(4).fill(" ".repeat(21)));
		card.lines = ["back"];
		await frame();
		expect(text()).toEqual(["back", "", "", ""]);
	});

	it("keeps identity across promotion, caches finalized blocks, and distinguishes invalidation from reset", async () => {
		const history = new Rows(Array.from({ length: 200 }, (_, i) => `history-${i}`));
		const card = new Rows(["first", "reading", "last", "extra", "tail"]);
		const status = new Rows(["tick"]);
		const blocks: ViewportBlock[] = [
			{ component: history, finalized: true },
			{ component: card },
			{ component: status },
		];
		const { tui, frame, text, terminal } = await setup(blocks);
		tui.scrollViewportTo(201);
		await frame();
		expect(text()[0]).toBe("reading");
		blocks[1] = { component: card, finalized: true };
		await frame();
		const finalizedCount = card.render.mock.calls.length;
		for (let i = 0; i < 10; i++) {
			status.lines = [`tick-${i}`];
			await frame();
		}
		expect(history.render).toHaveBeenCalledTimes(1);
		expect(card.render).toHaveBeenCalledTimes(finalizedCount);
		expect(text()[0]).toBe("reading");
		card.lines.unshift("inserted");
		blocks[1] = { component: card, finalized: true, revision: 1 };
		await frame();
		expect(text()[0]).toBe("reading");
		tui.invalidate();
		await frame();
		expect(history.render).toHaveBeenCalledTimes(2);
		expect(text()[0]).toBe("reading");
		tui.rebuild();
		await frame();
		expect(history.render).toHaveBeenCalledTimes(3);
		expect(text()[0]).toBe("reading");
		terminal.resize(19, 4);
		await frame();
		expect(history.render).toHaveBeenCalledTimes(4);
		tui.resetViewport();
		await frame();
		expect(tui.getViewportState()?.followingTail).toBe(true);
		expect(text().at(-1)).toBe("tick-9");
	});

	it("preserves the same reader anchor on height changes without silently reattaching at a clamped bottom", async () => {
		const card = new Rows(Array.from({ length: 20 }, (_, i) => `row-${i}`));
		const { tui, frame, terminal } = await setup([{ component: card }]);
		tui.scrollViewportTo(12);
		await frame();
		terminal.resize(21, 10);
		await frame();
		expect(tui.getViewportState()).toMatchObject({ offset: 10, followingTail: false });
		terminal.resize(21, 4);
		await frame();
		expect(tui.getViewportState()?.offset).toBe(12);
	});

	it("keeps a lower-screen anchor visible on height shrink and restores its desired position on expansion", async () => {
		const card = new Rows(Array.from({ length: 20 }, (_, i) => `row-${i}`));
		const { tui, frame, terminal } = await setup([{ component: card }]);
		tui.scrollViewportTo(5, 3);
		await frame();
		terminal.resize(21, 2);
		await frame();
		expect(tui.getViewportState()?.offset).toBe(7);
		expect(terminal.visibleLines()[1]).toMatch(/^row-8 /);
		terminal.resize(21, 4);
		await frame();
		expect(tui.getViewportState()?.offset).toBe(5);
		expect(terminal.visibleLines()[3]).toMatch(/^row-8 /);
	});

	it("maps padding-only Text reflow with zero equal raw rows without changing component spacing", async () => {
		const text = new Text("A\nB\nC", 0, 0);
		const wide = text.render(12);
		expect(text.render(9).some((row) => wide.includes(row))).toBe(false);
		const { tui, terminal, frame } = await setup(
			[
				{ component: new Rows(["before-0", "before-1", "before-2"]) },
				{ component: text },
				{ component: new Rows(["after-0", "after-1", "after-2", "after-3"]) },
			],
			13,
			3,
		);
		tui.scrollViewportTo(4);
		await frame();
		expect(terminal.visibleLines()[0]).toMatch(/^B {11}/);
		terminal.resize(10, 3);
		await frame();
		expect(terminal.visibleLines()[0]).toMatch(/^B {8}/);
		expect(tui.getViewportState()?.offset).toBe(4);
	});

	it("retains an interior grapheme through complete narrower/wider Text rewrapping and repeated content", async () => {
		const source = "0123456789".repeat(30);
		const text = new Text(`\x1b[31m${source}\x1b[0m`, 0, 0);
		const wide = text.render(12);
		expect(text.render(9).some((row) => wide.includes(row))).toBe(false);
		const { tui, terminal, frame } = await setup([{ component: text }], 13, 3);
		tui.scrollViewportTo(10);
		await frame();
		// Offset 120 is the first grapheme of row 10 at width 12, not the first repeated "0".
		terminal.resize(10, 3);
		await frame();
		expect(tui.getViewportState()?.offset).toBe(13);
		expect(terminal.visibleLines()[0].slice(0, 9)).toBe(source.slice(117, 126));
		terminal.resize(13, 3);
		await frame();
		expect(tui.getViewportState()?.offset).toBe(10);
		expect(terminal.visibleLines()[0].slice(0, 12)).toBe(source.slice(120, 132));
	});

	it("anchors ANSI-styled wide and combining graphemes, retaining ordered repeated occurrences", async () => {
		const text = new Text(`\x1b[32m${"界e\u0301x".repeat(80)}\x1b[0m`, 0, 0);
		const { tui, terminal, frame } = await setup([{ component: text }], 13, 3);
		tui.scrollViewportTo(10);
		await frame();
		// Each repetition is four cells; content cell 120 belongs to row 15 at width 8.
		terminal.resize(9, 3);
		await frame();
		expect(tui.getViewportState()?.offset).toBe(15);
		expect(terminal.visibleLines()[0]).toMatch(/^界e\u0301x界e\u0301x/);
		terminal.resize(13, 3);
		await frame();
		expect(tui.getViewportState()?.offset).toBe(10);
	});

	it("falls back to surviving content in the same block before adjacent blocks", async () => {
		const card = new Rows(["aaa", "bbb", "ccc", "ddd", "eee"]);
		const blocks: ViewportBlock[] = [
			{ component: new Rows(["prefix"]) },
			{ component: card },
			{ component: new Rows(["next-0", "next-1", "next-2", "next-3"]) },
		];
		const { tui, frame, text } = await setup(blocks, 21, 3);
		tui.scrollViewportTo(3);
		await frame();
		card.lines.splice(2, 1);
		await frame();
		expect(text()[0]).toBe("ddd");
		blocks.splice(1, 1);
		await frame();
		expect(text()[0]).toBe("next-0");
	});

	it("keeps a deterministic ordinal anchor within blank-only blocks", async () => {
		const blanks = new Rows(Array(12).fill(" "));
		const { tui, frame } = await setup([{ component: blanks }, { component: new Rows(["end"]) }]);
		tui.scrollViewportTo(5);
		await frame();
		blanks.lines = Array(14).fill("  ");
		await frame();
		expect(tui.getViewportState()).toMatchObject({ offset: 5, followingTail: false });
	});

	it("places cursor and overlays relative to the viewed slice rather than the document tail", async () => {
		const card = new Rows(["0", "1", `界e\u0301${CURSOR_MARKER}x`, "3", "4", "5", "6", "7", "8"]);
		const { tui, terminal, frame, text } = await setup([{ component: card }]);
		tui.scrollViewportTo(1);
		await frame();
		expect(terminal.cursorPosition()).toEqual({ row: 1, col: 3 });
		const overlay = tui.showOverlay(new Rows([`M${CURSOR_MARKER}ODAL`]), { row: 2, col: 0, width: 5 });
		await frame();
		expect(text()).toEqual(["1", "界e\u0301x", "MODAL", "4"]);
		expect(terminal.cursorPosition()).toEqual({ row: 2, col: 1 });
		overlay.hide();
		await frame();
		expect(terminal.cursorPosition()).toEqual({ row: 1, col: 3 });
		tui.scrollViewportTo(5);
		const mark = terminal.markWrites();
		await frame();
		expect(terminal.writesSince(mark)).toContain("\x1b[?25l");
	});

	it("bounds oversized overlays to the screen without shifting the base slice", async () => {
		const { tui, terminal, frame } = await setup([{ component: new Rows(["0", "1", "2", "3", "4", "5"]) }]);
		tui.scrollViewportTo(0);
		await frame();
		tui.showOverlay(new Rows(["a", "b", "c", "d", "e", "f"]), { width: 2, row: 0, col: 0 });
		await frame();
		expect(terminal.visibleLines().map((row) => row[0])).toEqual(["a", "b", "c", "d"]);
		expect(tui.getViewportState()?.offset).toBe(0);
	});

	it("fits tall overlays within margins and never emits an offscreen cursor row", async () => {
		const { tui, terminal, frame, text } = await setup([{ component: new Rows(["0", "1", "2", "3", "4", "5"]) }]);
		tui.scrollViewportTo(0);
		await frame();
		tui.showOverlay(new Rows(["a", "b", `${CURSOR_MARKER}c`, "d"]), { width: 1, margin: { top: 1 }, col: 0 });
		const mark = terminal.markWrites();
		await frame();
		expect(text()).toEqual(["0", "a", "b", "c"]);
		expect(terminal.cursorPosition()).toEqual({ row: 3, col: 0 });
		expect(terminal.writesSince(mark)).not.toMatch(/\x1b\[[5-9];\d+H/);
	});

	it.each(["kitty", "iterm2"] as const)(
		"paints %s placements atomically at absolute rows and exposes clipped spans",
		async (protocol) => {
			setCapabilities({ images: protocol, trueColor: true, hyperlinks: true });
			const image = new Image(
				"aW1hZ2U=",
				"image/png",
				{ fallbackColor: (s) => s },
				{ maxWidthCells: 6, maxHeightCells: 3, imageId: 44 },
				{ widthPx: 54, heightPx: 54 },
			);
			const { tui, terminal, frame } = await setup(
				[
					{ component: new Rows(["0", "1", "2"]) },
					{ component: image, finalized: true },
					{ component: new Rows(["6", "7", "8", "9", "10"]) },
				],
				41,
				5,
			);
			tui.scrollViewportTo(2);
			let mark = terminal.markWrites();
			await frame();
			const prefix = protocol === "kitty" ? "\x1b_Ga=T" : "\x1b]1337;File=";
			expect(terminal.writesSince(mark)).toContain(prefix);
			expect(terminal.writesSince(mark)).toContain(`\x1b[2;1H${prefix}`);
			expect(terminal.writesSince(mark)).not.toContain("\x1b[2A");
			tui.scrollViewportTo(4);
			mark = terminal.markWrites();
			await frame();
			expect(terminal.writesSince(mark)).not.toContain(prefix);
			expect(terminal.visibleLines()[0]).toContain("Image clipped");
			if (protocol === "kitty") expect(terminal.writesSince(mark)).toContain("a=d,d=I,i=44");
			tui.scrollViewportTo(0);
			mark = terminal.markWrites();
			await frame();
			expect(terminal.writesSince(mark)).not.toContain(prefix);
			expect(terminal.visibleLines()[3]).toContain("Image clipped");
		},
	);

	it("preserves an explicitly chosen screen-row offset through growth and reflow", async () => {
		const before = new Rows(["before-0", "before-1", "before-2"]);
		const text = new Text("A\nB\nC", 0, 0);
		const { tui, terminal, frame } = await setup(
			[{ component: before }, { component: text }, { component: new Rows(["after-0", "after-1", "after-2"]) }],
			13,
			3,
		);
		tui.scrollViewportTo(3, 1);
		await frame();
		before.lines.unshift("inserted");
		terminal.resize(10, 3);
		await frame();
		expect(terminal.visibleLines()[1]).toMatch(/^B {8}/);
		expect(tui.getViewportState()?.offset).toBe(4);
	});

	it("survives word wrapping, restyling and edits inside an aggregate card without first-match jumps", async () => {
		const phrase = "alpha beta gamma delta epsilon zeta ";
		const card = new Text(phrase.repeat(20), 0, 0);
		const { tui, terminal, frame } = await setup([{ component: card }], 18, 3);
		tui.scrollViewportTo(20);
		await frame();
		const before = terminal.visibleLines()[0].slice(0, 17).trim();
		card.setText(`\x1b[34m${"inserted header\n"}${phrase.repeat(20)}\x1b[0m`);
		await frame();
		expect(tui.getViewportState()?.offset).toBe(21);
		expect(terminal.visibleLines()[0].slice(0, 17).trim()).toBe(before);
		terminal.resize(12, 3);
		await frame();
		expect(tui.getViewportState()!.offset).toBeGreaterThan(21);
		terminal.resize(18, 3);
		await frame();
		expect(tui.getViewportState()?.offset).toBe(21);
	});

	it("handles a single-cell viewport and blank or removed projections without stale output", async () => {
		const blocks: ViewportBlock[] = [{ component: new Text("abcdef", 0, 0) }];
		const { tui, terminal, frame } = await setup(blocks, 1, 1);
		expect(terminal.visibleLines()).toEqual(["f"]);
		tui.scrollViewportTo(2);
		await frame();
		expect(terminal.visibleLines()).toEqual(["c"]);
		blocks.length = 0;
		await frame();
		expect(terminal.visibleLines()[0].trim()).toBe("");
		blocks.push({ component: new Text("uvwxyz", 0, 0) });
		await frame();
		expect(terminal.visibleLines()).toEqual(["u"]);
	});

	it("requires unique block identities and preserves the two unconfigured rendering contracts", async () => {
		const component = new Rows(["one"]);
		const blocks = [{ component }];
		const { tui, frame } = await setup(blocks);
		blocks.push({ component });
		await expect(frame()).rejects.toThrow("unique component");
		blocks.pop();
		expect(() => tui.setLiveRegionStart(component)).toThrow("with a viewport");
		const legacy = new TUI(new HeadlessTerminal());
		legacy.addChild(component);
		legacy.setLiveRegionStart(component);
		expect(() => legacy.configureViewport({ getBlocks: () => [] })).toThrow("committed live region");
		legacy.stop();
	});

	it("rejects required flushing on unsupported terminals", async () => {
		const { tui, terminal } = await setup([]);
		Object.defineProperty(terminal, "flush", { value: undefined });
		await expect(tui.renderNow({ requireFlush: true })).rejects.toThrow("Terminal flush is required");
	});

	it("supports immediate flush independently while preserving committed-mode preconditions", async () => {
		const { tui, terminal } = await setup([]);
		await expect(tui.commitNow()).rejects.toThrow("without a live region");
		vi.spyOn(terminal, "flush").mockRejectedValueOnce(new Error("transport failed"));
		await expect(tui.renderNow({ requireFlush: true })).rejects.toThrow("transport failed");
		tui.stop();
		await expect(tui.renderNow()).rejects.toThrow("stopped");
	});
});
