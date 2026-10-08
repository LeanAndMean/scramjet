import { EventEmitter } from "node:events";
import { afterEach, describe, expect, it, vi } from "vitest";

import { StdinBuffer } from "../src/stdin-buffer.js";
import { ProcessTerminal } from "../src/terminal.js";
import { TUI } from "../src/tui.js";

afterEach(() => {
	vi.restoreAllMocks();
	vi.unstubAllGlobals();
	vi.useRealTimers();
});

describe("mouse transport framing", () => {
	it.each([30, 1000])("recovers complete mouse continuations after a %sms prefix gap only in mouse mode", (gap) => {
		vi.useFakeTimers();
		const event = "\x1b[<64;10;5M";
		for (let split = 1; split < event.length; split++) {
			const buffer = new StdinBuffer();
			buffer.setMouseReporting(true);
			const events: string[] = [];
			buffer.on("data", (data) => events.push(data));
			buffer.process(event.slice(0, split));
			vi.advanceTimersByTime(gap);
			buffer.process(`${event.slice(split)}z`);
			expect(events).toEqual(split < 3 ? [event.slice(0, split), "z"] : ["z"]);
			buffer.destroy();
		}
	});

	it("replays speculative nonmouse text and incomplete candidates without reviving expired Escape", () => {
		vi.useFakeTimers();
		for (const prefix of ["\x1b", "\x1b["]) {
			for (const suffix of ["[draft]", "<draft>", "<64;10;", "<1234;5;6M", "hello"]) {
				const buffer = new StdinBuffer();
				buffer.setMouseReporting(true);
				const events: string[] = [];
				buffer.on("data", (data) => events.push(data));
				buffer.process(prefix);
				vi.advanceTimersByTime(1000);
				buffer.process(suffix);
				vi.advanceTimersByTime(11);
				expect(events).toEqual([prefix, ...suffix]);
				buffer.destroy();
			}
		}
	});

	it("keeps paste opaque, bounds speculation and resets at fresh escapes, mode changes and clear", () => {
		vi.useFakeTimers();
		const buffer = new StdinBuffer();
		buffer.setMouseReporting(true);
		const data = vi.fn();
		const paste = vi.fn();
		buffer.on("data", data);
		buffer.on("paste", paste);
		const expire = () => {
			buffer.process("\x1b[");
			vi.advanceTimersByTime(30);
			data.mockClear();
		};
		expire();
		buffer.process("<64;");
		buffer.process("\x1b[200~<64;10;5M\x1b[201~");
		expect(data.mock.calls.flat()).toEqual([..."<64;"]);
		expect(paste).toHaveBeenCalledExactlyOnceWith("<64;10;5M");
		expire();
		buffer.process("<64;");
		buffer.process("\x1b[A");
		expect(data.mock.calls.flat()).toEqual([..."<64;", "\x1b[A"]);
		expire();
		buffer.process("<64;");
		buffer.setMouseReporting(false);
		buffer.process("10;5M");
		expect(data.mock.calls.flat()).toEqual([..."<64;10;5M"]);
		buffer.setMouseReporting(true);
		expire();
		buffer.process("<64;");
		buffer.clear();
		buffer.process("text");
		expect(data.mock.calls.flat()).toEqual([..."text"]);
		buffer.destroy();
	});

	it("recovers a fragmented speculative suffix within the interval and replays it after the interval", () => {
		vi.useFakeTimers();
		for (const gap of [9, 11]) {
			const buffer = new StdinBuffer();
			buffer.setMouseReporting(true);
			const data = vi.fn();
			buffer.on("data", data);
			buffer.process("\x1b[");
			vi.advanceTimersByTime(30);
			buffer.process("<64;");
			vi.advanceTimersByTime(gap);
			buffer.process("10;5M");
			expect(data.mock.calls.flat()).toEqual(gap === 9 ? ["\x1b["] : ["\x1b[", ..."<64;10;5M"]);
			buffer.destroy();
		}
	});
	it("preserves a separately typed bracket after an expired Escape", () => {
		vi.useFakeTimers();
		const buffer = new StdinBuffer({ timeout: 10 });
		const events: string[] = [];
		buffer.on("data", (data) => events.push(data));
		try {
			buffer.process("\x1b");
			vi.advanceTimersByTime(1000);
			expect(events).toEqual(["\x1b"]);
			buffer.process("[");
			vi.advanceTimersByTime(20);
			for (const character of "draft]") buffer.process(character);
			expect(events).toEqual(["\x1b", "[", "d", "r", "a", "f", "t", "]"]);
		} finally {
			buffer.destroy();
		}
	});

	it("frames every byte split of an SGR event", () => {
		const event = "\x1b[<32;123;45M";
		for (let split = 1; split < event.length; split++) {
			const buffer = new StdinBuffer();
			const data = vi.fn();
			buffer.on("data", data);
			buffer.process(event.slice(0, split));
			expect(data).not.toHaveBeenCalled();
			buffer.process(event.slice(split));
			expect(data).toHaveBeenCalledExactlyOnceWith(event);
			buffer.destroy();
		}
	});

	it("quarantines recognized mouse tails after the 10ms timeout, resynchronizing at the terminator", () => {
		vi.useFakeTimers();
		const buffer = new StdinBuffer();
		const data = vi.fn();
		buffer.on("data", data);
		buffer.process("\x1b[<32;12");
		vi.advanceTimersByTime(11);
		buffer.process("3;45Mhello");
		expect(data.mock.calls.flat().join("")).toBe("hello");
		buffer.destroy();
	});

	it("does not turn late mouse fragments into text after any prefix timeout", () => {
		vi.useFakeTimers();
		const event = "\x1b[<32;123;45M";
		for (let split = 1; split < event.length; split++) {
			const buffer = new StdinBuffer();
			const data = vi.fn();
			buffer.on("data", data);
			buffer.process(event.slice(0, split));
			vi.advanceTimersByTime(11);
			buffer.process(event.slice(split));
			buffer.process("z");
			expect(
				data.mock.calls
					.flat()
					.filter((value: string) => !value.startsWith("\x1b"))
					.join(""),
			).toBe("z");
			buffer.destroy();
		}
	});

	it("bounds malformed mouse input and recovers for a subsequent key escape", () => {
		const buffer = new StdinBuffer();
		const data = vi.fn();
		buffer.on("data", data);
		buffer.process(`\x1b[<${"1".repeat(5000)}`);
		expect(buffer.getBuffer().length).toBeLessThan(64);
		buffer.process("\x1b[A");
		expect(data).toHaveBeenCalledExactlyOnceWith("\x1b[A");
		buffer.destroy();
	});
});

describe("ordinary Darwin input lifecycle", () => {
	function setup() {
		vi.useFakeTimers();
		const stdin = Object.assign(new EventEmitter(), {
			isTTY: true,
			isRaw: false,
			setEncoding: vi.fn(),
			setRawMode: vi.fn(),
			resume: vi.fn(),
			pause: vi.fn(),
		});
		const stdout = Object.assign(new EventEmitter(), { isTTY: true, write: vi.fn(() => true) });
		vi.stubGlobal("process", { ...process, platform: "darwin", stdin, stdout, kill: vi.fn() });
		const terminal = new ProcessTerminal();
		return { terminal, stdin, stdout };
	}

	it("starts retained Copy with one ordinary reader installed before stdin resumes", () => {
		const { terminal, stdin } = setup();
		const tui = new TUI(terminal);
		const input = vi.fn();
		tui.configureViewport({ getBlocks: () => [], copy: vi.fn(async () => {}) });
		tui.addInputListener(input);
		stdin.resume.mockImplementation(() => {
			expect(stdin.listenerCount("data")).toBe(1);
			stdin.emit("data", "x");
		});
		try {
			tui.start();
			tui.start();
			expect(input).toHaveBeenCalledExactlyOnceWith("x");
			expect(stdin.setEncoding).toHaveBeenCalledExactlyOnceWith("utf8");
			expect(stdin.resume).toHaveBeenCalledOnce();
		} finally {
			tui.stop();
		}
		expect(stdin.listenerCount("data")).toBe(0);
	});

	it("delivers fragmented keys, mouse and multiline Unicode paste once across clean restarts", () => {
		const { terminal, stdin, stdout } = setup();
		const input = vi.fn();
		const packets = ["\x1b[A", "\x1b[<32;12;4M", "\x1b[200~café 界 é\nsecond line\x1b[201~"];
		try {
			for (let round = 0; round < 2; round++) {
				terminal.start(input, vi.fn());
				terminal.setViewportMode(true);
				terminal.start(input, vi.fn());
				expect(stdin.listenerCount("data")).toBe(1);
				expect(stdout.listenerCount("resize")).toBe(1);
				input.mockClear();
				for (const packet of packets) for (const character of packet) stdin.emit("data", character);
				expect(input.mock.calls.flat()).toEqual(packets);
				stdin.emit("data", "\x1b[");
				terminal.stop();
				expect(stdin.listenerCount("data")).toBe(0);
				expect(stdout.listenerCount("resize")).toBe(0);
				stdin.emit("data", "ignored");
				vi.advanceTimersByTime(200);
				expect(input.mock.calls.flat()).toEqual(packets);
			}
		} finally {
			terminal.stop();
		}
		expect(stdin.setRawMode.mock.calls).toEqual([[true], [false], [true], [false]]);
		expect(stdin.pause).toHaveBeenCalledTimes(2);
	});

	it("resumes input after bounded drain without rearming reporting until restart", async () => {
		const { terminal, stdin, stdout } = setup();
		const input = vi.fn();
		try {
			terminal.start(input, vi.fn());
			terminal.setViewportMode(true);
			stdin.emit("data", "\x1b[?0u");
			expect(terminal.kittyProtocolActive).toBe(true);
			const draining = terminal.drainInput(100, 20);
			stdout.write.mockClear();
			for (let elapsed = 0; elapsed < 100; elapsed += 10) {
				stdin.emit("data", "x\x1b[?0u");
				await vi.advanceTimersByTimeAsync(10);
			}
			await draining;
			expect(input).not.toHaveBeenCalled();
			expect(stdin.listenerCount("data")).toBe(1);
			stdin.emit("data", "z\x1b[?0u");
			expect(input).toHaveBeenCalledExactlyOnceWith("z");
			await vi.advanceTimersByTimeAsync(200);
			expect(stdout.write).not.toHaveBeenCalled();
			expect(terminal.kittyProtocolActive).toBe(false);
			terminal.stop();
			terminal.start(input, vi.fn());
			terminal.setViewportMode(true);
			stdin.emit("data", "\x1b[?0u");
			expect(terminal.kittyProtocolActive).toBe(true);
			expect(stdout.write).toHaveBeenCalledWith("\x1b[>15u");
		} finally {
			terminal.stop();
		}
	});
});

describe("candidate terminal modes", () => {
	it("restores ordinary input after drain without accepting late keyboard replies", async () => {
		vi.useFakeTimers();
		const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
		vi.spyOn(process.stdin, "resume").mockReturnValue(process.stdin);
		vi.spyOn(process.stdin, "pause").mockReturnValue(process.stdin);
		vi.spyOn(process, "kill").mockReturnValue(true);
		const terminal = new ProcessTerminal();
		const input = vi.fn();
		terminal.start(input, vi.fn());
		const draining = terminal.drainInput();
		await vi.advanceTimersByTimeAsync(60);
		await draining;
		process.stdin.emit("data", "x\x1b[?0u");
		expect(input).toHaveBeenCalledExactlyOnceWith("x");
		expect(output.mock.calls.map(([value]) => value).join("")).not.toContain("\x1b[>7u");
		terminal.stop();
	});
	it("does not re-enable keyboard reporting from a query response received during a handoff drain", async () => {
		vi.useFakeTimers();
		const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
		vi.spyOn(process.stdin, "resume").mockReturnValue(process.stdin);
		vi.spyOn(process.stdin, "pause").mockReturnValue(process.stdin);
		vi.spyOn(process, "kill").mockReturnValue(true);
		const terminal = new ProcessTerminal();
		terminal.start(
			() => {},
			() => {},
		);
		terminal.setViewportMode(true);
		const draining = terminal.drainInput();
		process.stdin.emit("data", "\x1b[?0u");
		await vi.advanceTimersByTimeAsync(60);
		await draining;
		terminal.stop();
		expect(output.mock.calls.map(([value]) => value).join("")).not.toContain("\x1b[>15u");
	});
	it("requests explicit viewport key events and restores keyboard stacks in their owning buffers", () => {
		vi.useFakeTimers();
		const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
		vi.spyOn(process.stdin, "resume").mockReturnValue(process.stdin);
		vi.spyOn(process.stdin, "pause").mockReturnValue(process.stdin);
		vi.spyOn(process, "kill").mockReturnValue(true);
		const terminal = new ProcessTerminal();
		terminal.start(
			() => {},
			() => {},
		);
		process.stdin.emit("data", "\x1b[?0u");
		output.mockClear();
		terminal.setViewportMode(true);
		terminal.stop();
		const written = output.mock.calls.map(([value]) => value).join("");
		expect(written).toContain("\x1b[<u\x1b[?1049h");
		expect(written).toContain("\x1b[?1006h\x1b[>15u");
		expect(written).toContain("\x1b[<u\x1b[?1002l");
		expect(written).toContain("\x1b[?1049l\x1b[>7u");
		expect(written.endsWith("\x1b[<u")).toBe(true);
	});

	it("reenters the viewport and accepts pointer input during the resumed keyboard query", () => {
		vi.useFakeTimers();
		const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
		vi.spyOn(process.stdin, "resume").mockReturnValue(process.stdin);
		vi.spyOn(process.stdin, "pause").mockReturnValue(process.stdin);
		vi.spyOn(process, "kill").mockReturnValue(true);
		const terminal = new ProcessTerminal();
		const input = vi.fn();
		terminal.start(input, () => {});
		process.stdin.emit("data", "\x1b[?0u");
		expect(terminal.kittyProtocolActive).toBe(true);
		terminal.stop();
		terminal.start(input, () => {});
		terminal.setViewportMode(true);
		expect(terminal.kittyProtocolActive).toBe(false);
		process.stdin.emit("data", "\x1b[<64;2;2M\x1b[?0u");
		expect(input).toHaveBeenCalledExactlyOnceWith("\x1b[<64;2;2M");
		expect(terminal.kittyProtocolActive).toBe(true);
		const sequences = output.mock.calls.map(([value]) => value).join("");
		expect(sequences.lastIndexOf("\x1b[?1049h")).toBeLessThan(sequences.lastIndexOf("\x1b[>15u"));
		terminal.stop();
		const count = output.mock.calls.length;
		vi.advanceTimersByTime(200);
		expect(output).toHaveBeenCalledTimes(count);
	});
	it("owns alternate screen and mouse modes idempotently, cancels keyboard fallback on stop", () => {
		vi.useFakeTimers();
		const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
		vi.spyOn(process.stdin, "resume").mockReturnValue(process.stdin);
		vi.spyOn(process.stdin, "pause").mockReturnValue(process.stdin);
		vi.spyOn(process, "kill").mockReturnValue(true);
		const terminal = new ProcessTerminal();
		const input = vi.fn();
		terminal.start(input, () => {});
		terminal.setViewportMode(true);
		terminal.start(input, () => {});
		terminal.setViewportMode(true);
		expect(
			output.mock.calls
				.map(([value]) => value)
				.join("")
				.match(/\x1b\[\?1049h/g),
		).toHaveLength(1);
		terminal.stop();
		const count = output.mock.calls.length;
		terminal.stop();
		vi.advanceTimersByTime(200);
		expect(output).toHaveBeenCalledTimes(count);
		const written = output.mock.calls.map(([value]) => value).join("");
		expect(written).toContain("\x1b[?1002l\x1b[?1006l");
		expect(written).toContain("\x1b[?1049l");
		expect(written).not.toContain("\x1b[>4;2m");
		terminal.start(input, () => {});
		terminal.setViewportMode(true);
		vi.advanceTimersByTime(151);
		expect(output.mock.calls.map(([value]) => value).join("")).toContain("\x1b[>4;2m");
		terminal.stop();
	});
});
