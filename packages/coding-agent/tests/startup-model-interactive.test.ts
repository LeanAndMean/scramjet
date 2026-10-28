import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { InMemoryLastUsedModelsStorage } from "../src/core/last-used-models.js";
import { SettingsManager } from "../src/core/settings-manager.js";
import { createProductionInteractiveHarness } from "./helpers/interactive-harness.js";

vi.mock("../src/utils/tools-manager.js", () => ({ ensureTool: vi.fn(async () => undefined) }));

const harnesses: Awaited<ReturnType<typeof createProductionInteractiveHarness>>[] = [];
const directories: string[] = [];
afterEach(async () => {
	for (const h of harnesses.splice(0)) await h.dispose();
	for (const dir of directories.splice(0)) rmSync(dir, { recursive: true, force: true });
	vi.restoreAllMocks();
});

function storage(project: object = {}) {
	let durable = JSON.stringify({ theme: "pi-dark", quietStartup: true, dockEditor: false });
	let fail = false;
	const recall = new InMemoryLastUsedModelsStorage();
	const manager = SettingsManager.fromStorage(
		{
			withLock(scope, update) {
				const next = update(scope === "global" ? durable : JSON.stringify(project));
				if (scope === "global" && next !== undefined) {
					if (fail) throw new Error("startup preference save failed");
					durable = next;
				}
			},
		},
		recall,
	);
	return {
		manager,
		recall,
		saved: () => JSON.parse(durable),
		fail: () => {
			fail = true;
		},
	};
}

async function setup(manager: SettingsManager) {
	const h = await createProductionInteractiveHarness(100, 36, undefined, true, manager);
	harnesses.push(h);
	return h;
}

type Harness = Awaited<ReturnType<typeof setup>>;
async function settings(h: Harness) {
	h.extensionUI.setEditorText("/settings");
	h.terminal.sendInput("\r");
	await h.frame();
	for (const char of "Startup model") h.terminal.sendInput(char);
	return (await h.frame()).join("\n").replace(/\s+/g, " ");
}
async function toggle(h: Harness) {
	h.terminal.sendInput("\r");
	await h.session.settingsManager.flush();
	await new Promise((resolve) => setImmediate(resolve));
	return (await h.frame()).join("\n");
}

describe("interactive startup model preference", () => {
	it("explains recording, saves both choices, and does not change the live model or directory records", async () => {
		const store = storage();
		const h = await setup(store.manager);
		const model = h.session.model;
		const entries = structuredClone(h.session.sessionManager.getEntries());
		const before = store.recall.read();
		const frame = await settings(h);
		expect(frame).toContain("Per-directory");
		expect(frame).toContain("Both modes still record deliberate choices and directory paths.");
		expect(await toggle(h)).toContain("Global");
		expect(store.saved().startupModel).toBe("global");
		expect(await toggle(h)).toContain("Per-directory");
		expect(store.saved().startupModel).toBe("per-directory");
		expect(h.session.model).toBe(model);
		expect(h.session.sessionManager.getEntries()).toEqual(entries);
		expect(store.recall.read()).toBe(before);
	});

	it.each(["global", "per-directory"])("displays effective project %s after global edits", async (startupModel) => {
		const store = storage({ startupModel });
		const h = await setup(store.manager);
		expect(await settings(h)).toContain("Project settings override this value");
		const frame = await toggle(h);
		expect(frame).toContain(startupModel === "global" ? "Global" : "Per-directory");
		expect(store.manager.getStartupModel()).toBe(startupModel);
		expect(store.saved().startupModel).toBe(startupModel === "global" ? "per-directory" : "global");
	});

	it("reports an unsaved preference without claiming durable success", async () => {
		const store = storage();
		const h = await setup(store.manager);
		const warnings = vi.spyOn(h.mode, "showWarning");
		await settings(h);
		store.fail();
		expect(await toggle(h)).toContain("Changes not saved");
		expect(store.manager.getStartupModel()).toBe("global");
		expect(store.saved().startupModel).toBeUndefined();
		expect(warnings).toHaveBeenCalledWith(
			"Settings not saved. Changes may be lost on restart; project overrides still apply.",
		);
		expect(warnings.mock.calls.some(([message]) => message.includes("startup preference save failed"))).toBe(true);
	});

	it("reopens the profile preference and preserves malformed settings on blocked saves", async () => {
		const directory = mkdtempSync(join(tmpdir(), "startup-model-ui-"));
		directories.push(directory);
		const file = join(directory, "settings.json");
		writeFileSync(file, JSON.stringify({ theme: "pi-dark", quietStartup: true }));
		const h = await setup(SettingsManager.create(directory, directory));
		await settings(h);
		await toggle(h);
		const reopened = SettingsManager.create(directory, directory);
		expect(reopened.getStartupModel()).toBe("global");
		const restarted = await setup(reopened);
		expect(await settings(restarted)).toContain("Global");
		writeFileSync(file, "{broken-json");
		await reopened.reload();
		reopened.drainErrors();
		expect(await toggle(restarted)).toContain("Changes not saved");
		expect(readFileSync(file, "utf8")).toBe("{broken-json");
	});
});
