import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { InMemorySettingsStorage, type Settings, SettingsManager } from "../src/core/settings-manager.js";

function storageWith(global: unknown = {}, project: unknown = {}) {
	const storage = new InMemorySettingsStorage();
	storage.withLock("global", () => JSON.stringify(global));
	storage.withLock("project", () => JSON.stringify(project));
	return storage;
}

function storedGlobal(storage: InMemorySettingsStorage) {
	let value: unknown;
	storage.withLock("global", (current) => {
		value = JSON.parse(current!);
		return undefined;
	});
	return value;
}

describe("Markdown tool display settings", () => {
	it("defaults to rendered and accepts both explicit modes", () => {
		expect(SettingsManager.inMemory().getMarkdownToolDisplay()).toBe("rendered");
		for (const toolDisplay of ["rendered", "source"] as const) {
			const manager = SettingsManager.inMemory({ markdown: { toolDisplay, codeBlockIndent: "    " } });
			expect(manager.getMarkdownToolDisplay()).toBe(toolDisplay);
			expect(manager.getCodeBlockIndent()).toBe("    ");
			expect(manager.drainErrors()).toEqual([]);
		}
	});

	it.each([null, false, 1, "SOURCE", "", [], {}])("ignores invalid leaf %j per scope", (toolDisplay) => {
		const global = SettingsManager.fromStorage(storageWith({ markdown: { toolDisplay } }));
		expect(global.getMarkdownToolDisplay()).toBe("rendered");
		expect(global.getGlobalSettings().markdown).toEqual({});
		expect(global.drainErrors()).toEqual([
			{
				scope: "global",
				error: expect.objectContaining({ message: expect.stringContaining("markdown.toolDisplay") }),
			},
		]);
		const project = SettingsManager.fromStorage(
			storageWith({ markdown: { toolDisplay: "source" } }, { markdown: { toolDisplay, codeBlockIndent: "\t" } }),
		);
		expect(project.getMarkdownToolDisplay()).toBe("source");
		expect(project.getCodeBlockIndent()).toBe("\t");
		expect(project.getProjectSettings().markdown).toEqual({ codeBlockIndent: "\t" });
		expect(project.drainErrors()).toEqual([{ scope: "project", error: expect.any(Error) }]);
	});

	it.each([null, false, 12, "source", []])(
		"ignores malformed Markdown container %j without repairing storage",
		async (markdown) => {
			const storage = storageWith({ markdown }, { markdown });
			const manager = SettingsManager.fromStorage(storage);
			expect(manager.getMarkdownToolDisplay()).toBe("rendered");
			expect(manager.getCodeBlockIndent()).toBe("  ");
			expect(manager.drainErrors().map(({ scope }) => scope)).toEqual(["global", "project"]);
			manager.setTheme("custom");
			await manager.flush();
			expect(storedGlobal(storage)).toEqual({ markdown, theme: "custom" });
			manager.setMarkdownToolDisplay("source");
			await manager.flush();
			expect(storedGlobal(storage)).toEqual({ markdown, theme: "custom" });
			expect(manager.drainErrors()).toEqual([{ scope: "global", error: expect.any(Error) }]);
		},
	);

	it("normalizes cloned overrides without erasing valid lower values or mutating callers", () => {
		const manager = SettingsManager.inMemory({ markdown: { toolDisplay: "source", codeBlockIndent: "\t" } });
		const overrides = { markdown: { toolDisplay: "invalid", codeBlockIndent: "    " } };
		manager.applyOverrides(overrides as Partial<Settings>);
		expect(manager.getMarkdownToolDisplay()).toBe("source");
		expect(manager.getCodeBlockIndent()).toBe("    ");
		expect(overrides.markdown.toolDisplay).toBe("invalid");
		expect(manager.drainErrors()).toEqual([{ scope: "global", error: expect.any(Error) }]);
		manager.applyOverrides({ markdown: null } as unknown as Partial<Settings>);
		expect(manager.getMarkdownToolDisplay()).toBe("source");
		expect(manager.drainErrors()).toHaveLength(1);
		manager.applyOverrides({ markdown: { toolDisplay: "rendered" } });
		expect(manager.getMarkdownToolDisplay()).toBe("rendered");
	});

	it("persists only the global leaf and preserves nested siblings and concurrent disk edits", async () => {
		const storage = storageWith({ markdown: { codeBlockIndent: "\t", unknown: "initial" } });
		const manager = SettingsManager.fromStorage(storage);
		storage.withLock("global", () =>
			JSON.stringify({
				markdown: { codeBlockIndent: "    ", unknown: "concurrent" },
				theme: "concurrent-theme",
			}),
		);
		manager.setMarkdownToolDisplay("source");
		await manager.flush();
		expect(storedGlobal(storage)).toEqual({
			markdown: { codeBlockIndent: "    ", unknown: "concurrent", toolDisplay: "source" },
			theme: "concurrent-theme",
		});
		const reopened = SettingsManager.fromStorage(storage);
		expect(reopened.getMarkdownToolDisplay()).toBe("source");
		expect(reopened.getCodeBlockIndent()).toBe("    ");
	});

	it("keeps project precedence while saving global changes and queued unrelated settings", async () => {
		const storage = storageWith({}, { markdown: { toolDisplay: "rendered" } });
		const manager = SettingsManager.fromStorage(storage);
		manager.setMarkdownToolDisplay("source");
		manager.setShowImages(false);
		await manager.flush();
		expect(manager.getMarkdownToolDisplay()).toBe("rendered");
		expect(SettingsManager.fromStorage(storage).getMarkdownToolDisplay()).toBe("rendered");
		expect(storedGlobal(storage)).toEqual({ markdown: { toolDisplay: "source" }, terminal: { showImages: false } });
		storage.withLock("project", () => '{"markdown":{"codeBlockIndent":"    "}}');
		await manager.reload();
		expect(manager.getMarkdownToolDisplay()).toBe("source");
	});

	it("normalizes reload values and retains the last valid scope after a load failure", async () => {
		const storage = storageWith({ markdown: { toolDisplay: "source" } }, { markdown: { toolDisplay: "rendered" } });
		const manager = SettingsManager.fromStorage(storage);
		storage.withLock("project", () => '{"markdown":{"toolDisplay":"invalid"}}');
		await manager.reload();
		expect(manager.getMarkdownToolDisplay()).toBe("source");
		expect(manager.drainErrors()).toEqual([{ scope: "project", error: expect.any(Error) }]);
		storage.withLock("global", () => "{");
		storage.withLock("project", () => "{}");
		await manager.reload();
		expect(manager.getMarkdownToolDisplay()).toBe("source");
		expect(manager.drainErrors()).toEqual([{ scope: "global", error: expect.any(Error) }]);
		manager.setMarkdownToolDisplay("rendered");
		await manager.flush();
		expect(manager.drainErrors()).toEqual([{ scope: "global", error: expect.any(Error) }]);
	});

	const directories: string[] = [];
	afterEach(() => {
		for (const dir of directories.splice(0)) rmSync(dir, { recursive: true, force: true });
	});

	it("reopens the preference from the effective profile directory", async () => {
		const dir = mkdtempSync(join(tmpdir(), "markdown-settings-"));
		directories.push(dir);
		const agentDir = join(dir, "profile");
		const manager = SettingsManager.create(dir, agentDir);
		manager.setMarkdownToolDisplay("source");
		await manager.flush();
		expect(JSON.parse(readFileSync(join(agentDir, "settings.json"), "utf8"))).toEqual({
			markdown: { toolDisplay: "source" },
		});
		expect(SettingsManager.create(dir, agentDir).getMarkdownToolDisplay()).toBe("source");
		writeFileSync(join(agentDir, "settings.json"), '{"markdown":{"toolDisplay":"rendered"}}');
		await manager.reload();
		expect(manager.getMarkdownToolDisplay()).toBe("rendered");
	});
});
