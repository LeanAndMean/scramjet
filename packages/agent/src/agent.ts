import {
	type AssistantMessage,
	type ImageContent,
	type Message,
	type Model,
	type SimpleStreamOptions,
	streamSimple,
	type TextContent,
	type ThinkingBudgets,
	type Transport,
} from "@leanandmean/ai";
import { executeHarnessToolCall, runAgentLoop, runAgentLoopContinue } from "./agent-loop.js";
import type {
	AfterToolCallContext,
	AfterToolCallResult,
	AgentContext,
	AgentEvent,
	AgentLoopConfig,
	AgentLoopTurnUpdate,
	AgentMessage,
	AgentState,
	AgentTool,
	AgentToolCall,
	BeforeToolBatchContext,
	BeforeToolCallContext,
	BeforeToolCallResult,
	PrepareNextTurnContext,
	QueueMode,
	StreamFn,
	ToolExecutionMode,
} from "./types.js";

export type { QueueMode } from "./types.js";

function defaultConvertToLlm(messages: AgentMessage[]): Message[] {
	return messages.filter(
		(message) => message.role === "user" || message.role === "assistant" || message.role === "toolResult",
	);
}

const EMPTY_USAGE = {
	input: 0,
	output: 0,
	cacheRead: 0,
	cacheWrite: 0,
	totalTokens: 0,
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
};

const DEFAULT_MODEL = {
	id: "unknown",
	name: "unknown",
	api: "unknown",
	provider: "unknown",
	baseUrl: "",
	reasoning: false,
	input: [],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 0,
	maxTokens: 0,
} satisfies Model<any>;

// SCRAMJET-DIVERGENCE: provider-safe tool-call id generation for harness-originated calls (#244).
// The id must satisfy every provider's constraint (Anthropic's `^[a-zA-Z0-9_-]+$`, bounded length)
// and must never embed raw model ids. A random suffix (not a bare counter) avoids collisions with
// ids already present in a resumed transcript.
export function generateHarnessToolCallId(): string {
	const uuid = globalThis.crypto?.randomUUID?.();
	const suffix = uuid
		? uuid.replace(/-/g, "")
		: `${Date.now().toString(36)}${Math.floor(Math.random() * 1e9).toString(36)}`;
	return `harness-tool-${suffix}`.slice(0, 64);
}

type MutableAgentState = Omit<AgentState, "isStreaming" | "streamingMessage" | "pendingToolCalls" | "errorMessage"> & {
	isStreaming: boolean;
	streamingMessage?: AgentMessage;
	pendingToolCalls: Set<string>;
	errorMessage?: string;
};

function createMutableAgentState(
	initialState?: Partial<Omit<AgentState, "pendingToolCalls" | "isStreaming" | "streamingMessage" | "errorMessage">>,
): MutableAgentState {
	let tools = initialState?.tools?.slice() ?? [];
	let messages = initialState?.messages?.slice() ?? [];

	return {
		systemPrompt: initialState?.systemPrompt ?? "",
		model: initialState?.model ?? DEFAULT_MODEL,
		thinkingLevel: initialState?.thinkingLevel ?? "off",
		get tools() {
			return tools;
		},
		set tools(nextTools: AgentTool<any>[]) {
			tools = nextTools.slice();
		},
		get messages() {
			return messages;
		},
		set messages(nextMessages: AgentMessage[]) {
			messages = nextMessages.slice();
		},
		isStreaming: false,
		streamingMessage: undefined,
		pendingToolCalls: new Set<string>(),
		errorMessage: undefined,
	};
}

/** Options for constructing an {@link Agent}. */
export interface AgentOptions {
	initialState?: Partial<Omit<AgentState, "pendingToolCalls" | "isStreaming" | "streamingMessage" | "errorMessage">>;
	convertToLlm?: (messages: AgentMessage[]) => Message[] | Promise<Message[]>;
	transformContext?: (messages: AgentMessage[], signal?: AbortSignal) => Promise<AgentMessage[]>;
	streamFn?: StreamFn;
	getApiKey?: (provider: string) => Promise<string | undefined> | string | undefined;
	onPayload?: SimpleStreamOptions["onPayload"];
	onResponse?: SimpleStreamOptions["onResponse"];
	beforeToolBatch?: (context: BeforeToolBatchContext, signal?: AbortSignal) => Promise<void>;
	beforeToolCall?: (context: BeforeToolCallContext, signal?: AbortSignal) => Promise<BeforeToolCallResult | undefined>;
	afterToolCall?: (context: AfterToolCallContext, signal?: AbortSignal) => Promise<AfterToolCallResult | undefined>;
	// SCRAMJET-DIVERGENCE: prepareNextTurn receives the live turn context (previously the loop
	// context it passes was discarded by the Agent adapter) (#244).
	prepareNextTurn?: (
		ctx: PrepareNextTurnContext,
		signal?: AbortSignal,
	) => Promise<AgentLoopTurnUpdate | undefined> | AgentLoopTurnUpdate | undefined;
	steeringMode?: QueueMode;
	followUpMode?: QueueMode;
	sessionId?: string;
	thinkingBudgets?: ThinkingBudgets;
	transport?: Transport;
	maxRetryDelayMs?: number;
	toolExecution?: ToolExecutionMode;
}

class PendingMessageQueue {
	private messages: AgentMessage[] = [];

	constructor(public mode: QueueMode) {}

	enqueue(message: AgentMessage): void {
		this.messages.push(message);
	}

	hasItems(): boolean {
		return this.messages.length > 0;
	}

	drain(): AgentMessage[] {
		if (this.mode === "all") {
			const drained = this.messages.slice();
			this.messages = [];
			return drained;
		}

		const first = this.messages[0];
		if (!first) {
			return [];
		}
		this.messages = this.messages.slice(1);
		return [first];
	}

	clear(): void {
		this.messages = [];
	}
}

type ActiveRun = {
	promise: Promise<void>;
	resolve: () => void;
	abortController: AbortController;
};

// SCRAMJET-DIVERGENCE: harness-tool-invocation settlement records (#341).
/**
 * One harness-tool invocation and the deferred promise that `runHarnessTool` returns for it. The
 * promise settles only when the invocation's Agent-core pipeline completes (resolve) or when
 * reset/teardown revokes the invocation or an infrastructure failure escapes the pipeline (reject).
 */
type HarnessToolInvocation = {
	tool: AgentTool<any>;
	args: unknown;
	toolCallId: string;
	promise: Promise<void>;
	resolve: () => void;
	reject: (reason: Error) => void;
};

/**
 * Stateful wrapper around the low-level agent loop.
 *
 * `Agent` owns the current transcript, emits lifecycle events, executes tools,
 * and exposes queueing APIs for steering and follow-up messages.
 */
export class Agent {
	private _state: MutableAgentState;
	private readonly listeners = new Set<(event: AgentEvent, signal: AbortSignal) => Promise<void> | void>();
	private readonly steeringQueue: PendingMessageQueue;
	private readonly followUpQueue: PendingMessageQueue;

	public convertToLlm: (messages: AgentMessage[]) => Message[] | Promise<Message[]>;
	public transformContext?: (messages: AgentMessage[], signal?: AbortSignal) => Promise<AgentMessage[]>;
	public streamFn: StreamFn;
	public getApiKey?: (provider: string) => Promise<string | undefined> | string | undefined;
	public onPayload?: SimpleStreamOptions["onPayload"];
	public onResponse?: SimpleStreamOptions["onResponse"];
	// SCRAMJET-DIVERGENCE: beforeToolBatch hook — async message_end mutations must settle before tool-call extraction
	public beforeToolBatch?: (context: BeforeToolBatchContext, signal?: AbortSignal) => Promise<void>;
	public beforeToolCall?: (
		context: BeforeToolCallContext,
		signal?: AbortSignal,
	) => Promise<BeforeToolCallResult | undefined>;
	public afterToolCall?: (
		context: AfterToolCallContext,
		signal?: AbortSignal,
	) => Promise<AfterToolCallResult | undefined>;
	public prepareNextTurn?: (
		ctx: PrepareNextTurnContext,
		signal?: AbortSignal,
	) => Promise<AgentLoopTurnUpdate | undefined> | AgentLoopTurnUpdate | undefined;
	private activeRun?: ActiveRun;
	// SCRAMJET-DIVERGENCE: harness-tool-invocation primitive state (#244, settlement #341).
	/** Harness tool invocations queued while a run is active, drained at the next turn boundary. */
	private readonly harnessToolQueue: HarnessToolInvocation[] = [];
	/**
	 * Every harness-tool invocation whose public promise has not yet settled — queued, shifted and
	 * draining, or transient. Reset/teardown rejects this set; each invocation is removed as it
	 * settles. Queue length is not a proxy for this: a draining call has left the queue but is still
	 * unsettled (#341).
	 */
	private readonly unsettledHarnessTools = new Set<HarnessToolInvocation>();
	/**
	 * Markers for in-flight transient (idle) harness-tool runs; each resolves when its run settles.
	 * A set (not a single field) keeps nested and overlapping transient runs valid, and lets a new
	 * real run wait for transient work to finish before emitting its own events.
	 */
	private readonly transientRuns = new Set<Promise<void>>();
	/** Shared, never-aborted signal delivered to listeners during transient harness-tool runs. */
	private readonly transientRunAbort = new AbortController();
	/** Session identifier forwarded to providers for cache-aware backends. */
	public sessionId?: string;
	/** Optional per-level thinking token budgets forwarded to the stream function. */
	public thinkingBudgets?: ThinkingBudgets;
	/** Preferred transport forwarded to the stream function. */
	public transport: Transport;
	/** Optional cap for provider-requested retry delays. */
	public maxRetryDelayMs?: number;
	/** Tool execution strategy for assistant messages that contain multiple tool calls. */
	public toolExecution: ToolExecutionMode;

	constructor(options: AgentOptions = {}) {
		this._state = createMutableAgentState(options.initialState);
		this.convertToLlm = options.convertToLlm ?? defaultConvertToLlm;
		this.transformContext = options.transformContext;
		this.streamFn = options.streamFn ?? streamSimple;
		this.getApiKey = options.getApiKey;
		this.onPayload = options.onPayload;
		this.onResponse = options.onResponse;
		this.beforeToolBatch = options.beforeToolBatch;
		this.beforeToolCall = options.beforeToolCall;
		this.afterToolCall = options.afterToolCall;
		this.prepareNextTurn = options.prepareNextTurn;
		this.steeringQueue = new PendingMessageQueue(options.steeringMode ?? "one-at-a-time");
		this.followUpQueue = new PendingMessageQueue(options.followUpMode ?? "one-at-a-time");
		this.sessionId = options.sessionId;
		this.thinkingBudgets = options.thinkingBudgets;
		this.transport = options.transport ?? "auto";
		this.maxRetryDelayMs = options.maxRetryDelayMs;
		this.toolExecution = options.toolExecution ?? "parallel";
	}

	/**
	 * Subscribe to agent lifecycle events.
	 *
	 * Listener promises are awaited in subscription order and are included in
	 * the current run's settlement. Listeners also receive the active abort
	 * signal for the current run.
	 *
	 * `agent_end` is the final emitted event for a run, but the agent does not
	 * become idle until all awaited listeners for that event have settled.
	 */
	subscribe(listener: (event: AgentEvent, signal: AbortSignal) => Promise<void> | void): () => void {
		this.listeners.add(listener);
		return () => this.listeners.delete(listener);
	}

	/**
	 * Current agent state.
	 *
	 * Assigning `state.tools` or `state.messages` copies the provided top-level array.
	 */
	get state(): AgentState {
		return this._state;
	}

	/** Controls how queued steering messages are drained. */
	set steeringMode(mode: QueueMode) {
		this.steeringQueue.mode = mode;
	}

	get steeringMode(): QueueMode {
		return this.steeringQueue.mode;
	}

	/** Controls how queued follow-up messages are drained. */
	set followUpMode(mode: QueueMode) {
		this.followUpQueue.mode = mode;
	}

	get followUpMode(): QueueMode {
		return this.followUpQueue.mode;
	}

	/** Queue a message to be injected after the current assistant turn finishes. */
	steer(message: AgentMessage): void {
		this.steeringQueue.enqueue(message);
	}

	/** Queue a message to run only after the agent would otherwise stop. */
	followUp(message: AgentMessage): void {
		this.followUpQueue.enqueue(message);
	}

	/** Remove all queued steering messages. */
	clearSteeringQueue(): void {
		this.steeringQueue.clear();
	}

	/** Remove all queued follow-up messages. */
	clearFollowUpQueue(): void {
		this.followUpQueue.clear();
	}

	/** Remove all queued steering and follow-up messages. */
	clearAllQueues(): void {
		this.clearSteeringQueue();
		this.clearFollowUpQueue();
	}

	/** Returns true when either queue still contains pending messages. */
	hasQueuedMessages(): boolean {
		return this.steeringQueue.hasItems() || this.followUpQueue.hasItems();
	}

	/** Active abort signal for the current run, if any. */
	get signal(): AbortSignal | undefined {
		return this.activeRun?.abortController.signal;
	}

	/** Abort the current run, if one is active. */
	abort(): void {
		this.activeRun?.abortController.abort();
	}

	/**
	 * Resolve when the current run and all awaited event listeners have finished.
	 *
	 * This resolves after `agent_end` listeners settle.
	 */
	waitForIdle(): Promise<void> {
		return this.activeRun?.promise ?? Promise.resolve();
	}

	/** Clear transcript state, runtime state, and queued messages. */
	reset(): void {
		this._state.messages = [];
		this._state.isStreaming = false;
		this._state.streamingMessage = undefined;
		this._state.pendingToolCalls = new Set<string>();
		this._state.errorMessage = undefined;
		this.clearFollowUpQueue();
		this.clearSteeringQueue();
		// SCRAMJET-DIVERGENCE: harness-tool-invocation primitive (#244, settlement #341). reset()
		// rejects the public promise of every unsettled harness invocation so awaiting callers stop
		// waiting. The effect on the record row differs by kind: a queued (not-yet-draining) call loses
		// its row entirely, while a transient in-flight call keeps running — reset() does not abort
		// transientRunAbort — so its row is still emitted and persisted; only its promise is rejected.
		// The warning surfaces the rejection so a lost queued row is diagnosable.
		if (this.unsettledHarnessTools.size > 0) {
			const names = [...this.unsettledHarnessTools].map((record) => record.tool.name).join(", ");
			console.warn(
				`Agent.reset() rejected ${this.unsettledHarnessTools.size} unsettled harness tool call(s): ${names}`,
			);
		}
		this.rejectUnsettledHarnessTools(new Error("Agent.reset() discarded unsettled harness tool call"));
	}

	/** Start a new prompt from text, a single message, or a batch of messages. */
	async prompt(message: AgentMessage | AgentMessage[]): Promise<void>;
	async prompt(input: string, images?: ImageContent[]): Promise<void>;
	async prompt(input: string | AgentMessage | AgentMessage[], images?: ImageContent[]): Promise<void> {
		if (this.activeRun) {
			throw new Error(
				"Agent is already processing a prompt. Use steer() or followUp() to queue messages, or wait for completion.",
			);
		}
		const messages = this.normalizePromptInput(input, images);
		await this.runPromptMessages(messages);
	}

	/** Continue from the current transcript. The last message must be a user or tool-result message. */
	async continue(): Promise<void> {
		if (this.activeRun) {
			throw new Error("Agent is already processing. Wait for completion before continuing.");
		}

		const lastMessage = this._state.messages[this._state.messages.length - 1];
		if (!lastMessage) {
			throw new Error("No messages to continue from");
		}

		if (lastMessage.role === "assistant") {
			const queuedSteering = this.steeringQueue.drain();
			if (queuedSteering.length > 0) {
				await this.runPromptMessages(queuedSteering, { skipInitialSteeringPoll: true });
				return;
			}

			const queuedFollowUps = this.followUpQueue.drain();
			if (queuedFollowUps.length > 0) {
				await this.runPromptMessages(queuedFollowUps);
				return;
			}

			throw new Error("Cannot continue from message role: assistant");
		}

		await this.runContinuation();
	}

	private normalizePromptInput(
		input: string | AgentMessage | AgentMessage[],
		images?: ImageContent[],
	): AgentMessage[] {
		if (Array.isArray(input)) {
			return input;
		}

		if (typeof input !== "string") {
			return [input];
		}

		const content: Array<TextContent | ImageContent> = [{ type: "text", text: input }];
		if (images && images.length > 0) {
			content.push(...images);
		}
		return [{ role: "user", content, timestamp: Date.now() }];
	}

	private async runPromptMessages(
		messages: AgentMessage[],
		options: { skipInitialSteeringPoll?: boolean } = {},
	): Promise<void> {
		await this.runWithLifecycle(async (signal) => {
			await runAgentLoop(
				messages,
				this.createContextSnapshot(),
				this.createLoopConfig(options),
				(event) => this.processEvents(event),
				signal,
				this.streamFn,
			);
		});
	}

	private async runContinuation(): Promise<void> {
		await this.runWithLifecycle(async (signal) => {
			await runAgentLoopContinue(
				this.createContextSnapshot(),
				this.createLoopConfig(),
				(event) => this.processEvents(event),
				signal,
				this.streamFn,
			);
		});
	}

	private createContextSnapshot(): AgentContext {
		return {
			systemPrompt: this._state.systemPrompt,
			messages: this._state.messages.slice(),
			tools: this._state.tools.slice(),
		};
	}

	private createLoopConfig(options: { skipInitialSteeringPoll?: boolean } = {}): AgentLoopConfig {
		let skipInitialSteeringPoll = options.skipInitialSteeringPoll === true;
		return {
			model: this._state.model,
			reasoning: this._state.thinkingLevel === "off" ? undefined : this._state.thinkingLevel,
			sessionId: this.sessionId,
			onPayload: this.onPayload,
			onResponse: this.onResponse,
			transport: this.transport,
			thinkingBudgets: this.thinkingBudgets,
			maxRetryDelayMs: this.maxRetryDelayMs,
			toolExecution: this.toolExecution,
			beforeToolBatch: this.beforeToolBatch,
			beforeToolCall: this.beforeToolCall,
			afterToolCall: this.afterToolCall,
			// SCRAMJET-DIVERGENCE: always run prepareNextTurn so mid-run harness tool calls drain
			// into the live turn context before the next intra-run LLM call, and so the next call
			// self-heals onto the current model (routing after a mid-run switch) (#244).
			prepareNextTurn: async (ctx) => {
				await this.drainHarnessToolQueue(ctx, this.signal);
				const update = await this.prepareNextTurn?.(ctx, this.signal);
				return { ...update, model: update?.model ?? this._state.model };
			},
			convertToLlm: this.convertToLlm,
			transformContext: this.transformContext,
			getApiKey: this.getApiKey,
			getSteeringMessages: async () => {
				if (skipInitialSteeringPoll) {
					skipInitialSteeringPoll = false;
					return [];
				}
				return this.steeringQueue.drain();
			},
			getFollowUpMessages: async () => this.followUpQueue.drain(),
		};
	}

	private async runWithLifecycle(executor: (signal: AbortSignal) => Promise<void>): Promise<void> {
		if (this.activeRun) {
			throw new Error("Agent is already processing.");
		}

		const abortController = new AbortController();
		let resolvePromise = () => {};
		const promise = new Promise<void>((resolve) => {
			resolvePromise = resolve;
		});
		this.activeRun = { promise, resolve: resolvePromise, abortController };

		this._state.isStreaming = true;
		this._state.streamingMessage = undefined;
		this._state.errorMessage = undefined;

		// SCRAMJET-DIVERGENCE (#341): a failed final flush leaves later queued calls unable to execute.
		// Capture it (rejecting remnants below) and re-throw after finishRun so the error propagates out
		// of prompt()/continue() — not activeRun.promise, which finishRun always resolves — without a
		// lint-flagged throw inside a finally block. The { error } wrapper (not a bare `unknown` field)
		// is deliberate: it distinguishes "no drain error" from a falsy throw (e.g. `throw undefined`),
		// which a `!== undefined` check on a bare field would silently swallow.
		let finalDrainError: { error: unknown } | undefined;
		try {
			// Wait for in-flight transient harness-tool runs so their events do not interleave
			// with this run's transcript.
			while (this.transientRuns.size > 0) {
				await Promise.all(this.transientRuns);
			}
			await executor(abortController.signal);
		} catch (error) {
			await this.handleRunFailure(error, abortController.signal.aborted);
		} finally {
			try {
				// Flush harness tool calls queued during the run's final turn — including failed or
				// aborted runs — after the last prepareNextTurn drain point, so a queued call is
				// never stranded. Emits messages/tool events but no run framing.
				await this.drainHarnessToolQueue(undefined, abortController.signal);
			} catch (drainError) {
				// The failing call was already rejected by executeHarnessInvocation; reject the
				// remnants too so their promises settle.
				this.rejectUnsettledHarnessTools(drainError instanceof Error ? drainError : new Error(String(drainError)));
				finalDrainError = { error: drainError };
			} finally {
				this.finishRun();
			}
		}
		if (finalDrainError) {
			throw finalDrainError.error;
		}
	}

	private async handleRunFailure(error: unknown, aborted: boolean): Promise<void> {
		const failureMessage = {
			role: "assistant",
			content: [{ type: "text", text: "" }],
			api: this._state.model.api,
			provider: this._state.model.provider,
			model: this._state.model.id,
			usage: EMPTY_USAGE,
			stopReason: aborted ? "aborted" : "error",
			errorMessage: error instanceof Error ? error.message : String(error),
			timestamp: Date.now(),
		} satisfies AgentMessage;
		await this.processEvents({ type: "message_start", message: failureMessage });
		await this.processEvents({ type: "message_end", message: failureMessage });
		await this.processEvents({ type: "turn_end", message: failureMessage, toolResults: [] });
		await this.processEvents({ type: "agent_end", messages: [failureMessage] });
	}

	private finishRun(): void {
		this._state.isStreaming = false;
		this._state.streamingMessage = undefined;
		this._state.pendingToolCalls = new Set<string>();
		this.activeRun?.resolve();
		this.activeRun = undefined;
	}

	/**
	 * Reduce internal state for a loop event, then await listeners.
	 *
	 * `agent_end` only means no further loop events will be emitted. The run is
	 * considered idle later, after all awaited listeners for `agent_end` finish
	 * and `finishRun()` clears runtime-owned state.
	 */
	private async processEvents(event: AgentEvent): Promise<void> {
		switch (event.type) {
			case "message_start":
				this._state.streamingMessage = event.message;
				break;

			case "message_update":
				this._state.streamingMessage = event.message;
				break;

			case "message_end":
				this._state.streamingMessage = undefined;
				this._state.messages.push(event.message);
				break;

			case "tool_execution_start": {
				const pendingToolCalls = new Set(this._state.pendingToolCalls);
				pendingToolCalls.add(event.toolCallId);
				this._state.pendingToolCalls = pendingToolCalls;
				break;
			}

			case "tool_execution_end": {
				const pendingToolCalls = new Set(this._state.pendingToolCalls);
				pendingToolCalls.delete(event.toolCallId);
				this._state.pendingToolCalls = pendingToolCalls;
				break;
			}

			case "turn_end":
				if (event.message.role === "assistant" && event.message.errorMessage) {
					this._state.errorMessage = event.message.errorMessage;
				}
				break;

			case "agent_end":
				this._state.streamingMessage = undefined;
				break;
		}

		// SCRAMJET-DIVERGENCE: fall back to the shared transient-run signal while transient
		// harness-tool work (which intentionally has no `activeRun`) is in flight (#244).
		const signal =
			this.activeRun?.abortController.signal ??
			(this.transientRuns.size > 0 ? this.transientRunAbort.signal : undefined);
		if (!signal) {
			throw new Error("Agent listener invoked outside active run");
		}
		for (const listener of this.listeners) {
			await listener(event, signal);
		}
	}

	// SCRAMJET-DIVERGENCE: harness-tool-invocation primitive (#244, execution settlement #341).
	/**
	 * Execute a harness-originated tool call through the real execution pipeline.
	 *
	 * The tool object is supplied directly, so it need not appear in `state.tools` — this is what
	 * lets LLM-invisible harness tools execute. The call produces the same `tool_execution_*` and
	 * message events, persisted messages, and `beforeToolCall`/`afterToolCall` invocations as a
	 * normal tool execution, but emits **no** run/turn framing events (`agent_start`, `turn_start`,
	 * `turn_end`, `agent_end`). It is therefore intentionally invisible to every `agent_end`-keyed
	 * consumer (probe scheduling, compaction bookkeeping).
	 *
	 * When idle, the call executes immediately in a transient run scope. When a run is active, it is
	 * queued and drained at the next turn boundary — before the next intra-run LLM call — with an
	 * end-of-run flush for calls queued during the run's final turn, so a queued call is never stranded.
	 *
	 * **Settlement (#341).** The returned promise resolves only after the call's full Agent-core
	 * pipeline finishes: the synthetic assistant tool-call message, tool prepare/execute/finalize, the
	 * tool-result message, Agent state reduction, and every awaited listener for those events. A queued
	 * call therefore does not resolve at enqueue time — it resolves when the drain executes it. The
	 * promise rejects when reset or session teardown revokes an unsettled invocation, when an
	 * infrastructure/listener/hook failure escapes the pipeline, or when a failed final drain leaves a
	 * later queued call unable to execute. A tool's own `execute()` throwing is **not** a rejection: it
	 * becomes a normal `isError: true` tool result, so the promise resolves after that error-result
	 * pipeline completes. Rejection means the promised pipeline did not complete — it does not prove
	 * artifact absence (state and persistence may have partially completed), so callers must not blindly
	 * retry.
	 *
	 * This settles Agent-core execution only. It does **not** prove AgentSession persistence;
	 * `AgentSession.invokeHarnessTool` layers the persisted-settlement boundary on top of it.
	 *
	 * **Reentrancy.** Do not `await` this from work whose return is required for the Agent to reach the
	 * drain point (a model-callable `execute`, `beforeToolBatch`/`beforeToolCall`/`afterToolCall`/
	 * `prepareNextTurn`, or an awaited listener), or the queued call can never execute and the await
	 * deadlocks. Start/capture the promise, return from the gating callback, and await it from an
	 * independent continuation.
	 */
	async runHarnessTool(tool: AgentTool<any>, args: unknown, options: { toolCallId?: string } = {}): Promise<void> {
		const toolCallId = options.toolCallId ?? generateHarnessToolCallId();
		const record = this.createHarnessInvocation(tool, args, toolCallId);
		if (this.activeRun) {
			this.harnessToolQueue.push(record);
			return record.promise;
		}
		// Register the marker before the first event so processEvents sees transient work in
		// flight from the start, including for nested calls (a harness tool invoking another). The
		// marker tracks the underlying work even if reset/teardown rejects the public promise first,
		// so a later prompt still waits for the transient events to finish before emitting its own.
		let settleMarker!: () => void;
		const marker = new Promise<void>((resolve) => {
			settleMarker = resolve;
		});
		this.transientRuns.add(marker);
		void (async () => {
			try {
				await this.executeHarnessInvocation(record, this.transientRunAbort.signal, undefined);
			} catch {
				// The rejection is already recorded on record.promise; a transient run has no run-failure
				// path to propagate into, so nothing escapes here.
			} finally {
				this.transientRuns.delete(marker);
				settleMarker();
			}
		})();
		return record.promise;
	}

	// SCRAMJET-DIVERGENCE (#341): back a harness-tool invocation with a deferred promise and track it
	// as unsettled until it resolves or is rejected.
	private createHarnessInvocation(tool: AgentTool<any>, args: unknown, toolCallId: string): HarnessToolInvocation {
		let resolve!: () => void;
		let reject!: (reason: Error) => void;
		const promise = new Promise<void>((res, rej) => {
			resolve = res;
			reject = rej;
		});
		const record: HarnessToolInvocation = { tool, args, toolCallId, promise, resolve, reject };
		this.unsettledHarnessTools.add(record);
		return record;
	}

	// SCRAMJET-DIVERGENCE (#341): settle a harness invocation exactly once. Set membership is the
	// settled flag: unsettledHarnessTools.delete() returns false when the record was already removed
	// (e.g. underlying transient work finishing after reset already rejected it), so a later attempt
	// is a no-op.
	private settleHarnessInvocation(record: HarnessToolInvocation, error?: Error): void {
		if (!this.unsettledHarnessTools.delete(record)) return;
		if (error) record.reject(error);
		else record.resolve();
	}

	// SCRAMJET-DIVERGENCE (#341): run one invocation's pipeline and settle its record. Infrastructure
	// failures reject the record and rethrow so the drain path preserves current run-failure behavior;
	// a tool's own execute() error is not seen here (it resolves as an isError tool result).
	private async executeHarnessInvocation(
		record: HarnessToolInvocation,
		signal: AbortSignal | undefined,
		ctx: PrepareNextTurnContext | undefined,
	): Promise<void> {
		try {
			await this.emitHarnessToolCall(record.tool, record.args, record.toolCallId, signal, ctx);
		} catch (error) {
			this.settleHarnessInvocation(record, error instanceof Error ? error : new Error(String(error)));
			throw error;
		}
		this.settleHarnessInvocation(record);
	}

	// SCRAMJET-DIVERGENCE (#341): idempotently clear the queue and reject every still-unsettled
	// invocation (queued, draining, or transient). Used by reset() and by the final-drain failure path.
	rejectUnsettledHarnessTools(reason: Error): void {
		this.harnessToolQueue.length = 0;
		for (const record of [...this.unsettledHarnessTools]) {
			this.settleHarnessInvocation(record, reason);
		}
	}

	/** Drain all queued harness tool calls, splicing them into `ctx` when draining mid-run. */
	private async drainHarnessToolQueue(
		ctx: PrepareNextTurnContext | undefined,
		signal: AbortSignal | undefined,
	): Promise<void> {
		while (this.harnessToolQueue.length > 0) {
			const record = this.harnessToolQueue.shift()!;
			await this.executeHarnessInvocation(record, signal, ctx);
		}
	}

	/**
	 * Emit the full event sequence for one harness tool call: a synthetic single-toolCall assistant
	 * message, then the tool execution and its result message. When `ctx` is provided (mid-run drain),
	 * the synthetic messages are also spliced into the live turn context and returned-message set so
	 * the next intra-run LLM call includes them.
	 */
	private async emitHarnessToolCall(
		tool: AgentTool<any>,
		args: unknown,
		toolCallId: string,
		signal: AbortSignal | undefined,
		ctx: PrepareNextTurnContext | undefined,
	): Promise<void> {
		const toolCall: AgentToolCall = {
			type: "toolCall",
			id: toolCallId,
			name: tool.name,
			arguments: (args ?? {}) as Record<string, any>,
		};
		const assistantMessage = this.createHarnessAssistantMessage(toolCall);
		await this.processEvents({ type: "message_start", message: assistantMessage });
		await this.processEvents({ type: "message_end", message: assistantMessage });
		if (ctx) {
			ctx.context.messages.push(assistantMessage);
			ctx.newMessages.push(assistantMessage);
		}

		const hookContext = ctx?.context ?? this.createContextSnapshot();
		const toolResultMessage = await executeHarnessToolCall(
			tool,
			toolCall,
			hookContext,
			assistantMessage,
			{ beforeToolCall: this.beforeToolCall, afterToolCall: this.afterToolCall },
			signal,
			(event) => this.processEvents(event),
		);
		if (ctx) {
			ctx.context.messages.push(toolResultMessage);
			ctx.newMessages.push(toolResultMessage);
		}
	}

	private createHarnessAssistantMessage(toolCall: AgentToolCall): AssistantMessage {
		return {
			role: "assistant",
			content: [toolCall],
			api: this._state.model.api,
			provider: this._state.model.provider,
			model: this._state.model.id,
			usage: EMPTY_USAGE,
			stopReason: "toolUse",
			timestamp: Date.now(),
		};
	}
}
