// SCRAMJET-DIVERGENCE: independent input ownership keeps native Copy revocation responsive during UI stalls.
import { MessageChannel, type MessagePort, Worker } from "node:worker_threads";

export interface NativeCopyNotice {
	readonly generation: number;
	readonly focus: number;
	readonly registration: number;
	readonly lease: object;
}

export interface NativeCopyOptions {
	onCopyIntent(notice: NativeCopyNotice): void;
	onAvailability(available: boolean, reason?: string): void;
	onError(error: Error): void;
}

export interface NativeCopyControl {
	setLease(lease: object | null): void;
	isCurrent(notice: NativeCopyNotice): boolean;
	dispose(): void;
}

export type InputCommand =
	| { kind: "commit" | "stop" | "drain" | "endDrain" }
	| { kind: "osc"; hold: boolean }
	| { kind: "mouse"; enabled: boolean }
	| { kind: "lease"; lease: number }
	| { kind: "consumed"; bytes: number };

export type InputMessage =
	| { kind: "data" | "paste"; data: string; bytes: number }
	| { kind: "copy"; focus: number; registration: number; lease: number; bytes: number }
	| { kind: "availability"; available: boolean; reason?: string }
	| { kind: "fault"; reason: string; code?: "ERR_TERMINAL_INPUT_LOST" };

let nextGeneration = 0;
const DEADLINE_MS = 3000;

export class MacosInput {
	private readonly shared = new Int32Array(new SharedArrayBuffer(24));
	private readonly generation = ++nextGeneration;
	private readonly port: MessagePort;
	private readonly worker: Worker;
	private sequence = 0;
	private leaseId = 0;
	private lease: object | null = null;
	private state: "prepared" | "running" | "stopped" | "faulted" = "prepared";
	private failure?: Error;

	constructor(
		private readonly onInput: (kind: "data" | "paste", data: string) => void,
		private readonly onCopy: NativeCopyOptions["onCopyIntent"],
		private readonly onAvailability: NativeCopyOptions["onAvailability"],
		createWorker = (data: { port: MessagePort; shared: SharedArrayBuffer; generation: number }) =>
			new Worker(new URL("./macos-input-worker.js", import.meta.url), {
				workerData: data,
				transferList: [data.port],
			}),
		private readonly onError: NativeCopyOptions["onError"] = (error) => {
			throw error;
		},
	) {
		const channel = new MessageChannel();
		this.port = channel.port1;
		this.worker = createWorker({
			port: channel.port2,
			shared: this.shared.buffer as SharedArrayBuffer,
			generation: this.generation,
		});
		this.port.on("message", (message: InputMessage) => this.receive(message));
		this.worker.on("error", (error) => this.fail(error));
		this.worker.on("exit", () => {
			if (this.state !== "stopped")
				this.fail(
					new Error(
						"macOS input Worker exited without proving input/native release; exit this session before another terminal handoff.",
					),
				);
		});
	}

	prepare(): boolean {
		this.wait(this.shared, 4, () => Atomics.load(this.shared, 4) !== 0, "readiness");
		if (Atomics.load(this.shared, 4) === 2) {
			this.state = "stopped";
			this.port.close();
			this.onAvailability(
				false,
				"Native macOS Copy unavailable: input dependencies could not load. Reinstall with optional Koffi dependencies enabled; input remains on the direct reader.",
			);
			return false;
		}
		this.assertHealthy();
		return true;
	}

	checkHealth(): void {
		this.assertHealthy();
	}

	commit(): void {
		this.command({ kind: "commit" });
		this.state = "running";
	}

	holdOscInput(hold: boolean): void {
		this.command({ kind: "osc", hold });
	}
	setMouseReporting(enabled: boolean): void {
		this.command({ kind: "mouse", enabled });
	}

	setLease(lease: object | null): void {
		this.assertHealthy();
		if (this.lease === lease) return;
		this.lease = null;
		this.command({ kind: "lease", lease: lease ? ++this.leaseId : 0 });
		this.lease = lease;
	}

	isCurrent(notice: NativeCopyNotice): boolean {
		return (
			this.state === "running" &&
			Atomics.load(this.shared, 0) === 0 &&
			this.lease !== null &&
			notice.lease === this.lease &&
			notice.generation === this.generation &&
			notice.focus === Atomics.load(this.shared, 1) &&
			(notice.focus & 1) === 1 &&
			notice.registration !== 0 &&
			notice.registration === Atomics.load(this.shared, 2)
		);
	}

	async drain(maxMs: number, idleMs: number): Promise<void> {
		this.lease = null;
		this.command({ kind: "drain" });
		const end = performance.now() + maxMs;
		let activity = Atomics.load(this.shared, 3);
		let last = performance.now();
		while (performance.now() < end) {
			this.assertHealthy();
			const next = Atomics.load(this.shared, 3);
			if (next !== activity) {
				activity = next;
				last = performance.now();
			}
			if (performance.now() - last >= idleMs) break;
			await new Promise((resolve) => setTimeout(resolve, Math.min(idleMs, Math.max(1, end - performance.now()))));
		}
		this.command({ kind: "endDrain" });
	}

	stop(): void {
		if (this.state === "stopped") return;
		this.lease = null;
		this.command({ kind: "stop" });
		this.state = "stopped";
		this.port.close();
	}

	private command(command: InputCommand): void {
		this.assertHealthy();
		const ack = new Int32Array(new SharedArrayBuffer(8));
		const sequence = ++this.sequence;
		this.port.postMessage({ command, generation: this.generation, sequence, ack: ack.buffer });
		this.wait(ack, 0, () => Atomics.load(ack, 0) === sequence, command.kind);
		if (Atomics.load(ack, 1) !== 1) {
			this.failure = this.faultError(
				`macOS input ${command.kind} failed; input/native release is unproven. Exit this session; do not restart or hand off stdin.`,
			);
			this.state = "faulted";
			throw this.failure;
		}
	}

	private wait(words: Int32Array, index: number, done: () => boolean, operation: string): void {
		const deadline = performance.now() + DEADLINE_MS;
		while (!done()) {
			const remaining = deadline - performance.now();
			if (remaining <= 0 || Atomics.load(this.shared, 0) !== 0) {
				this.state = "faulted";
				this.failure = this.faultError(
					`macOS input ${operation} did not settle; ownership is unknown. Exit this session before attempting another terminal handoff.`,
				);
				throw this.failure;
			}
			Atomics.wait(words, index, Atomics.load(words, index), Math.min(remaining, 50));
		}
	}

	private faultError(
		reason: string,
		code = Atomics.load(this.shared, 0) === 2 ? "ERR_TERMINAL_INPUT_LOST" : undefined,
	): Error {
		const error: NodeJS.ErrnoException = new Error(reason);
		if (code) error.code = code;
		return error;
	}

	private assertHealthy(): void {
		if (this.failure) throw this.failure;
		if (Atomics.load(this.shared, 0) !== 0 || this.state === "stopped")
			throw this.faultError(
				"macOS input transport is not usable; do not restart or hand off stdin without proven release.",
			);
	}

	private fail(error: Error): void {
		if (this.state === "stopped" || this.failure) return;
		this.failure = error;
		this.state = "faulted";
		this.lease = null;
		this.onError(error);
	}

	private receive(message: InputMessage): void {
		if (this.state === "stopped") return;
		if (message.kind === "fault") {
			this.fail(this.faultError(message.reason, message.code));
			return;
		}
		if (this.state === "faulted") return;
		if (message.kind === "availability") {
			this.onAvailability(message.available, message.reason);
			return;
		}
		try {
			if (message.kind === "copy") {
				if (!this.lease || message.lease !== this.leaseId) return;
				const notice = { ...message, generation: this.generation, lease: this.lease };
				if (this.isCurrent(notice)) this.onCopy(notice);
			} else this.onInput(message.kind, message.data);
		} finally {
			if (this.state === "running")
				this.port.postMessage({ command: { kind: "consumed", bytes: message.bytes }, generation: this.generation });
		}
	}
}
