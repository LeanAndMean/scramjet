import { mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Context, Model } from "@leanandmean/ai";
import { createAssistantMessageEventStream } from "@leanandmean/ai";
import {
	AgentSession,
	type AgentSessionRuntime,
	AuthStorage,
	type CreateAgentSessionRuntimeFactory,
	createAgentSessionRuntime,
	DefaultResourceLoader,
	type ExtensionAPI,
	ModelRegistry,
	SessionManager,
	SettingsManager,
} from "@leanandmean/coding-agent";
import { afterEach, describe, expect, it, vi } from "vitest";
import { registerAutoContinue } from "../src/auto-continue.js";
import { COMMAND_STATUS_PROBE_TYPE } from "../src/command-status.js";
import { startCommand } from "../src/lifecycle.js";
import { registerModelChangeNotice } from "../src/model-change-notice.js";
import { registerModelIdentity } from "../src/model-identity.js";
import { MODEL_CHANGE_NOTICE_TOOL, type ScramjetState } from "../src/types.js";
import { freshState } from "./helpers.js";

type StreamMode = "success" | "fail-before-dispatch" | "fail-after-dispatch" | "overflow";
type CompactionMode = "success" | "cancel";

const modelA = testModel("model-a", "Model A");
const modelB = testModel("model-b", "Model B");
const modelC = testModel("model-c", "Model C");

function testModel(id: string, name: string): Model<"openai-chat"> {
	return {
		id,
		name,
		api: "openai-chat",
		provider: "openai",
		baseUrl: "https://api.openai.com",
		reasoning: false,
		input: ["text"],
		contextWindow: 1_000_000,
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	};
}

function assistantText(model: Model<"openai-chat">, text = `work by ${model.name}`): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text }],
		api: model.api,
		provider: model.provider,
		model: model.id,
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: "stop",
		timestamp: Date.now(),
	};
}

interface Capture {
	model: Model;
	systemPrompt: Context["systemPrompt"];
	messages: Context["messages"];
}

interface Fixture {
	session: AgentSession;
	sessionManager: SessionManager;
	captures: Capture[];
	state: ScramjetState;
	streamModes: StreamMode[];
	setCompactionMode(mode: CompactionMode): void;
	removeCompactionAuth(): void;
	restoreCompactionAuth(): void;
	pauseBeforeStream(): void;
	waitForPreparation(): Promise<void>;
	releaseStream(): void;
	drain(): Promise<void>;
}

async function makeFixture(options: { autoContinue?: boolean } = {}): Promise<Fixture> {
	const root = mkdtempSync(join(tmpdir(), "scramjet-model-identity-boundary-"));
	const cwd = join(root, "cwd");
	const agentDir = join(root, "agent");
	mkdirSync(cwd, { recursive: true });
	mkdirSync(agentDir, { recursive: true });

	const settingsManager = SettingsManager.inMemory();
	const sessionManager = SessionManager.inMemory(cwd);
	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("openai", "fake");
	const modelRegistry = ModelRegistry.create(authStorage, join(agentDir, "models.json"));
	const captures: Capture[] = [];
	const streamModes: StreamMode[] = [];
	let compactionMode: CompactionMode = "success";
	let streamGate: Promise<void> | null = null;
	let releaseStream = () => {};
	let preparationReached: Promise<void> = Promise.resolve();
	let markPreparationReached = () => {};
	const state = freshState();
	state.registry.set("test:command", {
		name: "test:command",
		filePath: "/test/command.md",
		body: "test command",
	});

	const factory = (pi: ExtensionAPI) => {
		registerModelIdentity(pi, state);
		registerModelChangeNotice(pi, state);
		if (options.autoContinue) registerAutoContinue(pi, state);
		pi.on("session_before_compact", (event) => {
			if (compactionMode === "cancel") return { cancel: true };
			const firstKeptEntryId = [...event.branchEntries]
				.reverse()
				.find((entry) => entry.type === "message" && (entry as any).message.role === "user")?.id;
			if (!firstKeptEntryId) throw new Error("expected a user entry to retain");
			return {
				compaction: {
					summary: "Earlier conversation compacted.",
					firstKeptEntryId,
					tokensBefore: event.preparation.tokensBefore,
					details: undefined,
				},
			};
		});
	};
	const resourceLoader = new DefaultResourceLoader({
		cwd,
		agentDir,
		settingsManager,
		extensionFactories: [factory],
	});
	await resourceLoader.reload();

	let session!: AgentSession;
	const agent = new Agent({
		initialState: { systemPrompt: "BASE", model: modelA, tools: [] },
		beforeProviderCall: (context, model) => (session as any)._extensionRunner.emitBeforeProviderCall(context, model),
		onPayload: (payload, model) => (session as any)._extensionRunner.emitBeforeProviderRequest(payload, model),
		streamFn: async (model, context, options) => {
			captures.push({ model, systemPrompt: context.systemPrompt, messages: [...context.messages] });
			const mode = streamModes.shift() ?? "success";
			if (mode === "fail-before-dispatch") throw new Error("failed before provider dispatch");
			await options?.onPayload?.({}, model);
			if (mode === "fail-after-dispatch") throw new Error("failed after provider dispatch");
			const message =
				mode === "overflow"
					? {
							...assistantText(model as Model<"openai-chat">),
							content: [{ type: "text" as const, text: "" }],
							stopReason: "error" as const,
							errorMessage: "Provider returned error: maximum context length is 1000000 tokens",
						}
					: assistantText(model as Model<"openai-chat">);
			const stream = createAssistantMessageEventStream();
			stream.push({ type: "start", partial: message });
			stream.push({ type: "done", reason: message.stopReason, message });
			return stream;
		},
		getApiKey: async () => {
			markPreparationReached();
			if (streamGate) await streamGate;
			return "fake";
		},
	});
	session = new AgentSession({
		agent,
		cwd,
		agentDir,
		modelRegistry,
		settingsManager,
		sessionManager,
		resourceLoader,
		sessionStartEvent: { type: "session_start", hasUI: false, mode: "sdk" } as never,
	});
	await session.bindExtensions({});

	return {
		session,
		sessionManager,
		captures,
		state,
		streamModes,
		setCompactionMode: (mode) => {
			compactionMode = mode;
		},
		removeCompactionAuth: () => authStorage.removeRuntimeApiKey("openai"),
		restoreCompactionAuth: () => authStorage.setRuntimeApiKey("openai", "fake"),
		pauseBeforeStream: () => {
			preparationReached = new Promise((resolve) => {
				markPreparationReached = resolve;
			});
			streamGate = new Promise((resolve) => {
				releaseStream = () => {
					streamGate = null;
					resolve();
				};
			});
		},
		waitForPreparation: () => preparationReached,
		releaseStream: () => releaseStream(),
		drain: () => (session as unknown as { _drainAgentEventQueue(): Promise<void> })._drainAgentEventQueue(),
	};
}

interface RuntimeFixture {
	runtime: AgentSessionRuntime;
	captures: Capture[];
	drain(): Promise<void>;
}

async function makeRuntimeFixture(): Promise<RuntimeFixture> {
	const root = mkdtempSync(join(tmpdir(), "scramjet-model-identity-runtime-"));
	const cwd = join(root, "cwd");
	const agentDir = join(root, "agent");
	const sessionDir = join(root, "sessions");
	mkdirSync(cwd, { recursive: true });
	mkdirSync(agentDir, { recursive: true });
	mkdirSync(sessionDir, { recursive: true });

	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("openai", "fake");
	const captures: Capture[] = [];
	const models = [modelA, modelB, modelC];

	const buildSession = async (
		sessionManager: SessionManager,
		sessionStartEvent: Parameters<CreateAgentSessionRuntimeFactory>[0]["sessionStartEvent"],
	) => {
		const settingsManager = SettingsManager.inMemory();
		const modelRegistry = ModelRegistry.create(authStorage, join(agentDir, "models.json"));
		const restored = sessionManager.buildSessionContext();
		const initialModel =
			models.find((model) => model.provider === restored.model?.provider && model.id === restored.model.modelId) ??
			modelA;
		const factory = (pi: ExtensionAPI) => registerModelIdentity(pi, freshState());
		const resourceLoader = new DefaultResourceLoader({
			cwd,
			agentDir,
			settingsManager,
			extensionFactories: [factory],
		});
		await resourceLoader.reload();

		let session!: AgentSession;
		const agent = new Agent({
			initialState: { systemPrompt: "BASE", model: initialModel, tools: [], messages: restored.messages },
			beforeProviderCall: (context, model) =>
				(session as any)._extensionRunner.emitBeforeProviderCall(context, model),
			onPayload: (payload, model) => (session as any)._extensionRunner.emitBeforeProviderRequest(payload, model),
			streamFn: async (model, context, options) => {
				captures.push({ model, systemPrompt: context.systemPrompt, messages: [...context.messages] });
				await options?.onPayload?.({}, model);
				const message = assistantText(model as Model<"openai-chat">);
				const stream = createAssistantMessageEventStream();
				stream.push({ type: "start", partial: message });
				stream.push({ type: "done", reason: "stop", message });
				return stream;
			},
			getApiKey: async () => "fake",
		});
		session = new AgentSession({
			agent,
			cwd,
			agentDir,
			modelRegistry,
			settingsManager,
			sessionManager,
			resourceLoader,
			sessionStartEvent,
		});
		return {
			session,
			modelFallbackMessage: undefined,
			services: { cwd, agentDir, settingsManager, modelRegistry, resourceLoader, authStorage },
			diagnostics: [],
		};
	};

	const createRuntime: CreateAgentSessionRuntimeFactory = (options) =>
		buildSession(options.sessionManager, options.sessionStartEvent);
	const sessionManager = SessionManager.create(cwd, sessionDir);
	const runtime = await createAgentSessionRuntime(createRuntime, { cwd, agentDir, sessionManager });
	runtime.setRebindSession((session) => session.bindExtensions({}));
	await runtime.session.bindExtensions({});

	return {
		runtime,
		captures,
		drain: () => (runtime.session as unknown as { _drainAgentEventQueue(): Promise<void> })._drainAgentEventQueue(),
	};
}

function promptText(capture: Capture): string {
	return typeof capture.systemPrompt === "string"
		? capture.systemPrompt
		: capture.systemPrompt.map((section) => section.text).join("\n");
}

function identityText(capture: Capture): string {
	return promptText(capture).split("# Model Identity")[1] ?? "";
}

function noticeCount(fx: Fixture): number {
	return fx.sessionManager
		.getEntries()
		.filter(
			(entry) =>
				entry.type === "message" &&
				(entry as any).message.role === "assistant" &&
				(entry as any).message.content.some(
					(part: any) => part.type === "toolCall" && part.name === MODEL_CHANGE_NOTICE_TOOL,
				),
		).length;
}

async function prompt(fx: Fixture, text: string): Promise<Capture> {
	const before = fx.captures.length;
	await fx.session.prompt(text, { source: "interactive" });
	await fx.drain();
	return fx.captures[before];
}

async function runFailedPrompt(fx: Fixture, text: string): Promise<Capture> {
	const before = fx.captures.length;
	await fx.session.prompt(text, { source: "interactive" });
	await fx.drain();
	return fx.captures[before];
}

describe("model identity at real provider and compaction boundaries", () => {
	let fx: Fixture | undefined;
	let runtimeFx: RuntimeFixture | undefined;

	afterEach(async () => {
		await fx?.session.dispose();
		await runtimeFx?.runtime.dispose().catch(() => {});
		fx = undefined;
		runtimeFx = undefined;
	});

	it("preserves current B and material A/B contributors across real manual compaction", async () => {
		fx = await makeFixture();
		await prompt(fx, "A contributes");
		await fx.session.setModel(modelB);
		await new Promise((resolve) => setTimeout(resolve, 550));
		await prompt(fx, "B contributes");
		expect(noticeCount(fx)).toBe(1);

		await fx.session.compact();
		const capture = await prompt(fx, "publish after compaction");
		const identity = identityText(capture);

		expect(capture.model.id).toBe("model-b");
		expect(identity).toContain("imminent response is: Model B");
		expect(identity).toContain("model-a (ID: model-a");
		expect(identity).toContain("model-b (ID: model-b");
		expect(identity).not.toContain('Single credited model: "Reviewed by Model A"');
		expect(
			capture.messages.some((message: any) =>
				message.content?.some?.((part: any) => part.name === MODEL_CHANGE_NOTICE_TOOL),
			),
		).toBe(false);
	});

	it("preserves a switch made after preparation when the older request dispatches", async () => {
		fx = await makeFixture();
		fx.pauseBeforeStream();
		const firstPrompt = fx.session.prompt("prepare A", { source: "interactive" });
		await fx.waitForPreparation();
		await fx.session.setModel(modelB);
		fx.releaseStream();
		await firstPrompt;
		await fx.drain();
		await vi.waitFor(() => expect(noticeCount(fx!)).toBe(1), { timeout: 1500 });

		const capture = await prompt(fx, "continue on B");

		expect(capture.model.id).toBe("model-b");
		expect(identityText(capture)).toContain("imminent response is: Model A");
		expect(
			capture.messages.some((message: any) =>
				message.content?.some?.((part: any) => part.name === MODEL_CHANGE_NOTICE_TOOL),
			),
		).toBe(true);
	});

	it("reopens identity for the automatic overflow retry", async () => {
		fx = await makeFixture();
		await prompt(fx, "A contributes");
		const continueSpy = vi.spyOn(fx.session.agent, "continue");
		const before = fx.captures.length;
		fx.streamModes.push("overflow");

		await fx.session.prompt("overflow", { source: "interactive" });
		await vi.waitFor(
			() => {
				expect(continueSpy).toHaveBeenCalled();
				expect(fx!.captures.length).toBeGreaterThan(before + 1);
			},
			{ timeout: 2000 },
		);
		await fx.drain();
		const retry = fx.captures[before + 1];

		expect(retry.model.id).toBe("model-a");
		expect(identityText(retry)).toContain("imminent response is: Model A");
		expect(identityText(retry)).toContain("model-a (ID: model-a");
	});

	it("allows the scheduled probe to freeze the first post-compaction identity safely", async () => {
		fx = await makeFixture({ autoContinue: true });
		await prompt(fx, "A contributes");
		await fx.session.setModel(modelB);
		await new Promise((resolve) => setTimeout(resolve, 550));
		await prompt(fx, "B contributes");
		await fx.session.compact();
		const before = fx.captures.length;
		startCommand(fx.state, "test:command");

		await (fx.session as any)._extensionRunner.emit({ type: "agent_end", messages: [] });
		await vi.waitFor(() => expect(fx!.captures.length).toBeGreaterThan(before));
		await fx.drain();
		const probe = fx.captures[before];
		const ordinary = await prompt(fx, "continue after probe");

		expect(probe.model.id).toBe("model-b");
		expect(
			fx.sessionManager
				.getEntries()
				.some((entry: any) => entry.type === "custom_message" && entry.customType === COMMAND_STATUS_PROBE_TYPE),
		).toBe(true);
		expect(identityText(probe)).toContain("imminent response is: Model B");
		expect(identityText(probe)).toContain("model-a (ID: model-a");
		expect(identityText(probe)).toContain("model-b (ID: model-b");
		expect(
			probe.messages.some((message: any) =>
				message.content?.some?.((part: any) => part.name === MODEL_CHANGE_NOTICE_TOOL),
			),
		).toBe(false);
		expect(identityText(ordinary)).toBe(identityText(probe));
	});

	it("reconstructs branch-local contributors across tree, fork, and resume boundaries", async () => {
		runtimeFx = await makeRuntimeFixture();
		const { runtime, captures } = runtimeFx;
		await runtime.session.prompt("A contributes", { source: "interactive" });
		await runtimeFx.drain();
		const commonTip = runtime.session.sessionManager.getLeafId();

		await runtime.session.setModel(modelB);
		await runtime.session.prompt("B contributes", { source: "interactive" });
		await runtimeFx.drain();
		const bTip = runtime.session.sessionManager.getLeafId();

		await runtime.session.navigateTree(commonTip);
		await runtime.session.setModel(modelC);
		await runtime.session.prompt("C contributes", { source: "interactive" });
		await runtimeFx.drain();
		const cTip = runtime.session.sessionManager.getLeafId();

		await runtime.session.navigateTree(bTip);
		await runtime.session.setModel(modelB);
		let before = captures.length;
		await runtime.session.prompt("inspect B branch", { source: "interactive" });
		await runtimeFx.drain();
		const branchB = identityText(captures[before]);
		expect(branchB).toContain("model-a (ID: model-a");
		expect(branchB).toContain("model-b (ID: model-b");
		expect(branchB).not.toContain("model-c (ID: model-c");

		await runtime.session.navigateTree(cTip);
		await runtime.session.setModel(modelC);
		before = captures.length;
		await runtime.session.prompt("inspect C branch", { source: "interactive" });
		await runtimeFx.drain();
		const branchC = identityText(captures[before]);
		expect(branchC).toContain("model-a (ID: model-a");
		expect(branchC).toContain("model-c (ID: model-c");
		expect(branchC).not.toContain("model-b (ID: model-b");

		const originalPath = runtime.session.sessionFile;
		if (!originalPath) throw new Error("expected persisted session path");
		await runtime.fork(bTip, { position: "at" });
		before = captures.length;
		await runtime.session.prompt("inspect fork", { source: "interactive" });
		await runtimeFx.drain();
		const fork = identityText(captures[before]);
		expect(fork).toContain("model-a (ID: model-a");
		expect(fork).toContain("model-b (ID: model-b");
		expect(fork).not.toContain("model-c (ID: model-c");

		await runtime.switchSession(originalPath);
		before = captures.length;
		await runtime.session.prompt("inspect resumed C branch", { source: "interactive" });
		await runtimeFx.drain();
		const resumed = identityText(captures[before]);
		expect(resumed).toContain("model-a (ID: model-a");
		expect(resumed).toContain("model-c (ID: model-c");
		expect(resumed).not.toContain("model-b (ID: model-b");
	});

	it("keeps no-switch attribution coherent without creating a notice artifact", async () => {
		fx = await makeFixture();
		await prompt(fx, "A contributes once");
		await prompt(fx, "A contributes twice");
		await fx.session.compact();
		const capture = await prompt(fx, "A continues");

		expect(capture.model.id).toBe("model-a");
		expect(identityText(capture)).toContain("imminent response is: Model A");
		expect(identityText(capture)).toContain("- model-a (ID: model-a");
		expect(noticeCount(fx)).toBe(0);
	});

	it("keeps ordered unique contributors through repeated compaction epochs", async () => {
		fx = await makeFixture();
		await prompt(fx, "A contributes");
		await fx.session.setModel(modelB);
		await new Promise((resolve) => setTimeout(resolve, 550));
		await prompt(fx, "B contributes");
		await fx.session.compact();
		await fx.session.setModel(modelC);
		await prompt(fx, "C contributes");
		await fx.session.compact();
		const capture = await prompt(fx, "C continues");
		const frozenCapture = await prompt(fx, "C continues again");
		const identity = identityText(capture);

		expect(capture.model.id).toBe("model-c");
		expect(identityText(frozenCapture)).toBe(identity);
		expect(identity.indexOf("model-a (ID: model-a")).toBeLessThan(identity.indexOf("model-b (ID: model-b"));
		expect(identity.indexOf("model-b (ID: model-b")).toBeLessThan(identity.indexOf("model-c (ID: model-c"));
		expect(identity.match(/^- model-[abc] \(ID:/gm)).toHaveLength(3);
	});

	it("identifies selection-only C as current but not as a prior contributor", async () => {
		fx = await makeFixture();
		await prompt(fx, "A contributes");
		await prompt(fx, "A contributes again");
		await fx.session.compact();
		await fx.session.setModel(modelC);
		const capture = await prompt(fx, "first C request");
		const identity = identityText(capture);

		expect(capture.model.id).toBe("model-c");
		expect(identity).toContain("imminent response is: Model C");
		expect(identity).toContain("- model-a (ID: model-a");
		expect(identity).not.toContain("- model-c (ID: model-c");
	});

	it.each(["cancel", "failure"] as const)("leaves frozen identity unchanged after compaction %s", async (kind) => {
		fx = await makeFixture();
		const first = await prompt(fx, "freeze A");
		if (kind === "cancel") {
			await prompt(fx, "make compaction eligible");
			fx.setCompactionMode("cancel");
			await expect(fx.session.compact()).rejects.toThrow("Compaction cancelled");
		} else {
			fx.removeCompactionAuth();
			await expect(fx.session.compact()).rejects.toThrow("No API key");
			fx.restoreCompactionAuth();
		}
		const next = await prompt(fx, "continue A");
		expect(identityText(next)).toBe(identityText(first));
	});

	it("keeps an epoch open when the request fails before provider dispatch", async () => {
		fx = await makeFixture();
		fx.streamModes.push("fail-before-dispatch");
		await runFailedPrompt(fx, "prepare A");
		await fx.session.setModel(modelB);
		const retry = await prompt(fx, "retry on B");

		expect(retry.model.id).toBe("model-b");
		expect(identityText(retry)).toContain("imminent response is: Model B");
	});

	it("freezes after dispatch failure without crediting the failed response", async () => {
		fx = await makeFixture();
		fx.streamModes.push("fail-after-dispatch");
		const failed = await runFailedPrompt(fx, "dispatch A");
		await fx.session.setModel(modelB);
		const retry = await prompt(fx, "retry on B");
		const identity = identityText(retry);

		expect(retry.model.id).toBe("model-b");
		expect(identity).toBe(identityText(failed));
		expect(identity).toContain("imminent response is: Model A");
		expect(identity).toContain("No prior material provider responses");
	});
});
