import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import lockfile from "proper-lockfile";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FileLastUsedModelsStorage, InMemoryLastUsedModelsStorage } from "../src/core/last-used-models.js";
import { FileSettingsStorage, InMemorySettingsStorage, SettingsManager } from "../src/core/settings-manager.js";

const directories: string[] = [];
function temporary() {
	const dir = mkdtempSync(join(tmpdir(), "directory-preferences-"));
	directories.push(dir);
	return dir;
}
afterEach(() => {
	vi.restoreAllMocks();
	for (const dir of directories.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function readSettings(storage: InMemorySettingsStorage) {
	let value = {};
	storage.withLock("global", (current) => {
		value = JSON.parse(current ?? "{}");
		return undefined;
	});
	return value;
}

describe("directory model settings composition", () => {
	it("keeps old factories private and two-argument selection global-only", async () => {
		const storage = new InMemorySettingsStorage();
		const first = SettingsManager.fromStorage(storage);
		first.setDefaultModelAndProvider("provider", "model", "/a");
		expect(first.getDirectoryDefaultModel("/a")).toEqual({ provider: "provider", modelId: "model" });
		await first.flush();
		const second = SettingsManager.fromStorage(storage);
		expect(second.getDirectoryDefaultModel("/a")).toBeUndefined();
		expect(second.getDefaultModel()).toBe("model");
		second.setDefaultModelAndProvider("other", "global");
		await second.flush();
		expect(second.getDirectoryDefaultModel("/a")).toBeUndefined();
		expect(SettingsManager.inMemory().getDirectoryDefaultModel("/a")).toBeUndefined();
	});

	it("composes only a profile-global file with defensive lexical lookup", async () => {
		const dir = temporary();
		const cwd = join(dir, "project");
		const profile = join(dir, "profile");
		const manager = SettingsManager.create(cwd, profile);
		expect(existsSync(profile)).toBe(false);
		manager.setDefaultModelAndProvider("p", "m", cwd);
		const identity = manager.getDirectoryDefaultModel(join(cwd, "child", ".."))!;
		identity.modelId = "mutated";
		expect(manager.getDirectoryDefaultModel(cwd)?.modelId).toBe("m");
		await manager.flush();
		expect(JSON.parse(readFileSync(join(profile, "last-used-models.json"), "utf8"))).toEqual({
			[cwd]: { provider: "p", modelId: "m" },
		});
		expect(JSON.parse(readFileSync(join(profile, "settings.json"), "utf8"))).toEqual({
			defaultProvider: "p",
			defaultModel: "m",
		});
		expect(existsSync(cwd)).toBe(false);
		expect(SettingsManager.create(cwd, profile).getDirectoryDefaultModel(cwd)?.modelId).toBe("m");
	});

	it("defaults to per-directory and changes lookup without changing recorded choices", async () => {
		const storage = new InMemorySettingsStorage();
		const recall = new InMemoryLastUsedModelsStorage();
		const manager = SettingsManager.fromStorage(storage, recall);
		expect(manager.getStartupModel()).toBe("per-directory");
		manager.setDefaultModelAndProvider("p", "first", "/a");
		await manager.flush();
		const before = recall.read();
		manager.setStartupModel("global");
		await manager.flush();
		expect(manager.getDirectoryDefaultModel("/a")).toBeUndefined();
		expect(recall.read()).toBe(before);
		manager.setDefaultModelAndProvider("p", "latest", "/a");
		await manager.flush();
		expect(manager.getDefaultModel()).toBe("latest");
		expect(manager.getDirectoryDefaultModel("/a")).toBeUndefined();
		expect(SettingsManager.fromStorage(storage, recall).getStartupModel()).toBe("global");
		const recorded = recall.read();
		manager.setStartupModel("per-directory");
		await manager.reload();
		expect(recall.read()).toBe(recorded);
		expect(manager.getDirectoryDefaultModel("/a")).toEqual({ provider: "p", modelId: "latest" });
	});

	it("keeps project startup policy effective while editing the global preference", async () => {
		const storage = new InMemorySettingsStorage();
		storage.withLock("project", () => JSON.stringify({ startupModel: "global" }));
		const manager = SettingsManager.fromStorage(storage);
		manager.setStartupModel("per-directory");
		await manager.flush();
		expect(manager.getStartupModel()).toBe("global");
		expect(readSettings(storage)).toMatchObject({ startupModel: "per-directory" });
		expect(manager.getProjectSettings()).toEqual({ startupModel: "global" });
		storage.withLock("project", () => "{}");
		await manager.reload();
		expect(manager.getStartupModel()).toBe("per-directory");
	});

	it("retains programmatic startup policy through selection, saves and reload", async () => {
		const manager = SettingsManager.inMemory();
		manager.applyOverrides({ startupModel: "global" });
		manager.setDefaultModelAndProvider("p", "m", "/a");
		manager.setStartupModel("per-directory");
		await manager.reload();
		expect(manager.getStartupModel()).toBe("global");
		expect(manager.getDirectoryDefaultModel("/a")).toBeUndefined();
		expect(manager.getGlobalSettings().startupModel).toBe("per-directory");
	});

	it.each([null, true, "directory", {}, []])(
		"diagnoses invalid startup policy %j without rewriting it",
		async (invalid) => {
			const storage = new InMemorySettingsStorage();
			storage.withLock("global", () => JSON.stringify({ startupModel: "global" }));
			storage.withLock("project", () => JSON.stringify({ startupModel: invalid }));
			const manager = SettingsManager.fromStorage(storage);
			expect(manager.getStartupModel()).toBe("global");
			expect(manager.drainErrors()).toMatchObject([{ scope: "project", error: expect.any(Error) }]);
			manager.applyOverrides({ startupModel: invalid } as never);
			expect(manager.getStartupModel()).toBe("global");
			expect(manager.drainErrors()[0].error.message).toContain("startupModel must be");
			await manager.reload();
			expect(manager.getStartupModel()).toBe("global");
			expect(manager.drainErrors()).toHaveLength(1);
			storage.withLock("project", (current) => {
				expect(JSON.parse(current!).startupModel).toEqual(invalid);
				return undefined;
			});
			storage.withLock("global", () => JSON.stringify({ startupModel: invalid }));
			storage.withLock("project", () => "{}");
			await manager.reload();
			expect(manager.getStartupModel()).toBe("per-directory");
			expect(manager.drainErrors()[0].scope).toBe("global");
		},
	);

	it.each(["defaultProvider", "defaultModel"] as const)(
		"suppresses the whole recall identity for defined project %s",
		async (field) => {
			const storage = new InMemorySettingsStorage();
			const recall = new InMemoryLastUsedModelsStorage();
			const manager = SettingsManager.fromStorage(storage, recall);
			manager.setDefaultModelAndProvider("p", "m", "/a");
			await manager.flush();
			storage.withLock("project", () => JSON.stringify({ [field]: "explicit" }));
			await manager.reload();
			expect(manager.getDirectoryDefaultModel("/a")).toBeUndefined();
		},
	);

	it.each(["defaultProvider", "defaultModel"] as const)(
		"retains defined programmatic %s authority across saves and reload",
		async (field) => {
			const recall = new InMemoryLastUsedModelsStorage();
			const manager = SettingsManager.inMemory({}, recall);
			manager.setDefaultModelAndProvider("p", "m", "/a");
			manager.applyOverrides({ [field]: "explicit" });
			manager.applyOverrides({ [field]: undefined });
			manager.setTheme("pi-dark");
			await manager.reload();
			expect(manager.getDirectoryDefaultModel("/a")).toBeUndefined();
			expect(field === "defaultModel" ? manager.getDefaultModel() : manager.getDefaultProvider()).toBe("explicit");
		},
	);

	it.each([
		[false, false],
		[true, false],
		[false, true],
		[true, true],
	])("independent writes: global failure %s, recall failure %s", async (globalFailure, recallFailure) => {
		const dir = temporary();
		const settingsPath = join(dir, "settings.json");
		const recallPath = join(dir, "last-used-models.json");
		writeFileSync(settingsPath, '{"defaultProvider":"old","defaultModel":"old"}');
		writeFileSync(recallPath, JSON.stringify({ [resolve("/a")]: { provider: "old", modelId: "old" } }));
		const storage = new FileSettingsStorage(dir, dir);
		const recall = new FileLastUsedModelsStorage(recallPath);
		const manager = SettingsManager.fromStorage(storage, recall);
		const order: string[] = [];
		const globalWrite = storage.withLock.bind(storage);
		vi.spyOn(storage, "withLock").mockImplementation((scope, update, write) => {
			order.push("global");
			if (globalFailure) throw new Error(`settings ${JSON.stringify(settingsPath)}: write failed`);
			globalWrite(scope, update, write);
		});
		const recallWrite = recall.withLock.bind(recall);
		vi.spyOn(recall, "withLock").mockImplementation(async (update) => {
			order.push("recall");
			if (recallFailure) throw new Error("write failed");
			await recallWrite(update);
		});
		manager.setDefaultModelAndProvider("new", "new", "/a");
		await manager.flush();
		expect(order).toEqual(["global", "recall"]);
		expect(JSON.parse(readFileSync(settingsPath, "utf8")).defaultModel).toBe(globalFailure ? "old" : "new");
		expect(JSON.parse(readFileSync(recallPath, "utf8"))[resolve("/a")].modelId).toBe(recallFailure ? "old" : "new");
		const errors = manager.drainErrors();
		expect(errors).toHaveLength(Number(globalFailure) + Number(recallFailure));
		expect(errors.every(({ scope }) => scope === "global")).toBe(true);
		if (globalFailure) expect(errors.some(({ error }) => error.message.includes(settingsPath))).toBe(true);
		if (recallFailure) expect(errors.some(({ error }) => error.message.includes(recallPath))).toBe(true);
		expect(manager.drainErrors()).toEqual([]);
		vi.restoreAllMocks();
		manager.setDefaultModelAndProvider("recovered", "recovered", "/a");
		await manager.flush();
		expect(JSON.parse(readFileSync(settingsPath, "utf8")).defaultModel).toBe("recovered");
		expect(JSON.parse(readFileSync(recallPath, "utf8"))[resolve("/a")].modelId).toBe("recovered");
	});

	it("settles recall before the next global selection and flush/reload, while retaining newest pending lookup", async () => {
		let release!: () => void;
		const gate = new Promise<void>((done) => {
			release = done;
		});
		const storage = new InMemorySettingsStorage();
		const recall = new InMemoryLastUsedModelsStorage();
		const original = recall.withLock.bind(recall);
		let entered!: () => void;
		const started = new Promise<void>((done) => {
			entered = done;
		});
		vi.spyOn(recall, "withLock").mockImplementationOnce(async (update) => {
			entered();
			await gate;
			await original(update);
		});
		const manager = SettingsManager.fromStorage(storage, recall);
		manager.setDefaultModelAndProvider("p", "first", "/a");
		manager.setDefaultModelAndProvider("p", "second", "/a");
		expect(manager.getDirectoryDefaultModel("/a")?.modelId).toBe("second");
		await started;
		expect(readSettings(storage)).toMatchObject({ defaultModel: "first" });
		let settled = false;
		const reload = manager.reload().then(() => {
			settled = true;
		});
		await Promise.resolve();
		expect(settled).toBe(false);
		release();
		await reload;
		expect(manager.getDirectoryDefaultModel("/a")?.modelId).toBe("second");
		expect(readSettings(storage)).toMatchObject({ defaultModel: "second" });
	});

	it("does not replay captured model fields in a later thinking/unrelated snapshot", async () => {
		const storage = new InMemorySettingsStorage();
		const manager = SettingsManager.fromStorage(storage);
		manager.setDefaultModelAndProvider("p", "first", "/a");
		manager.setDefaultThinkingLevel("high");
		const original = storage.withLock.bind(storage);
		let writes = 0;
		vi.spyOn(storage, "withLock").mockImplementation((scope, update) => {
			original(scope, update);
			if (++writes === 1)
				original("global", (current) =>
					JSON.stringify({ ...JSON.parse(current!), defaultProvider: "other", defaultModel: "other" }),
				);
		});
		await manager.flush();
		expect(readSettings(storage)).toMatchObject({
			defaultProvider: "other",
			defaultModel: "other",
			defaultThinkingLevel: "high",
		});
	});

	it.each(["settings.json", "last-used-models.json"])(
		"preserves malformed %s without blocking the independent file",
		async (broken) => {
			const dir = temporary();
			writeFileSync(join(dir, broken), "{private malformed data");
			const manager = SettingsManager.create(dir, dir);
			manager.drainErrors();
			manager.setDefaultModelAndProvider("p", "m", "/a");
			await manager.flush();
			expect(readFileSync(join(dir, broken), "utf8")).toBe("{private malformed data");
			const other = broken === "settings.json" ? "last-used-models.json" : "settings.json";
			expect(readFileSync(join(dir, other), "utf8")).toContain('"m"');
			const errors = manager.drainErrors();
			expect(errors.length).toBeGreaterThan(0);
			expect(errors.some(({ error }) => error.message.includes(join(dir, broken)))).toBe(true);
			expect(errors.every(({ error }) => !error.message.includes("private malformed data"))).toBe(true);
		},
	);

	it("reports a committed recall write's unlock failure without rollback or retry", async () => {
		const dir = temporary();
		const manager = SettingsManager.create(dir, dir);
		const original = lockfile.lock.bind(lockfile);
		vi.spyOn(lockfile, "lock").mockImplementation(async (...args) => {
			const release = await original(...args);
			return async () => {
				await release();
				throw new Error("unlock failed");
			};
		});
		manager.setDefaultModelAndProvider("p", "m", "/a");
		await manager.flush();
		expect(JSON.parse(readFileSync(join(dir, "last-used-models.json"), "utf8"))[resolve("/a")].modelId).toBe("m");
		expect(manager.drainErrors()[0].error.message).toContain("replacement committed");
		expect(lockfile.lock).toHaveBeenCalledOnce();
	});

	it("locks before reading a previously absent settings file and calls update once", () => {
		const dir = temporary();
		const path = join(dir, "settings.json");
		const storage = new FileSettingsStorage(dir, dir);
		vi.spyOn(lockfile, "lockSync").mockImplementation(() => {
			writeFileSync(path, '{"theme":"concurrent"}');
			return () => {};
		});
		const update = vi.fn((current) => {
			expect(JSON.parse(current!)).toEqual({ theme: "concurrent" });
			return current;
		});
		storage.withLock("global", update, true);
		expect(update).toHaveBeenCalledTimes(1);
	});
});
