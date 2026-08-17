import { mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Context, Model } from "@leanandmean/ai";
import { createAssistantMessageEventStream } from "@leanandmean/ai";
import {
	AgentSession,
	AuthStorage,
	DefaultResourceLoader,
	type ExtensionAPI,
	ModelRegistry,
	SessionManager,
	SettingsManager,
} from "@leanandmean/coding-agent";
import { afterEach, describe, expect, it } from "vitest";
import { registerModelChangeNotice } from "../src/model-change-notice.js";
import { registerModelIdentity } from "../src/model-identity.js";
import { MODEL_CHANGE_NOTICE_TOOL } from "../src/types.js";
import { freshState } from "./helpers.js";

type StreamMode = "success" | "fail-before-dispatch" | "fail-after-dispatch";
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
	streamModes: StreamMode[];
	setCompactionMode(mode: CompactionMode): void;
	removeCompactionAuth(): void;
	restoreCompactionAuth(): void;
	drain(): Promise<void>;
}

async function makeFixture(): Promise<Fixture> {
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

	const factory = (pi: ExtensionAPI) => {
		const state = freshState();
		registerModelIdentity(pi, state);
		registerModelChangeNotice(pi, state);
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
		sessionStartEvent: { type: "session_start", hasUI: false, mode: "sdk" } as never,
	});
	await session.bindExtensions({});

	return {
		session,
		sessionManager,
		captures,
		streamModes,
		setCompactionMode: (mode) => {
			compactionMode = mode;
		},
		removeCompactionAuth: () => authStorage.removeRuntimeApiKey("openai"),
		restoreCompactionAuth: () => authStorage.setRuntimeApiKey("openai", "fake"),
		drain: () => (session as unknown as { _drainAgentEventQueue(): Promise<void> })._drainAgentEventQueue(),
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

	afterEach(async () => {
		await fx?.session.dispose();
		fx = undefined;
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
		const identity = identityText(capture);

		expect(capture.model.id).toBe("model-c");
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
