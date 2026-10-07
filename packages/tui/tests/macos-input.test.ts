import { Worker } from "node:worker_threads";
import { describe, expect, it, vi } from "vitest";
import { type InputMessage, MacosInput } from "../src/macos-input.js";
import { MacosInputReader } from "../src/macos-input-worker.js";

function fixture() {
	const shared = new Int32Array(new SharedArrayBuffer(24));
	const input: Buffer[] = [];
	const events: InputMessage[] = [];
	const native = {
		register: vi.fn((_id: number) => 0),
		unregister: vi.fn(() => 0),
		dispose: vi.fn(() => 0),
		foreground: vi.fn(() => true),
		pump: vi.fn<(copy: (id: number) => void) => void>(),
	};
	const reader = new MacosInputReader(shared, {
		read: () => input.shift() ?? null,
		native,
		send: (event) => events.push(event),
	});
	return { reader, shared, input, events, native };
}

describe("macOS input ownership", () => {
	it("does not read before commit and synchronously releases resources on stop", () => {
		const f = fixture();
		f.input.push(Buffer.from("x"));
		f.reader.tick();
		expect(f.events).toEqual([]);
		f.reader.command({ kind: "commit" });
		f.reader.tick();
		expect(f.events).toContainEqual({ kind: "data", data: "x", bytes: 1 });
		f.reader.command({ kind: "stop" });
		f.input.push(Buffer.from("y"));
		f.reader.tick();
		expect(f.input).toHaveLength(1);
		expect(f.native.dispose).toHaveBeenCalledOnce();
	});

	it("frames split UTF-8 and paste once, without treating pasted focus as terminal focus", () => {
		const f = fixture();
		f.reader.command({ kind: "commit" });
		const bytes = Buffer.from("é\x1b[200~\x1b[I界\ntext\x1b[201~");
		for (const byte of bytes) {
			f.input.push(Buffer.from([byte]));
			f.reader.tick();
		}
		expect(
			f.events
				.filter((e) => e.kind === "data")
				.map((e) => e.data)
				.join(""),
		).toBe("é");
		expect(f.events.filter((e) => e.kind === "paste").map((e) => e.data)).toEqual(["\x1b[I界\ntext"]);
		expect(Atomics.load(f.shared, 1)).toBe(0);
		f.reader.command({ kind: "stop" });
	});

	it("observes the entire read batch before pumping hotkeys and rejects stale registration IDs", () => {
		const f = fixture();
		f.reader.command({ kind: "commit" });
		f.reader.command({ kind: "mouse", enabled: true });
		f.reader.command({ kind: "lease", lease: 1 });
		f.input.push(Buffer.from("\x1b[I"));
		f.reader.tick();
		const id = f.native.register.mock.calls[0][0];
		f.native.pump.mockImplementation((copy) => copy(id));
		f.input.push(Buffer.from("x"), Buffer.from("\x1b[O"));
		f.reader.tick();
		expect(f.events.filter((e) => e.kind === "copy")).toEqual([]);
		f.input.push(Buffer.from("\x1b[I"));
		f.reader.tick();
		expect(f.native.register.mock.calls[1][0]).not.toBe(id);
		expect(f.events.filter((e) => e.kind === "copy")).toEqual([]);
		f.reader.command({ kind: "stop" });
	});

	it("revokes before backpressure pauses reads and preserves an oversized atomic paste", () => {
		const f = fixture();
		f.reader.command({ kind: "commit" });
		f.reader.command({ kind: "mouse", enabled: true });
		f.reader.command({ kind: "lease", lease: 1 });
		f.input.push(Buffer.from("\x1b[I"));
		f.reader.tick();
		const text = "界".repeat(400000);
		f.input.push(Buffer.from(`\x1b[200~${text}\x1b[201~`));
		f.reader.tick();
		expect(f.native.unregister).toHaveBeenCalled();
		expect(f.events.find((e) => e.kind === "paste")?.data).toBe(text);
		f.input.push(Buffer.from("\x1b[O"));
		f.reader.tick();
		expect(f.input).toHaveLength(1);
		f.reader.command({ kind: "consumed", bytes: f.events.reduce((sum, e) => sum + ("bytes" in e ? e.bytes : 0), 0) });
		f.reader.tick();
		expect(f.input).toHaveLength(0);
		expect(f.native.register).toHaveBeenCalledTimes(1);
		f.reader.command({ kind: "stop" });
	});

	it("measures incomplete raw input during drain and discards framed output", () => {
		const f = fixture();
		f.reader.command({ kind: "commit" });
		f.reader.command({ kind: "drain" });
		f.input.push(Buffer.from("\x1b["));
		f.reader.tick();
		expect(Atomics.load(f.shared, 3)).toBe(1);
		expect(f.events).toEqual([]);
		f.reader.command({ kind: "stop" });
	});

	it("holds OSC framing, revokes for partial input and preserves mouse parsing order", () => {
		const f = fixture();
		f.reader.command({ kind: "commit" });
		f.reader.command({ kind: "mouse", enabled: true });
		f.reader.command({ kind: "lease", lease: 1 });
		f.input.push(Buffer.from("\x1b[<0;2;3M"));
		f.reader.tick();
		expect(f.native.register).toHaveBeenCalledOnce();
		f.reader.command({ kind: "osc", hold: true });
		f.input.push(Buffer.from("\x1b]11;rgb:"));
		f.reader.tick();
		expect(f.native.unregister).toHaveBeenCalledOnce();
		f.input.push(Buffer.from("ffff/0000/0000\x07"));
		f.reader.tick();
		expect(f.events).toContainEqual({ kind: "data", data: "\x1b]11;rgb:ffff/0000/0000\x07", bytes: 24 });
		f.reader.command({ kind: "stop" });
	});

	it("keeps focus unknown until a local event, and additionally requires foreground ownership", () => {
		const f = fixture();
		f.reader.command({ kind: "commit" });
		f.reader.command({ kind: "mouse", enabled: true });
		f.reader.command({ kind: "lease", lease: 1 });
		f.reader.tick();
		expect(f.native.register).not.toHaveBeenCalled();
		f.native.foreground.mockReturnValue(false);
		f.input.push(Buffer.from("\x1b[I"));
		f.reader.tick();
		expect(f.native.register).not.toHaveBeenCalled();
		f.native.foreground.mockReturnValue(true);
		f.reader.tick();
		expect(f.native.register).toHaveBeenCalledOnce();
		f.reader.command({ kind: "stop" });
	});

	it("leaves a conflict unowned without retrying the same lease", () => {
		const f = fixture();
		f.native.register.mockReturnValue(-9878);
		f.reader.command({ kind: "commit" });
		f.reader.command({ kind: "mouse", enabled: true });
		f.reader.command({ kind: "lease", lease: 1 });
		f.input.push(Buffer.from("\x1b[I"));
		f.reader.tick();
		f.reader.tick();
		expect(f.native.register).toHaveBeenCalledOnce();
		expect(f.native.unregister).not.toHaveBeenCalled();
		expect(f.events).toContainEqual(expect.objectContaining({ kind: "availability", available: false }));
		f.reader.command({ kind: "stop" });
	});

	it("revokes before yielding a saturated read budget", () => {
		const f = fixture();
		f.reader.command({ kind: "commit" });
		f.reader.command({ kind: "mouse", enabled: true });
		f.reader.command({ kind: "lease", lease: 1 });
		f.input.push(Buffer.from("\x1b[I"));
		f.reader.tick();
		for (let i = 0; i < 17; i++) f.input.push(Buffer.from("x"));
		f.reader.tick();
		expect(f.native.unregister).toHaveBeenCalledOnce();
		expect(f.input).toHaveLength(1);
		f.input.push(Buffer.from("\x1b[O"));
		f.reader.tick();
		expect(f.native.register).toHaveBeenCalledOnce();
		f.reader.command({ kind: "stop" });
	});

	it("does not certify release when native unregistration fails", () => {
		const f = fixture();
		f.reader.command({ kind: "commit" });
		f.reader.command({ kind: "mouse", enabled: true });
		f.reader.command({ kind: "lease", lease: 1 });
		f.input.push(Buffer.from("\x1b[I"));
		f.reader.tick();
		f.native.unregister.mockReturnValue(-1);
		expect(() => f.reader.command({ kind: "stop" })).toThrow(/unregister/i);
		expect(f.native.dispose).not.toHaveBeenCalled();
		expect(() => f.reader.command({ kind: "stop" })).toThrow(/unknown/);
		expect(f.native.unregister).toHaveBeenCalledOnce();
	});
});

describe("synchronous Worker settlement", () => {
	it.each(["failure", "timeout", "exit"])(
		"faults instead of restarting after uncertain %s settlement",
		async (mode) => {
			let worker!: Worker;
			const transport = new MacosInput(
				vi.fn(),
				vi.fn(),
				vi.fn(),
				({ port, shared }) => {
					worker = new Worker(
						`
				const { workerData } = require('node:worker_threads');
				const { port, shared, mode } = workerData;
				const state = new Int32Array(shared);
				Atomics.store(state, 4, 1); Atomics.notify(state, 4);
				port.on('message', (m) => {
					if (mode === 'exit') process.exit(1);
					if (mode === 'timeout') return;
					const ack = new Int32Array(m.ack);
					Atomics.store(ack, 1, -1);
					Atomics.store(ack, 0, m.sequence); Atomics.notify(ack, 0);
				});
			`,
						{ eval: true, workerData: { port, shared, mode }, transferList: [port] },
					);
					return worker;
				},
				vi.fn(),
			);
			try {
				expect(transport.prepare()).toBe(true);
				expect(() => transport.commit()).toThrow(/unproven|unknown/);
				expect(() => transport.commit()).toThrow();
				expect(() => transport.stop()).toThrow();
			} finally {
				await worker.terminate();
			}
		},
	);
	it("progresses while the main thread waits and binds status to each request", () => {
		const transport = new MacosInput(
			vi.fn(),
			vi.fn(),
			vi.fn(),
			({ port, shared, generation }) =>
				new Worker(
					`
			const { workerData } = require('node:worker_threads');
			const { port, shared, generation } = workerData;
			const state = new Int32Array(shared);
			Atomics.store(state, 4, 1); Atomics.notify(state, 4);
			port.on('message', (m) => {
				if (!m.ack) return;
				const ack = new Int32Array(m.ack);
				Atomics.store(ack, 1, m.generation === generation ? 1 : -1);
				Atomics.store(ack, 0, m.sequence); Atomics.notify(ack, 0);
				if (m.command.kind === 'stop') port.close();
			});
		`,
					{ eval: true, workerData: { port, shared, generation }, transferList: [port] },
				),
		);
		expect(transport.prepare()).toBe(true);
		transport.commit();
		transport.holdOscInput(true);
		transport.stop();
	});
});
