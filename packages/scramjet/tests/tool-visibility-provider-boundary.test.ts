import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
	type AssistantMessage,
	type Context,
	createAssistantMessageEventStream,
	getApiProvider,
	inspectProviderRequestToolInventory,
	type Model,
	registerApiProvider,
	type SimpleStreamOptions,
	type StreamFunction,
} from "@leanandmean/ai";
import { streamSimpleAnthropic } from "@leanandmean/ai/anthropic";
import {
	AuthStorage,
	createAgentSession,
	DefaultResourceLoader,
	ModelRegistry,
	SessionManager,
	SettingsManager,
} from "@leanandmean/coding-agent";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { initScramjet } from "../src/index.js";
import { createLogger } from "../src/logger.js";
import { classifyToolVisibility, registerToolVisibilityDiagnostics } from "../src/tool-visibility-diagnostics.js";
import { freshState, lifecycleFor, recordingPi } from "./helpers.js";

const api = "openai-responses" as const;
const originalProvider = getApiProvider(api);
const temporaryDirectories: string[] = [];
const payloads: unknown[] = [];
const assistantResponses: Array<Pick<AssistantMessage, "content" | "stopReason">> = [];

const testModel: Model<typeof api> = {
	id: "test-model",
	name: "Test Model",
	api,
	provider: "openai",
	baseUrl: "https://api.openai.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 128_000,
	maxTokens: 4096,
};

function assistantMessage(): AssistantMessage {
	const response = assistantResponses.shift();
	return {
		role: "assistant",
		content: response?.content ?? [{ type: "text", text: "done" }],
		api,
		provider: "openai",
		model: testModel.id,
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: response?.stopReason ?? "stop",
		timestamp: Date.now(),
	};
}

const fakeStream: StreamFunction<typeof api, SimpleStreamOptions> = (model, context, options) => {
	const stream = createAssistantMessageEventStream();
	void (async () => {
		const payload = {
			tools: (context.tools ?? []).map((tool) => ({
				name: tool.name,
				description: tool.description,
				parameters: tool.parameters,
			})),
		};
		const replacement = await options?.onPayload?.(payload, model);
		payloads.push(replacement === undefined ? payload : replacement);
		const message = assistantMessage();
		stream.push({ type: "start", partial: message });
		stream.push({ type: "done", reason: message.stopReason as "stop" | "aborted", message });
	})();
	return stream;
};

beforeAll(() => {
	if (!originalProvider) throw new Error("openai-responses provider is not registered");
	registerApiProvider({ api, stream: fakeStream, streamSimple: fakeStream, handlesSystemPromptSections: true });
});

afterAll(() => {
	if (!originalProvider) return;
	registerApiProvider({
		api,
		stream: originalProvider.stream as StreamFunction<typeof api>,
		streamSimple: originalProvider.streamSimple as StreamFunction<typeof api, SimpleStreamOptions>,
		handlesSystemPromptSections: true,
	});
});

afterEach(() => {
	payloads.length = 0;
	assistantResponses.length = 0;
	vi.restoreAllMocks();
	vi.unstubAllEnvs();
	for (const directory of temporaryDirectories.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function event(
	requestContextToolNames: readonly string[],
	inventory: ReturnType<typeof inspectProviderRequestToolInventory>,
	model: { provider: string; id: string; api: string } = { provider: "openai", id: "test-model", api },
) {
	return { type: "provider_request_tool_inventory" as const, model, requestContextToolNames, inventory };
}

describe("tool visibility classification", () => {
	it("classifies exact parity and reports sorted names", () => {
		expect(
			classifyToolVisibility(
				event(["zeta", "alpha", "alpha"], { status: "observed", toolNames: ["alpha", "zeta"] }),
			),
		).toEqual({
			classification: "parity",
			requestContextToolNames: ["alpha", "zeta"],
			serializedToolNames: ["alpha", "zeta"],
			missingNames: [],
			unexpectedNames: [],
		});
	});

	it("does not reconcile case differences for non-Anthropic APIs", () => {
		expect(classifyToolVisibility(event(["read"], { status: "observed", toolNames: ["Read"] }))).toEqual({
			classification: "mismatch",
			requestContextToolNames: ["read"],
			serializedToolNames: ["Read"],
			missingNames: ["read"],
			unexpectedNames: ["Read"],
		});
	});

	it("classifies missing and unexpected exact names as a mismatch", () => {
		expect(
			classifyToolVisibility(event(["alpha", "missing"], { status: "observed", toolNames: ["alpha", "extra"] })),
		).toEqual({
			classification: "mismatch",
			requestContextToolNames: ["alpha", "missing"],
			serializedToolNames: ["alpha", "extra"],
			missingNames: ["missing"],
			unexpectedNames: ["extra"],
		});
	});

	it("accepts only unique Anthropic case-insensitive identity matches", () => {
		const model = { provider: "anthropic", id: "claude-test", api: "anthropic-messages" };
		expect(
			classifyToolVisibility(event(["read", "other"], { status: "observed", toolNames: ["Read", "other"] }, model)),
		).toMatchObject({
			classification: "parity",
			serializedToolNames: ["Read", "other"],
		});
		expect(
			classifyToolVisibility(event(["read", "Read"], { status: "observed", toolNames: ["READ"] }, model)),
		).toEqual({
			classification: "uninspectable",
			requestContextToolNames: ["Read", "read"],
			serializedToolNames: ["READ"],
			reason: "ambiguous-anthropic-tool-name",
		});
		expect(
			classifyToolVisibility(event(["read"], { status: "observed", toolNames: ["Read", "read"] }, model)),
		).toMatchObject({ classification: "mismatch", unexpectedNames: ["Read"] });
	});

	it("reserves exact Anthropic matches before case-fold reconciliation", () => {
		const model = { provider: "anthropic", id: "claude-test", api: "anthropic-messages" };
		expect(
			classifyToolVisibility(event(["read", "Read"], { status: "observed", toolNames: ["read", "READ"] }, model)),
		).toEqual({
			classification: "parity",
			requestContextToolNames: ["Read", "read"],
			serializedToolNames: ["READ", "read"],
			missingNames: [],
			unexpectedNames: [],
		});
	});

	it("keeps malformed and unsupported observations inconclusive", () => {
		expect(
			classifyToolVisibility(event(["alpha"], { status: "malformed", reason: "tool-name-empty" })),
		).toMatchObject({
			classification: "uninspectable",
			inventoryStatus: "malformed",
			reason: "tool-name-empty",
		});
		expect(classifyToolVisibility(event(["alpha"], { status: "unsupported" }))).toMatchObject({
			classification: "uninspectable",
			inventoryStatus: "unsupported",
		});
	});
});

describe("tool visibility journaling", () => {
	it("logs parity and uninspectable observations at debug and mismatches once at warn", async () => {
		const { pi, emit } = recordingPi();
		const state = freshState({ lifecycleGeneration: 7, lifecycle: lifecycleFor("running", "test:command") });
		state.logger = createLogger(pi);
		state.logger.setHasUI(true);
		registerToolVisibilityDiagnostics(pi, state);

		await emit("provider_request_tool_inventory", event(["alpha"], { status: "observed", toolNames: ["alpha"] }));
		await emit("provider_request_tool_inventory", event(["alpha"], { status: "observed", toolNames: [] }));
		await emit("provider_request_tool_inventory", event(["alpha"], { status: "unsupported" }));

		const entries = pi.appended.map((entry: any) => entry.data);
		expect(entries.map((entry: any) => [entry.level, entry.category, entry.message])).toEqual([
			["debug", "tool-visibility", "provider tool inventory parity"],
			["warn", "tool-visibility", "provider tool inventory mismatch"],
			["debug", "tool-visibility", "provider tool inventory uninspectable"],
		]);
		expect(entries[1].data).toMatchObject({
			provider: "openai",
			model: "test-model",
			api,
			lifecycleGeneration: 7,
			phase: "running",
			requestContextToolNames: ["alpha"],
			serializedToolNames: [],
			missingNames: ["alpha"],
			unexpectedNames: [],
		});
	});

	it("persists no omitted provider fields and remains non-fatal when persistence fails", async () => {
		const secret = "secret-that-must-not-be-journaled";
		const recorded = recordingPi();
		const recordedState = freshState({ logger: createLogger(recorded.pi) });
		registerToolVisibilityDiagnostics(recorded.pi, recordedState);
		const observed = event(["alpha"], { status: "observed", toolNames: ["alpha"] });
		(observed as any).payload = {
			headers: { authorization: secret },
			messages: [secret],
			tools: [{ name: "alpha", description: secret, parameters: { secret } }],
		};
		await recorded.emit("provider_request_tool_inventory", observed);
		expect(JSON.stringify(recorded.pi.appended)).not.toContain(secret);

		const failing = recordingPi();
		failing.pi.appendEntry = () => {
			throw new Error("disk unavailable");
		};
		const stderr = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
		const failingState = freshState({ logger: createLogger(failing.pi) });
		registerToolVisibilityDiagnostics(failing.pi, failingState);
		await expect(failing.emit("provider_request_tool_inventory", observed)).resolves.toBeUndefined();
		expect(stderr).toHaveBeenCalledTimes(1);
		expect(String(stderr.mock.calls[0]?.[0])).not.toContain(secret);
	});
});

describe("production Scramjet provider boundary", () => {
	async function createFixture(tools?: string[], projectCommand = false) {
		const root = mkdtempSync(join(tmpdir(), "scramjet-tool-visibility-"));
		temporaryDirectories.push(root);
		const cwd = join(root, "cwd");
		const agentDir = join(root, "agent");
		mkdirSync(cwd, { recursive: true });
		mkdirSync(agentDir, { recursive: true });
		if (projectCommand) {
			const commandsDir = join(cwd, ".scramjet", "testset", "commands");
			mkdirSync(commandsDir, { recursive: true });
			writeFileSync(
				join(commandsDir, "testset:command.md"),
				"---\ndescription: Test active command\n---\n\n## Goals\n\n- Exercise active-command tool eligibility.\n",
			);
		}
		const settingsManager = SettingsManager.inMemory({ retry: { enabled: false } });
		const authStorage = AuthStorage.inMemory();
		authStorage.setRuntimeApiKey("openai", "fake");
		const modelRegistry = ModelRegistry.inMemory(authStorage);
		const resourceLoader = new DefaultResourceLoader({ cwd, agentDir, settingsManager, builtinInit: initScramjet });
		await resourceLoader.reload();
		return createAgentSession({
			cwd,
			agentDir,
			model: testModel,
			authStorage,
			modelRegistry,
			sessionManager: SessionManager.inMemory(cwd),
			settingsManager,
			resourceLoader,
			tools,
		});
	}

	it("keeps production model-callable tools active, guided, and serialized while respecting exact lists", async () => {
		const { session } = await createFixture();
		try {
			const defaults = session.getActiveToolNames();
			expect(defaults).toContain("suggest_scramjet_next_steps");
			expect(defaults).toContain("switch_scramjet_model");
			expect(defaults).not.toContain("scramjet_model_change_notice");
			expect(session.getToolDefinition("scramjet_model_change_notice")?.activation).toBe("harness-only");
			expect(session.systemPrompt).toContain("suggest_scramjet_next_steps");
			expect(session.systemPrompt).toContain("switch_scramjet_model");
			expect(session.systemPrompt).not.toContain("scramjet_model_change_notice");

			await session.prompt("capture defaults");
			const diagnostic = session.sessionManager
				.getEntries()
				.find(
					(entry: any) =>
						entry.type === "custom" &&
						entry.customType === "scramjet:log" &&
						entry.data.category === "tool-visibility",
				);
			expect((diagnostic as any)?.data).toMatchObject({
				level: "debug",
				message: "provider tool inventory parity",
				data: { classification: "parity" },
			});
			const serializedDefaults = inspectProviderRequestToolInventory(api, payloads.at(-1));
			expect(serializedDefaults.status).toBe("observed");
			if (serializedDefaults.status === "observed") {
				expect(serializedDefaults.toolNames).toContain("suggest_scramjet_next_steps");
				expect(serializedDefaults.toolNames).toContain("switch_scramjet_model");
				expect(serializedDefaults.toolNames).not.toContain("scramjet_model_change_notice");
			}

			session.setActiveToolsByName(["suggest_scramjet_next_steps", "scramjet_model_change_notice"]);
			expect(session.getActiveToolNames()).toEqual(["suggest_scramjet_next_steps"]);
			expect(session.systemPrompt).toContain("suggest_scramjet_next_steps");
			expect(session.systemPrompt).not.toContain("switch_scramjet_model");
			await session.prompt("capture exact list");
			expect(inspectProviderRequestToolInventory(api, payloads.at(-1))).toEqual({
				status: "observed",
				toolNames: ["suggest_scramjet_next_steps"],
			});
		} finally {
			await session.dispose();
		}
	});

	it("restores active, guided, and serialized tool parity after an aborted turn", async () => {
		const { session } = await createFixture();
		assistantResponses.push({ content: [{ type: "text", text: "partial" }], stopReason: "aborted" });
		try {
			await session.prompt("abort this turn");
			await session.prompt("capture after abort");

			const activeNames = [...session.getActiveToolNames()].sort();
			expect(activeNames).toContain("suggest_scramjet_next_steps");
			expect(activeNames).toContain("switch_scramjet_model");
			expect(activeNames).not.toContain("scramjet_model_change_notice");
			expect(session.systemPrompt).toContain("suggest_scramjet_next_steps");
			expect(session.systemPrompt).toContain("switch_scramjet_model");
			expect(session.systemPrompt).not.toContain("scramjet_model_change_notice");
			expect(inspectProviderRequestToolInventory(api, payloads.at(-1))).toEqual({
				status: "observed",
				toolNames: activeNames,
			});
		} finally {
			await session.dispose();
		}
	});

	it("applies a startup allowlist consistently", async () => {
		const { session } = await createFixture(["switch_scramjet_model"]);
		try {
			expect(session.getActiveToolNames()).toEqual(["switch_scramjet_model"]);
			expect(session.getToolDefinition("suggest_scramjet_next_steps")).toBeUndefined();
			expect(session.systemPrompt).toContain("switch_scramjet_model");
			expect(session.systemPrompt).not.toContain("suggest_scramjet_next_steps");
			await session.prompt("capture allowlist");
			expect(inspectProviderRequestToolInventory(api, payloads.at(-1))).toEqual({
				status: "observed",
				toolNames: ["switch_scramjet_model"],
			});
		} finally {
			await session.dispose();
		}
	});

	it("keeps the suggestion schema present during active-command rejection", async () => {
		vi.stubEnv("SCRAMJET_OFFLINE", "1");
		const custom = vi.fn(async () => "0");
		const { session } = await createFixture(undefined, true);
		await session.bindExtensions({
			uiContext: {
				custom,
				pasteToEditor: vi.fn(),
				setStatus: vi.fn(),
				setTitle: vi.fn(),
				setTitleProvider: vi.fn(),
				notify: vi.fn(),
			} as any,
		});
		assistantResponses.push(
			{
				content: [
					{
						type: "toolCall",
						id: "suggest-while-active",
						name: "suggest_scramjet_next_steps",
						arguments: {
							next_steps: [{ message: "Follow up", fresh_session: false, reason: "Continue later" }],
						},
					},
				],
				stopReason: "toolUse",
			},
			{ content: [{ type: "text", text: "active command done" }], stopReason: "stop" },
		);
		try {
			await session.prompt("/testset:command", { source: "interactive" });
			await (session as any)._drainAgentEventQueue();
			const inventories = payloads.map((payload) => inspectProviderRequestToolInventory(api, payload));
			expect(inventories.length).toBeGreaterThanOrEqual(2);
			for (const inventory of inventories) {
				expect(inventory.status === "observed" && inventory.toolNames).toContain("suggest_scramjet_next_steps");
			}
			const rejectedResult = session.state.messages.find(
				(message: any) => message.role === "toolResult" && message.toolCallId === "suggest-while-active",
			) as any;
			expect(rejectedResult?.content[0]?.text).toMatch(/invocation is accepted only when no command is active/i);
			expect(custom).not.toHaveBeenCalled();
		} finally {
			await session.dispose();
		}
	});

	it("stores an eligible idle suggestion and drains it to the selector", async () => {
		vi.stubEnv("SCRAMJET_OFFLINE", "1");
		const custom = vi.fn(async () => null);
		const pasteToEditor = vi.fn();
		const { session } = await createFixture();
		await session.bindExtensions({
			uiContext: {
				custom,
				pasteToEditor,
				setStatus: vi.fn(),
				setTitle: vi.fn(),
				setTitleProvider: vi.fn(),
				notify: vi.fn(),
			} as any,
		});
		assistantResponses.push(
			{
				content: [
					{
						type: "toolCall",
						id: "suggest-while-idle",
						name: "suggest_scramjet_next_steps",
						arguments: {
							next_steps: [{ message: "Follow up", fresh_session: false, reason: "Continue now" }],
						},
					},
				],
				stopReason: "toolUse",
			},
			{ content: [{ type: "text", text: "idle suggestion done" }], stopReason: "stop" },
		);
		try {
			await session.prompt("recommend a follow-up", { source: "interactive" });
			await (session as any)._drainAgentEventQueue();
			const acceptedResult = session.state.messages.find(
				(message: any) => message.role === "toolResult" && message.toolCallId === "suggest-while-idle",
			) as any;
			expect(acceptedResult?.content[0]?.text).toMatch(/suggestion accepted/i);
			await vi.waitFor(() => expect(custom).toHaveBeenCalledTimes(1));
			await vi.waitFor(() =>
				expect(
					session.state.messages.some(
						(message: any) =>
							message.role === "toolResult" && message.toolName === "scramjet_next_step_selection",
					),
				).toBe(true),
			);
			expect(pasteToEditor).not.toHaveBeenCalled();
		} finally {
			await session.dispose();
		}
	});

	it("observes genuine Anthropic OAuth canonicalization without network transport", async () => {
		const model: Model<"anthropic-messages"> = {
			id: "claude-test",
			name: "Claude Test",
			api: "anthropic-messages",
			provider: "anthropic",
			baseUrl: "https://api.anthropic.com",
			reasoning: false,
			input: ["text"],
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
			contextWindow: 128_000,
			maxTokens: 4096,
		};
		const context: Context = {
			messages: [{ role: "user", content: "hello", timestamp: 0 }],
			tools: [{ name: "read", description: "read", parameters: { type: "object", properties: {} } }],
		};
		let inventory: ReturnType<typeof inspectProviderRequestToolInventory> | undefined;
		const stream = streamSimpleAnthropic(model, context, {
			apiKey: "sk-ant-oat-test",
			onPayload: (payload) => {
				inventory = inspectProviderRequestToolInventory(model.api, payload);
				throw new Error("halt-before-network");
			},
		});
		await stream.result();
		expect(inventory).toEqual({ status: "observed", toolNames: ["Read"] });
		expect(classifyToolVisibility(event(["read"], inventory!, model))).toMatchObject({
			classification: "parity",
			requestContextToolNames: ["read"],
			serializedToolNames: ["Read"],
		});
	});
});
