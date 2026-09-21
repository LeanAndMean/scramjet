import { afterEach, describe, expect, it, vi } from "vitest";
import { StdinBuffer } from "../src/stdin-buffer.js";
import { ProcessTerminal } from "../src/terminal.js";

afterEach(() => {
	vi.restoreAllMocks();
	vi.useRealTimers();
});

describe("mouse transport framing", () => {
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

describe("candidate terminal modes", () => {
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
