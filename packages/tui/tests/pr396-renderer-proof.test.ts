import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Component } from "../src/tui.js";
import { TUI } from "../src/tui.js";
import { HeadlessTerminal } from "./helpers/headless-terminal.js";

class MutableComponent implements Component {
	constructor(private readonly lines: string[]) {}

	render(): string[] {
		return [...this.lines];
	}
}

async function render(terminal: HeadlessTerminal): Promise<void> {
	await vi.runAllTimersAsync();
	await terminal.flush();
}

describe("PR 396 renderer regression proof", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("bottom-anchors the tail-windowed live region after a height shrink", async () => {
		const terminal = new HeadlessTerminal(30, 5);
		const tui = new TUI(terminal);
		const history = new MutableComponent([
			"history-1",
			"history-2",
			"history-3",
			"history-4",
			"history-5",
			"history-6",
		]);
		const live = new MutableComponent(["one", "two", "three", "editor"]);
		tui.addChild(history);
		tui.addChild(live);
		tui.setLiveRegionStart(live);
		tui.start();
		await render(terminal);

		terminal.resize(30, 3);
		await render(terminal);

		expect(terminal.visibleLines()).toEqual(["", "three", "editor"]);
	});
});
