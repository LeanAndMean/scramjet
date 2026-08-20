import { randomUUID } from "node:crypto";
import { mkdir, open, readFile, realpath, rename, unlink, writeFile } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import lockfile from "proper-lockfile";
import { OutputThroughputHistory, type OutputThroughputSample } from "./output-throughput.js";

// SCRAMJET-DIVERGENCE: Persist bounded, profile-scoped provider throughput across concurrent processes (#476).

const HISTORY_VERSION = 1;
const MAX_SAMPLES_PER_MODEL = 20;
const MAX_HISTORY_SAMPLES = 20000;
const MAX_STRING_LENGTH = 512;
const queues = new Map<string, Promise<void>>();

export interface OutputThroughputHistoryDiagnostic {
	operation: "refresh" | "lock" | "read" | "validate" | "write" | "rename" | "unlink" | "unlock";
	path: string;
	message: string;
	cause?: unknown;
}

interface HistoryDocument {
	version: 1;
	samples: OutputThroughputSample[];
}

export class OutputThroughputHistoryStore {
	private history = new OutputThroughputHistory();
	private queueKey?: Promise<string>;
	private pending: Promise<void> = Promise.resolve();
	private readonly reportedDiagnostics = new Set<string>();

	constructor(
		readonly path: string,
		private readonly onDiagnostic?: (diagnostic: OutputThroughputHistoryDiagnostic) => void,
	) {}

	snapshot(): OutputThroughputHistory {
		return historyFromSamples(this.history.allSamples());
	}

	async refresh(): Promise<OutputThroughputHistory> {
		await this.enqueue(async () => {
			const document = await this.readDocument("refresh");
			if (document) this.history = historyFromSamples(document.samples);
		});
		return this.snapshot();
	}

	submit(sample: OutputThroughputSample): void {
		if (!isValidSample(sample)) {
			this.report("validate", "Invalid output-throughput sample was not persisted; existing history was preserved");
			return;
		}
		const normalized = normalizeSample(sample);
		this.pending = this.enqueue(() => this.update(normalized));
	}

	async flush(): Promise<void> {
		await this.pending;
	}

	private async update(sample: OutputThroughputSample): Promise<void> {
		await mkdir(dirname(this.path), { recursive: true });
		const anchorPath = join(dirname(this.path), `${basename(this.path, ".json")}.lock`);
		try {
			await writeFile(anchorPath, "", { flag: "a" });
		} catch (cause) {
			this.report("lock", "Could not prepare the throughput-history lock; existing history was preserved", cause);
			return;
		}

		let release: (() => Promise<void>) | undefined;
		try {
			release = await lockfile.lock(anchorPath, {
				realpath: false,
				retries: { retries: 8, factor: 2, minTimeout: 25, maxTimeout: 1000, randomize: true },
				stale: 30000,
			});
			const current = await this.readDocument("read");
			if (!current) return;
			const samples = mergeSamples(current.samples, sample);
			const document: HistoryDocument = { version: HISTORY_VERSION, samples };
			const temporaryPath = join(dirname(this.path), `.${basename(this.path)}.${process.pid}.${randomUUID()}.tmp`);
			let temporaryCreated = false;
			try {
				const handle = await open(temporaryPath, "wx", 0o600);
				temporaryCreated = true;
				try {
					await handle.writeFile(`${JSON.stringify(document, null, 2)}\n`, "utf8");
					await handle.sync();
				} finally {
					await handle.close();
				}
				await rename(temporaryPath, this.path);
				temporaryCreated = false;
				this.history = historyFromSamples(samples);
			} catch (cause) {
				this.report("write", "Could not commit output-throughput history; existing history was preserved", cause);
			} finally {
				if (temporaryCreated) {
					await unlink(temporaryPath).catch((cause) => {
						this.report("unlink", "Could not remove the temporary output-throughput history file", cause);
					});
				}
			}
		} catch (cause) {
			this.report("lock", "Could not lock output-throughput history; existing history was preserved", cause);
		} finally {
			await release?.().catch((cause) => {
				this.report("unlock", "Could not release the output-throughput history lock", cause);
			});
		}
	}

	private async readDocument(operation: "refresh" | "read"): Promise<HistoryDocument | undefined> {
		let raw: string;
		try {
			raw = await readFile(this.path, "utf8");
		} catch (cause) {
			if (isMissingFile(cause)) return { version: HISTORY_VERSION, samples: [] };
			this.report(
				operation,
				"Could not read output-throughput history; the last valid snapshot was preserved",
				cause,
			);
			return undefined;
		}
		try {
			const value: unknown = JSON.parse(raw);
			if (!isHistoryDocument(value)) throw new Error("unsupported version or invalid schema");
			return { version: HISTORY_VERSION, samples: value.samples.map(normalizeSample) };
		} catch (cause) {
			this.report(
				"validate",
				"Output-throughput history is malformed or unsupported; original bytes and the last valid snapshot were preserved",
				cause,
			);
			return undefined;
		}
	}

	private async enqueue(operation: () => Promise<void>): Promise<void> {
		try {
			if (!this.queueKey) this.queueKey = canonicalPath(this.path);
			const queueKey = this.queueKey;
			let key: string;
			try {
				key = await queueKey;
			} catch (cause) {
				if (this.queueKey === queueKey) this.queueKey = undefined;
				throw cause;
			}
			const previous = queues.get(key) ?? Promise.resolve();
			const current = previous.catch(() => undefined).then(operation);
			queues.set(key, current);
			try {
				await current;
			} finally {
				if (queues.get(key) === current) queues.delete(key);
			}
		} catch (cause) {
			this.report("write", "Output-throughput history operation failed; existing history was preserved", cause);
		}
	}

	private report(operation: OutputThroughputHistoryDiagnostic["operation"], message: string, cause?: unknown): void {
		const normalizedCause = cause instanceof Error ? cause.message : cause === undefined ? "" : String(cause);
		const key = `${resolve(this.path)}\0${operation}\0${normalizedCause}`;
		if (this.reportedDiagnostics.has(key)) return;
		this.reportedDiagnostics.add(key);
		this.onDiagnostic?.({ operation, path: this.path, message, cause });
	}
}

async function canonicalPath(path: string): Promise<string> {
	await mkdir(dirname(path), { recursive: true });
	const parent = await realpath(dirname(path));
	return join(parent, basename(path));
}

function isHistoryDocument(value: unknown): value is HistoryDocument {
	if (!value || typeof value !== "object") return false;
	const document = value as { version?: unknown; samples?: unknown };
	return (
		document.version === HISTORY_VERSION &&
		Array.isArray(document.samples) &&
		document.samples.length <= MAX_HISTORY_SAMPLES &&
		document.samples.every(isValidSample)
	);
}

function isValidSample(value: unknown): value is OutputThroughputSample {
	if (!value || typeof value !== "object") return false;
	const sample = value as Partial<OutputThroughputSample>;
	return (
		isBoundedString(sample.provider) &&
		isBoundedString(sample.model) &&
		(sample.responseModel === undefined || isBoundedString(sample.responseModel)) &&
		isPositiveFinite(sample.outputTokens) &&
		isPositiveFinite(sample.durationMs) &&
		Number.isFinite(sample.observedAt) &&
		(sample.observedAt ?? -1) >= 0
	);
}

function normalizeSample(sample: OutputThroughputSample): OutputThroughputSample {
	return {
		provider: sample.provider,
		model: sample.model,
		...(sample.responseModel === undefined ? {} : { responseModel: sample.responseModel }),
		outputTokens: sample.outputTokens,
		durationMs: sample.durationMs,
		observedAt: sample.observedAt,
	};
}

function isBoundedString(value: unknown): value is string {
	return typeof value === "string" && value.length > 0 && value.length <= MAX_STRING_LENGTH;
}

function isPositiveFinite(value: unknown): value is number {
	return typeof value === "number" && Number.isFinite(value) && value > 0;
}

function mergeSamples(
	samples: readonly OutputThroughputSample[],
	added: OutputThroughputSample,
): OutputThroughputSample[] {
	const unique = new Map<string, OutputThroughputSample>();
	for (const sample of [...samples, added]) unique.set(sampleIdentity(sample), normalizeSample(sample));
	const byModel = new Map<string, OutputThroughputSample[]>();
	for (const sample of unique.values()) {
		const key = `${sample.provider.length}:${sample.provider}${sample.model}`;
		const group = byModel.get(key) ?? [];
		group.push(sample);
		byModel.set(key, group);
	}
	return [...byModel.values()]
		.flatMap((group) => group.sort((left, right) => left.observedAt - right.observedAt).slice(-MAX_SAMPLES_PER_MODEL))
		.sort((left, right) => left.observedAt - right.observedAt)
		.slice(-MAX_HISTORY_SAMPLES);
}

function sampleIdentity(sample: OutputThroughputSample): string {
	return JSON.stringify([
		sample.provider,
		sample.model,
		sample.responseModel ?? null,
		sample.outputTokens,
		sample.durationMs,
		sample.observedAt,
	]);
}

function historyFromSamples(samples: readonly OutputThroughputSample[]): OutputThroughputHistory {
	const history = new OutputThroughputHistory();
	for (const sample of samples) history.add(sample);
	return history;
}

function isMissingFile(cause: unknown): boolean {
	return cause instanceof Error && "code" in cause && cause.code === "ENOENT";
}
