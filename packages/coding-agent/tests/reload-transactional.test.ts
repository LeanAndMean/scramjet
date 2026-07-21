/**
 * Stage 5 of issue 361: `AgentSession.reload()` must validate the required Scramjet builtin
 * before any irreversible live mutation. The builtin is a required product component, so a
 * throwing builtin cannot be allowed to leave a reloaded session running without it.
 *
 * The reorder gates `emitSessionShutdownEvent` and `resetApiProviders()` behind a successful
 * `resourceLoader.reload()` (which validates the builtin via the Stage 1 atomic path). A throwing
 * builtin therefore rejects before session_shutdown fires and before the API providers are reset;
 * the current runner, resources, and providers survive. On success, the `session_shutdown →
 * session_start` ordering is preserved.
 *
 * The fixture stands up a real `AgentSession` over an in-memory stack with a `DefaultResourceLoader`
 * whose builtin (a) records session_shutdown/session_start through the extension runner and (b) can
 * be armed to throw on a later reload.
 */

import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Model } from "@leanandmean/ai";
import {
	createAssistantMessageEventStream,
	getApiProvider,
	registerApiProvider,
	unregisterApiProviders,
} from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
import { AgentSession } from "../src/core/agent-session.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader, RequiredBuiltinInitError } from "../src/core/resource-loader.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";

const testModel: Model<"openai-chat"> = {
	id: "test-model",
	name: "Test Model",
	api: "openai-chat",
	provider: "openai",
	baseUrl: "https://api.openai.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};

function assistantText(text: string): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text }],
		api: "openai-chat",
		provider: "openai",
		model: "test-model",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: "stop",
		timestamp: Date.now(),
	};
}

async function buildFixture() {
	const dir = mkdtempSync(join(tmpdir(), "reload-transactional-"));
	const cwd = join(dir, "cwd");
	const agentDir = join(dir, "agent");

	const settingsManager = SettingsManager.inMemory();
	const authStorage = AuthStorage.inMemory();
	const modelRegistry = ModelRegistry.inMemory(authStorage);
	const sessionManager = SessionManager.inMemory(cwd);

	// The builtin records real lifecycle events through the extension runner and can be armed to
	// throw on a later reload (mirroring a required product component that fails to re-initialize).
	const events: string[] = [];
	const control = { armed: false };
	const resourceLoader = new DefaultResourceLoader({
		cwd,
		agentDir,
		settingsManager,
		builtinInit: (pi) => {
			if (control.armed) {
				throw new Error("reload builtin boom");
			}
			pi.on("session_shutdown", () => {
				events.push("session_shutdown");
			});
			pi.on("session_start", () => {
				events.push("session_start");
			});
		},
	});
	await resourceLoader.reload();

	const agent = new Agent({
		initialState: { systemPrompt: "", model: testModel, tools: [] },
		streamFn() {
			return createAssistantMessageEventStream(assistantText("ok"));
		},
		getApiKey: async () => "fake",
	});

	const session = new AgentSession({
		agent,
		cwd,
		agentDir,
		modelRegistry,
		settingsManager,
		sessionManager,
		resourceLoader,
	});

	// Binding wires the session-level handlers and emits the initial session_start, which is the
	// precondition for reload() to re-emit session_start.
	await session.bindExtensions({ shutdownHandler: async () => {} });

	return { session, events, control };
}

describe("AgentSession.reload() — transactional required-builtin validation (Stage 5)", () => {
	it("a throwing builtin rejects before session_shutdown and before resetApiProviders", async () => {
		const fx = await buildFixture();
		expect(fx.events).toEqual(["session_start"]);

		// A sentinel provider is wiped by resetApiProviders() (which re-registers only the built-ins),
		// so its survival proves resetApiProviders() was never reached.
		const sentinelSource = "reload-transactional-sentinel";
		const sentinelStream = (() => {
			throw new Error("sentinel stream must never be invoked");
		}) as never;
		registerApiProvider(
			{ api: "reload-sentinel-api", stream: sentinelStream, streamSimple: sentinelStream },
			sentinelSource,
		);

		try {
			fx.control.armed = true;
			await expect(fx.session.reload()).rejects.toBeInstanceOf(RequiredBuiltinInitError);

			expect(fx.events).not.toContain("session_shutdown");
			expect(fx.events).toEqual(["session_start"]);
			expect(getApiProvider("reload-sentinel-api")).toBeDefined();
		} finally {
			unregisterApiProviders(sentinelSource);
		}
	});

	it("the session remains reloadable after a failed reload", async () => {
		const fx = await buildFixture();

		fx.control.armed = true;
		await expect(fx.session.reload()).rejects.toBeInstanceOf(RequiredBuiltinInitError);
		expect(fx.events).toEqual(["session_start"]);

		// The preserved runtime is still live: a subsequent successful reload completes end-to-end.
		fx.control.armed = false;
		await fx.session.reload();
		expect(fx.events).toEqual(["session_start", "session_shutdown", "session_start"]);
	});

	it("a successful reload preserves session_shutdown → session_start ordering", async () => {
		const fx = await buildFixture();

		await fx.session.reload();

		expect(fx.events).toEqual(["session_start", "session_shutdown", "session_start"]);
	});
});
