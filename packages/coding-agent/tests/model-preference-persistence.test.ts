import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentSession } from "../src/core/agent-session.js";
import { createAgentSessionFromServices, createAgentSessionServices } from "../src/core/agent-session-services.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader } from "../src/core/resource-loader.js";
import { createAgentSession } from "../src/core/sdk.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";
import { ModelSelectorComponent } from "../src/modes/interactive/components/model-selector.js";
import { initTheme } from "../src/modes/interactive/theme/theme.js";

initTheme("pi-dark");

const roots: string[] = [];
afterEach(() => {
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});
const defaults = { defaultProvider: "saved", defaultModel: "saved-id", defaultThinkingLevel: "low" };

async function fixture(
	persistModelPreferences: boolean | undefined,
	throughServices: boolean,
	scoped: boolean,
	nonReasoningDetour = false,
) {
	const root = mkdtempSync(join(tmpdir(), "model-preferences-"));
	roots.push(root);
	const agentDir = join(root, "agent");
	mkdirSync(agentDir);
	const settingsPath = join(agentDir, "settings.json");
	writeFileSync(settingsPath, JSON.stringify(defaults));
	const authStorage = AuthStorage.inMemory();
	const modelRegistry = ModelRegistry.inMemory(authStorage);
	const models = ["a", "b"].map((id) => {
		authStorage.setRuntimeApiKey(`preference-${id}`, "synthetic-key");
		modelRegistry.registerProvider(`preference-${id}`, {
			api: "openai-completions",
			apiKey: "KEY_ENV",
			baseUrl: "https://unused.invalid/v1",
			models: [
				{
					id,
					name: id,
					reasoning: !nonReasoningDetour || id !== "b",
					input: ["text"],
					contextWindow: 10000,
					maxTokens: 1000,
					cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				},
			],
		});
		return modelRegistry.find(`preference-${id}`, id)!;
	});
	vi.spyOn(modelRegistry, "getAvailable").mockResolvedValue(models);
	const settingsManager = SettingsManager.create(root, agentDir);
	const selections: string[] = [];
	const resourceLoaderOptions = {
		builtinInit: (pi: import("../src/core/extensions/types.js").ExtensionAPI) => {
			pi.on("model_select", (event) => {
				selections.push(event.model.id);
			});
		},
		noExtensions: true,
		noSkills: true,
		noPromptTemplates: true,
		noThemes: true,
		noContextFiles: true,
		systemPrompt: "test",
	};
	const sessionManager = SessionManager.inMemory(root);
	const options = {
		model: models[0],
		thinkingLevel: "low" as const,
		persistModelPreferences,
		scopedModels: scoped ? models.map((model) => ({ model })) : undefined,
		sessionManager,
		tools: [],
	};
	let session: AgentSession;
	if (throughServices) {
		const services = await createAgentSessionServices({
			cwd: root,
			agentDir,
			authStorage,
			modelRegistry,
			settingsManager,
			resourceLoaderOptions,
		});
		({ session } = await createAgentSessionFromServices({ services, ...options }));
	} else {
		const resourceLoader = new DefaultResourceLoader({
			cwd: root,
			agentDir,
			settingsManager,
			...resourceLoaderOptions,
		});
		await resourceLoader.reload();
		({ session } = await createAgentSession({
			cwd: root,
			agentDir,
			authStorage,
			modelRegistry,
			settingsManager,
			resourceLoader,
			...options,
		}));
	}
	await session.bindExtensions({});
	const events: string[] = [];
	session.subscribe((event) => events.push(event.type));
	return { session, settingsManager, settingsPath, sessionManager, modelRegistry, models, selections, events };
}

describe("model preference persistence policy", () => {
	it.each(
		[false, true, undefined].flatMap((policy) =>
			["direct", "scoped", "available"].flatMap((path) =>
				(["high", "off"] as const).map((level) => ({ policy, path, level })),
			),
		),
	)("policy=$policy path=$path retains $level through a non-reasoning detour", async ({ policy, path, level }) => {
		const f = await fixture(policy, path === "scoped", path === "scoped", true);
		const changes: string[] = [];
		const unsubscribe = f.session.subscribe((event) => {
			if (event.type === "thinking_level_changed") changes.push(event.level);
		});
		try {
			f.session.setThinkingLevel(level);
			if (path === "direct") await f.session.setModel(f.models[1]);
			else await f.session.cycleModel();
			expect(f.session.model).toBe(f.models[1]);
			expect(f.session.thinkingLevel).toBe("off");
			if (path === "direct") await f.session.setModel(f.models[0]);
			else await f.session.cycleModel();
			expect(f.session.model).toBe(f.models[0]);
			expect(f.session.thinkingLevel).toBe(level);
			expect(changes).toEqual(level === "high" ? ["high", "off", "high"] : ["off"]);
			expect(
				f.sessionManager
					.getEntries()
					.filter((entry) => entry.type === "thinking_level_change")
					.map((entry) => entry.thinkingLevel),
			).toEqual(level === "high" ? ["low", "high", "off", "high"] : ["low", "off"]);
			expect(f.selections).toEqual(["b", "a"]);
			await f.settingsManager.flush();
			expect(f.settingsManager.drainErrors()).toEqual([]);
			expect(JSON.parse(readFileSync(f.settingsPath, "utf8"))).toEqual(
				policy === false
					? defaults
					: { defaultProvider: f.models[0].provider, defaultModel: "a", defaultThinkingLevel: level },
			);
		} finally {
			unsubscribe();
			f.session.dispose();
		}
	});

	it("retains the local preference across reload without adopting unsupported requests", async () => {
		const f = await fixture(false, false, false, true);
		try {
			f.session.setThinkingLevel("high");
			await f.session.setModel(f.models[1]);
			const count = f.sessionManager.getEntries().length;
			f.session.setThinkingLevel("low");
			expect(f.sessionManager.getEntries()).toHaveLength(count);
			await f.session.reload();
			expect(f.session.thinkingLevel).toBe("off");
			await f.session.setModel(f.models[0]);
			expect(f.session.thinkingLevel).toBe("high");
			await f.settingsManager.flush();
			expect(JSON.parse(readFileSync(f.settingsPath, "utf8"))).toEqual(defaults);
		} finally {
			f.session.dispose();
		}
	});

	it("explicit scoped effort overrides and updates the local preference", async () => {
		const f = await fixture(false, true, true, true);
		try {
			f.session.setThinkingLevel("high");
			await f.session.cycleModel();
			f.session.setScopedModels([{ model: f.models[0], thinkingLevel: "medium" }, { model: f.models[1] }]);
			await f.session.cycleModel();
			expect(f.session.thinkingLevel).toBe("medium");
			f.session.setScopedModels(f.models.map((model) => ({ model })));
			await f.session.cycleModel();
			await f.session.cycleModel();
			expect(f.session.thinkingLevel).toBe("medium");
			await f.settingsManager.flush();
			expect(JSON.parse(readFileSync(f.settingsPath, "utf8"))).toEqual(defaults);
		} finally {
			f.session.dispose();
		}
	});

	it("remembers the effective level after capability clamping, not the unsupported request", async () => {
		const f = await fixture(false, false, false, true);
		try {
			f.session.setThinkingLevel("high");
			await f.session.setModel(f.models[1]);
			const limitedModel = { ...f.models[0], thinkingLevelMap: { high: null, xhigh: null, max: null } };
			await f.session.setModel(limitedModel);
			expect(f.session.thinkingLevel).toBe("medium");
			await f.session.setModel(f.models[1]);
			await f.session.setModel(f.models[0]);
			expect(f.session.thinkingLevel).toBe("medium");
			await f.settingsManager.flush();
			expect(JSON.parse(readFileSync(f.settingsPath, "utf8"))).toEqual(defaults);
		} finally {
			f.session.dispose();
		}
	});

	it.each([false, true, undefined])("picker selection respects policy=%s", async (policy) => {
		const f = await fixture(policy, false, false);
		try {
			let selection: Promise<void> | undefined;
			const requestRender = vi.fn();
			const selector = new ModelSelectorComponent(
				{ requestRender } as any,
				f.session.model,
				f.modelRegistry,
				[],
				(model) => {
					selection = f.session.setModel(model);
				},
				vi.fn(),
				"b",
			);
			await vi.waitFor(() => expect(requestRender).toHaveBeenCalled());
			selector.handleInput("\r");
			expect(selection).toBeDefined();
			await selection;
			expect(f.session.model).toBe(f.models[1]);
			expect(f.selections).toEqual(["b"]);
			expect(f.sessionManager.buildSessionContext().model).toEqual({ provider: f.models[1].provider, modelId: "b" });
			await f.settingsManager.flush();
			expect(f.settingsManager.drainErrors()).toEqual([]);
			expect(JSON.parse(readFileSync(f.settingsPath, "utf8"))).toEqual(
				policy === false ? defaults : { ...defaults, defaultProvider: f.models[1].provider, defaultModel: "b" },
			);

			vi.spyOn(f.modelRegistry, "hasConfiguredAuth").mockReturnValue(false);
			selector.handleInput("\r");
			await expect(selection).rejects.toThrow(`No API key for ${f.models[1].provider}/b`);
			expect(f.selections).toEqual(["b"]);
			await f.settingsManager.flush();
			expect(f.settingsManager.drainErrors()).toEqual([]);
		} finally {
			f.session.dispose();
		}
	});
	it.each([
		[false, false, false],
		[false, true, true],
		[true, false, true],
		[undefined, true, false],
	])("policy=%s services=%s scoped=%s preserves live and journal changes", async (policy, services, scoped) => {
		const f = await fixture(policy, services, scoped);
		try {
			await f.session.setModel(f.models[1]);
			f.session.setThinkingLevel("high");
			expect(f.session.model).toBe(f.models[1]);
			expect(f.session.thinkingLevel).toBe("high");
			expect(f.sessionManager.buildSessionContext()).toMatchObject({
				model: { provider: f.models[1].provider, modelId: "b" },
				thinkingLevel: "high",
			});
			expect(f.events).toContain("thinking_level_changed");
			expect(f.selections).toEqual(["b"]);
			await f.settingsManager.flush();
			expect(f.settingsManager.drainErrors()).toEqual([]);
			const disk = JSON.parse(readFileSync(f.settingsPath, "utf8"));
			expect(disk).toEqual(
				policy === false
					? defaults
					: { defaultProvider: f.models[1].provider, defaultModel: "b", defaultThinkingLevel: "high" },
			);

			const cycled = await f.session.cycleModel();
			expect(cycled?.isScoped).toBe(scoped);
			expect(f.session.model).toBe(f.models[0]);
			expect(f.selections).toEqual(["b", "a"]);
			expect(f.sessionManager.buildSessionContext().model).toEqual({ provider: f.models[0].provider, modelId: "a" });
			await f.settingsManager.flush();
			expect(f.settingsManager.drainErrors()).toEqual([]);
			expect(JSON.parse(readFileSync(f.settingsPath, "utf8"))).toEqual(
				policy === false
					? defaults
					: { defaultProvider: f.models[0].provider, defaultModel: "a", defaultThinkingLevel: "high" },
			);
			f.settingsManager.setBlockImages(true);
			await f.settingsManager.flush();
			expect(JSON.parse(readFileSync(f.settingsPath, "utf8")).images.blockImages).toBe(true);
		} finally {
			f.session.dispose();
		}
	});
});
