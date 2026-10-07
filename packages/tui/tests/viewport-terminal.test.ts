import { EventEmitter } from "node:events";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MacosInput } from "../src/macos-input.js";

vi.mock("../src/macos-input.js", () => ({ MacosInput: vi.fn() }));

import { StdinBuffer } from "../src/stdin-buffer.js";
import { ProcessTerminal } from "../src/terminal.js";

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

describe("native input terminal contract", () => {
	function setup() {
		const order: string[] = [];
		const native = {
			prepare: vi.fn(() => {
				order.push("prepare");
				return true;
			}),
			commit: vi.fn(() => order.push("commit")),
			stop: vi.fn(() => order.push("stop")),
			setMouseReporting: vi.fn(() => order.push("mouse")),
			holdOscInput: vi.fn(() => order.push("osc")),
			setLease: vi.fn(),
			isCurrent: vi.fn(),
			drain: vi.fn(),
		};
		vi.mocked(MacosInput).mockImplementation(() => native as unknown as MacosInput);
		const stdin = Object.assign(new EventEmitter(), {
			isTTY: true,
			isRaw: false,
			readableLength: 0,
			readableFlowing: false,
			setEncoding: vi.fn(),
			setRawMode: vi.fn(),
			resume: vi.fn(),
			pause: vi.fn(),
		});
		const stdout = Object.assign(new EventEmitter(), {
			isTTY: true,
			write: vi.fn(() => {
				order.push("write");
				return true;
			}),
		});
		vi.stubGlobal("process", { ...process, platform: "darwin", stdin, stdout, kill: vi.fn() });
		const terminal = new ProcessTerminal();
		const options = { onCopyIntent: vi.fn(), onAvailability: vi.fn(), onError: vi.fn() };
		return { terminal, options, native, stdin, stdout, order };
	}

	it("commits the Worker before output without ever resuming a direct reader", () => {
		const f = setup();
		f.terminal.configureNativeCopy(f.options);
		f.terminal.start(vi.fn(), vi.fn());
		expect(f.order.slice(0, 4)).toEqual(["prepare", "mouse", "commit", "write"]);
		expect(f.stdin.listenerCount("data")).toBe(0);
		expect(f.stdin.resume).not.toHaveBeenCalled();
		f.order.length = 0;
		f.terminal.setViewportMode(true);
		expect(f.order).toEqual(["mouse", "write"]);
		f.order.length = 0;
		f.terminal.holdOscInput(true);
		f.terminal.write("query");
		expect(f.order).toEqual(["osc", "write"]);
		f.terminal.stop();
	});

	it("refuses a second terminal reader while the Worker owns stdin", () => {
		const f = setup();
		f.terminal.configureNativeCopy(f.options);
		f.terminal.start(vi.fn(), vi.fn());
		try {
			expect(() => new ProcessTerminal().start(vi.fn(), vi.fn())).toThrow(/owned/);
		} finally {
			f.terminal.stop();
		}
	});

	it("initializes pre-start viewport parser mode before committing ownership", () => {
		const f = setup();
		f.terminal.setViewportMode(true);
		f.terminal.configureNativeCopy(f.options);
		f.order.length = 0;
		f.terminal.start(vi.fn(), vi.fn());
		expect(f.native.setMouseReporting).toHaveBeenCalledWith(true);
		expect(f.order.slice(0, 3)).toEqual(["prepare", "mouse", "commit"]);
		f.terminal.stop();
	});

	it("falls back only when preparation proves that ownership was not acquired", () => {
		const f = setup();
		f.native.prepare.mockReturnValue(false);
		f.terminal.configureNativeCopy(f.options);
		f.terminal.start(vi.fn(), vi.fn());
		expect(f.native.commit).not.toHaveBeenCalled();
		expect(f.stdin.listenerCount("data")).toBe(1);
		expect(f.stdin.resume).toHaveBeenCalledOnce();
		f.terminal.stop();
	});

	it("does not replace a reader after uncertain commit or restore terminal state after failed stop", () => {
		const f = setup();
		f.terminal.configureNativeCopy(f.options);
		f.native.commit.mockImplementation(() => {
			throw new Error("commit uncertain");
		});
		expect(() => f.terminal.start(vi.fn(), vi.fn())).toThrow("commit uncertain");
		expect(() => f.terminal.start(vi.fn(), vi.fn())).toThrow("unproven");
		f.native.stop.mockImplementation(() => {
			throw new Error("release uncertain");
		});
		expect(() => f.terminal.stop()).toThrow("release uncertain");
		expect(f.stdin.resume).not.toHaveBeenCalled();
		expect(f.stdout.write).not.toHaveBeenCalled();
		expect(f.stdin.setRawMode).toHaveBeenCalledExactlyOnceWith(true);
		f.native.stop.mockImplementation(() => {});
		f.terminal.stop();
	});

	it("cancels output timers before an uncertain stop without writing restoration sequences", () => {
		vi.useFakeTimers();
		const f = setup();
		f.terminal.configureNativeCopy(f.options);
		f.terminal.start(vi.fn(), vi.fn());
		f.terminal.setProgress(true);
		f.stdout.write.mockClear();
		f.native.stop.mockImplementation(() => {
			throw new Error("release uncertain");
		});
		expect(() => f.terminal.stop()).toThrow("release uncertain");
		vi.advanceTimersByTime(2000);
		expect(f.stdout.write).not.toHaveBeenCalled();
		f.native.stop.mockImplementation(() => {});
		f.terminal.stop();
	});

	it("does not rearm Copy from a live paint during input drain", async () => {
		const f = setup();
		const control = f.terminal.configureNativeCopy(f.options);
		f.terminal.start(vi.fn(), vi.fn());
		let finish!: () => void;
		f.native.drain.mockImplementation(
			() =>
				new Promise<void>((resolve) => {
					finish = resolve;
				}),
		);
		const draining = f.terminal.drainInput();
		f.native.setLease.mockClear();
		try {
			control.setLease({});
			expect(f.native.setLease).not.toHaveBeenCalled();
		} finally {
			finish();
			await draining;
			f.terminal.stop();
		}
	});

	it("defers late configuration without transferring the direct parser", () => {
		const f = setup();
		f.terminal.start(vi.fn(), vi.fn());
		f.stdin.emit("data", "\x1b[");
		f.terminal.configureNativeCopy(f.options);
		expect(f.native.prepare).not.toHaveBeenCalled();
		expect(f.stdin.listenerCount("data")).toBe(1);
		expect(f.options.onAvailability).toHaveBeenCalledWith(false, expect.stringContaining("restart"));
		f.terminal.stop();
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
