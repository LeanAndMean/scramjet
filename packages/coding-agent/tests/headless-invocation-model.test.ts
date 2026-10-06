import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { parseArgs } from "../src/cli/args.js";
import { type CreateAgentSessionRuntimeFactory, createAgentSessionRuntime } from "../src/core/agent-session-runtime.js";
import { createAgentSessionFromServices, createAgentSessionServices } from "../src/core/agent-session-services.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";
import { buildSessionOptions, registerInvocationModel, resolveInvocationModelDefinition } from "../src/main.js";

const roots: string[] = [];
afterEach(() => {
	vi.unstubAllEnvs();
	vi.unstubAllGlobals();
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});
const definition = {
	api: "openai-completions",
	baseUrl: "https://fixture.invalid/custom/v1",
	id: "literal/Model:high",
	apiKeyEnv: "INVOCATION_TEST_KEY",
	contextWindow: 10000,
	maxTokens: 123,
};
function captureDefinition() {
	const parsed = parseArgs(["--print", "--model-definition", JSON.stringify(definition)]);
	return { parsed, invocation: resolveInvocationModelDefinition(parsed, true)! };
}
function captureFetch() {
	const calls: Array<{ url: string; headers: Headers; body: any }> = [];
	vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
		const request = new Request(input, init);
		calls.push({ url: request.url, headers: request.headers, body: await request.json() });
		const chunk = {
			id: "fixture",
			object: "chat.completion.chunk",
			created: 0,
			model: definition.id,
			choices: [{ index: 0, delta: { content: "route marker" }, finish_reason: "stop" }],
		};
		return new Response(`data: ${JSON.stringify(chunk)}\n\ndata: [DONE]\n\n`, {
			headers: { "content-type": "text/event-stream" },
		});
	});
	return calls;
}

async function fixture() {
	const root = mkdtempSync(join(tmpdir(), "headless-invocation-"));
	roots.push(root);
	const agentDir = join(root, "agent");
	mkdirSync(agentDir);
	const authStorage = AuthStorage.inMemory();
	vi.stubEnv("INVOCATION_TEST_KEY", "captured-synthetic-key");
	const { parsed, invocation } = captureDefinition();
	const selections: string[] = [];
	const factory: CreateAgentSessionRuntimeFactory = async ({ cwd, sessionManager, sessionStartEvent, inherited }) => {
		const services = await createAgentSessionServices({
			cwd,
			agentDir,
			authStorage,
			settingsManager: SettingsManager.inMemory({
				defaultProvider: "alternate",
				defaultModel: "alternate-id",
				enabledModels: ["alternate/*"],
			}),
			resourceLoaderOptions: {
				noExtensions: true,
				noSkills: true,
				noPromptTemplates: true,
				noThemes: true,
				noContextFiles: true,
				systemPrompt: "test",
				builtinInit: (pi) => {
					pi.registerProvider("alternate", {
						api: "openai-completions",
						baseUrl: "https://unused.invalid/v1",
						apiKey: "alternate-key",
						models: [
							{
								id: "alternate-id",
								name: "alternate",
								reasoning: true,
								input: ["text"],
								cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
								contextWindow: 10000,
								maxTokens: 500,
							},
						],
					});
					pi.on("model_select", (event) => {
						selections.push(event.model.id);
					});
				},
			},
		});
		expect(services.modelRegistry.find("alternate", "alternate-id")).toBeDefined();
		const model = registerInvocationModel(invocation, "invocation-test", services.modelRegistry, authStorage);
		const { options } = buildSessionOptions(
			parsed,
			[],
			sessionManager.buildSessionContext().messages.length > 0,
			services.modelRegistry,
			services.settingsManager,
			inherited,
			model,
		);
		return {
			...(await createAgentSessionFromServices({ services, sessionManager, sessionStartEvent, ...options })),
			services,
			diagnostics: services.diagnostics,
		};
	};
	const runtime = await createAgentSessionRuntime(factory, {
		cwd: root,
		agentDir,
		sessionManager: SessionManager.create(root, join(root, "sessions")),
	});
	await runtime.session.bindExtensions({});
	runtime.setRebindSession(async (session) => {
		await session.bindExtensions({});
	});
	return { runtime, authStorage, invocation, selections, root };
}

describe("invocation route through registry, CLI selection and session owners", () => {
	it("fails identity collisions and registration/lookup errors rather than falling back", () => {
		vi.stubEnv("INVOCATION_TEST_KEY", "synthetic-key");
		const { invocation } = captureDefinition();
		const auth = AuthStorage.inMemory();
		const registry = ModelRegistry.inMemory(auth);
		registerInvocationModel(invocation, "invocation-test", registry, auth);
		expect(() => registerInvocationModel(invocation, "invocation-test", registry, auth)).toThrow("identity collided");
		vi.spyOn(registry, "registerProvider").mockImplementation(() => {
			throw new Error("registration rejected");
		});
		expect(() => registerInvocationModel(invocation, "other-id", registry, auth)).toThrow("registration rejected");
		vi.restoreAllMocks();
		vi.spyOn(registry, "find").mockReturnValue(undefined);
		expect(() => registerInvocationModel(invocation, "other-id", registry, auth)).toThrow("registration failed");
		vi.restoreAllMocks();
	});

	it("snapshots explicit credentials above stored/provider credentials and retains secret-free metadata on refresh", async () => {
		const f = await fixture();
		try {
			f.authStorage.set("invocation-test", { type: "api_key", key: "stored-key" });
			vi.stubEnv("INVOCATION_TEST_KEY", "changed-key");
			expect(await f.runtime.services.modelRegistry.getApiKeyAndHeaders(f.runtime.session.model!)).toMatchObject({
				ok: true,
				apiKey: "captured-synthetic-key",
			});
			f.authStorage.set("invocation-test", {
				type: "oauth",
				access: "oauth-key",
				refresh: "refresh",
				expires: Date.now() + 60000,
			});
			expect(await f.runtime.services.modelRegistry.getApiKeyAndHeaders(f.runtime.session.model!)).toMatchObject({
				ok: true,
				apiKey: "captured-synthetic-key",
			});
			f.runtime.services.modelRegistry.refresh();
			const model = f.runtime.services.modelRegistry.find("invocation-test", definition.id)!;
			expect(model).toMatchObject({
				id: definition.id,
				baseUrl: definition.baseUrl,
				contextWindow: 10000,
				maxTokens: 123,
			});
			expect(JSON.stringify(f.runtime.services.modelRegistry.getAll())).not.toContain("captured-synthetic-key");
			expect(JSON.stringify(model)).not.toContain("INVOCATION_TEST_KEY");
		} finally {
			f.runtime.session.dispose();
		}
	});

	it("retains route/key/limits after reload, new, fork and resume; deliberate new-session switches still inherit", async () => {
		const f = await fixture();
		const calls = captureFetch();
		try {
			vi.stubEnv("INVOCATION_TEST_KEY", undefined);
			await f.runtime.session.prompt("initial");
			const savedPath = f.runtime.session.sessionFile!;
			await f.runtime.session.reload();
			await f.runtime.session.prompt("after reload");
			await f.runtime.newSession();
			await f.runtime.session.prompt("after new");
			const assistant = f.runtime.session.sessionManager
				.getBranch()
				.find((entry) => entry.type === "message" && entry.message.role === "assistant")!;
			await f.runtime.fork(assistant.id, { position: "at" });
			await f.runtime.session.prompt("after fork");
			await f.runtime.switchSession(savedPath);
			await f.runtime.session.prompt("after resume");
			expect(calls).toHaveLength(5);
			for (const call of calls) {
				expect(call.url).toBe(`${definition.baseUrl}/chat/completions`);
				expect(call.headers.get("authorization")).toBe("Bearer captured-synthetic-key");
				expect(call.body.model).toBe(definition.id);
				expect(call.body.max_completion_tokens).toBe(123);
			}
			const alternate = f.runtime.services.modelRegistry.find("alternate", "alternate-id")!;
			await f.runtime.session.setModel(alternate);
			f.runtime.session.setThinkingLevel("high");
			await f.runtime.newSession();
			expect(f.runtime.session.model?.id).toBe("alternate-id");
			expect(f.runtime.session.thinkingLevel).toBe("high");
			f.runtime.session.setThinkingLevel("low");
			expect(f.runtime.services.settingsManager.getDefaultThinkingLevel()).toBeUndefined();
			await f.runtime.switchSession(savedPath);
			expect(f.runtime.session.model?.id).toBe(definition.id);
			expect(f.runtime.session.sessionManager.buildSessionContext().model).toEqual({
				provider: "invocation-test",
				modelId: definition.id,
			});
		} finally {
			f.runtime.session.dispose();
		}
	});

	it("explicit invocation startup replaces restored branch identity and clamped thinking without duplicate entries", async () => {
		const f = await fixture();
		try {
			const manager = SessionManager.inMemory(f.root);
			manager.appendModelChange("alternate", "alternate-id");
			manager.appendThinkingLevelChange("high");
			manager.appendMessage({ role: "user", content: "prior", timestamp: 0 });
			const options = {
				services: f.runtime.services,
				sessionManager: manager,
				model: f.runtime.session.model,
				persistModelPreferences: false,
			};
			const { session } = await createAgentSessionFromServices(options);
			expect(manager.buildSessionContext()).toMatchObject({
				model: { provider: "invocation-test", modelId: definition.id },
				thinkingLevel: "off",
			});
			const count = manager.getEntries().length;
			session.dispose();
			const again = await createAgentSessionFromServices(options);
			expect(manager.getEntries()).toHaveLength(count);
			again.session.dispose();
		} finally {
			f.runtime.session.dispose();
		}
	});
});
