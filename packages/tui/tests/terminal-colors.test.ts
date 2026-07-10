import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { StdinBuffer } from "../src/stdin-buffer.js";
import type { Terminal } from "../src/terminal.js";
import { isOsc11Response, parseOsc11Response } from "../src/terminal-colors.js";
import { TUI } from "../src/tui.js";

// --- Pure parser tests ---

describe("parseOsc11Response", () => {
	describe("BEL terminator", () => {
		it("parses 4-digit rgb channels", () => {
			const result = parseOsc11Response("\x1b]11;rgb:ffff/ffff/ffff\x07");
			expect(result).toEqual({ r: 1, g: 1, b: 1 });
		});

		it("parses 2-digit rgb channels", () => {
			const result = parseOsc11Response("\x1b]11;rgb:ff/ff/ff\x07");
			expect(result).toEqual({ r: 1, g: 1, b: 1 });
		});

		it("parses 1-digit rgb channels", () => {
			const result = parseOsc11Response("\x1b]11;rgb:f/f/f\x07");
			expect(result).toEqual({ r: 1, g: 1, b: 1 });
		});

		it("parses 3-digit rgb channels", () => {
			const result = parseOsc11Response("\x1b]11;rgb:fff/000/800\x07");
			expect(result).toEqual({ r: 1, g: 0, b: expect.closeTo(0x800 / 0xfff, 5) });
		});

		it("parses mixed-width channels", () => {
			const result = parseOsc11Response("\x1b]11;rgb:ff/ffff/0\x07");
			expect(result).toEqual({ r: 1, g: 1, b: 0 });
		});

		it("parses black", () => {
			const result = parseOsc11Response("\x1b]11;rgb:0000/0000/0000\x07");
			expect(result).toEqual({ r: 0, g: 0, b: 0 });
		});

		it("normalizes to 0-1 range (4-digit)", () => {
			const result = parseOsc11Response("\x1b]11;rgb:8000/4000/c000\x07");
			expect(result).toEqual({
				r: expect.closeTo(0x8000 / 0xffff, 5),
				g: expect.closeTo(0x4000 / 0xffff, 5),
				b: expect.closeTo(0xc000 / 0xffff, 5),
			});
		});

		it("normalizes to 0-1 range (2-digit)", () => {
			const result = parseOsc11Response("\x1b]11;rgb:80/40/c0\x07");
			expect(result).toEqual({
				r: expect.closeTo(0x80 / 0xff, 5),
				g: expect.closeTo(0x40 / 0xff, 5),
				b: expect.closeTo(0xc0 / 0xff, 5),
			});
		});
	});

	describe("ST terminator (ESC \\)", () => {
		it("parses 4-digit rgb channels", () => {
			const result = parseOsc11Response("\x1b]11;rgb:ffff/ffff/ffff\x1b\\");
			expect(result).toEqual({ r: 1, g: 1, b: 1 });
		});

		it("parses 2-digit rgb channels", () => {
			const result = parseOsc11Response("\x1b]11;rgb:00/00/00\x1b\\");
			expect(result).toEqual({ r: 0, g: 0, b: 0 });
		});

		it("parses 1-digit rgb channels", () => {
			const result = parseOsc11Response("\x1b]11;rgb:0/0/0\x1b\\");
			expect(result).toEqual({ r: 0, g: 0, b: 0 });
		});
	});

	describe("hash forms", () => {
		it("parses #RRGGBB (6 digits)", () => {
			const result = parseOsc11Response("\x1b]11;#ffffff\x07");
			expect(result).toEqual({ r: 1, g: 1, b: 1 });
		});

		it("parses #RRGGBB black", () => {
			const result = parseOsc11Response("\x1b]11;#000000\x07");
			expect(result).toEqual({ r: 0, g: 0, b: 0 });
		});

		it("parses #RRGGBB mixed", () => {
			const result = parseOsc11Response("\x1b]11;#804020\x07");
			expect(result).toEqual({
				r: expect.closeTo(0x80 / 0xff, 5),
				g: expect.closeTo(0x40 / 0xff, 5),
				b: expect.closeTo(0x20 / 0xff, 5),
			});
		});

		it("parses #RGB (3 digits)", () => {
			const result = parseOsc11Response("\x1b]11;#fff\x07");
			expect(result).toEqual({ r: 1, g: 1, b: 1 });
		});

		it("parses #RGB black", () => {
			const result = parseOsc11Response("\x1b]11;#000\x07");
			expect(result).toEqual({ r: 0, g: 0, b: 0 });
		});

		it("parses #RRRGGGBBB (9 digits)", () => {
			const result = parseOsc11Response("\x1b]11;#fffffffff\x07");
			expect(result).toEqual({ r: 1, g: 1, b: 1 });
		});

		it("parses #RRRRGGGGBBBB (12 digits)", () => {
			const result = parseOsc11Response("\x1b]11;#ffffffffffff\x07");
			expect(result).toEqual({ r: 1, g: 1, b: 1 });
		});

		it("parses #RRRRGGGGBBBB with ST terminator", () => {
			const result = parseOsc11Response("\x1b]11;#000000000000\x1b\\");
			expect(result).toEqual({ r: 0, g: 0, b: 0 });
		});
	});

	describe("full-sequence anchoring", () => {
		it("rejects string not starting with ESC ]", () => {
			expect(parseOsc11Response("11;rgb:ffff/ffff/ffff\x07")).toBeUndefined();
		});

		it("rejects string without terminator", () => {
			expect(parseOsc11Response("\x1b]11;rgb:ffff/ffff/ffff")).toBeUndefined();
		});

		it("rejects trailing content after BEL terminator", () => {
			expect(parseOsc11Response("\x1b]11;rgb:ffff/ffff/ffff\x07extra")).toBeUndefined();
		});

		it("rejects trailing content after ST terminator", () => {
			expect(parseOsc11Response("\x1b]11;rgb:ffff/ffff/ffff\x1b\\extra")).toBeUndefined();
		});

		it("rejects leading content before ESC", () => {
			expect(parseOsc11Response("prefix\x1b]11;rgb:ffff/ffff/ffff\x07")).toBeUndefined();
		});
	});

	describe("wrong OSC IDs", () => {
		it("rejects OSC 10 (foreground)", () => {
			expect(parseOsc11Response("\x1b]10;rgb:ffff/ffff/ffff\x07")).toBeUndefined();
		});

		it("rejects OSC 12 (cursor)", () => {
			expect(parseOsc11Response("\x1b]12;rgb:ffff/ffff/ffff\x07")).toBeUndefined();
		});

		it("rejects OSC 111 (reset background)", () => {
			expect(parseOsc11Response("\x1b]111;rgb:ffff/ffff/ffff\x07")).toBeUndefined();
		});
	});

	describe("malformed payload", () => {
		it("rejects empty payload", () => {
			expect(parseOsc11Response("\x1b]11;\x07")).toBeUndefined();
		});

		it("rejects non-rgb non-hash payload", () => {
			expect(parseOsc11Response("\x1b]11;foo\x07")).toBeUndefined();
		});

		it("rejects rgb with missing channel", () => {
			expect(parseOsc11Response("\x1b]11;rgb:ffff/ffff\x07")).toBeUndefined();
		});

		it("rejects rgb with empty channel", () => {
			expect(parseOsc11Response("\x1b]11;rgb:/ffff/ffff\x07")).toBeUndefined();
		});

		it("rejects rgb with non-hex characters", () => {
			expect(parseOsc11Response("\x1b]11;rgb:gggg/ffff/ffff\x07")).toBeUndefined();
		});

		it("rejects rgb with more than 4 digits per channel", () => {
			expect(parseOsc11Response("\x1b]11;rgb:fffff/ffff/ffff\x07")).toBeUndefined();
		});

		it("rejects hash with invalid length (5 digits)", () => {
			expect(parseOsc11Response("\x1b]11;#fffff\x07")).toBeUndefined();
		});

		it("rejects hash with invalid length (7 digits)", () => {
			expect(parseOsc11Response("\x1b]11;#fffffff\x07")).toBeUndefined();
		});

		it("rejects hash with non-hex characters", () => {
			expect(parseOsc11Response("\x1b]11;#gggggg\x07")).toBeUndefined();
		});
	});
});

describe("isOsc11Response", () => {
	it("recognizes BEL-terminated response", () => {
		expect(isOsc11Response("\x1b]11;rgb:ffff/ffff/ffff\x07")).toBe(true);
	});

	it("recognizes ST-terminated response", () => {
		expect(isOsc11Response("\x1b]11;rgb:0000/0000/0000\x1b\\")).toBe(true);
	});

	it("rejects OSC 10", () => {
		expect(isOsc11Response("\x1b]10;rgb:ffff/ffff/ffff\x07")).toBe(false);
	});

	it("rejects non-OSC sequence", () => {
		expect(isOsc11Response("\x1b[?1u")).toBe(false);
	});

	it("rejects plain text", () => {
		expect(isOsc11Response("hello")).toBe(false);
	});
});

// --- StdinBuffer hold tests ---

describe("StdinBuffer OSC hold", () => {
	let buffer: StdinBuffer;
	let emitted: string[];

	beforeEach(() => {
		vi.useFakeTimers();
		buffer = new StdinBuffer({ timeout: 10 });
		emitted = [];
		buffer.on("data", (data) => emitted.push(data));
	});

	afterEach(() => {
		buffer.destroy();
		vi.useRealTimers();
	});

	it("without hold, incomplete OSC flushes after timeout", () => {
		buffer.process("\x1b]11;rgb:ff");
		expect(emitted).toEqual([]);
		vi.advanceTimersByTime(10);
		expect(emitted).toEqual(["\x1b]11;rgb:ff"]);
	});

	it("with hold active, incomplete OSC does NOT flush after timeout", () => {
		buffer.holdOscInput(true);
		buffer.process("\x1b]11;rgb:ff");
		expect(emitted).toEqual([]);
		vi.advanceTimersByTime(10);
		expect(emitted).toEqual([]);
		vi.advanceTimersByTime(100);
		expect(emitted).toEqual([]);
	});

	it("with hold active, complete OSC emits immediately", () => {
		buffer.holdOscInput(true);
		buffer.process("\x1b]11;rgb:ffff/ffff/ffff\x07");
		expect(emitted).toEqual(["\x1b]11;rgb:ffff/ffff/ffff\x07"]);
	});

	it("split response reassembles when hold is active", () => {
		buffer.holdOscInput(true);
		buffer.process("\x1b]11;rgb:ff");
		vi.advanceTimersByTime(15); // past normal timeout
		expect(emitted).toEqual([]);
		buffer.process("ff/ffff/ffff\x07");
		expect(emitted).toEqual(["\x1b]11;rgb:ffff/ffff/ffff\x07"]);
	});

	it("non-OSC input still flushes normally during hold", () => {
		buffer.holdOscInput(true);
		buffer.process("a");
		expect(emitted).toEqual(["a"]);
	});

	it("non-OSC incomplete escape still flushes at 10ms during hold", () => {
		buffer.holdOscInput(true);
		buffer.process("\x1b");
		expect(emitted).toEqual([]);
		vi.advanceTimersByTime(10);
		expect(emitted).toEqual(["\x1b"]);
	});

	it("releasing hold flushes held buffer", () => {
		buffer.holdOscInput(true);
		buffer.process("\x1b]11;rgb:ff");
		vi.advanceTimersByTime(50);
		expect(emitted).toEqual([]);
		buffer.holdOscInput(false);
		// Should schedule the normal flush timeout
		vi.advanceTimersByTime(10);
		expect(emitted).toEqual(["\x1b]11;rgb:ff"]);
	});

	it("keyboard input during hold is concatenated into the held buffer", () => {
		buffer.holdOscInput(true);
		buffer.process("\x1b]11;rgb:ff");
		vi.advanceTimersByTime(5);
		buffer.process("x");
		// Appended to the incomplete OSC sequence — nothing emits until hold is released
		expect(emitted).toEqual([]);
	});

	it("held OSC prefix completes and emits when terminator arrives", () => {
		buffer.holdOscInput(true);
		buffer.process("\x1b]11;rgb:");
		expect(emitted).toEqual([]);
		// Terminator completes the sequence — extractCompleteSequences emits it immediately
		buffer.process("ff/ff/ff\x1b\\");
		expect(emitted).toEqual(["\x1b]11;rgb:ff/ff/ff\x1b\\"]);
	});
});

// --- TUI query lifecycle tests ---

class FakeTerminal implements Terminal {
	writes: string[] = [];
	private inputHandler?: (data: string) => void;
	private _holdingOsc = false;

	start(onInput: (data: string) => void, _onResize: () => void): void {
		this.inputHandler = onInput;
	}

	stop(): void {
		this.inputHandler = undefined;
	}

	async drainInput(): Promise<void> {}

	write(data: string): void {
		this.writes.push(data);
	}

	get columns(): number {
		return 80;
	}
	get rows(): number {
		return 24;
	}
	get kittyProtocolActive(): boolean {
		return false;
	}

	moveBy(): void {}
	hideCursor(): void {}
	showCursor(): void {}
	clearLine(): void {}
	clearFromCursor(): void {}
	clearScreen(): void {}
	setTitle(): void {}
	setProgress(): void {}

	holdOscInput(hold: boolean): void {
		this._holdingOsc = hold;
	}

	get isHoldingOsc(): boolean {
		return this._holdingOsc;
	}

	// Test helper: inject input as if it arrived from the terminal
	injectInput(data: string): void {
		this.inputHandler?.(data);
	}
}

describe("TUI.queryTerminalBackgroundColor", () => {
	let terminal: FakeTerminal;
	let tui: TUI;

	beforeEach(() => {
		vi.useFakeTimers();
		terminal = new FakeTerminal();
		tui = new TUI(terminal);
		tui.start();
		terminal.writes = []; // clear startup writes (hideCursor, etc.)
	});

	afterEach(() => {
		tui.stop();
		vi.useRealTimers();
	});

	it("writes the OSC 11 query to the terminal", () => {
		tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
		expect(terminal.writes).toContain("\x1b]11;?\x1b\\");
	});

	it("activates OSC hold on the terminal", () => {
		tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
		expect(terminal.isHoldingOsc).toBe(true);
	});

	it("resolves with parsed RGB on valid response", async () => {
		const promise = tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
		terminal.injectInput("\x1b]11;rgb:ffff/ffff/ffff\x07");
		const result = await promise;
		expect(result).toEqual({ r: 1, g: 1, b: 1 });
	});

	it("resolves undefined on timeout", async () => {
		const promise = tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
		vi.advanceTimersByTime(100);
		const result = await promise;
		expect(result).toBeUndefined();
	});

	it("deactivates hold after response", async () => {
		const promise = tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
		terminal.injectInput("\x1b]11;rgb:0000/0000/0000\x07");
		await promise;
		expect(terminal.isHoldingOsc).toBe(false);
	});

	it("deactivates hold after timeout", async () => {
		const promise = tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
		vi.advanceTimersByTime(100);
		await promise;
		expect(terminal.isHoldingOsc).toBe(false);
	});

	it("resolves undefined on malformed response", async () => {
		const promise = tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
		terminal.injectInput("\x1b]11;bogus\x07");
		const result = await promise;
		expect(result).toBeUndefined();
	});

	it("single-flight: second call returns same promise", () => {
		const p1 = tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
		const p2 = tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
		expect(p1).toBe(p2);
	});

	it("consumes OSC 11 response before input listeners", async () => {
		const listener = vi.fn();
		tui.addInputListener(listener);
		const promise = tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
		terminal.injectInput("\x1b]11;rgb:8080/8080/8080\x07");
		await promise;
		expect(listener).not.toHaveBeenCalled();
	});

	it("forwards non-OSC input to listeners during query", () => {
		const received: string[] = [];
		tui.addInputListener((data) => {
			received.push(data);
			return undefined;
		});
		tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
		terminal.injectInput("x");
		expect(received).toContain("x");
	});

	describe("late response (discard credit)", () => {
		it("consumes one late response after timeout", async () => {
			const promise = tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
			vi.advanceTimersByTime(100);
			await promise;

			const listener = vi.fn();
			tui.addInputListener(listener);
			terminal.injectInput("\x1b]11;rgb:ffff/ffff/ffff\x07");
			expect(listener).not.toHaveBeenCalled();
		});

		it("does not consume a second late response", async () => {
			const promise = tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
			vi.advanceTimersByTime(100);
			await promise;

			// First late response consumed
			terminal.injectInput("\x1b]11;rgb:ffff/ffff/ffff\x07");

			const listener = vi.fn();
			tui.addInputListener(listener);
			// Second would not be consumed
			terminal.injectInput("\x1b]11;rgb:0000/0000/0000\x07");
			expect(listener).toHaveBeenCalledWith("\x1b]11;rgb:0000/0000/0000\x07");
		});
	});

	describe("stop behavior", () => {
		it("resolves pending query as undefined on stop", async () => {
			const promise = tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
			tui.stop();
			const result = await promise;
			expect(result).toBeUndefined();
		});

		it("preserves discard credit across stop/start", async () => {
			const promise = tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
			vi.advanceTimersByTime(100);
			await promise; // timed out, discard credit active

			tui.stop();
			tui.start();
			terminal.writes = [];

			const listener = vi.fn();
			tui.addInputListener(listener);
			terminal.injectInput("\x1b]11;rgb:ffff/ffff/ffff\x07");
			expect(listener).not.toHaveBeenCalled();
		});

		it("does not re-issue query after stop/start", () => {
			tui.queryTerminalBackgroundColor({ timeoutMs: 100 });
			terminal.writes = [];
			tui.stop();
			tui.start();
			expect(terminal.writes).not.toContain("\x1b]11;?\x1b\\");
		});
	});
});
