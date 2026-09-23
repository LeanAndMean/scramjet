import { stripVTControlCharacters } from "node:util";
import { afterEach, describe, expect, it, vi } from "vitest";
import { StdinBuffer } from "../src/stdin-buffer.js";
import { ProcessTerminal } from "../src/terminal.js";
import { type Component, TUI } from "../src/tui.js";
import { sliceByColumn, visibleWidth } from "../src/utils.js";
import { RetainedViewport } from "../src/viewport.js";
import { HeadlessTerminal } from "./helpers/headless-terminal.js";

const focusOut = "\x1b[O";
const focusIn = "\x1b[I";
const mouse = (button: number, x: number, y: number, action = "M") => `\x1b[<${button};${x};${y}${action}`;

class Rows implements Component {
	readonly handleInput = vi.fn((_data: string) => {});
	constructor(public lines: string[]) {}
	render(_width: number): string[] {
		return [...this.lines];
	}
	invalidate(): void {}
}

const running: { tui: TUI; terminal: HeadlessTerminal }[] = [];
const viewports: RetainedViewport[] = [];
const processTerminals: ProcessTerminal[] = [];

afterEach(async () => {
	for (const { tui } of running) tui.stop();
	for (const { terminal } of running.splice(0)) await terminal.flush();
	for (const viewport of viewports.splice(0)) viewport.cancelInteraction();
	for (const terminal of processTerminals.splice(0)) terminal.stop();
	vi.restoreAllMocks();
	vi.unstubAllEnvs();
	vi.useRealTimers();
});

async function mount(lines: string[], width = 31, height = 5) {
	const card = new Rows(lines);
	const terminal = new HeadlessTerminal(width, height);
	const tui = new TUI(terminal);
	const copy = vi.fn(async (_text: string) => {});
	const downstream = vi.fn((_data: string) => undefined);
	running.push({ tui, terminal });
	tui.configureViewport({ getBlocks: () => [{ component: card }], copy });
	tui.addInputListener(downstream);
	tui.setFocus(card);
	tui.start();
	const frame = () => tui.renderNow({ requireFlush: true });
	const text = () => terminal.visibleLines().map((line) => sliceByColumn(line, 0, width - 1, true).trimEnd());
	await frame();
	return { card, terminal, tui, copy, downstream, frame, text };
}

function gestureFixture(count = 100) {
	vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
	const card = new Rows(Array.from({ length: count }, (_, i) => `row-${i}`));
	const render = vi.fn();
	const viewport = new RetainedViewport({ getBlocks: () => [{ component: card }] }, render);
	viewports.push(viewport);
	const paint = () => {
		const logical = viewport.update(30, 5);
		viewport.slice(logical, 30, false);
		viewport.markPainted();
	};
	const input = (data: string) => viewport.handleInput(data, false, false);
	paint();
	viewport.scrollTo(10);
	paint();
	input(mouse(0, 1, 2));
	input(mouse(32, 6, 5));
	return { viewport, render, paint, input };
}

describe("viewport focus contracts", () => {
	it.each(["selection", "thumb"] as const)(
		"focus loss cancels unfinished %s without reattaching or dispatching keys",
		async (gesture) => {
			vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
			const f = await mount(Array.from({ length: 100 }, (_, i) => `row-${i}`));
			f.tui.scrollViewportTo(10);
			await f.frame();
			if (gesture === "selection") {
				f.terminal.sendInput(mouse(0, 1, 2));
				f.terminal.sendInput(mouse(32, 6, 5));
			} else {
				f.terminal.sendInput(mouse(0, 31, 2));
				f.terminal.sendInput(mouse(32, 31, 3));
			}
			await vi.advanceTimersByTimeAsync(160);
			await f.frame();
			const offset = f.tui.getViewportState()!.offset;
			expect(offset).toBeGreaterThan(10);
			expect(f.tui.getViewportState()!.followingTail).toBe(false);
			f.terminal.sendInput(focusOut);
			f.terminal.sendInput(mouse(32, gesture === "selection" ? 6 : 31, 1));
			f.terminal.sendInput(focusIn);
			f.terminal.sendInput(mouse(32, gesture === "selection" ? 6 : 31, 1));
			await vi.advanceTimersByTimeAsync(240);
			await f.frame();
			expect({
				offset: f.tui.getViewportState()!.offset,
				followingTail: f.tui.getViewportState()!.followingTail,
				focusedInput: f.card.handleInput.mock.calls.flat(),
				downstreamInput: f.downstream.mock.calls.flat(),
				copies: f.copy.mock.calls,
				intervals: vi.getTimerCount(),
				selectionHeld: Array.from({ length: 5 }, (_, row) => f.terminal.cell(row, 0).inverse).some(Boolean),
			}).toEqual({
				offset,
				followingTail: false,
				focusedInput: [],
				downstreamInput: [],
				copies: [],
				intervals: 0,
				selectionHeld: false,
			});
		},
	);

	it("preserves completed selection across focus changes until explicit copy", async () => {
		const f = await mount(Array.from({ length: 100 }, (_, i) => `row-${i}`));
		f.tui.scrollViewportTo(10);
		await f.frame();
		f.terminal.sendInput(mouse(0, 1, 2));
		f.terminal.sendInput(mouse(32, 7, 2));
		f.terminal.sendInput(mouse(0, 7, 2, "m"));
		await f.frame();
		expect(f.terminal.cell(1, 0).inverse).toBe(true);
		f.card.lines[11] = "changed-after-selection";
		f.terminal.sendInput(focusOut);
		f.terminal.sendInput(focusIn);
		await f.frame();
		expect(f.copy).not.toHaveBeenCalled();
		f.terminal.sendInput(mouse(2, 3, 2));
		await f.frame();
		expect(f.copy).toHaveBeenCalledExactlyOnceWith("row-11");
	});

	it.each([
		{ name: "inside", x: 6, y: 5 },
		{ name: "outside", x: 32, y: 6 },
	])("release $name bounds stops edge scrolling", ({ x, y }) => {
		const f = gestureFixture();
		vi.advanceTimersByTime(240);
		expect(f.viewport.state.offset).toBe(13);
		f.paint();
		f.input(mouse(0, x, y, "m"));
		f.render.mockClear();
		vi.advanceTimersByTime(240);
		expect({
			offset: f.viewport.state.offset,
			renders: f.render.mock.calls.length,
			intervals: vi.getTimerCount(),
		}).toEqual({ offset: 13, renders: 0, intervals: 0 });
	});

	it("stops redundant boundary renders but allows the held drag to move away", () => {
		const f = gestureFixture(20);
		vi.advanceTimersByTime(640);
		f.paint();
		expect(f.viewport.state.offset).toBe(15);
		f.render.mockClear();
		vi.advanceTimersByTime(320);
		expect.soft(f.render).not.toHaveBeenCalled();
		f.input(mouse(32, 6, 1));
		vi.advanceTimersByTime(160);
		expect(f.viewport.state.offset).toBe(13);
	});
});

describe("held-button wheel selection", () => {
	it.each([
		{ direction: "down", buttons: Array(8).fill(65), endRow: 66 },
		{ direction: "up", buttons: Array(8).fill(64), endRow: 18 },
		{ direction: "down then past the origin", buttons: [...Array(8).fill(65), ...Array(10).fill(64)], endRow: 36 },
		{ direction: "up then past the origin", buttons: [...Array(8).fill(64), ...Array(10).fill(65)], endRow: 48 },
	])("extends $direction with a stationary pointer, then freezes on release", async ({ buttons, endRow }) => {
		for (const paintEachEvent of [false, true]) {
			const lines = Array.from({ length: 100 }, (_, i) => `row-${i}`);
			const f = await mount(lines, 31, 7);
			f.tui.scrollViewportTo(40);
			await f.frame();
			f.terminal.sendInput(mouse(0, 4, 3));
			f.terminal.sendInput(mouse(32, 5, 3));
			for (const button of buttons) {
				f.terminal.sendInput(mouse(button, 5, 3));
				if (paintEachEvent) await f.frame();
			}
			await f.frame();
			expect(f.tui.getViewportState()?.offset).toBe(endRow - 2);
			expect(f.terminal.cell(2, 3).inverse).toBe(endRow > 42);
			expect(f.terminal.cell(2, 4).inverse).toBe(endRow < 42);
			f.terminal.sendInput(mouse(0, 5, 3, "m"));
			await f.frame();
			const released = f.terminal.visibleLines();
			for (const button of [...Array(4).fill(65), ...Array(4).fill(64)]) {
				f.terminal.sendInput(mouse(button, 5, 3));
				await f.frame();
			}
			expect(f.terminal.visibleLines()).toEqual(released);
			expect(f.terminal.cell(2, 3).inverse).toBe(endRow > 42);
			expect(f.terminal.cell(2, 4).inverse).toBe(endRow < 42);
			expect(f.copy).not.toHaveBeenCalled();
			f.terminal.sendInput(mouse(2, 4, 3));
			await f.frame();
			const first = Math.min(42, endRow);
			const last = Math.max(42, endRow);
			expect(f.copy).toHaveBeenCalledExactlyOnceWith(
				[
					lines[first].slice(endRow > 42 ? 3 : 4),
					...lines.slice(first + 1, last),
					lines[last].slice(0, endRow > 42 ? 4 : 3),
				].join("\n"),
			);
			expect(f.downstream).not.toHaveBeenCalled();
			expect(f.card.handleInput).not.toHaveBeenCalled();
		}
	});

	it.each([
		{ direction: "down", buttons: Array(8).fill(65), endRow: 67 },
		{ direction: "up", buttons: Array(8).fill(64), endRow: 19 },
		{ direction: "reversing past the origin", buttons: [...Array(8).fill(65), ...Array(10).fill(64)], endRow: 37 },
	])("tracks mixed motion and wheel input $direction through immediate release", async ({ buttons, endRow }) => {
		for (const paintEachEvent of [false, true]) {
			for (const wheelFirst of [false, true]) {
				const lines = Array.from({ length: 100 }, (_, i) => `row-${i} abcdefghijklmnop`);
				const f = await mount(lines, 31, 7);
				f.tui.scrollViewportTo(40);
				await f.frame();
				f.terminal.sendInput(mouse(0, 2, 3));
				f.terminal.sendInput(mouse(32, 4, 3));
				for (const [index, button] of buttons.entries()) {
					const x = 5 + (index % 4);
					const y = 2 + (index % 3);
					for (const event of wheelFirst ? [button, 32] : [32, button]) {
						f.terminal.sendInput(mouse(event, x, y));
						if (paintEachEvent) await f.frame();
					}
				}
				f.terminal.sendInput(mouse(32, 9, 4));
				f.terminal.sendInput(mouse(0, 9, 4, "m"));
				await f.frame();
				expect.soft(f.tui.getViewportState()?.offset).toBe(endRow - 3);
				expect.soft(f.terminal.cell(3, 7).inverse).toBe(endRow > 42);
				expect.soft(f.terminal.cell(3, 8).inverse).toBe(endRow < 42);
				f.terminal.sendInput(mouse(32, 15, 2));
				f.terminal.sendInput(mouse(65, 15, 2));
				f.terminal.sendInput(mouse(2, 15, 2));
				await f.frame();
				const first = Math.min(42, endRow);
				const last = Math.max(42, endRow);
				expect
					.soft(f.copy)
					.toHaveBeenCalledExactlyOnceWith(
						[
							lines[first].slice(endRow > 42 ? 1 : 8),
							...lines.slice(first + 1, last),
							lines[last].slice(0, endRow > 42 ? 8 : 1),
						].join("\n"),
					);
			}
		}
	});

	it("applies a final column adjustment to the scrolled row's displayed graphemes", async () => {
		const lines = Array.from({ length: 100 }, (_, i) => `row-${i}`);
		lines[45] = "\x1b[31mA界e\u0301Z\x1b[0m";
		const f = await mount(lines, 31, 7);
		f.tui.scrollViewportTo(40);
		await f.frame();
		for (const data of [mouse(0, 1, 3), mouse(32, 2, 3), mouse(65, 2, 3), mouse(32, 5, 3), mouse(0, 5, 3, "m")])
			f.terminal.sendInput(data);
		await f.frame();
		expect(f.terminal.cell(2, 3).inverse).toBe(true);
		expect(f.terminal.cell(2, 4).inverse).toBe(false);
		f.terminal.sendInput(mouse(2, 5, 3));
		await f.frame();
		expect(f.copy).toHaveBeenCalledExactlyOnceWith("row-42\nrow-43\nrow-44\nA界e\u0301");
	});

	it("starts dragging after wheel input against the resulting view", async () => {
		const lines = Array.from({ length: 100 }, (_, i) => `row-${i} abcdefghijklmnop`);
		const f = await mount(lines, 31, 7);
		f.tui.scrollViewportTo(40);
		await f.frame();
		f.terminal.sendInput(mouse(0, 2, 3));
		f.terminal.sendInput(mouse(65, 2, 3));
		f.terminal.sendInput(mouse(32, 9, 4));
		f.terminal.sendInput(mouse(0, 9, 4, "m"));
		f.terminal.sendInput(mouse(2, 9, 4));
		await f.frame();
		expect(f.copy).toHaveBeenCalledExactlyOnceWith(
			[lines[42].slice(1), ...lines.slice(43, 46), lines[46].slice(0, 8)].join("\n"),
		);
	});

	it("keeps mixed input coherent while entering and leaving edge autoscroll", async () => {
		vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
		const lines = Array.from({ length: 100 }, (_, i) => `row-${i} abcdefghijklmnop`);
		const f = await mount(lines, 31, 7);
		f.tui.scrollViewportTo(40);
		await f.frame();
		f.terminal.sendInput(mouse(0, 2, 2));
		f.terminal.sendInput(mouse(32, 6, 7));
		f.terminal.sendInput(mouse(65, 6, 7));
		f.terminal.sendInput(mouse(32, 10, 7));
		await f.frame();
		expect.soft(f.terminal.cell(6, 8).inverse).toBe(true);
		expect.soft(f.terminal.cell(6, 9).inverse).toBe(false);
		vi.advanceTimersByTime(80);
		f.terminal.sendInput(mouse(32, 12, 3));
		f.terminal.sendInput(mouse(0, 12, 3, "m"));
		vi.advanceTimersByTime(800);
		await f.frame();
		expect(f.tui.getViewportState()?.offset).toBe(44);
		f.terminal.sendInput(mouse(2, 12, 3));
		await f.frame();
		expect
			.soft(f.copy)
			.toHaveBeenCalledExactlyOnceWith(
				[lines[41].slice(1), ...lines.slice(42, 46), lines[46].slice(0, 11)].join("\n"),
			);
	});

	it.each([
		{ y: 3, offset: 43, endRow: 45 },
		{ y: 7, offset: 45, endRow: 51 },
	])("uses wheel pointer row $y when an edge tick precedes the next motion", async ({ y, offset, endRow }) => {
		vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
		const lines = Array.from({ length: 100 }, (_, i) => `row-${i} abcdefghijklmnop`);
		const f = await mount(lines, 31, 7);
		f.tui.scrollViewportTo(40);
		await f.frame();
		f.terminal.sendInput(mouse(0, 2, 2));
		f.terminal.sendInput(mouse(32, 6, 7));
		f.terminal.sendInput(mouse(65, 12, y));
		vi.advanceTimersByTime(160);
		await f.frame();
		expect.soft(f.tui.getViewportState()?.offset).toBe(offset);
		f.terminal.sendInput(mouse(0, 12, y, "m"));
		f.terminal.sendInput(mouse(2, 12, y));
		await f.frame();
		expect
			.soft(f.copy)
			.toHaveBeenCalledExactlyOnceWith(
				[lines[41].slice(1), ...lines.slice(42, endRow), lines[endRow].slice(0, 11)].join("\n"),
			);
	});

	it("does not turn a click without a drag into a wheel selection", async () => {
		const f = await mount(
			Array.from({ length: 100 }, (_, i) => `row-${i}`),
			31,
			7,
		);
		f.tui.scrollViewportTo(40);
		await f.frame();
		f.terminal.sendInput(mouse(0, 4, 3));
		for (let i = 0; i < 8; i++) f.terminal.sendInput(mouse(65, 4, 3));
		f.terminal.sendInput(mouse(0, 4, 3, "m"));
		await f.frame();
		f.terminal.sendInput(mouse(2, 4, 3));
		await f.frame();
		expect(f.copy).not.toHaveBeenCalled();
	});

	it("keeps an already completed selection unchanged while wheeling", async () => {
		const f = await mount(
			Array.from({ length: 100 }, (_, i) => `row-${i}`),
			31,
			7,
		);
		f.tui.scrollViewportTo(40);
		await f.frame();
		for (const data of [mouse(0, 1, 3), mouse(32, 7, 3), mouse(0, 7, 3, "m")]) f.terminal.sendInput(data);
		for (let i = 0; i < 8; i++) f.terminal.sendInput(mouse(65, 7, 3));
		await f.frame();
		expect(f.tui.getViewportState()?.offset).toBe(64);
		f.terminal.sendInput(mouse(2, 7, 3));
		await f.frame();
		expect(f.copy).toHaveBeenCalledExactlyOnceWith("row-42");
	});

	it("clamps a held selection at both document boundaries", async () => {
		const f = await mount(
			Array.from({ length: 20 }, (_, i) => `row-${i}`),
			31,
			7,
		);
		f.tui.scrollViewportTo(10);
		await f.frame();
		f.terminal.sendInput(mouse(0, 4, 3));
		f.terminal.sendInput(mouse(32, 5, 3));
		for (let i = 0; i < 10; i++) f.terminal.sendInput(mouse(64, 5, 3));
		await f.frame();
		expect(f.tui.getViewportState()?.offset).toBe(0);
		expect(f.terminal.cell(2, 4).inverse).toBe(true);
		for (let i = 0; i < 20; i++) f.terminal.sendInput(mouse(65, 5, 3));
		await f.frame();
		expect(f.tui.getViewportState()?.offset).toBe(13);
		f.terminal.sendInput(mouse(0, 4, 3, "m"));
		f.terminal.sendInput(mouse(2, 4, 3));
		await f.frame();
		expect(f.copy).toHaveBeenCalledExactlyOnceWith("-12\nrow-13\nrow-14\nrow-");
	});
});

describe("focus transport contracts", () => {
	it.each([
		{ name: "enables", sequence: "\x1b[?1004h" },
		{ name: "disables", sequence: "\x1b[?1004l" },
	])("$name focus reporting once across idempotent lifecycle", ({ sequence }) => {
		vi.useFakeTimers();
		vi.stubEnv("PI_TUI_WRITE_LOG", "");
		const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
		vi.spyOn(process.stdin, "resume").mockReturnValue(process.stdin);
		vi.spyOn(process.stdin, "pause").mockReturnValue(process.stdin);
		vi.spyOn(process.stdin, "setEncoding").mockReturnValue(process.stdin);
		if (process.stdin.setRawMode) vi.spyOn(process.stdin, "setRawMode").mockReturnValue(process.stdin);
		vi.spyOn(process, "kill").mockReturnValue(true);
		const terminal = new ProcessTerminal();
		processTerminals.push(terminal);
		const input = vi.fn();
		const resize = vi.fn();
		terminal.start(input, resize);
		terminal.setViewportMode(true);
		terminal.start(input, resize);
		terminal.setViewportMode(true);
		terminal.stop();
		terminal.stop();
		const written = output.mock.calls.map(([value]) => String(value)).join("");
		expect(written.split(sequence).length - 1).toBe(1);
	});

	it.each([focusIn, focusOut])("frames every byte split of %j", (sequence) => {
		for (let split = 1; split < sequence.length; split++) {
			const buffer = new StdinBuffer();
			buffer.setMouseReporting(true);
			const data = vi.fn();
			buffer.on("data", data);
			try {
				buffer.process(sequence.slice(0, split));
				expect(data).not.toHaveBeenCalled();
				buffer.process(`${sequence.slice(split)}z`);
				expect(data.mock.calls.flat()).toEqual([sequence, "z"]);
			} finally {
				buffer.destroy();
			}
		}
	});

	it("keeps pasted focus and pointer bytes opaque", () => {
		const buffer = new StdinBuffer();
		buffer.setMouseReporting(true);
		const data = vi.fn();
		const paste = vi.fn();
		buffer.on("data", data);
		buffer.on("paste", paste);
		const payload = `${focusOut}${mouse(32, 6, 5)}${focusIn}`;
		try {
			buffer.process(`\x1b[200~${payload}\x1b[201~`);
			expect(data).not.toHaveBeenCalled();
			expect(paste).toHaveBeenCalledExactlyOnceWith(payload);
		} finally {
			buffer.destroy();
		}
	});
});

it("observes only visible terminal columns after shrinking while preserving printed spaces", async () => {
	const terminal = new HeadlessTerminal(12, 2);
	terminal.write("\x1b[?1049hABCDEFGHIJKL");
	await terminal.flush();
	terminal.resize(6, 2);
	terminal.write("\x1b[H\x1b[2Ksmall");
	await terminal.flush();
	expect(terminal.visibleLines()[0]).toBe("small");
	terminal.write("\rsmall ");
	await terminal.flush();
	expect(terminal.visibleLines()[0]).toBe("small ");
});

describe("overwidth containment contracts", () => {
	const wide = "\x1b[31mAB界e\u0301XYZ1234-INVISIBLE-SUFFIX\x1b[0m";
	it("contains visible overwidth without throwing and recovers", async () => {
		const f = await mount(["safe", "after"], 13, 6);
		f.card.lines = [wide, "after"];
		const mark = f.terminal.markWrites();
		await expect(f.frame()).resolves.toBeUndefined();
		const displayed = f.terminal.visibleLines();
		expect(displayed.some((line) => line.startsWith("AB界e\u0301"))).toBe(true);
		expect(displayed.some((line) => line.trim() === "after")).toBe(true);
		expect(displayed.every((line) => visibleWidth(line) <= 13)).toBe(true);
		expect(f.terminal.writesSince(mark)).not.toContain("INVISIBLE-SUFFIX");
		expect(f.text().some((line) => /clip|width|truncat/i.test(line))).toBe(true);
		expect(f.terminal.bufferLines()).toHaveLength(6);
		expect(f.card.lines[0]).toBe(wide);
		f.card.lines = ["fixed", "after"];
		await expect(f.frame()).resolves.toBeUndefined();
		expect(f.text()[0]).toBe("fixed");
		expect(f.text().some((line) => /clip|width|truncat/i.test(line))).toBe(false);
	});

	it("normalizes tabs before containing overwide rows", async () => {
		const f = await mount(["safe"], 13, 6);
		f.card.lines = ["ABC\tDEFGHIJKLMNOPQRSTUVWXYZ"];
		await expect(f.frame()).resolves.toBeUndefined();
		expect(f.text()[0]).toBe("ABC   DEFGHI");
		expect(f.text().some((line) => /clip|width|truncat/i.test(line))).toBe(true);
	});

	it("copies displayed columns rather than an overwide middle-row suffix", async () => {
		const card = new Rows(["START", wide, "END"]);
		const copy = vi.fn(async (_text: string) => {});
		const viewport = new RetainedViewport({ getBlocks: () => [{ component: card }], copy });
		viewports.push(viewport);
		const logical = viewport.update(12, 6);
		const projected = viewport.slice(logical, 12, false).lines;
		const displayedMiddle = stripVTControlCharacters(sliceByColumn(projected[1], 0, 12, true));
		expect(displayedMiddle).toContain("AB界e\u0301");
		viewport.markPainted();
		for (const data of [mouse(0, 1, 1), mouse(32, 4, 3), mouse(0, 4, 3, "m")])
			viewport.handleInput(data, false, false);
		expect(copy).not.toHaveBeenCalled();
		viewport.handleInput(mouse(2, 1, 1), false, false);
		await Promise.resolve();
		expect(copy).toHaveBeenCalledExactlyOnceWith(`START\n${displayedMiddle}\nEND`);
		expect(card.lines[1]).toBe(wide);
	});
});
