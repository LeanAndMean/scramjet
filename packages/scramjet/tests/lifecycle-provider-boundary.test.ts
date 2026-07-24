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
	type SessionEntry,
	SessionManager,
	SettingsManager,
} from "@leanandmean/coding-agent";
import { afterEach, describe, expect, it } from "vitest";
import { registerAutoContinue } from "../src/auto-continue.js";
import { registerDormantCommandNotice } from "../src/command-status.js";
import { COMMAND_START_TYPE, registerHistory } from "../src/history.js";
import { activeCommandName, beginProbe, startCommand } from "../src/lifecycle.js";
import type { CommandDef, ScramjetState } from "../src/types.js";
import { registerUserInputTool } from "../src/user-input.js";
import { derivedPhase, freshState } from "./helpers.js";

// issue 352 (Stage 2): prove the provider-boundary invariant through real public
// operations. Stage 1 pinned replayHistory() in isolation; this file stands up a
// real deterministic Agent / AgentSession / AgentSessionRuntime with a persisted
// session, drives command starts through the real input handler, and then exercises
// the actual switchSession() / fork() / navigateTree() / compact() operations —
// asserting that the reconstructed dormant fact reaches the *next provider request's*
// Context.systemPrompt, not merely the in-memory lifecycle.
//
// The narrow Scramjet handlers (history, dormant notice, auto-continue) are installed
// through DefaultResourceLoader.extensionFactories in production order, over a fresh
// ScramjetState per session — mirroring production, where each session gets fresh state
// and replay is the only bridge across a session replacement.

const testModel: Model<"openai-chat"> = {
	id: "test-model",
	name: "Test Model",
	api: "openai-chat",
	provider: "openai",
	baseUrl: "https://api.openai.com",
	reasoning: false,
	input: ["text"],
	// A large context window keeps auto-compaction (the threshold check) from firing so the
	// only compaction is the explicit compact() the compaction test exercises.
	contextWindow: 1_000_000,
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

function cmdDef(name: string): CommandDef {
	return { name, filePath: `/fake/${name}.md`, body: `Body of ${name}` };
}

// A capture of one provider request: the assembled system prompt (sections or string)
// and a shallow copy of the message list.
interface StreamCapture {
	systemPrompt: Context["systemPrompt"];
	messages: Context["messages"];
}

interface Fixture {
	runtime: AgentSessionRuntime;
	captures: StreamCapture[];
	/** The ScramjetState of the current (possibly replaced) session. */
	state(): ScramjetState;
	/** Drain the current session's async agent-event queue (so agent_end handlers run). */
	drain(): Promise<void>;
	/**
	 * Start a top-level command through the real input path (journals a command-start) and
	 * drain agent_end. Used by the reconstruction cases, whose fixtures omit auto-continue,
	 * so no probe machinery runs and the command stays live (running) with only the
	 * command-start journaled — the shape replayHistory reconstructs as dormant.
	 */
	startCommand(name: string): Promise<void>;
	userInputTool(): any;
}

// auto-continue is registered only where its compaction stabilization is exercised.
// Its probe machinery would otherwise drive an async probe turn on every command-start
// work turn (via sendMessage(triggerTurn)), which is nondeterministic and irrelevant to
// the resume/fork/navigate reconstructions (delivered by history + the dormant notice).
async function makeFixture(opts: { autoContinue?: boolean } = {}): Promise<Fixture> {
	const root = mkdtempSync(join(tmpdir(), "scramjet-provider-boundary-"));
	const cwd = join(root, "cwd");
	const agentDir = join(root, "agent");
	const sessionDir = join(root, "sessions");
	mkdirSync(cwd, { recursive: true });
	mkdirSync(agentDir, { recursive: true });
	mkdirSync(sessionDir, { recursive: true });

	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("openai", "fake");

	const captures: StreamCapture[] = [];
	let currentState!: ScramjetState;
	let currentUserInputTool: any;

	const buildSession = async (
		sessionManager: SessionManager,
		sessionStartEvent: Parameters<CreateAgentSessionRuntimeFactory>[0]["sessionStartEvent"],
	) => {
		const settingsManager = SettingsManager.inMemory();
		const modelRegistry = ModelRegistry.create(authStorage, join(agentDir, "models.json"));

		// Fresh state per session, with the command registry seeded so the input handler
		// journals command starts and agent_end can resolve the active command's def.
		const state = freshState({ enabled: true });
		state.registry.set("a:cmd", cmdDef("a:cmd"));
		state.registry.set("b:cmd", cmdDef("b:cmd"));
		currentState = state;

		const factory = (pi: ExtensionAPI) => {
			// Production order: history (rebuild + input), dormant notice (system prompt
			// section), then auto-continue (compaction stabilization) where exercised.
			registerHistory(pi, state);
			registerDormantCommandNotice(pi, state);
			const inputPi = new Proxy(pi, {
				get(target, property, receiver) {
					if (property !== "registerTool") return Reflect.get(target, property, receiver);
					return (tool: any) => {
						currentUserInputTool = tool;
						target.registerTool(tool);
					};
				},
			});
			registerUserInputTool(inputPi, state);
			if (opts.autoContinue) registerAutoContinue(pi, state);
			// Test-local deterministic compaction output — no network. Mirrors an extension
			// that supplies compaction content via session_before_compact.
			pi.on("session_before_compact", async (event: unknown) => {
				const prep = (event as { preparation: { firstKeptEntryId: string; tokensBefore: number } }).preparation;
				return {
					compaction: {
						summary: "test compaction summary",
						firstKeptEntryId: prep.firstKeptEntryId,
						tokensBefore: prep.tokensBefore,
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

		const agent = new Agent({
			initialState: { systemPrompt: "", model: testModel, tools: [] },
			streamFn: (_model, context) => {
				captures.push({ systemPrompt: context.systemPrompt, messages: [...context.messages] });
				const message = assistantText("ok");
				const stream = createAssistantMessageEventStream();
				stream.push({ type: "start", partial: message });
				stream.push({ type: "done", reason: "stop", message });
				return stream;
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
			sessionStartEvent,
		});

		return {
			session,
			modelFallbackMessage: undefined,
			services: { cwd, agentDir, settingsManager, modelRegistry, resourceLoader, authStorage },
			diagnostics: [],
		};
	};

	const createRuntime: CreateAgentSessionRuntimeFactory = (opts) =>
		buildSession(opts.sessionManager, opts.sessionStartEvent);

	const sessionManager = SessionManager.create(cwd, sessionDir);
	const runtime = await createAgentSessionRuntime(createRuntime, { cwd, agentDir, sessionManager });

	// session_start fires from bindExtensions(), not the constructor — bind the initial
	// session and rebind after every switch/fork so replayHistory runs on the replacement.
	runtime.setRebindSession(async (session) => {
		await session.bindExtensions({});
	});
	await runtime.session.bindExtensions({});

	const drain = () =>
		(runtime.session as unknown as { _drainAgentEventQueue(): Promise<void> })._drainAgentEventQueue();

	return {
		runtime,
		captures,
		state: () => currentState,
		drain,
		startCommand: async (name: string) => {
			await runtime.session.prompt(`/${name}`, { source: "interactive" });
			await drain();
			// Defensive: cancel any lifecycle timers. A no-op in these fixtures (auto-continue
			// is not registered, so nothing schedules a probe), but keeps the helper robust if
			// a caller ever opts into auto-continue.
			currentState.clearLifecycleTimers?.();
		},
		userInputTool: () => currentUserInputTool,
	};
}

function systemPromptText(capture: StreamCapture): string {
	const sp = capture.systemPrompt;
	if (typeof sp === "string") return sp;
	if (Array.isArray(sp)) return sp.map((s) => s.text).join("\n");
	return "";
}

function dormantSectionCount(capture: StreamCapture): number {
	const sp = capture.systemPrompt;
	if (!Array.isArray(sp)) return 0;
	return sp.filter((s) => s.id === "scramjet:dormant-command").length;
}

function countOccurrences(haystack: string, needle: string): number {
	let count = 0;
	let idx = haystack.indexOf(needle);
	while (idx !== -1) {
		count++;
		idx = haystack.indexOf(needle, idx + needle.length);
	}
	return count;
}

// The decision-relevant dormant fact: heading present exactly once, the selected
// command named, and recovery instructions for both continuing and terminal reporting.
function expectDormantNotice(capture: StreamCapture, command: string): void {
	expect(dormantSectionCount(capture)).toBe(1);
	const text = systemPromptText(capture);
	expect(countOccurrences(text, "# Dormant Scramjet Command")).toBe(1);
	expect(text).toContain(`\`${command}\``);
	expect(text).toContain('"continuing"');
	expect(text).toContain('"completed"');
}

function expectNoDormantNotice(capture: StreamCapture): void {
	expect(dormantSectionCount(capture)).toBe(0);
	expect(systemPromptText(capture)).not.toContain("# Dormant Scramjet Command");
}

function userMessageEntries(runtime: AgentSessionRuntime): SessionEntry[] {
	return runtime.session.sessionManager
		.getBranch()
		.filter((e) => e.type === "message" && (e as { message: { role: string } }).message.role === "user");
}

describe("lifecycle provider boundary (issue 352 Stage 2)", () => {
	let fx: Fixture | undefined;

	afterEach(async () => {
		await fx?.runtime.dispose().catch(() => {});
		fx = undefined;
	});

	it("resume (switchSession) delivers the reconstructed dormant fact to the next provider request", async () => {
		fx = await makeFixture();
		await fx.startCommand("a:cmd");
		expect(activeCommandName(fx.state().lifecycle)).toBe("a:cmd");

		const sessionPath = fx.runtime.session.sessionFile;
		if (!sessionPath) throw new Error("expected a persisted session file");

		await fx.runtime.switchSession(sessionPath);
		expect(derivedPhase(fx.state().lifecycle)).toBe("dormant");
		expect(activeCommandName(fx.state().lifecycle)).toBe("a:cmd");

		const before = fx.captures.length;
		await fx.runtime.session.prompt("continue the work", { source: "interactive" });
		expect(fx.captures.length).toBeGreaterThan(before);
		expectDormantNotice(fx.captures[before], "a:cmd");
	});

	it("fork selects ancestry — the next provider request omits the later branch's command", async () => {
		fx = await makeFixture();
		await fx.startCommand("a:cmd");
		// Capture the tip of the a:cmd region before starting b:cmd, so the fork excludes
		// every b:cmd entry regardless of the (realistic) probe turn that follows each start.
		const aTip = fx.runtime.session.sessionManager.getLeafId();
		await fx.startCommand("b:cmd");

		// Fork at the a:cmd tip: the forked ancestry contains only a:cmd.
		await fx.runtime.fork(aTip, { position: "at" });
		expect(derivedPhase(fx.state().lifecycle)).toBe("dormant");
		expect(activeCommandName(fx.state().lifecycle)).toBe("a:cmd");

		const before = fx.captures.length;
		await fx.runtime.session.prompt("keep going", { source: "interactive" });
		const capture = fx.captures[before];
		expectDormantNotice(capture, "a:cmd");
		// The later branch's command must not leak into the selected ancestry.
		expect(systemPromptText(capture)).not.toContain("`b:cmd`");
	});

	it("tree navigation delivers the branch-appropriate dormant fact across sibling branches", async () => {
		fx = await makeFixture();

		// Branch A: a:cmd off the root.
		await fx.startCommand("a:cmd");
		const aTip = fx.runtime.session.sessionManager.getLeafId();
		const firstUser = userMessageEntries(fx.runtime)[0].id;

		// Rewind to the root (navigating to the first user message sets the leaf to its
		// parent), then start b:cmd as a sibling branch off the same root.
		await fx.runtime.session.navigateTree(firstUser);
		await fx.startCommand("b:cmd");
		const bTip = fx.runtime.session.sessionManager.getLeafId();

		// Navigate to sibling A → the next provider request carries a:cmd dormant.
		await fx.runtime.session.navigateTree(aTip);
		expect(activeCommandName(fx.state().lifecycle)).toBe("a:cmd");
		let before = fx.captures.length;
		await fx.runtime.session.prompt("inspect A", { source: "interactive" });
		expectDormantNotice(fx.captures[before], "a:cmd");

		// Navigate to sibling B → the next provider request carries b:cmd dormant.
		await fx.runtime.session.navigateTree(bTip);
		expect(activeCommandName(fx.state().lifecycle)).toBe("b:cmd");
		before = fx.captures.length;
		await fx.runtime.session.prompt("inspect B", { source: "interactive" });
		const captureB = fx.captures[before];
		expectDormantNotice(captureB, "b:cmd");
		expect(systemPromptText(captureB)).not.toContain("`a:cmd`");
	});

	it.each([
		["confirm", { type: "confirm", message: "Continue?" }, "yes"],
		["select", { type: "select", message: "Pick", options: [{ value: "a", label: "A" }] }, "a"],
	] as const)("tree navigation invalidates unresolved same-name %s input", async (_type, params, answer) => {
		fx = await makeFixture();
		await fx.startCommand("a:cmd");
		const commandStart = fx.runtime.session.sessionManager
			.getEntries()
			.find((entry) => entry.type === "custom" && entry.customType === COMMAND_START_TYPE);
		if (!commandStart) throw new Error("expected command-start entry");
		let resolveInput: (value: unknown) => void = () => {};
		const pending = fx.userInputTool().execute("pending-input", params, undefined, undefined, {
			ui: {
				custom: () =>
					new Promise((resolve) => {
						resolveInput = resolve;
					}),
			},
		});
		await Promise.resolve();

		await fx.runtime.session.navigateTree(commandStart.id);
		expect(activeCommandName(fx.state().lifecycle)).toBe("a:cmd");
		resolveInput(answer);
		const result = await pending;

		expect(result.details.error).toBe("stale-result");
		expect(derivedPhase(fx.state().lifecycle)).toBe("dormant");
	});

	it("compaction while probing stabilizes to dormant and delivers it immediately to the next provider request", async () => {
		fx = await makeFixture({ autoContinue: true });

		// Build enough history to compact with plain turns (no active command, no probe).
		await fx.runtime.session.prompt("hello one", { source: "interactive" });
		await fx.drain();
		await fx.runtime.session.prompt("hello two", { source: "interactive" });
		await fx.drain();

		// Establish the live probeInFlight precondition through the same lifecycle mutations
		// the real probe machinery uses (startCommand arms, beginProbe puts a probe in flight),
		// without a work turn that would trigger an async probe turn. The operation under test
		// is the real public compact() and its session_compact stabilization.
		startCommand(fx.state(), "a:cmd");
		beginProbe(fx.state(), "test-precondition");
		expect(derivedPhase(fx.state().lifecycle)).toBe("probing");

		await fx.runtime.session.compact();
		// session_compact stabilizes probing → dormant (no replay involved).
		expect(derivedPhase(fx.state().lifecycle)).toBe("dormant");
		expect(activeCommandName(fx.state().lifecycle)).toBe("a:cmd");

		const before = fx.captures.length;
		await fx.runtime.session.prompt("continue", { source: "interactive" });
		expectDormantNotice(fx.captures[before], "a:cmd");
	});

	// Exit negative control. A truly unknown slash clears the active command live and
	// journals a durable command-exited outcome, so replay reconstructs idle and the
	// resumed session shows no dormant notice.
	it("exited workflow (unknown slash) resumes to idle with no dormant notice", async () => {
		fx = await makeFixture();
		await fx.startCommand("a:cmd");

		// A genuinely unknown slash: clears the active command live and journals the durable exit.
		await fx.runtime.session.prompt("/typo-or-removed", { source: "interactive" });
		await fx.drain();
		expect(activeCommandName(fx.state().lifecycle)).toBeNull();

		const sessionPath = fx.runtime.session.sessionFile;
		if (!sessionPath) throw new Error("expected a persisted session file");
		await fx.runtime.switchSession(sessionPath);

		const before = fx.captures.length;
		await fx.runtime.session.prompt("anything", { source: "interactive" });
		expectNoDormantNotice(fx.captures[before]);
	});
});
