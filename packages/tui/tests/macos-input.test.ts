import { Worker } from "node:worker_threads";
import { describe, expect, it, vi } from "vitest";
import { type InputMessage, MacosInput } from "../src/macos-input.js";
import { createNative, MacosInputReader } from "../src/macos-input-worker.js";

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

	it("does not rearm after a focus-out prefix expires before its suffix arrives", () => {
		vi.useFakeTimers();
		const f = fixture();
		try {
			f.reader.command({ kind: "commit" });
			f.reader.command({ kind: "mouse", enabled: true });
			f.reader.command({ kind: "lease", lease: 1 });
			f.input.push(Buffer.from("\x1b[I"));
			f.reader.tick();
			f.input.push(Buffer.from("\x1b["));
			f.reader.tick();
			vi.advanceTimersByTime(11);
			f.reader.tick();
			f.input.push(Buffer.from("O"));
			f.reader.tick();
			expect(f.native.register).toHaveBeenCalledTimes(1);
		} finally {
			f.reader.command({ kind: "stop" });
			vi.useRealTimers();
		}
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

describe("native API boundary", () => {
	it("validates parameter status and signature, preserves the event ID and checks release statuses", () => {
		let callback!: (next: unknown, event: unknown) => number;
		let queued = false;
		let parameterStatus = 0;
		let signature = 0x5343524d;
		const unregister = vi.fn(() => 0);
		const remove = vi.fn(() => 0);
		const funcs: Record<string, (...args: unknown[]) => unknown> = {
			InstallEventHandler: (...args) => {
				(args[5] as unknown[])[0] = {};
				return 0;
			},
			RegisterEventHotKey: (...args) => {
				(args[5] as unknown[])[0] = {};
				return 0;
			},
			"int32 UnregisterEventHotKey(void *)": unregister,
			"int32 RemoveEventHandler(void *)": remove,
			ReceiveNextEvent: (...args) => {
				if (!queued) return -9875;
				queued = false;
				(args[4] as unknown[])[0] = {};
				return 0;
			},
			"int32 SendEventToEventTarget(void *, void *)": () => callback(null, {}),
			GetEventParameter: (...args) => {
				Object.assign(args[6] as object, { signature, id: 71 });
				return parameterStatus;
			},
		};
		const koffi = {
			load: () => ({ func: (name: string) => funcs[name] ?? (() => 0) }),
			struct: () => ({}),
			proto: () => ({}),
			pointer: (value: unknown) => value,
			out: (value: unknown) => value,
			register: (fn: typeof callback) => {
				callback = fn;
				return {};
			},
			unregister: vi.fn(),
		};
		const native = createNative(koffi as unknown as typeof import("koffi"), 1);
		expect(native.register(99)).toBe(0);
		const copy = vi.fn();
		queued = true;
		parameterStatus = -1;
		native.pump(copy);
		queued = true;
		parameterStatus = 0;
		signature = 0;
		native.pump(copy);
		expect(copy).not.toHaveBeenCalled();
		queued = true;
		signature = 0x5343524d;
		native.pump(copy);
		expect(copy).toHaveBeenCalledExactlyOnceWith(71);
		unregister.mockReturnValue(-1);
		expect(native.unregister()).toBe(-1);
		remove.mockReturnValue(-1);
		expect(native.dispose()).toBe(-1);
		expect(koffi.unregister).not.toHaveBeenCalled();
		remove.mockReturnValue(0);
		expect(native.dispose()).toBe(0);
		expect(koffi.unregister).toHaveBeenCalledOnce();
	});
});

describe("synchronous Worker settlement", () => {
	it("rechecks atomic focus and registration even before queued notices can be delivered", async () => {
		let shared!: Int32Array;
		const copy = vi.fn();
		const transport = new MacosInput(vi.fn(), copy, vi.fn(), (data) => {
			shared = new Int32Array(data.shared);
			return new Worker(
				`
				const { workerData } = require('node:worker_threads');
				const { port, shared } = workerData;
				const state = new Int32Array(shared);
				Atomics.store(state, 4, 1); Atomics.notify(state, 4);
				port.on('message', (m) => {
					if (!m.ack) return;
					if (m.command.kind === 'lease' && m.command.lease) {
						Atomics.store(state, 1, 3); Atomics.store(state, 2, m.command.lease);
						port.postMessage({ kind: 'copy', focus: 3, registration: m.command.lease, lease: m.command.lease, bytes: 64 });
					}
					const ack = new Int32Array(m.ack);
					Atomics.store(ack, 1, 1); Atomics.store(ack, 0, m.sequence); Atomics.notify(ack, 0);
					if (m.command.kind === 'stop') port.close();
				});
			`,
				{ eval: true, workerData: data, transferList: [data.port] },
			);
		});
		try {
			transport.prepare();
			transport.commit();
			transport.setLease({});
			Atomics.store(shared, 1, 4);
			await new Promise((resolve) => setTimeout(resolve, 20));
			expect(copy).not.toHaveBeenCalled();
			transport.setLease({});
			await vi.waitFor(() => expect(copy).toHaveBeenCalledOnce());
			const notice = copy.mock.calls[0][0];
			expect(transport.isCurrent(notice)).toBe(true);
			Atomics.store(shared, 2, 19);
			expect(transport.isCurrent(notice)).toBe(false);
		} finally {
			transport.stop();
		}
	});
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
