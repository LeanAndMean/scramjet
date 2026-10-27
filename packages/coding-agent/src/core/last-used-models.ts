import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { mkdir, open, readFile, rename, unlink } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import lockfile from "proper-lockfile";

// SCRAMJET-DIVERGENCE: Keep directory model recall in a separate profile-global document.

export interface LastUsedModelIdentity {
	provider: string;
	modelId: string;
}

export interface LastUsedModelsStorage {
	read(): string | undefined;
	withLock(update: (current: string | undefined) => string): Promise<void>;
}

export class InMemoryLastUsedModelsStorage implements LastUsedModelsStorage {
	private content: string | undefined;
	private pending: Promise<void> = Promise.resolve();

	read(): string | undefined {
		return this.content;
	}

	withLock(update: (current: string | undefined) => string): Promise<void> {
		const operation = this.pending.then(() => {
			this.content = update(this.content);
		});
		this.pending = operation.catch(() => undefined);
		return operation;
	}
}

class RecallFileError extends Error {}

export class FileLastUsedModelsStorage implements LastUsedModelsStorage {
	readonly path: string;

	constructor(path: string) {
		this.path = resolve(path);
	}

	read(): string | undefined {
		try {
			return readFileSync(this.path, "utf8");
		} catch (error) {
			if (isMissing(error)) return undefined;
			throw new RecallFileError(`Last-used models ${JSON.stringify(this.path)}: read failed`);
		}
	}

	async withLock(update: (current: string | undefined) => string): Promise<void> {
		let release: (() => Promise<void>) | undefined;
		let temporaryPath: string | undefined;
		let committed = false;
		let operation = "mkdir";
		const failures: string[] = [];
		try {
			await mkdir(dirname(this.path), { recursive: true });
			operation = "lock";
			release = await lockfile.lock(this.path, {
				realpath: false,
				retries: { retries: 8, factor: 2, minTimeout: 25, maxTimeout: 1000 },
			});
			operation = "read";
			const current = await readFile(this.path, "utf8").catch((error: unknown) => {
				if (isMissing(error)) return undefined;
				throw error;
			});
			operation = "update/validate";
			const next = update(current);
			operation = "open";
			const candidatePath = join(dirname(this.path), `.${basename(this.path)}.${randomUUID()}.tmp`);
			const handle = await open(candidatePath, "wx", 0o600);
			temporaryPath = candidatePath;
			try {
				operation = "write";
				await handle.writeFile(next, "utf8");
			} finally {
				try {
					await handle.close();
				} catch {
					failures.push("close");
				}
			}
			if (failures.length === 0) {
				operation = "rename";
				await rename(temporaryPath, this.path);
				temporaryPath = undefined;
				committed = true;
			}
		} catch {
			failures.push(operation);
		} finally {
			if (temporaryPath) {
				try {
					await unlink(temporaryPath);
				} catch {
					failures.push("temporary cleanup");
				}
			}
			try {
				await release?.();
			} catch {
				failures.push("unlock");
			}
		}
		if (failures.length > 0) {
			throw new RecallFileError(
				`Last-used models ${JSON.stringify(this.path)}: ${failures.join(", ")} failed; ${committed ? "replacement committed" : "destination not replaced"}`,
			);
		}
	}
}

export class LastUsedModelsStore {
	private records: Record<string, unknown> = {};
	private readonly selections = new Map<string, LastUsedModelIdentity>();
	private pending: Promise<void> = Promise.resolve();
	private errors: Error[] = [];

	constructor(private readonly storage: LastUsedModelsStorage = new InMemoryLastUsedModelsStorage()) {
		this.load();
	}

	get(cwd: string): LastUsedModelIdentity | undefined {
		const key = resolve(cwd);
		const value = this.selections.get(key) ?? (Object.hasOwn(this.records, key) ? this.records[key] : undefined);
		return isIdentity(value) ? { provider: value.provider, modelId: value.modelId } : undefined;
	}

	set(cwd: string, identity: LastUsedModelIdentity, after: Promise<void> = Promise.resolve()): void {
		if (!isIdentity(identity)) {
			this.errors.push(new Error(`${this.label()}: update rejected an invalid model identity`));
			return;
		}
		const key = resolve(cwd);
		const captured = { provider: identity.provider, modelId: identity.modelId };
		this.selections.set(key, captured);
		this.pending = this.pending.then(async () => {
			await after;
			let updated: Record<string, unknown> | undefined;
			try {
				await this.storage.withLock((current) => {
					updated = this.parse(current);
					Object.defineProperty(updated, key, {
						value: captured,
						enumerable: true,
						configurable: true,
						writable: true,
					});
					return `${JSON.stringify(updated, null, 2)}\n`;
				});
				this.records = updated!;
			} catch (error) {
				this.report("update", error);
				Object.defineProperty(this.records, key, {
					value: captured,
					enumerable: true,
					configurable: true,
					writable: true,
				});
			} finally {
				if (this.selections.get(key) === captured) this.selections.delete(key);
			}
		});
	}

	async flush(): Promise<void> {
		await this.pending;
	}

	reload(): Promise<void> {
		this.pending = this.pending.then(() => this.load());
		return this.pending;
	}

	drainErrors(): Error[] {
		const errors = this.errors;
		this.errors = [];
		return errors;
	}

	private load(): void {
		try {
			this.records = this.parse(this.storage.read());
		} catch (error) {
			this.report("read/reload", error);
		}
	}

	private parse(content: string | undefined): Record<string, unknown> {
		if (content === undefined) return {};
		let value: unknown;
		try {
			value = JSON.parse(content);
		} catch {
			throw new Error("Invalid recall document");
		}
		if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid recall document");
		const records = value as Record<string, unknown>;
		if (Object.values(records).some((record) => !isIdentity(record))) {
			this.errors.push(
				new Error(`${this.label()}: validation ignored invalid entries; their stored values are preserved`),
			);
		}
		return records;
	}

	private label(): string {
		return this.storage instanceof FileLastUsedModelsStorage
			? `Last-used models ${JSON.stringify(this.storage.path)}`
			: "Last-used models";
	}

	private report(operation: string, error: unknown): void {
		this.errors.push(
			error instanceof RecallFileError
				? error
				: new Error(
						`${this.label()}: ${operation} failed; storage was not reset and the last valid state was retained`,
					),
		);
	}
}

function isIdentity(value: unknown): value is LastUsedModelIdentity {
	if (!value || typeof value !== "object" || Array.isArray(value)) return false;
	const identity = value as Partial<LastUsedModelIdentity>;
	return (
		Object.hasOwn(identity, "provider") &&
		Object.hasOwn(identity, "modelId") &&
		typeof identity.provider === "string" &&
		identity.provider.trim().length > 0 &&
		typeof identity.modelId === "string" &&
		identity.modelId.trim().length > 0
	);
}

function isMissing(error: unknown): boolean {
	return error instanceof Error && "code" in error && error.code === "ENOENT";
}
