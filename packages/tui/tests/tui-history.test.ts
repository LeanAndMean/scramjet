import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Editor, type EditorTheme } from "../src/components/editor.js";
import { Image } from "../src/components/image.js";
import { hyperlink, resetCapabilitiesCache, setCapabilities } from "../src/terminal-image.js";
import type { Component } from "../src/tui.js";
import { Container, CURSOR_MARKER, TUI } from "../src/tui.js";
import { HeadlessTerminal } from "./helpers/headless-terminal.js";

class MutableComponent implements Component {
	constructor(public lines: string[] = []) {}
	invalidate(): void {}
	render(): string[] {
		return [...this.lines];
	}
}

class CountingComponent extends MutableComponent {
	renderCount = 0;
	override render(): string[] {
		this.renderCount += 1;
		return super.render();
	}
}

async function render(tui: TUI, terminal: HeadlessTerminal): Promise<void> {
	void tui;
	await vi.runAllTimersAsync();
	await terminal.flush();
}

describe("TUI committed history", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => {
		resetCapabilitiesCache();
		vi.useRealTimers();
	});

	it("bounds mutable output to the live canvas", async () => {
		const terminal = new HeadlessTerminal(30, 5);
		const tui = new TUI(terminal);
		const history = new MutableComponent(["history"]);
		const live = new MutableComponent(["live-1", "live-2", "live-3", "live-4", "live-5", "editor"]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);

		expect(terminal.writes.join("")).not.toContain("live-1");
		expect(terminal.visibleLines().join("\n")).toContain("editor");
	});

	it("does not clear or replay history when oversized live output shrinks", async () => {
		const terminal = new HeadlessTerminal(30, 5);
		const tui = new TUI(terminal);
		const history = new MutableComponent([
			"HISTORY-FIRST",
			"history-2",
			"history-3",
			"history-4",
			"history-5",
			"HISTORY-LAST",
		]);
		const live = new MutableComponent(["one", "two", "three", "four", "editor"]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);
		terminal.scrollLines(-4);
		const viewportY = terminal.viewportY;
		expect(viewportY).toBeGreaterThan(0);
		const visibleLines = terminal.visibleLines();
		const mark = terminal.markWrites();

		live.lines = ["short", "editor"];
		tui.requestRender();
		await render(tui, terminal);

		const output = terminal.writesSince(mark);
		expect(output).not.toContain("\x1b[2J");
		expect(output).not.toContain("\x1b[3J");
		expect(output).not.toContain("HISTORY-FIRST");
		const buffer = terminal.bufferLines().join("\n");
		expect(buffer.match(/HISTORY-FIRST/g)).toHaveLength(1);
		expect(buffer.match(/HISTORY-LAST/g)).toHaveLength(1);
		expect(terminal.viewportY).toBe(viewportY);
		expect(terminal.visibleLines()).toEqual(visibleLines);
	});

	it("keeps OSC 8 hyperlinks balanced when tail-windowing live output", async () => {
		const terminal = new HeadlessTerminal(30, 3);
		const tui = new TUI(terminal);
		const history = new MutableComponent(["history"]);
		const live = new MutableComponent([
			hyperlink("omitted", "https://example.com/omitted"),
			hyperlink("retained-one", "https://example.com/one"),
			hyperlink("retained-two", "https://example.com/two"),
		]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);

		const output = terminal.writes.join("");
		expect(output).not.toContain("https://example.com/omitted");
		for (const url of ["https://example.com/one", "https://example.com/two"]) {
			expect(output).toContain(`\x1b]8;;${url}\x1b\\`);
		}
		expect(output.match(/\x1b\]8;;https:\/\/example\.com\/(?:one|two)\x1b\\/g)).toHaveLength(2);
		expect(output.match(/\x1b\]8;;\x1b\\/g)).toHaveLength(2);
	});

	it("omits an iTerm2 placement when its reserved rows are clipped", async () => {
		setCapabilities({ images: "iterm2", trueColor: true, hyperlinks: true });
		const terminal = new HeadlessTerminal(30, 3);
		const tui = new TUI(terminal);
		const history = new MutableComponent(["COMMITTED-HISTORY"]);
		const live = new Container();
		live.addChild(
			new Image(
				"image-data",
				"image/png",
				{ fallbackColor: (text) => text },
				{ maxWidthCells: 6, maxHeightCells: 6 },
				{ widthPx: 54, heightPx: 108 },
			),
		);
		live.addChild(new MutableComponent(["editor"]));
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);

		const output = terminal.writes.join("");
		expect(output).not.toContain("\x1b]1337;File=");
		expect(output).not.toContain("\x1b[5A");
		expect(terminal.bufferLines().join("\n")).toContain("COMMITTED-HISTORY");
	});

	it("reuses committed renders during routine live updates", async () => {
		const terminal = new HeadlessTerminal(30, 5);
		const tui = new TUI(terminal);
		const history = new CountingComponent(["history"]);
		const live = new MutableComponent(["editor"]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);
		expect(history.renderCount).toBe(1);

		live.lines = ["status", "editor"];
		tui.requestRender();
		await render(tui, terminal);
		expect(history.renderCount).toBe(1);

		history.lines.push("finalized");
		tui.commit();
		await render(tui, terminal);
		expect(history.renderCount).toBe(2);

		tui.rebuild();
		await render(tui, terminal);
		expect(history.renderCount).toBe(3);
	});

	it.each(["changes", "shrinks"])("rejects committed history that %s", async (mutation) => {
		const terminal = new HeadlessTerminal(30, 5);
		const tui = new TUI(terminal);
		const history = new MutableComponent(["stable-history"]);
		const live = new MutableComponent(["editor"]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);

		history.lines = mutation === "changes" ? ["changed-history"] : [];
		tui.commit();

		await expect(render(tui, terminal)).rejects.toThrow(
			"Committed history changed; use rebuild() for a deliberate rebuild",
		);
	});

	it("commits a complete tail-windowed response exactly once", async () => {
		const terminal = new HeadlessTerminal(30, 5);
		const tui = new TUI(terminal);
		const history = new Container();
		const response = new MutableComponent(["answer-1", "answer-2", "answer-3", "answer-4", "answer-5"]);
		const live = new Container();
		const editor = new MutableComponent(["editor"]);
		live.addChild(response);
		live.addChild(editor);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);
		const mark = terminal.markWrites();

		live.removeChild(response);
		history.addChild(response);
		tui.commit();
		await render(tui, terminal);

		const output = terminal.writesSince(mark);
		expect(output.match(/answer-1/g)).toHaveLength(1);
		expect(output).not.toContain("\x1b[3J");
		const buffer = terminal.bufferLines().join("\n");
		for (let i = 1; i <= 5; i++) expect(buffer.match(new RegExp(`answer-${i}`, "g"))).toHaveLength(1);
		expect(buffer.indexOf("answer-1")).toBeLessThan(buffer.indexOf("answer-5"));
		expect(buffer.indexOf("answer-5")).toBeLessThan(buffer.indexOf("editor"));
	});

	it("treats height resize as routine and width resize as a deliberate rebuild", async () => {
		const terminal = new HeadlessTerminal(30, 5);
		const tui = new TUI(terminal);
		const history = new MutableComponent(["RESIZE-HISTORY"]);
		const live = new MutableComponent(["one", "two", "three", "editor"]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);

		let mark = terminal.markWrites();
		terminal.resize(30, 7);
		await render(tui, terminal);
		expect(terminal.writesSince(mark)).not.toContain("\x1b[3J");
		expect(terminal.writesSince(mark)).not.toContain("RESIZE-HISTORY");

		mark = terminal.markWrites();
		terminal.resize(30, 4);
		await render(tui, terminal);
		expect(terminal.writesSince(mark)).not.toContain("\x1b[3J");
		expect(terminal.writesSince(mark)).not.toContain("RESIZE-HISTORY");
		expect(terminal.visibleLines().join("\n")).toContain("editor");

		mark = terminal.markWrites();
		terminal.resize(24, 4);
		await render(tui, terminal);
		expect(terminal.writesSince(mark)).toContain("\x1b[3J");
		expect(terminal.writesSince(mark)).toContain("RESIZE-HISTORY");
	});

	it("shows and dismisses overlays without replaying history", async () => {
		const terminal = new HeadlessTerminal(30, 6);
		const tui = new TUI(terminal);
		const history = new MutableComponent(["OVERLAY-HISTORY"]);
		const live = new MutableComponent(["base", "editor"]);
		const overlay = new MutableComponent(["overlay"]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);
		let mark = terminal.markWrites();

		const handle = tui.showOverlay(overlay, { anchor: "top-left", width: 10 });
		await render(tui, terminal);
		expect(terminal.visibleLines().join("\n")).toContain("overlay");
		expect(terminal.writesSince(mark)).not.toContain("OVERLAY-HISTORY");

		mark = terminal.markWrites();
		handle.hide();
		await render(tui, terminal);
		expect(terminal.visibleLines().join("\n")).not.toContain("overlay");
		expect(terminal.writesSince(mark)).not.toContain("OVERLAY-HISTORY");
	});

	it("rebuilds retained history through the deliberate rebuild API", async () => {
		const terminal = new HeadlessTerminal(30, 6);
		const tui = new TUI(terminal);
		const history = new MutableComponent(["OLD-HISTORY"]);
		const live = new MutableComponent(["editor"]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);

		history.lines = ["NEW-HISTORY"];
		const mark = terminal.markWrites();
		tui.rebuild();
		await render(tui, terminal);

		const output = terminal.writesSince(mark);
		expect(output).toContain("\x1b[3J");
		expect(output).toContain("NEW-HISTORY");
		expect(terminal.bufferLines().join("\n")).not.toContain("OLD-HISTORY");
	});

	it("keeps growing and shrinking live controls inside the bounded canvas", async () => {
		const terminal = new HeadlessTerminal(30, 6);
		const tui = new TUI(terminal);
		const history = new MutableComponent(["CONTROL-HISTORY"]);
		const live = new MutableComponent(["editor"]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);

		for (const lines of [
			["status", "widget-1", "widget-2", "suggestion-1", "suggestion-2", "editor"],
			["suggestion-1", "editor"],
			["editor"],
		]) {
			const mark = terminal.markWrites();
			live.lines = lines;
			tui.requestRender();
			await render(tui, terminal);
			const output = terminal.writesSince(mark);
			expect(output).not.toContain("CONTROL-HISTORY");
			expect(output).not.toContain("\x1b[2J");
			expect(output).not.toContain("\x1b[3J");
			expect(terminal.visibleLines().join("\n")).toContain("editor");
		}
	});

	it("dismisses real editor autocomplete without replaying history or leaving ghost rows", async () => {
		const terminal = new HeadlessTerminal(40, 8);
		const tui = new TUI(terminal);
		const history = new MutableComponent(["AUTOCOMPLETE-HISTORY"]);
		const editorTheme: EditorTheme = {
			borderColor: (text) => text,
			selectList: {
				selectedPrefix: (text) => text,
				selectedText: (text) => text,
				description: (text) => text,
				scrollInfo: (text) => text,
				noMatch: (text) => text,
			},
		};
		const editor = new Editor(tui, editorTheme);
		editor.setAutocompleteProvider({
			getSuggestions: async () => ({
				prefix: "/",
				items: [{ value: "/command", label: "/command", description: "suggestion-marker" }],
			}),
			applyCompletion: (lines, cursorLine, cursorCol) => ({ lines, cursorLine, cursorCol }),
		});
		tui.addChild(history);
		tui.addChild(editor);
		tui.setLiveRegionStart(editor);
		tui.setFocus(editor);
		tui.start();
		await render(tui, terminal);

		terminal.sendInput("/");
		await render(tui, terminal);
		expect(terminal.visibleLines().join("\n")).toContain("→ /command");

		const mark = terminal.markWrites();
		terminal.sendInput("\x1b");
		await render(tui, terminal);
		const output = terminal.writesSince(mark);
		expect(output).not.toContain("AUTOCOMPLETE-HISTORY");
		expect(output).not.toContain("\x1b[2J");
		expect(output).not.toContain("\x1b[3J");
		expect(terminal.visibleLines().join("\n")).not.toContain("→ /command");
	});

	it("positions the hardware cursor from a marker in a clipped live tail", async () => {
		const terminal = new HeadlessTerminal(30, 5);
		const tui = new TUI(terminal, true);
		const history = new MutableComponent(["history"]);
		const live = new MutableComponent([
			"clipped-1",
			"clipped-2",
			"clipped-3",
			"clipped-4",
			`prompt>${CURSOR_MARKER}`,
		]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);

		expect(terminal.visibleLines().join("\n")).not.toContain("clipped-1");
		expect(terminal.writes.join("")).not.toContain(CURSOR_MARKER);
		expect(terminal.cursorPosition()).toEqual({ row: 4, col: 7 });
	});

	it("retains Kitty ownership until a forced rebuild deletes existing images", async () => {
		const terminal = new HeadlessTerminal(30, 6);
		const tui = new TUI(terminal);
		const history = new MutableComponent(["\x1b_Gi=41,a=T;history\x1b\\"]);
		const live = new MutableComponent(["\x1b_Gi=42,a=T;live\x1b\\"]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);
		const mark = terminal.markWrites();

		tui.rebuild();
		await render(tui, terminal);

		const output = terminal.writesSince(mark);
		expect(output).toContain("\x1b_Ga=d,d=I,i=41,q=2\x1b\\");
		expect(output).toContain("\x1b_Ga=d,d=I,i=42,q=2\x1b\\");
	});

	it.each([
		["removeChild()", (tui: TUI, live: Component) => tui.removeChild(live), true],
		["clear()", (tui: TUI) => tui.clear(), false],
		[
			"direct children mutation",
			(tui: TUI, live: Component) => tui.children.splice(tui.children.indexOf(live), 1),
			true,
		],
	])(
		"safely resets committed rendering after %s detaches the live boundary",
		async (_name, detach, retainsHistory) => {
			const terminal = new HeadlessTerminal(30, 6);
			const tui = new TUI(terminal);
			const history = new MutableComponent(["HISTORY", "\x1b_Gi=61,a=T;history\x1b\\"]);
			const live = new MutableComponent(["LIVE", "\x1b_Gi=62,a=T;live\x1b\\"]);
			tui.addChild(history);
			tui.addChild(live);
			tui.setLiveRegionStart(live);
			tui.start();
			await render(tui, terminal);
			const mark = terminal.markWrites();

			detach(tui, live);
			tui.requestRender();
			await expect(render(tui, terminal)).resolves.toBeUndefined();

			const output = terminal.writesSince(mark);
			expect(output).toContain("\x1b[2J");
			expect(output).toContain("\x1b[3J");
			expect(output).toContain("\x1b_Ga=d,d=I,i=61,q=2\x1b\\");
			expect(output).toContain("\x1b_Ga=d,d=I,i=62,q=2\x1b\\");
			expect(terminal.bufferLines().join("\n").includes("HISTORY")).toBe(retainsHistory);
		},
	);

	it("deletes only live Kitty placements during routine repaint with complete APC sequences", async () => {
		const terminal = new HeadlessTerminal(30, 6);
		const tui = new TUI(terminal);
		const history = new MutableComponent(["\x1b_Gi=51,a=T;history\x1b\\"]);
		const live = new MutableComponent(["\x1b_Gi=52,a=T;live\x1b\\"]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);
		const mark = terminal.markWrites();

		live.lines = ["\x1b_Gi=53,a=T;replacement\x1b\\"];
		tui.requestRender();
		await render(tui, terminal);

		const output = terminal.writesSince(mark);
		expect(output).toContain("\x1b_Ga=d,d=I,i=52,q=2\x1b\\");
		expect(output).not.toContain("i=51,q=2");
		expect((output.match(/\x1b_G/g) ?? []).length).toBe((output.match(/\x1b\\/g) ?? []).length);
	});

	it("clears all mutable rows when live output becomes empty", async () => {
		const terminal = new HeadlessTerminal(30, 5);
		const tui = new TUI(terminal);
		const history = new MutableComponent(["history"]);
		const live = new MutableComponent(["stale-one", "stale-two"]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(tui, terminal);

		const mark = terminal.markWrites();
		live.lines = [];
		tui.requestRender();
		await render(tui, terminal);

		const output = terminal.writesSince(mark);
		expect(output).not.toContain("\x1b[2J");
		expect(output).not.toContain("\x1b[3J");
		expect(output).not.toContain("history");
		expect(terminal.visibleLines().join("\n")).not.toContain("stale-");

		live.lines = ["new-live"];
		tui.requestRender();
		await render(tui, terminal);
		expect(terminal.visibleLines().join("\n")).toContain("new-live");
	});
});
