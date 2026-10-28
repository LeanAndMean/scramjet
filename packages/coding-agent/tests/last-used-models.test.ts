import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import * as fs from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { inspect } from "node:util";
import lockfile from "proper-lockfile";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
	FileLastUsedModelsStorage,
	InMemoryLastUsedModelsStorage,
	type LastUsedModelsStorage,
	LastUsedModelsStore,
} from "../src/core/last-used-models.js";

vi.mock("node:fs/promises", async (importOriginal) => ({
	...(await importOriginal<typeof import("node:fs/promises")>()),
}));

const directories: string[] = [];
const identity = (modelId: string) => ({ provider: " provider/exact ", modelId });

async function temporaryPath() {
	const directory = await fs.mkdtemp(join(tmpdir(), "last-used-models-"));
	directories.push(directory);
	return join(directory, "profile", "last-used-models.json");
}

function barrier() {
	let release!: () => void;
	const promise = new Promise<void>((resolve) => {
		release = resolve;
	});
	return { promise, release };
}

async function seed(path: string, bytes: string) {
	await fs.mkdir(dirname(path), { recursive: true });
	await fs.writeFile(path, bytes);
}

afterEach(async () => {
	vi.restoreAllMocks();
	await Promise.all(directories.splice(0).map((directory) => fs.rm(directory, { recursive: true, force: true })));
});

describe("LastUsedModelsStore", () => {
	it("reads missing files without creating a profile, lock, or CWD files", async () => {
		const path = await temporaryPath();
		const store = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
		expect(store.get("project")).toBeUndefined();
		await store.reload();
		expect(existsSync(dirname(path))).toBe(false);
		expect(store.drainErrors()).toEqual([]);
	});

	it("uses exact lexical CWDs and defensive identities in private memory", async () => {
		const first = new LastUsedModelsStore();
		const second = new LastUsedModelsStore();
		const submitted = identity(" model/one:exact ");
		first.set("project/child/..", submitted);
		submitted.modelId = "changed";
		expect(first.get(resolve("project"))).toEqual(identity(" model/one:exact "));
		expect(first.get("project/child")).toBeUndefined();
		expect(first.get("sibling")).toBeUndefined();
		expect(second.get("project")).toBeUndefined();
		const copy = first.get("project")!;
		copy.provider = "changed";
		await first.flush();
		expect(first.get("project")).toEqual(identity(" model/one:exact "));
		await first.reload();
		expect(first.get("project")).toEqual(identity(" model/one:exact "));
	});

	it("round-trips the directory-keyed document with private file permissions", async () => {
		const path = await temporaryPath();
		const store = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
		store.set("project", identity("one"));
		store.set("project/child", identity("two"));
		store.set("sibling", identity("three"));
		await store.flush();
		expect(store.drainErrors()).toEqual([]);
		expect(JSON.parse(readFileSync(path, "utf8"))).toEqual({
			[resolve("project")]: identity("one"),
			[resolve("project/child")]: identity("two"),
			[resolve("sibling")]: identity("three"),
		});
		expect(statSync(path).mode & 0o777).toBe(0o600);
		expect(readdirSync(dirname(path))).toEqual(["last-used-models.json"]);
		const reloaded = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
		expect(reloaded.get("project")).toEqual(identity("one"));
	});

	it.each(["", "{broken SECRET", "null", "[]", "42", '"text"'])(
		"preserves invalid whole documents and reports every blocked attempt: %j",
		async (bytes) => {
			const path = await temporaryPath();
			await seed(path, bytes);
			const store = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
			const initial = store.drainErrors();
			expect(initial).toHaveLength(1);
			if (bytes === "" || bytes.startsWith("{broken")) {
				expect(inspect(initial, { depth: null })).toContain("SyntaxError");
				expect(inspect(initial, { depth: null })).toContain("JSON.parse");
			}
			for (const model of ["one", "two"]) {
				store.set("project", identity(model));
				await store.flush();
				expect(readFileSync(path, "utf8")).toBe(bytes);
				const errors = store.drainErrors();
				expect(errors).toHaveLength(1);
				expect(errors[0]?.message).toContain("update");
				if (bytes === "" || bytes.startsWith("{broken")) {
					expect(inspect(errors, { depth: null })).toContain("SyntaxError");
					expect(inspect(errors, { depth: null })).toContain("JSON.parse");
				}
			}
		},
	);

	it("ignores invalid entries, preserves siblings, and repairs only a deliberately selected entry", async () => {
		const path = await temporaryPath();
		const bad = { provider: " ", modelId: "SECRET" };
		const original = { [resolve("bad")]: bad, [resolve("good")]: identity("good"), __proto__: null };
		await seed(path, JSON.stringify(original));
		const store = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
		expect(store.get("bad")).toBeUndefined();
		expect(store.get("good")).toEqual(identity("good"));
		const warnings = store.drainErrors();
		expect(warnings).toHaveLength(1);
		expect(warnings[0]?.message).toContain("ignored invalid entries");
		expect(inspect(warnings, { depth: null })).not.toContain("SECRET");
		store.set("new", identity("new"));
		await store.flush();
		expect(JSON.parse(readFileSync(path, "utf8"))[resolve("bad")]).toEqual(bad);
		store.set("bad", identity("repaired"));
		await store.flush();
		expect(JSON.parse(readFileSync(path, "utf8"))).toEqual({
			...original,
			[resolve("new")]: identity("new"),
			[resolve("bad")]: identity("repaired"),
		});
	});

	it("uses own-property lookup and assignment without prototype mutation", async () => {
		const storage = new InMemoryLastUsedModelsStorage();
		await storage.withLock(() => '{"__proto__":{"provider":"p","modelId":"m"}}');
		const store = new LastUsedModelsStore(storage);
		expect(store.get("toString")).toBeUndefined();
		store.set("__proto__", identity("safe"));
		await store.flush();
		expect(Object.hasOwn(JSON.parse(storage.read()!), "__proto__")).toBe(true);
		expect(store.get("__proto__")).toEqual(identity("safe"));
	});

	it("preserves the last valid state on failed reload and recovers on later valid reload", async () => {
		let raw: string | undefined = JSON.stringify({ [resolve("project")]: identity("old") });
		const storage: LastUsedModelsStorage = {
			read: () => raw,
			withLock: async (update) => {
				raw = update(raw);
			},
		};
		const store = new LastUsedModelsStore(storage);
		raw = "broken";
		await store.reload();
		expect(store.get("project")).toEqual(identity("old"));
		expect(store.drainErrors()).toHaveLength(1);
		raw = JSON.stringify({ [resolve("project")]: identity("new") });
		await store.reload();
		expect(store.get("project")).toEqual(identity("new"));
	});

	it("reads after acquiring the first-creation lock, preserving a competing writer", async () => {
		const path = await temporaryPath();
		const realLock = lockfile.lock.bind(lockfile);
		vi.spyOn(lockfile, "lock").mockImplementationOnce(async (target, options) => {
			expect(existsSync(path)).toBe(false);
			expect(options).toMatchObject({ realpath: false, retries: expect.any(Object) });
			await fs.writeFile(path, JSON.stringify({ [resolve("other")]: identity("competing") }));
			return realLock(target, options);
		});
		const store = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
		store.set("project", identity("selected"));
		await store.flush();
		expect(store.drainErrors()).toEqual([]);
		expect(JSON.parse(readFileSync(path, "utf8"))).toEqual({
			[resolve("other")]: identity("competing"),
			[resolve("project")]: identity("selected"),
		});
	});

	it.each([false, true])("serializes two stores with explicit barriers (same key: %s)", async (sameKey) => {
		const path = await temporaryPath();
		const held = barrier();
		const entered = barrier();
		const waiting = barrier();
		const realLock = lockfile.lock.bind(lockfile);
		let calls = 0;
		vi.spyOn(lockfile, "lock").mockImplementation((target, options) => {
			if (++calls === 2) waiting.release();
			return realLock(target, options);
		});
		const realRename = fs.rename;
		vi.spyOn(fs, "rename").mockImplementationOnce(async (...args) => {
			entered.release();
			await held.promise;
			await realRename(...args);
		});
		const first = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
		const second = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
		let unexpectedRelease: (() => Promise<void>) | undefined;
		first.set("one", identity("first"));
		try {
			await entered.promise;
			await expect(
				realLock(path, { realpath: false, retries: 0 }).then((release) => {
					unexpectedRelease = release;
				}),
			).rejects.toMatchObject({ code: "ELOCKED" });
			second.set(sameKey ? "one" : "two", identity("second"));
			await waiting.promise;
			held.release();
			await Promise.all([first.flush(), second.flush()]);
			expect(first.drainErrors()).toEqual([]);
			expect(second.drainErrors()).toEqual([]);
			expect(JSON.parse(readFileSync(path, "utf8"))).toEqual(
				sameKey
					? {
							[resolve("one")]: identity("second"),
						}
					: {
							[resolve("one")]: identity("first"),
							[resolve("two")]: identity("second"),
						},
			);
		} finally {
			held.release();
			try {
				await unexpectedRelease?.();
			} finally {
				await Promise.all([first.flush(), second.flush()]);
			}
		}
	});

	it("captures queued updates and never regresses a newer pending lookup", async () => {
		const entered = barrier();
		const held = barrier();
		const secondEntered = barrier();
		const secondHeld = barrier();
		let raw: string | undefined;
		let calls = 0;
		const store = new LastUsedModelsStore({
			read: () => raw,
			withLock: async (update) => {
				calls++;
				if (calls === 1) {
					entered.release();
					await held.promise;
				}
				if (calls === 2) {
					secondEntered.release();
					await secondHeld.promise;
				}
				raw = update(raw);
			},
		});
		store.set("one", identity("old"));
		await entered.promise;
		store.set("one", identity("new"));
		store.set("two", identity("sibling"));
		held.release();
		await secondEntered.promise;
		expect(store.get("one")).toEqual(identity("new"));
		secondHeld.release();
		await store.flush();
		expect(JSON.parse(raw!)).toEqual({ [resolve("one")]: identity("new"), [resolve("two")]: identity("sibling") });
	});

	it("retains original read and update failures without losing valid state", async () => {
		let fail = false;
		const cause = new Error("underlying failure");
		const readFailure = Object.assign(new Error("read sentinel", { cause }), { code: "EREAD" });
		const updateFailure = Object.assign(new Error("update sentinel", { cause }), { code: "EUPDATE" });
		const store = new LastUsedModelsStore({
			read: () => {
				if (fail) throw readFailure;
				return JSON.stringify({ [resolve("one")]: identity("one") });
			},
			withLock: async () => {
				throw updateFailure;
			},
		});
		fail = true;
		await store.reload();
		expect(store.get("one")).toEqual(identity("one"));
		store.set("two", identity("two"));
		await store.flush();
		const errors = store.drainErrors();
		expect(errors).toHaveLength(2);
		expect(errors[0].cause).toBe(readFailure);
		expect(errors[1].cause).toBe(updateFailure);
		for (const failure of [readFailure, updateFailure]) {
			expect(inspect(errors, { depth: null })).toContain(failure.stack!.split("\n")[1].trim());
			expect(inspect(errors, { depth: null })).toContain(failure.code);
		}
		expect(readFailure.cause).toBe(cause);
	});

	it.each(["mkdir", "lock", "read", "open", "write", "close", "rename"])(
		"preserves bytes and cleans temporary files after %s failure",
		async (operation) => {
			const path = await temporaryPath();
			const bytes = JSON.stringify({ [resolve("one")]: identity("old") });
			await seed(path, bytes);
			const store = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
			const failure = Object.assign(new Error("operation sentinel", { cause: new Error("root cause") }), {
				code: "EACCES",
			});
			if (operation === "lock") vi.spyOn(lockfile, "lock").mockRejectedValueOnce(failure);
			else if (operation === "write" || operation === "close") {
				const realOpen = fs.open;
				vi.spyOn(fs, "open").mockImplementationOnce(async (...args) => {
					const handle = await realOpen(...args);
					if (operation === "write") vi.spyOn(handle, "writeFile").mockRejectedValueOnce(failure);
					else {
						const realClose = handle.close.bind(handle);
						vi.spyOn(handle, "close").mockImplementationOnce(async () => {
							await realClose();
							throw failure;
						});
					}
					return handle;
				});
			} else if (operation === "read") vi.spyOn(fs, "readFile").mockRejectedValueOnce(failure);
			else if (operation === "mkdir") vi.spyOn(fs, "mkdir").mockRejectedValueOnce(failure);
			else if (operation === "open") vi.spyOn(fs, "open").mockRejectedValueOnce(failure);
			else vi.spyOn(fs, "rename").mockRejectedValueOnce(failure);
			store.set("one", identity("new"));
			await store.flush();
			expect(readFileSync(path, "utf8")).toBe(bytes);
			expect(readdirSync(dirname(path))).toEqual(["last-used-models.json"]);
			const errors = store.drainErrors();
			expect(errors).toHaveLength(1);
			expect(errors[0]?.message).toContain(path);
			expect((errors[0].cause as Error).cause).toBe(failure);
			expect(inspect(errors, { depth: null })).toContain(failure.stack!.split("\n")[1].trim());
			expect(inspect(errors, { depth: null })).toContain("EACCES");
			expect(inspect(errors, { depth: null })).toContain("root cause");
		},
	);

	it("reports temporary cleanup failure separately from a failed rename", async () => {
		const path = await temporaryPath();
		await seed(path, "{}");
		const store = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
		const renameFailure = Object.assign(new Error("rename sentinel"), { code: "ERENAME" });
		const cleanupFailure = { code: "ECLEANUP", detail: "cleanup sentinel" };
		vi.spyOn(fs, "rename").mockRejectedValueOnce(renameFailure);
		vi.spyOn(fs, "unlink").mockRejectedValueOnce(cleanupFailure);
		store.set("one", identity("new"));
		await store.flush();
		expect(readFileSync(path, "utf8")).toBe("{}");
		const errors = store.drainErrors();
		expect(errors).toHaveLength(1);
		expect(errors[0]?.message).toContain("rename failed, temporary cleanup failed");
		expect(errors[0]?.message).toContain("destination not replaced");
		expect(errors[0]?.cause).toBeInstanceOf(AggregateError);
		const causes = (errors[0].cause as AggregateError).errors;
		expect(causes).toHaveLength(2);
		expect(causes[0].cause).toBe(renameFailure);
		expect(causes[1].cause).toBe(cleanupFailure);
		expect(inspect(causes, { depth: null })).toContain(renameFailure.stack!.split("\n")[1].trim());
		expect(inspect(causes, { depth: null })).toContain("ERENAME");
		expect(inspect(causes, { depth: null })).toContain("cleanup sentinel");
		expect(readdirSync(dirname(path)).filter((name) => name.endsWith(".tmp"))).toHaveLength(1);
		expect(existsSync(`${path}.lock`)).toBe(false);
	});

	it("retains simultaneous write, close, cleanup, and unlock failures without replacing the destination", async () => {
		const path = await temporaryPath();
		await seed(path, "{}");
		const store = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
		const writeFailure = Object.assign(new Error("write sentinel"), { code: "EWRITE" });
		const closeFailure = new Error("close sentinel");
		const cleanupFailure = { detail: "cleanup sentinel" };
		const unlockFailure = "unlock sentinel";
		const realOpen = fs.open;
		vi.spyOn(fs, "open").mockImplementationOnce(async (...args) => {
			const handle = await realOpen(...args);
			vi.spyOn(handle, "writeFile").mockRejectedValueOnce(writeFailure);
			const close = handle.close.bind(handle);
			vi.spyOn(handle, "close").mockImplementationOnce(async () => {
				await close();
				throw closeFailure;
			});
			return handle;
		});
		vi.spyOn(fs, "unlink").mockRejectedValueOnce(cleanupFailure);
		const realLock = lockfile.lock.bind(lockfile);
		vi.spyOn(lockfile, "lock").mockImplementationOnce(async (target, options) => {
			const release = await realLock(target, options);
			return async () => {
				await release();
				throw unlockFailure;
			};
		});
		store.set("one", identity("new"));
		await store.flush();
		expect(readFileSync(path, "utf8")).toBe("{}");
		expect(existsSync(`${path}.lock`)).toBe(false);
		const errors = store.drainErrors();
		expect(errors).toHaveLength(1);
		expect(errors[0].message).toContain("destination not replaced");
		expect(errors[0].cause).toBeInstanceOf(AggregateError);
		const causes = (errors[0].cause as AggregateError).errors;
		expect(causes).toHaveLength(4);
		expect(causes[0].cause).toBe(closeFailure);
		expect(causes[1].cause).toBe(writeFailure);
		expect(causes[2].cause).toBe(cleanupFailure);
		expect(causes[3].cause).toBe(unlockFailure);
		const diagnostic = inspect(causes, { depth: null });
		expect(diagnostic).toContain(writeFailure.stack!.split("\n")[1].trim());
		expect(diagnostic).toContain("EWRITE");
		expect(diagnostic).toContain(closeFailure.stack!.split("\n")[1].trim());
		expect(diagnostic).toContain("cleanup sentinel");
		expect(diagnostic).toContain(unlockFailure);
	});

	it("reports file read errors with safe path and operation information", async () => {
		const path = await temporaryPath();
		await fs.mkdir(path, { recursive: true });
		const store = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
		const errors = store.drainErrors();
		expect(errors).toHaveLength(1);
		expect(errors[0]?.message).toContain(path);
		expect(errors[0]?.message).toContain("read failed");
	});

	it.each([null, [], {}, { provider: "p" }, { provider: 3, modelId: "m" }, { provider: "p", modelId: " " }])(
		"ignores malformed records without erasing their values: %j",
		async (bad) => {
			const storage = new InMemoryLastUsedModelsStorage();
			await storage.withLock(() => JSON.stringify({ [resolve("bad")]: bad }));
			const store = new LastUsedModelsStore(storage);
			expect(store.get("bad")).toBeUndefined();
			expect(store.drainErrors()).toHaveLength(1);
			store.set("other", identity("good"));
			await store.flush();
			expect(JSON.parse(storage.read()!)[resolve("bad")]).toEqual(bad);
		},
	);

	it("settles queued writes before reload and flush without filesystem access", async () => {
		const held = barrier();
		const entered = barrier();
		let raw: string | undefined;
		const read = vi.fn(() => raw);
		const mkdir = vi.spyOn(fs, "mkdir");
		const lock = vi.spyOn(lockfile, "lock");
		const store = new LastUsedModelsStore({
			read,
			withLock: async (update) => {
				entered.release();
				await held.promise;
				raw = update(raw);
			},
		});
		store.set("one", identity("new"));
		await entered.promise;
		let settled = false;
		const reload = store.reload();
		const flush = store.flush().then(() => {
			settled = true;
		});
		await Promise.resolve();
		expect(settled).toBe(false);
		expect(read).toHaveBeenCalledTimes(1);
		held.release();
		await Promise.all([reload, flush]);
		expect(settled).toBe(true);
		expect(read).toHaveBeenCalledTimes(2);
		expect(store.get("one")).toEqual(identity("new"));
		expect(mkdir).not.toHaveBeenCalled();
		expect(lock).not.toHaveBeenCalled();
	});

	it("retains committed data and truthfully reports post-rename unlock failure without retry", async () => {
		const path = await temporaryPath();
		const failure = Object.assign(new Error("unlock sentinel"), { code: "EUNLOCK" });
		const realLock = lockfile.lock.bind(lockfile);
		const lock = vi.spyOn(lockfile, "lock").mockImplementationOnce(async (target, options) => {
			const release = await realLock(target, options);
			return async () => {
				await release();
				throw failure;
			};
		});
		const store = new LastUsedModelsStore(new FileLastUsedModelsStorage(path));
		store.set("one", identity("new"));
		await store.flush();
		expect(JSON.parse(readFileSync(path, "utf8"))).toEqual({ [resolve("one")]: identity("new") });
		expect(store.get("one")).toEqual(identity("new"));
		expect(lock).toHaveBeenCalledTimes(1);
		const errors = store.drainErrors();
		expect(errors).toHaveLength(1);
		expect(errors[0]?.message).toContain("committed");
		expect((errors[0].cause as Error).cause).toBe(failure);
	});

	it.each([undefined, null, false, 0, "storage refused", 17, { code: "CUSTOM", detail: "storage refused" }])(
		"preserves non-Error storage failures: %j",
		async (failure) => {
			const store = new LastUsedModelsStore({
				read: () => {
					throw failure;
				},
				withLock: async () => {
					throw failure;
				},
			});
			const initial = store.drainErrors();
			store.set("one", identity("new"));
			await store.flush();
			for (const errors of [initial, store.drainErrors()]) {
				expect(errors).toHaveLength(1);
				expect(errors[0]).toHaveProperty("cause", failure);
				expect(errors[0].cause).toBe(failure);
			}
			expect(store.get("one")).toEqual(identity("new"));
		},
	);

	it.each([
		{ provider: "", modelId: "m" },
		{ provider: "p", modelId: " \n " },
	])("rejects invalid selected identities without modifying storage", async (invalid) => {
		const storage = new InMemoryLastUsedModelsStorage();
		const store = new LastUsedModelsStore(storage);
		store.set("one", invalid);
		await store.flush();
		expect(storage.read()).toBeUndefined();
		expect(store.drainErrors()).toHaveLength(1);
	});
});
