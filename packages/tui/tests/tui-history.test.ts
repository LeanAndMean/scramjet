import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Component } from "../src/tui.js";
import { Container, TUI } from "../src/tui.js";
import { HeadlessTerminal } from "./helpers/headless-terminal.js";

class MutableComponent implements Component {
	constructor(public lines: string[] = []) {}
	invalidate(): void {}
	render(): string[] {
		return [...this.lines];
	}
}

async function render(tui: TUI, terminal: HeadlessTerminal): Promise<void> {
	void tui;
	await vi.runAllTimersAsync();
	await terminal.flush();
}

describe("TUI committed history", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

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
		expect(terminal.visibleLines().join("\n")).toContain("editor");
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
		terminal.resize(24, 7);
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

		tui.requestRender(true);
		await render(tui, terminal);

		const output = terminal.writesSince(mark);
		expect(output).toContain("\x1b_Ga=d,d=I,i=41,q=2\x1b\\");
		expect(output).toContain("\x1b_Ga=d,d=I,i=42,q=2\x1b\\");
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
