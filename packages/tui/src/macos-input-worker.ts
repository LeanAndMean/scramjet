// SCRAMJET-DIVERGENCE: this Worker exclusively owns framing, focus and native registration, never clipboard text.
import { execFileSync } from "node:child_process";
import { readSync } from "node:fs";
import { createRequire } from "node:module";
import { StringDecoder } from "node:string_decoder";
import { isMainThread, type MessagePort, workerData } from "node:worker_threads";
import type { InputCommand, InputMessage } from "./macos-input.js";
import { StdinBuffer } from "./stdin-buffer.js";

interface NativeCopy {
	register(id: number): number;
	unregister(): number;
	dispose(): number;
	foreground(): boolean;
	pump(copy: (id: number) => void): void;
}

const BACKLOG_BYTES = 256 * 1024;
const READ_BUDGET = 16;

export class MacosInputReader {
	private readonly parser = new StdinBuffer({ timeout: 10 });
	private readonly decoder = new StringDecoder("utf8");
	private running = false;
	private stopped = false;
	private draining = false;
	private mouse = false;
	private lease = 0;
	private registration = 0;
	private nextRegistration = 0;
	private focus = 0;
	private backlog = 0;
	private contentionLease = 0;
	private contentionAttempts = 0;
	private contentionUntil = 0;
	private releaseUnknown = false;

	constructor(
		private readonly shared: Int32Array,
		private readonly io: {
			read(): Buffer | null;
			native?: NativeCopy;
			send(message: InputMessage): void;
		},
	) {
		this.parser.on("data", (data) => {
			if (this.mouse) {
				if (data === "\x1b[O" || data === "\x1b" || data === "\x1b[") this.setFocus(false);
				else if (data === "\x1b[I" || /^\x1b\[<(?:0|1|2);[1-9]\d{0,4};[1-9]\d{0,4}M$/.test(data))
					this.setFocus(true);
			}
			this.forward("data", data);
		});
		this.parser.on("paste", (data) => this.forward("paste", data));
	}

	command(command: InputCommand): void {
		if (this.stopped) throw new Error("Input reader already stopped");
		switch (command.kind) {
			case "commit":
				this.running = true;
				break;
			case "osc":
				this.parser.holdOscInput(command.hold);
				break;
			case "mouse":
				if (!command.enabled) {
					this.revoke();
					this.setFocus(false);
				}
				this.mouse = command.enabled;
				this.parser.setMouseReporting(command.enabled);
				break;
			case "lease":
				this.revoke();
				this.contentionAttempts = 0;
				this.lease = command.lease;
				break;
			case "consumed":
				this.backlog = Math.max(0, this.backlog - command.bytes);
				break;
			case "drain":
				this.setFocus(false);
				this.lease = 0;
				this.draining = true;
				break;
			case "endDrain":
				this.draining = false;
				break;
			case "stop":
				this.running = false;
				this.parser.destroy();
				this.revoke();
				if (this.io.native) {
					this.releaseUnknown = true;
					if (this.io.native.dispose() !== 0) throw new Error("Native Copy handler removal failed");
					this.releaseUnknown = false;
				}
				this.stopped = true;
				this.setFocus(false);
				break;
		}
	}

	tick(): void {
		if (!this.running || this.stopped) return;
		if (this.backlog >= BACKLOG_BYTES) {
			this.revoke();
			return;
		}
		let exhausted = true;
		for (let count = 0; count < READ_BUDGET; count++) {
			const data = this.io.read();
			if (!data) {
				exhausted = false;
				break;
			}
			Atomics.add(this.shared, 3, 1);
			const text = this.decoder.write(data);
			if (text) this.parser.process(text);
			if (this.backlog >= BACKLOG_BYTES) break;
		}
		if (exhausted || this.backlog >= BACKLOG_BYTES || this.parser.getBuffer()) {
			this.revoke();
			return;
		}
		const native = this.io.native;
		if (!native) return;
		const eligible =
			this.mouse && !this.draining && this.lease !== 0 && (this.focus & 1) === 1 && native.foreground();
		if (!eligible) this.revoke();
		else if (!this.registration && this.contentionLease !== this.lease) {
			const id = ++this.nextRegistration;
			const now = performance.now();
			const status = this.contentionAttempts > 0 && now > this.contentionUntil ? -9878 : native.register(id);
			if (status === 0) {
				this.contentionAttempts = 0;
				this.registration = id;
				Atomics.store(this.shared, 2, id);
				if (this.contentionLease) {
					this.contentionLease = 0;
					this.io.send({ kind: "availability", available: true });
				}
			} else {
				if (this.contentionAttempts === 0) this.contentionUntil = now + 40;
				this.contentionAttempts++;
				// Focus transfer can reach the new Worker just before the old Worker releases ownership.
				if (status === -9878 && this.contentionAttempts < 3 && now < this.contentionUntil) return;
				this.contentionLease = this.lease;
				this.io.send({
					kind: "availability",
					available: false,
					reason: `Native Command+C registration failed (${status}); another application may own the shortcut. Clear and reselect after resolving the conflict.`,
				});
			}
		}
		native.pump((id) => {
			if (id !== this.registration || !id || !eligible || !native.foreground()) return;
			this.backlog += 64;
			this.io.send({ kind: "copy", focus: this.focus, registration: id, lease: this.lease, bytes: 64 });
			if (this.backlog >= BACKLOG_BYTES) this.revoke();
		});
	}

	private setFocus(focused: boolean): void {
		this.contentionAttempts = 0;
		this.focus = ((this.focus >>> 1) + 1) * 2 + Number(focused);
		Atomics.store(this.shared, 1, this.focus);
		this.revoke();
	}

	private revoke(): void {
		Atomics.store(this.shared, 2, 0);
		if (this.releaseUnknown)
			throw new Error("Native Copy unregister ownership is unknown; cleanup cannot be retried");
		if (!this.registration) return;
		this.releaseUnknown = true;
		if (this.io.native!.unregister() !== 0) throw new Error("Native Copy unregister failed; ownership is unknown");
		this.releaseUnknown = false;
		this.registration = 0;
	}

	private forward(kind: "data" | "paste", data: string): void {
		if (this.draining || !this.running) return;
		const bytes = Buffer.byteLength(data);
		this.backlog += bytes;
		if (this.backlog >= BACKLOG_BYTES) this.revoke();
		this.io.send({ kind, data, bytes });
	}
}

function terminalAncestor(): number | undefined {
	if (process.env.SSH_CONNECTION || process.env.SSH_TTY || process.env.TMUX || process.env.STY) return undefined;
	const output = execFileSync("/bin/ps", ["-ax", "-o", "pid=,ppid=,comm="], {
		encoding: "utf8",
		timeout: 1000,
		maxBuffer: 1024 * 1024,
	});
	const processes = new Map<number, { parent: number; command: string }>();
	for (const line of output.split("\n")) {
		const match = /^\s*(\d+)\s+(\d+)\s+(.+)$/.exec(line);
		if (match) processes.set(Number(match[1]), { parent: Number(match[2]), command: match[3] });
	}
	for (let pid = process.pid, count = 0; count < 64; count++) {
		const entry = processes.get(pid);
		if (!entry || /(?:^|\/)(?:sshd|tmux|screen)(?:\s|$)/.test(entry.command)) return undefined;
		if (/\/(?:Terminal|iTerm)\.app\/Contents\/MacOS\/(?:Terminal|iTerm2)$/.test(entry.command)) return pid;
		pid = entry.parent;
	}
	return undefined;
}

class NativeOwnershipError extends Error {}

export function createNative(koffi: typeof import("koffi"), terminalPid: number): NativeCopy {
	const carbon = koffi.load("/System/Library/Frameworks/Carbon.framework/Carbon");
	const Spec = koffi.struct({ eventClass: "uint32", eventKind: "uint32" });
	const ID = koffi.struct({ signature: "uint32", id: "uint32" });
	const PSN = koffi.struct({ high: "uint32", low: "uint32" });
	const Callback = koffi.proto("int32 ScramjetCopyHandler(void *, void *, void *)");
	const target = carbon.func("void *GetApplicationEventTarget()");
	const install = carbon.func("InstallEventHandler", "int32", [
		"void *",
		koffi.pointer(Callback),
		"uint32",
		koffi.pointer(Spec),
		"void *",
		koffi.out(koffi.pointer("void *")),
	]);
	const register = carbon.func("RegisterEventHotKey", "int32", [
		"uint32",
		"uint32",
		ID,
		"void *",
		"uint32",
		koffi.out(koffi.pointer("void *")),
	]);
	const unregister = carbon.func("int32 UnregisterEventHotKey(void *)");
	const remove = carbon.func("int32 RemoveEventHandler(void *)");
	const receive = carbon.func("ReceiveNextEvent", "int32", [
		"uint32",
		"void *",
		"double",
		"uint8",
		koffi.out(koffi.pointer("void *")),
	]);
	const send = carbon.func("int32 SendEventToEventTarget(void *, void *)");
	const dispatcher = carbon.func("void *GetEventDispatcherTarget()");
	const release = carbon.func("void ReleaseEvent(void *)");
	const front = carbon.func("GetFrontProcess", "int32", [koffi.out(koffi.pointer(PSN))]);
	const pidOf = carbon.func("GetProcessPID", "int32", [koffi.pointer(PSN), koffi.out(koffi.pointer("int32"))]);
	const parameter = carbon.func("GetEventParameter", "int32", [
		"void *",
		"uint32",
		"uint32",
		"void *",
		"uint32",
		"void *",
		koffi.out(koffi.pointer(ID)),
	]);
	let copy: ((id: number) => void) | undefined;
	let callbackError: unknown;
	let reference: unknown;
	const callback = koffi.register((_next: unknown, event: unknown) => {
		const identity = { signature: 0, id: 0 };
		try {
			if (
				parameter(event, 0x2d2d2d2d, 0x686b6964, null, 8, null, identity) === 0 &&
				identity.signature === 0x5343524d
			)
				copy?.(identity.id);
		} catch (error) {
			callbackError = error;
		}
		return 0;
	}, koffi.pointer(Callback));
	const handler = [null];
	let installed: number;
	try {
		installed = install(target(), callback, 1, { eventClass: 0x6b657962, eventKind: 5 }, null, handler);
	} catch {
		throw new NativeOwnershipError("Native Copy handler installation has unknown ownership; exit this process.");
	}
	if ((installed === 0) !== Boolean(handler[0]))
		throw new NativeOwnershipError(
			"Native Copy handler installation returned ambiguous ownership; exit this process.",
		);
	if (installed !== 0) {
		koffi.unregister(callback);
		throw new Error(`Native Copy handler installation failed (${installed})`);
	}
	return {
		register(id) {
			const result = [null];
			// Non-exclusive registration can succeed while an exclusive owner silently suppresses delivery.
			const status = register(8, 256, { signature: 0x5343524d, id }, target(), 1, result);
			if (status === 0) {
				if (!result[0]) throw new Error("Native Copy registration succeeded without an ownership reference");
				reference = result[0];
			} else if (result[0]) throw new Error("Native Copy registration returned ambiguous ownership");
			return status;
		},
		unregister() {
			const status = unregister(reference);
			if (status === 0) reference = undefined;
			return status;
		},
		dispose() {
			const status = remove(handler[0]);
			if (status === 0) koffi.unregister(callback);
			return status;
		},
		foreground() {
			const psn = { high: 0, low: 0 };
			const pid = [0];
			return front(psn) === 0 && pidOf(psn, pid) === 0 && pid[0] === terminalPid;
		},
		pump(onCopy) {
			copy = onCopy;
			try {
				for (let count = 0; count < 32; count++) {
					const event = [null];
					const status = receive(0, null, 0, 1, event);
					if (status === -9875) break;
					if (status !== 0) throw new Error(`Native Copy event receive failed (${status})`);
					try {
						send(event[0], dispatcher());
					} finally {
						release(event[0]);
					}
					if (callbackError) throw callbackError;
				}
			} finally {
				copy = undefined;
			}
		},
	};
}

function run(port: MessagePort, shared: Int32Array, generation: number): void {
	let read: () => Buffer | null;
	let koffi: typeof import("koffi");
	try {
		koffi = createRequire(import.meta.url)("koffi");
		const libc = koffi.load("/usr/lib/libSystem.B.dylib");
		const PollFD = koffi.struct({ fd: "int", events: "short", revents: "short" });
		const poll = libc.func("poll", "int", [koffi.inout(koffi.pointer(PollFD)), "uint32", "int"]);
		const buffer = Buffer.alloc(16384);
		read = () => {
			const descriptor = { fd: 0, events: 1, revents: 0 };
			const ready = poll(descriptor, 1, 0);
			if (ready < 0) throw new Error("macOS input poll failed");
			if (descriptor.revents & 0x38) throw new Error("macOS terminal input was lost");
			if (!(descriptor.revents & 1)) return null;
			try {
				const count = readSync(0, buffer, 0, buffer.length, null);
				if (count === 0) throw new Error("macOS terminal input reached EOF");
				return buffer.subarray(0, count);
			} catch (error) {
				if (["EAGAIN", "EINTR"].includes((error as NodeJS.ErrnoException).code ?? "")) return null;
				throw error;
			}
		};
	} catch {
		Atomics.store(shared, 4, 2);
		Atomics.notify(shared, 4);
		port.close();
		return;
	}
	let native: NativeCopy | undefined;
	try {
		const terminalPid = terminalAncestor();
		if (!terminalPid)
			throw new Error(
				"Native Command+C requires a verified local Apple Terminal or iTerm2 ancestor without a multiplexer; use application Copy on this path.",
			);
		native = createNative(koffi, terminalPid);
		port.postMessage({ kind: "availability", available: true });
	} catch (error) {
		if (error instanceof NativeOwnershipError) {
			Atomics.store(shared, 0, 1);
			Atomics.store(shared, 4, 1);
			Atomics.notify(shared, 4);
			port.postMessage({ kind: "fault", reason: error.message });
			port.close();
			return;
		}
		port.postMessage({
			kind: "availability",
			available: false,
			reason: error instanceof Error ? error.message : "Native Command+C unavailable; use application Copy.",
		});
	}
	const reader = new MacosInputReader(shared, { read, native, send: (message) => port.postMessage(message) });
	let timer: ReturnType<typeof setInterval>;
	const fail = (error: unknown) => {
		clearInterval(timer);
		try {
			reader.command({ kind: "stop" });
		} catch {
			/* Unknown native ownership remains a transport fault. */
		}
		Atomics.store(shared, 0, 1);
		port.postMessage({
			kind: "fault",
			reason: `macOS input failed: ${error instanceof Error ? error.message : "unknown error"}. Exit this session; input/native release cannot be assumed.`,
		});
	};
	process.on("uncaughtException", fail);
	timer = setInterval(() => {
		try {
			reader.tick();
		} catch (error) {
			fail(error);
		}
	}, 3);
	port.on(
		"message",
		(message: { command: InputCommand; generation: number; sequence: number; ack?: SharedArrayBuffer }) => {
			let status = -1;
			try {
				if (message.generation !== generation || Atomics.load(shared, 0))
					throw new Error("Stale or faulted input command");
				reader.command(message.command);
				status = 1;
				if (message.command.kind === "stop") clearInterval(timer);
			} catch (error) {
				fail(error);
			}
			if (message.ack) {
				const ack = new Int32Array(message.ack);
				Atomics.store(ack, 1, status);
				Atomics.store(ack, 0, message.sequence);
				Atomics.notify(ack, 0);
			}
			if (message.command.kind === "stop" && status === 1) {
				process.removeListener("uncaughtException", fail);
				port.close();
			}
		},
	);
	Atomics.store(shared, 4, 1);
	Atomics.notify(shared, 4);
}

if (!isMainThread && workerData?.port) run(workerData.port, new Int32Array(workerData.shared), workerData.generation);
