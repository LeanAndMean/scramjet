# Pi API Surface

Generated from Pi 0.74.0 / pi-coding-agent 0.74.0-scramjet.4.

This file is generated from installed TypeScript declaration files. Do not edit it directly; run `node scripts/generate-pi-api-surface.js` instead.

## Table of contents

- [@earendil-works/pi-agent-core](#earendil-works-pi-agent-core)
- [@earendil-works/pi-ai](#earendil-works-pi-ai)
- [@earendil-works/pi-coding-agent](#earendil-works-pi-coding-agent)
- [@earendil-works/pi-tui](#earendil-works-pi-tui)

## @earendil-works/pi-agent-core

### agent

#### Agent

Kind: class

```ts
/**
 * Stateful wrapper around the low-level agent loop.
 *
 * `Agent` owns the current transcript, emits lifecycle events, executes tools,
 * and exposes queueing APIs for steering and follow-up messages.
 */
export declare class Agent {
    private _state;
    private readonly listeners;
    private readonly steeringQueue;
    private readonly followUpQueue;
    convertToLlm: (messages: AgentMessage[]) => Message[] | Promise<Message[]>;
    transformContext?: (messages: AgentMessage[], signal?: AbortSignal) => Promise<AgentMessage[]>;
    streamFn: StreamFn;
    getApiKey?: (provider: string) => Promise<string | undefined> | string | undefined;
    onPayload?: SimpleStreamOptions["onPayload"];
    onResponse?: SimpleStreamOptions["onResponse"];
    beforeToolCall?: (context: BeforeToolCallContext, signal?: AbortSignal) => Promise<BeforeToolCallResult | undefined>;
    afterToolCall?: (context: AfterToolCallContext, signal?: AbortSignal) => Promise<AfterToolCallResult | undefined>;
    private activeRun?;
    /** Session identifier forwarded to providers for cache-aware backends. */
    sessionId?: string;
    /** Optional per-level thinking token budgets forwarded to the stream function. */
    thinkingBudgets?: ThinkingBudgets;
    /** Preferred transport forwarded to the stream function. */
    transport: Transport;
    /** Optional cap for provider-requested retry delays. */
    maxRetryDelayMs?: number;
    /** Tool execution strategy for assistant messages that contain multiple tool calls. */
    toolExecution: ToolExecutionMode;
    constructor(options?: AgentOptions);
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
    subscribe(listener: (event: AgentEvent, signal: AbortSignal) => Promise<void> | void): () => void;
    /**
     * Current agent state.
     *
     * Assigning `state.tools` or `state.messages` copies the provided top-level array.
     */
    get state(): AgentState;
    /** Controls how queued steering messages are drained. */
    set steeringMode(mode: QueueMode);
    get steeringMode(): QueueMode;
    /** Controls how queued follow-up messages are drained. */
    set followUpMode(mode: QueueMode);
    get followUpMode(): QueueMode;
    /** Queue a message to be injected after the current assistant turn finishes. */
    steer(message: AgentMessage): void;
    /** Queue a message to run only after the agent would otherwise stop. */
    followUp(message: AgentMessage): void;
    /** Remove all queued steering messages. */
    clearSteeringQueue(): void;
    /** Remove all queued follow-up messages. */
    clearFollowUpQueue(): void;
    /** Remove all queued steering and follow-up messages. */
    clearAllQueues(): void;
    /** Returns true when either queue still contains pending messages. */
    hasQueuedMessages(): boolean;
    /** Active abort signal for the current run, if any. */
    get signal(): AbortSignal | undefined;
    /** Abort the current run, if one is active. */
    abort(): void;
    /**
     * Resolve when the current run and all awaited event listeners have finished.
     *
     * This resolves after `agent_end` listeners settle.
     */
    waitForIdle(): Promise<void>;
    /** Clear transcript state, runtime state, and queued messages. */
    reset(): void;
    /** Start a new prompt from text, a single message, or a batch of messages. */
    prompt(message: AgentMessage | AgentMessage[]): Promise<void>;
    prompt(input: string, images?: ImageContent[]): Promise<void>;
    /** Continue from the current transcript. The last message must be a user or tool-result message. */
    continue(): Promise<void>;
    private normalizePromptInput;
    private runPromptMessages;
    private runContinuation;
    private createContextSnapshot;
    private createLoopConfig;
    private runWithLifecycle;
    private handleRunFailure;
    private finishRun;
    private processEvents;
}
```

#### AgentOptions

Kind: interface

```ts
/** Options for constructing an {@link Agent}. */
export interface AgentOptions {
    initialState?: Partial<Omit<AgentState, "pendingToolCalls" | "isStreaming" | "streamingMessage" | "errorMessage">>;
    convertToLlm?: (messages: AgentMessage[]) => Message[] | Promise<Message[]>;
    transformContext?: (messages: AgentMessage[], signal?: AbortSignal) => Promise<AgentMessage[]>;
    streamFn?: StreamFn;
    getApiKey?: (provider: string) => Promise<string | undefined> | string | undefined;
    onPayload?: SimpleStreamOptions["onPayload"];
    onResponse?: SimpleStreamOptions["onResponse"];
    beforeToolCall?: (context: BeforeToolCallContext, signal?: AbortSignal) => Promise<BeforeToolCallResult | undefined>;
    afterToolCall?: (context: AfterToolCallContext, signal?: AbortSignal) => Promise<AfterToolCallResult | undefined>;
    steeringMode?: QueueMode;
    followUpMode?: QueueMode;
    sessionId?: string;
    thinkingBudgets?: ThinkingBudgets;
    transport?: Transport;
    maxRetryDelayMs?: number;
    toolExecution?: ToolExecutionMode;
}
```

### agent-loop

#### AgentEventSink

Kind: type

```ts
export type AgentEventSink = (event: AgentEvent) => Promise<void> | void;
```

#### agentLoop

Kind: function

```ts
/**
 * Start an agent loop with a new prompt message.
 * The prompt is added to the context and events are emitted for it.
 */
export declare function agentLoop(prompts: AgentMessage[], context: AgentContext, config: AgentLoopConfig, signal?: AbortSignal, streamFn?: StreamFn): EventStream<AgentEvent, AgentMessage[]>;
```

#### agentLoopContinue

Kind: function

```ts
/**
 * Continue an agent loop from the current context without adding a new message.
 * Used for retries - context already has user message or tool results.
 *
 * **Important:** The last message in context must convert to a `user` or `toolResult` message
 * via `convertToLlm`. If it doesn't, the LLM provider will reject the request.
 * This cannot be validated here since `convertToLlm` is only called once per turn.
 */
export declare function agentLoopContinue(context: AgentContext, config: AgentLoopConfig, signal?: AbortSignal, streamFn?: StreamFn): EventStream<AgentEvent, AgentMessage[]>;
```

#### runAgentLoop

Kind: function

```ts
export declare function runAgentLoop(prompts: AgentMessage[], context: AgentContext, config: AgentLoopConfig, emit: AgentEventSink, signal?: AbortSignal, streamFn?: StreamFn): Promise<AgentMessage[]>;
```

#### runAgentLoopContinue

Kind: function

```ts
export declare function runAgentLoopContinue(context: AgentContext, config: AgentLoopConfig, emit: AgentEventSink, signal?: AbortSignal, streamFn?: StreamFn): Promise<AgentMessage[]>;
```

### proxy

#### ProxyAssistantMessageEvent

Kind: type

```ts
/**
 * Proxy event types - server sends these with partial field stripped to reduce bandwidth.
 */
export type ProxyAssistantMessageEvent = {
    type: "start";
} | {
    type: "text_start";
    contentIndex: number;
} | {
    type: "text_delta";
    contentIndex: number;
    delta: string;
} | {
    type: "text_end";
    contentIndex: number;
    contentSignature?: string;
} | {
    type: "thinking_start";
    contentIndex: number;
} | {
    type: "thinking_delta";
    contentIndex: number;
    delta: string;
} | {
    type: "thinking_end";
    contentIndex: number;
    contentSignature?: string;
} | {
    type: "toolcall_start";
    contentIndex: number;
    id: string;
    toolName: string;
} | {
    type: "toolcall_delta";
    contentIndex: number;
    delta: string;
} | {
    type: "toolcall_end";
    contentIndex: number;
} | {
    type: "done";
    reason: Extract<StopReason, "stop" | "length" | "toolUse">;
    usage: AssistantMessage["usage"];
} | {
    type: "error";
    reason: Extract<StopReason, "aborted" | "error">;
    errorMessage?: string;
    usage: AssistantMessage["usage"];
};
```

#### ProxyStreamOptions

Kind: interface

```ts
export interface ProxyStreamOptions extends ProxySerializableStreamOptions {
    /** Local abort signal for the proxy request */
    signal?: AbortSignal;
    /** Auth token for the proxy server */
    authToken: string;
    /** Proxy server URL (e.g., "https://genai.example.com") */
    proxyUrl: string;
}
```

#### streamProxy

Kind: function

```ts
export declare function streamProxy(model: Model<any>, context: Context, options: ProxyStreamOptions): ProxyMessageEventStream;
```

### types

#### AfterToolCallContext

Kind: interface

```ts
/** Context passed to `afterToolCall`. */
export interface AfterToolCallContext {
    /** The assistant message that requested the tool call. */
    assistantMessage: AssistantMessage;
    /** The raw tool call block from `assistantMessage.content`. */
    toolCall: AgentToolCall;
    /** Validated tool arguments for the target tool schema. */
    args: unknown;
    /** The executed tool result before any `afterToolCall` overrides are applied. */
    result: AgentToolResult<any>;
    /** Whether the executed tool result is currently treated as an error. */
    isError: boolean;
    /** Current agent context at the time the tool call is finalized. */
    context: AgentContext;
}
```

#### AfterToolCallResult

Kind: interface

```ts
/**
 * Partial override returned from `afterToolCall`.
 *
 * Merge semantics are field-by-field:
 * - `content`: if provided, replaces the tool result content array in full
 * - `details`: if provided, replaces the tool result details value in full
 * - `isError`: if provided, replaces the tool result error flag
 * - `terminate`: if provided, replaces the early-termination hint
 *
 * Omitted fields keep the original executed tool result values.
 * There is no deep merge for `content` or `details`.
 */
export interface AfterToolCallResult {
    content?: (TextContent | ImageContent)[];
    details?: unknown;
    isError?: boolean;
    /**
     * Hint that the agent should stop after the current tool batch.
     * Early termination only happens when every finalized tool result in the batch sets this to true.
     */
    terminate?: boolean;
}
```

#### AgentContext

Kind: interface

```ts
/** Context snapshot passed into the low-level agent loop. */
export interface AgentContext {
    /** System prompt included with the request. */
    systemPrompt: string | SystemPromptSection[];
    /** Transcript visible to the model. */
    messages: AgentMessage[];
    /** Tools available for this run. */
    tools?: AgentTool<any>[];
}
```

#### AgentEvent

Kind: type

```ts
/**
 * Events emitted by the Agent for UI updates.
 *
 * `agent_end` is the last event emitted for a run, but awaited `Agent.subscribe()`
 * listeners for that event are still part of run settlement. The agent becomes
 * idle only after those listeners finish.
 */
export type AgentEvent = {
    type: "agent_start";
} | {
    type: "agent_end";
    messages: AgentMessage[];
} | {
    type: "turn_start";
} | {
    type: "turn_end";
    message: AgentMessage;
    toolResults: ToolResultMessage[];
} | {
    type: "message_start";
    message: AgentMessage;
} | {
    type: "message_update";
    message: AgentMessage;
    assistantMessageEvent: AssistantMessageEvent;
} | {
    type: "message_end";
    message: AgentMessage;
} | {
    type: "tool_execution_start";
    toolCallId: string;
    toolName: string;
    args: any;
} | {
    type: "tool_execution_update";
    toolCallId: string;
    toolName: string;
    args: any;
    partialResult: any;
} | {
    type: "tool_execution_end";
    toolCallId: string;
    toolName: string;
    result: any;
    isError: boolean;
};
```

#### AgentLoopConfig

Kind: interface

```ts
export interface AgentLoopConfig extends SimpleStreamOptions {
    model: Model<any>;
    /**
     * Converts AgentMessage[] to LLM-compatible Message[] before each LLM call.
     *
     * Each AgentMessage must be converted to a UserMessage, AssistantMessage, or ToolResultMessage
     * that the LLM can understand. AgentMessages that cannot be converted (e.g., UI-only notifications,
     * status messages) should be filtered out.
     *
     * Contract: must not throw or reject. Return a safe fallback value instead.
     * Throwing interrupts the low-level agent loop without producing a normal event sequence.
     *
     * @example
     * ```typescript
     * convertToLlm: (messages) => messages.flatMap(m => {
     *   if (m.role === "custom") {
     *     // Convert custom message to user message
     *     return [{ role: "user", content: m.content, timestamp: m.timestamp }];
     *   }
     *   if (m.role === "notification") {
     *     // Filter out UI-only messages
     *     return [];
     *   }
     *   // Pass through standard LLM messages
     *   return [m];
     * })
     * ```
     */
    convertToLlm: (messages: AgentMessage[]) => Message[] | Promise<Message[]>;
    /**
     * Optional transform applied to the context before `convertToLlm`.
     *
     * Use this for operations that work at the AgentMessage level:
     * - Context window management (pruning old messages)
     * - Injecting context from external sources
     *
     * Contract: must not throw or reject. Return the original messages or another
     * safe fallback value instead.
     *
     * @example
     * ```typescript
     * transformContext: async (messages) => {
     *   if (estimateTokens(messages) > MAX_TOKENS) {
     *     return pruneOldMessages(messages);
     *   }
     *   return messages;
     * }
     * ```
     */
    transformContext?: (messages: AgentMessage[], signal?: AbortSignal) => Promise<AgentMessage[]>;
    /**
     * Resolves an API key dynamically for each LLM call.
     *
     * Useful for short-lived OAuth tokens (e.g., GitHub Copilot) that may expire
     * during long-running tool execution phases.
     *
     * Contract: must not throw or reject. Return undefined when no key is available.
     */
    getApiKey?: (provider: string) => Promise<string | undefined> | string | undefined;
    /**
     * Called after each turn fully completes and `turn_end` has been emitted.
     *
     * If it returns true, the loop emits `agent_end` and exits before polling steering or follow-up queues,
     * without starting another LLM call. The current assistant response and any tool executions finish normally.
     *
     * Use this to request a graceful stop after the current turn, e.g. before context gets too full.
     *
     * Contract: must not throw or reject. Throwing interrupts the low-level agent loop without producing a normal event sequence.
     */
    shouldStopAfterTurn?: (context: ShouldStopAfterTurnContext) => boolean | Promise<boolean>;
    /**
     * Returns steering messages to inject into the conversation mid-run.
     *
     * Called after the current assistant turn finishes executing its tool calls, unless `shouldStopAfterTurn` exits first.
     * If messages are returned, they are added to the context before the next LLM call.
     * Tool calls from the current assistant message are not skipped.
     *
     * Use this for "steering" the agent while it's working.
     *
     * Contract: must not throw or reject. Return [] when no steering messages are available.
     */
    getSteeringMessages?: () => Promise<AgentMessage[]>;
    /**
     * Returns follow-up messages to process after the agent would otherwise stop.
     *
     * Called when the agent has no more tool calls and no steering messages.
     * If messages are returned, they're added to the context and the agent
     * continues with another turn.
     *
     * Use this for follow-up messages that should wait until the agent finishes.
     *
     * Contract: must not throw or reject. Return [] when no follow-up messages are available.
     */
    getFollowUpMessages?: () => Promise<AgentMessage[]>;
    /**
     * Tool execution mode.
     * - "sequential": execute tool calls one by one
     * - "parallel": preflight tool calls sequentially, then execute allowed tools concurrently;
     *   emit `tool_execution_end` in tool completion order after each tool is finalized,
     *   then emit tool-result message artifacts later in assistant source order
     *
     * Default: "parallel"
     */
    toolExecution?: ToolExecutionMode;
    /**
     * Called before a tool is executed, after arguments have been validated.
     *
     * Return `{ block: true }` to prevent execution. The loop emits an error tool result instead.
     * The hook receives the agent abort signal and is responsible for honoring it.
     */
    beforeToolCall?: (context: BeforeToolCallContext, signal?: AbortSignal) => Promise<BeforeToolCallResult | undefined>;
    /**
     * Called after a tool finishes executing, before `tool_execution_end` and tool-result message events are emitted.
     *
     * Return an `AfterToolCallResult` to override parts of the executed tool result:
     * - `content` replaces the full content array
     * - `details` replaces the full details payload
     * - `isError` replaces the error flag
     * - `terminate` replaces the early-termination hint
     *
     * Any omitted fields keep their original values. No deep merge is performed.
     * The hook receives the agent abort signal and is responsible for honoring it.
     */
    afterToolCall?: (context: AfterToolCallContext, signal?: AbortSignal) => Promise<AfterToolCallResult | undefined>;
}
```

#### AgentMessage

Kind: type

```ts
/**
 * AgentMessage: Union of LLM messages + custom messages.
 * This abstraction allows apps to add custom message types while maintaining
 * type safety and compatibility with the base LLM messages.
 */
export type AgentMessage = Message | CustomAgentMessages[keyof CustomAgentMessages];
```

#### AgentState

Kind: interface

```ts
/**
 * Public agent state.
 *
 * `tools` and `messages` use accessor properties so implementations can copy
 * assigned arrays before storing them.
 */
export interface AgentState {
    /** System prompt sent with each model request. */
    systemPrompt: string | SystemPromptSection[];
    /** Active model used for future turns. */
    model: Model<any>;
    /** Requested reasoning level for future turns. */
    thinkingLevel: ThinkingLevel;
    /** Available tools. Assigning a new array copies the top-level array. */
    set tools(tools: AgentTool<any>[]);
    get tools(): AgentTool<any>[];
    /** Conversation transcript. Assigning a new array copies the top-level array. */
    set messages(messages: AgentMessage[]);
    get messages(): AgentMessage[];
    /**
     * True while the agent is processing a prompt or continuation.
     *
     * This remains true until awaited `agent_end` listeners settle.
     */
    readonly isStreaming: boolean;
    /** Partial assistant message for the current streamed response, if any. */
    readonly streamingMessage?: AgentMessage;
    /** Tool call ids currently executing. */
    readonly pendingToolCalls: ReadonlySet<string>;
    /** Error message from the most recent failed or aborted assistant turn, if any. */
    readonly errorMessage?: string;
}
```

#### AgentTool

Kind: interface

```ts
/** Tool definition used by the agent runtime. */
export interface AgentTool<TParameters extends TSchema = TSchema, TDetails = any> extends Tool<TParameters> {
    /** Human-readable label for UI display. */
    label: string;
    /**
     * Optional compatibility shim for raw tool-call arguments before schema validation.
     * Must return an object that matches `TParameters`.
     */
    prepareArguments?: (args: unknown) => Static<TParameters>;
    /** Execute the tool call. Throw on failure instead of encoding errors in `content`. */
    execute: (toolCallId: string, params: Static<TParameters>, signal?: AbortSignal, onUpdate?: AgentToolUpdateCallback<TDetails>) => Promise<AgentToolResult<TDetails>>;
    /**
     * Per-tool execution mode override.
     * - "sequential": this tool must execute one at a time with other tool calls.
     * - "parallel": this tool can execute concurrently with other tool calls.
     *
     * If omitted, the default execution mode applies.
     */
    executionMode?: ToolExecutionMode;
}
```

#### AgentToolCall

Kind: type

```ts
/** A single tool call content block emitted by an assistant message. */
export type AgentToolCall = Extract<AssistantMessage["content"][number], {
    type: "toolCall";
}>;
```

#### AgentToolResult

Kind: interface

```ts
/** Final or partial result produced by a tool. */
export interface AgentToolResult<T> {
    /** Text or image content returned to the model. */
    content: (TextContent | ImageContent)[];
    /** Arbitrary structured details for logs or UI rendering. */
    details: T;
    /**
     * Hint that the agent should stop after the current tool batch.
     * Early termination only happens when every finalized tool result in the batch sets this to true.
     */
    terminate?: boolean;
}
```

#### AgentToolUpdateCallback

Kind: type

```ts
/** Callback used by tools to stream partial execution updates. */
export type AgentToolUpdateCallback<T = any> = (partialResult: AgentToolResult<T>) => void;
```

#### BeforeToolCallContext

Kind: interface

```ts
/** Context passed to `beforeToolCall`. */
export interface BeforeToolCallContext {
    /** The assistant message that requested the tool call. */
    assistantMessage: AssistantMessage;
    /** The raw tool call block from `assistantMessage.content`. */
    toolCall: AgentToolCall;
    /** Validated tool arguments for the target tool schema. */
    args: unknown;
    /** Current agent context at the time the tool call is prepared. */
    context: AgentContext;
}
```

#### BeforeToolCallResult

Kind: interface

```ts
/**
 * Result returned from `beforeToolCall`.
 *
 * Returning `{ block: true }` prevents the tool from executing. The loop emits an error tool result instead.
 * `reason` becomes the text shown in that error result. If omitted, a default blocked message is used.
 */
export interface BeforeToolCallResult {
    block?: boolean;
    reason?: string;
}
```

#### CustomAgentMessages

Kind: interface

```ts
/**
 * Extensible interface for custom app messages.
 * Apps can extend via declaration merging:
 *
 * @example
 * ```typescript
 * declare module "@mariozechner/agent" {
 *   interface CustomAgentMessages {
 *     artifact: ArtifactMessage;
 *     notification: NotificationMessage;
 *   }
 * }
 * ```
 */
export interface CustomAgentMessages {
}
```

#### ShouldStopAfterTurnContext

Kind: interface

```ts
/** Context passed to `shouldStopAfterTurn`. */
export interface ShouldStopAfterTurnContext {
    /** The assistant message that completed the turn. */
    message: AssistantMessage;
    /** Tool result messages passed to the preceding `turn_end` event. */
    toolResults: ToolResultMessage[];
    /** Current agent context after the turn's assistant message and tool results have been appended. */
    context: AgentContext;
    /** Messages that this loop invocation will return if it exits at this point. Prompt runs include the initial prompt messages; continuation runs do not include pre-existing context messages. */
    newMessages: AgentMessage[];
}
```

#### StreamFn

Kind: type

```ts
/**
 * Stream function used by the agent loop.
 *
 * Contract:
 * - Must not throw or return a rejected promise for request/model/runtime failures.
 * - Must return an AssistantMessageEventStream.
 * - Failures must be encoded in the returned stream via protocol events and a
 *   final AssistantMessage with stopReason "error" or "aborted" and errorMessage.
 */
export type StreamFn = (...args: Parameters<typeof streamSimple>) => ReturnType<typeof streamSimple> | Promise<ReturnType<typeof streamSimple>>;
```

#### ThinkingLevel

Kind: type

```ts
/**
 * Thinking/reasoning level for models that support it.
 * Note: "xhigh" is only supported by selected model families. Use model thinking-level metadata
 * from @earendil-works/pi-ai to detect support for a concrete model.
 */
export type ThinkingLevel = "off" | "minimal" | "low" | "medium" | "high" | "xhigh";
```

#### ToolExecutionMode

Kind: type

```ts
/**
 * Configuration for how tool calls from a single assistant message are executed.
 *
 * - "sequential": each tool call is prepared, executed, and finalized before the next one starts.
 * - "parallel": tool calls are prepared sequentially, then allowed tools execute concurrently.
 *   `tool_execution_end` is emitted in tool completion order after each tool is finalized,
 *   while tool-result message artifacts are emitted later in assistant source order.
 */
export type ToolExecutionMode = "sequential" | "parallel";
```

## @earendil-works/pi-ai

### api-registry

#### ApiProvider

Kind: interface

```ts
export interface ApiProvider<TApi extends Api = Api, TOptions extends StreamOptions = StreamOptions> {
    api: TApi;
    stream: StreamFunction<TApi, TOptions>;
    streamSimple: StreamFunction<TApi, SimpleStreamOptions>;
    /**
     * Declares that this provider accepts `SystemPromptSection[]` in
     * `Context.systemPrompt`. When omitted, the registry flattens a sections
     * array to the equivalent single string (on a shallow context copy) before
     * dispatching, so providers written against the legacy `string` contract
     * keep working unchanged.
     */
    handlesSystemPromptSections?: boolean;
}
```

#### ApiStreamFunction

Kind: type

```ts
export type ApiStreamFunction = (model: Model<Api>, context: Context, options?: StreamOptions) => AssistantMessageEventStream;
```

#### ApiStreamSimpleFunction

Kind: type

```ts
export type ApiStreamSimpleFunction = (model: Model<Api>, context: Context, options?: SimpleStreamOptions) => AssistantMessageEventStream;
```

#### clearApiProviders

Kind: function

```ts
export declare function clearApiProviders(): void;
```

#### getApiProvider

Kind: function

```ts
export declare function getApiProvider(api: Api): ApiProviderInternal | undefined;
```

#### getApiProviders

Kind: function

```ts
export declare function getApiProviders(): ApiProviderInternal[];
```

#### registerApiProvider

Kind: function

```ts
export declare function registerApiProvider<TApi extends Api, TOptions extends StreamOptions>(provider: ApiProvider<TApi, TOptions>, sourceId?: string): void;
```

#### unregisterApiProviders

Kind: function

```ts
export declare function unregisterApiProviders(sourceId: string): void;
```

### env-api-keys

#### findEnvKeys

Kind: function

```ts
/**
 * Find configured environment variables that can provide an API key for a provider.
 *
 * This only reports actual API key variables. It intentionally excludes ambient
 * credential sources such as AWS profiles, AWS IAM credentials, and Google
 * Application Default Credentials.
 */
export declare function findEnvKeys(provider: KnownProvider): string[] | undefined;

export declare function findEnvKeys(provider: string): string[] | undefined;
```

#### getEnvApiKey

Kind: function

```ts
/**
 * Get API key for provider from known environment variables, e.g. OPENAI_API_KEY.
 *
 * Will not return API keys for providers that require OAuth tokens.
 */
export declare function getEnvApiKey(provider: KnownProvider): string | undefined;

export declare function getEnvApiKey(provider: string): string | undefined;
```

### external/typebox/build/type/types/schema.d.mts

#### TSchema

Kind: re-export

```ts
export type { Static, TSchema } from "typebox";
```

### external/typebox/build/type/types/static.d.mts

#### Static

Kind: re-export

```ts
export type { Static, TSchema } from "typebox";
```

### external/typebox/build/typebox.d.mts

#### Type

Kind: re-export

```ts
export { Type } from "typebox";
```

### models

#### calculateCost

Kind: function

```ts
export declare function calculateCost<TApi extends Api>(model: Model<TApi>, usage: Usage): Usage["cost"];
```

#### clampThinkingLevel

Kind: function

```ts
export declare function clampThinkingLevel<TApi extends Api>(model: Model<TApi>, level: ModelThinkingLevel): ModelThinkingLevel;
```

#### getModel

Kind: function

```ts
export declare function getModel<TProvider extends KnownProvider, TModelId extends keyof (typeof MODELS)[TProvider]>(provider: TProvider, modelId: TModelId): Model<ModelApi<TProvider, TModelId>>;
```

#### getModels

Kind: function

```ts
export declare function getModels<TProvider extends KnownProvider>(provider: TProvider): Model<ModelApi<TProvider, keyof (typeof MODELS)[TProvider]>>[];
```

#### getProviders

Kind: function

```ts
export declare function getProviders(): KnownProvider[];
```

#### getSupportedThinkingLevels

Kind: function

```ts
export declare function getSupportedThinkingLevels<TApi extends Api>(model: Model<TApi>): ModelThinkingLevel[];
```

#### modelsAreEqual

Kind: function

```ts
/**
 * Check if two models are equal by comparing both their id and provider.
 * Returns false if either model is null or undefined.
 */
export declare function modelsAreEqual<TApi extends Api>(a: Model<TApi> | null | undefined, b: Model<TApi> | null | undefined): boolean;
```

### providers/amazon-bedrock

#### BedrockOptions

Kind: interface

```ts
export interface BedrockOptions extends StreamOptions {
    region?: string;
    profile?: string;
    toolChoice?: "auto" | "any" | "none" | {
        type: "tool";
        name: string;
    };
    reasoning?: ThinkingLevel;
    thinkingBudgets?: ThinkingBudgets;
    interleavedThinking?: boolean;
    /**
     * Controls how Claude's thinking content is returned in responses.
     * - "summarized": Thinking blocks contain summarized thinking text (default here).
     * - "omitted": Thinking content is redacted but the signature still travels back
     *   for multi-turn continuity, reducing time-to-first-text-token.
     *
     * Note: Anthropic's API default for Claude Opus 4.7 and Mythos Preview is
     * "omitted". We default to "summarized" here to keep behavior consistent with
     * older Claude 4 models. Only applies to Claude models on Bedrock.
     */
    thinkingDisplay?: BedrockThinkingDisplay;
    /** Key-value pairs attached to the inference request for cost allocation tagging.
     * Keys: max 64 chars, no `aws:` prefix. Values: max 256 chars. Max 50 pairs.
     * Tags appear in AWS Cost Explorer split cost allocation data.
     * @see https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseStream.html */
    requestMetadata?: Record<string, string>;
    /** Bearer token for Bedrock API key authentication.
     * When set, bypasses SigV4 signing and sends Authorization: Bearer <token> instead.
     * Requires `bedrock:CallWithBearerToken` IAM permission on the token's identity.
     * Set via AWS_BEARER_TOKEN_BEDROCK env var or pass directly.
     * @see https://docs.aws.amazon.com/service-authorization/latest/reference/list_amazonbedrock.html */
    bearerToken?: string;
}
```

#### BedrockThinkingDisplay

Kind: type

```ts
export type BedrockThinkingDisplay = "summarized" | "omitted";
```

### providers/anthropic

#### AnthropicEffort

Kind: type

```ts
export type AnthropicEffort = "low" | "medium" | "high" | "xhigh" | "max";
```

#### AnthropicOptions

Kind: interface

```ts
export interface AnthropicOptions extends StreamOptions {
    /**
     * Enable extended thinking.
     * For Opus 4.6 and Sonnet 4.6: uses adaptive thinking (model decides when/how much to think).
     * For older models: uses budget-based thinking with thinkingBudgetTokens.
     */
    thinkingEnabled?: boolean;
    /**
     * Token budget for extended thinking (older models only).
     * Ignored for Opus 4.6 and Sonnet 4.6, which use adaptive thinking.
     */
    thinkingBudgetTokens?: number;
    /**
     * Effort level for adaptive thinking (Opus 4.6+ and Sonnet 4.6).
     * Controls how much thinking Claude allocates:
     * - "max": Always thinks with no constraints (Opus 4.6 only)
     * - "xhigh": Highest reasoning level (Opus 4.7)
     * - "high": Always thinks, deep reasoning (default)
     * - "medium": Moderate thinking, may skip for simple queries
     * - "low": Minimal thinking, skips for simple tasks
     * Ignored for older models.
     */
    effort?: AnthropicEffort;
    /**
     * Controls how thinking content is returned in API responses.
     * - "summarized": Thinking blocks contain summarized thinking text (default here).
     * - "omitted": Thinking blocks return an empty thinking field; the encrypted
     *   signature still travels back for multi-turn continuity. Use for faster
     *   time-to-first-text-token when your UI does not surface thinking.
     *
     * Note: Anthropic's API default for Claude Opus 4.7 and Claude Mythos Preview
     * is "omitted". We default to "summarized" here to keep behavior consistent
     * with older Claude 4 models. Set this explicitly to "omitted" to opt in.
     */
    thinkingDisplay?: AnthropicThinkingDisplay;
    interleavedThinking?: boolean;
    toolChoice?: "auto" | "any" | "none" | {
        type: "tool";
        name: string;
    };
    /**
     * Pre-built Anthropic client instance. When provided, skips internal client
     * construction entirely. Use this to inject alternative SDK clients such as
     * `AnthropicVertex` that shares the same messaging API.
     */
    client?: Anthropic;
}
```

#### AnthropicThinkingDisplay

Kind: type

```ts
export type AnthropicThinkingDisplay = "summarized" | "omitted";
```

### providers/azure-openai-responses

#### AzureOpenAIResponsesOptions

Kind: interface

```ts
export interface AzureOpenAIResponsesOptions extends StreamOptions {
    reasoningEffort?: "minimal" | "low" | "medium" | "high" | "xhigh";
    reasoningSummary?: "auto" | "detailed" | "concise" | null;
    azureApiVersion?: string;
    azureResourceName?: string;
    azureBaseUrl?: string;
    azureDeploymentName?: string;
}
```

### providers/faux

#### fauxAssistantMessage

Kind: function

```ts
export declare function fauxAssistantMessage(content: string | FauxContentBlock | FauxContentBlock[], options?: {
    stopReason?: AssistantMessage["stopReason"];
    errorMessage?: string;
    responseId?: string;
    timestamp?: number;
}): AssistantMessage;
```

#### FauxContentBlock

Kind: type

```ts
export type FauxContentBlock = TextContent | ThinkingContent | ToolCall;
```

#### FauxModelDefinition

Kind: interface

```ts
export interface FauxModelDefinition {
    id: string;
    name?: string;
    reasoning?: boolean;
    input?: ("text" | "image")[];
    cost?: {
        input: number;
        output: number;
        cacheRead: number;
        cacheWrite: number;
    };
    contextWindow?: number;
    maxTokens?: number;
}
```

#### FauxProviderRegistration

Kind: interface

```ts
export interface FauxProviderRegistration {
    api: string;
    models: [Model<string>, ...Model<string>[]];
    getModel(): Model<string>;
    getModel(modelId: string): Model<string> | undefined;
    state: {
        callCount: number;
    };
    setResponses: (responses: FauxResponseStep[]) => void;
    appendResponses: (responses: FauxResponseStep[]) => void;
    getPendingResponseCount: () => number;
    unregister: () => void;
}
```

#### FauxResponseFactory

Kind: type

```ts
export type FauxResponseFactory = (context: Context, options: StreamOptions | undefined, state: {
    callCount: number;
}, model: Model<string>) => AssistantMessage | Promise<AssistantMessage>;
```

#### FauxResponseStep

Kind: type

```ts
export type FauxResponseStep = AssistantMessage | FauxResponseFactory;
```

#### fauxText

Kind: function

```ts
export declare function fauxText(text: string): TextContent;
```

#### fauxThinking

Kind: function

```ts
export declare function fauxThinking(thinking: string): ThinkingContent;
```

#### fauxToolCall

Kind: function

```ts
export declare function fauxToolCall(name: string, arguments_: ToolCall["arguments"], options?: {
    id?: string;
}): ToolCall;
```

#### registerFauxProvider

Kind: function

```ts
export declare function registerFauxProvider(options?: RegisterFauxProviderOptions): FauxProviderRegistration;
```

#### RegisterFauxProviderOptions

Kind: interface

```ts
export interface RegisterFauxProviderOptions {
    api?: string;
    provider?: string;
    models?: FauxModelDefinition[];
    tokensPerSecond?: number;
    tokenSize?: {
        min?: number;
        max?: number;
    };
}
```

### providers/google

#### GoogleOptions

Kind: interface

```ts
export interface GoogleOptions extends StreamOptions {
    toolChoice?: "auto" | "none" | "any";
    thinking?: {
        enabled: boolean;
        budgetTokens?: number;
        level?: GoogleThinkingLevel;
    };
}
```

### providers/google-shared

#### GoogleThinkingLevel

Kind: type

```ts
/**
 * Thinking level for Gemini 3 models.
 * Mirrors Google's ThinkingLevel enum values.
 */
export type GoogleThinkingLevel = "THINKING_LEVEL_UNSPECIFIED" | "MINIMAL" | "LOW" | "MEDIUM" | "HIGH";
```

### providers/google-vertex

#### GoogleVertexOptions

Kind: interface

```ts
export interface GoogleVertexOptions extends StreamOptions {
    toolChoice?: "auto" | "none" | "any";
    thinking?: {
        enabled: boolean;
        budgetTokens?: number;
        level?: GoogleThinkingLevel;
    };
    project?: string;
    location?: string;
}
```

### providers/mistral

#### MistralOptions

Kind: interface

```ts
export interface MistralOptions extends StreamOptions {
    toolChoice?: "auto" | "none" | "any" | "required" | {
        type: "function";
        function: {
            name: string;
        };
    };
    promptMode?: "reasoning";
    reasoningEffort?: MistralReasoningEffort;
}
```

### providers/openai-codex-responses

#### OpenAICodexResponsesOptions

Kind: interface

```ts
export interface OpenAICodexResponsesOptions extends StreamOptions {
    reasoningEffort?: "none" | "minimal" | "low" | "medium" | "high" | "xhigh";
    reasoningSummary?: "auto" | "concise" | "detailed" | "off" | "on" | null;
    serviceTier?: ResponseCreateParamsStreaming["service_tier"];
    textVerbosity?: "low" | "medium" | "high";
}
```

#### OpenAICodexWebSocketDebugStats

Kind: interface

```ts
export interface OpenAICodexWebSocketDebugStats {
    requests: number;
    connectionsCreated: number;
    connectionsReused: number;
    cachedContextRequests: number;
    storeTrueRequests: number;
    fullContextRequests: number;
    deltaRequests: number;
    lastInputItems: number;
    lastDeltaInputItems?: number;
    lastPreviousResponseId?: string;
    websocketFailures: number;
    sseFallbacks: number;
    websocketFallbackActive?: boolean;
    lastWebSocketError?: string;
}
```

### providers/openai-completions

#### OpenAICompletionsOptions

Kind: interface

```ts
export interface OpenAICompletionsOptions extends StreamOptions {
    toolChoice?: "auto" | "none" | "required" | {
        type: "function";
        function: {
            name: string;
        };
    };
    reasoningEffort?: "minimal" | "low" | "medium" | "high" | "xhigh";
}
```

### providers/openai-responses

#### OpenAIResponsesOptions

Kind: interface

```ts
export interface OpenAIResponsesOptions extends StreamOptions {
    reasoningEffort?: "minimal" | "low" | "medium" | "high" | "xhigh";
    reasoningSummary?: "auto" | "detailed" | "concise" | null;
    serviceTier?: ResponseCreateParamsStreaming["service_tier"];
}
```

### providers/register-builtins

#### registerBuiltInApiProviders

Kind: function

```ts
export declare function registerBuiltInApiProviders(): void;
```

#### resetApiProviders

Kind: function

```ts
export declare function resetApiProviders(): void;
```

#### setBedrockProviderModule

Kind: function

```ts
export declare function setBedrockProviderModule(module: BedrockProviderModule): void;
```

#### streamAnthropic

Kind: const

```ts
export declare const streamAnthropic: StreamFunction<"anthropic-messages", AnthropicOptions>;
```

#### streamAzureOpenAIResponses

Kind: const

```ts
export declare const streamAzureOpenAIResponses: StreamFunction<"azure-openai-responses", AzureOpenAIResponsesOptions>;
```

#### streamGoogle

Kind: const

```ts
export declare const streamGoogle: StreamFunction<"google-generative-ai", GoogleOptions>;
```

#### streamGoogleVertex

Kind: const

```ts
export declare const streamGoogleVertex: StreamFunction<"google-vertex", GoogleVertexOptions>;
```

#### streamMistral

Kind: const

```ts
export declare const streamMistral: StreamFunction<"mistral-conversations", MistralOptions>;
```

#### streamOpenAICodexResponses

Kind: const

```ts
export declare const streamOpenAICodexResponses: StreamFunction<"openai-codex-responses", OpenAICodexResponsesOptions>;
```

#### streamOpenAICompletions

Kind: const

```ts
export declare const streamOpenAICompletions: StreamFunction<"openai-completions", OpenAICompletionsOptions>;
```

#### streamOpenAIResponses

Kind: const

```ts
export declare const streamOpenAIResponses: StreamFunction<"openai-responses", OpenAIResponsesOptions>;
```

#### streamSimpleAnthropic

Kind: const

```ts
export declare const streamSimpleAnthropic: StreamFunction<"anthropic-messages", SimpleStreamOptions>;
```

#### streamSimpleAzureOpenAIResponses

Kind: const

```ts
export declare const streamSimpleAzureOpenAIResponses: StreamFunction<"azure-openai-responses", SimpleStreamOptions>;
```

#### streamSimpleGoogle

Kind: const

```ts
export declare const streamSimpleGoogle: StreamFunction<"google-generative-ai", SimpleStreamOptions>;
```

#### streamSimpleGoogleVertex

Kind: const

```ts
export declare const streamSimpleGoogleVertex: StreamFunction<"google-vertex", SimpleStreamOptions>;
```

#### streamSimpleMistral

Kind: const

```ts
export declare const streamSimpleMistral: StreamFunction<"mistral-conversations", SimpleStreamOptions>;
```

#### streamSimpleOpenAICodexResponses

Kind: const

```ts
export declare const streamSimpleOpenAICodexResponses: StreamFunction<"openai-codex-responses", SimpleStreamOptions>;
```

#### streamSimpleOpenAICompletions

Kind: const

```ts
export declare const streamSimpleOpenAICompletions: StreamFunction<"openai-completions", SimpleStreamOptions>;
```

#### streamSimpleOpenAIResponses

Kind: const

```ts
export declare const streamSimpleOpenAIResponses: StreamFunction<"openai-responses", SimpleStreamOptions>;
```

### session-resources

#### cleanupSessionResources

Kind: function

```ts
export declare function cleanupSessionResources(sessionId?: string): void;
```

#### registerSessionResourceCleanup

Kind: function

```ts
export declare function registerSessionResourceCleanup(cleanup: SessionResourceCleanup): () => void;
```

#### SessionResourceCleanup

Kind: type

```ts
export type SessionResourceCleanup = (sessionId?: string) => void;
```

### stream

#### complete

Kind: function

```ts
export declare function complete<TApi extends Api>(model: Model<TApi>, context: Context, options?: ProviderStreamOptions): Promise<AssistantMessage>;
```

#### completeSimple

Kind: function

```ts
export declare function completeSimple<TApi extends Api>(model: Model<TApi>, context: Context, options?: SimpleStreamOptions): Promise<AssistantMessage>;
```

#### stream

Kind: function

```ts
export declare function stream<TApi extends Api>(model: Model<TApi>, context: Context, options?: ProviderStreamOptions): AssistantMessageEventStream;
```

#### streamSimple

Kind: function

```ts
export declare function streamSimple<TApi extends Api>(model: Model<TApi>, context: Context, options?: SimpleStreamOptions): AssistantMessageEventStream;
```

### types

#### AnthropicMessagesCompat

Kind: interface

```ts
/** Compatibility settings for Anthropic Messages-compatible APIs. */
export interface AnthropicMessagesCompat {
    /**
     * Whether the provider accepts per-tool `eager_input_streaming`.
     * When false, the Anthropic provider omits `tools[].eager_input_streaming`
     * and sends the legacy `fine-grained-tool-streaming-2025-05-14` beta header
     * for tool-enabled requests.
     * Default: true.
     */
    supportsEagerToolInputStreaming?: boolean;
    /** Whether the provider supports Anthropic long cache retention (`cache_control.ttl: "1h"`). Default: true. */
    supportsLongCacheRetention?: boolean;
}
```

#### Api

Kind: type

```ts
export type Api = KnownApi | (string & {});
```

#### AssistantMessage

Kind: interface

```ts
export interface AssistantMessage {
    role: "assistant";
    content: (TextContent | ThinkingContent | ToolCall)[];
    api: Api;
    provider: Provider;
    model: string;
    responseModel?: string;
    responseId?: string;
    diagnostics?: AssistantMessageDiagnostic[];
    usage: Usage;
    stopReason: StopReason;
    errorMessage?: string;
    timestamp: number;
}
```

#### AssistantMessageEvent

Kind: type

```ts
/**
 * Event protocol for AssistantMessageEventStream.
 *
 * Streams should emit `start` before partial updates, then terminate with either:
 * - `done` carrying the final successful AssistantMessage, or
 * - `error` carrying the final AssistantMessage with stopReason "error" or "aborted"
 *   and errorMessage.
 */
export type AssistantMessageEvent = {
    type: "start";
    partial: AssistantMessage;
} | {
    type: "text_start";
    contentIndex: number;
    partial: AssistantMessage;
} | {
    type: "text_delta";
    contentIndex: number;
    delta: string;
    partial: AssistantMessage;
} | {
    type: "text_end";
    contentIndex: number;
    content: string;
    partial: AssistantMessage;
} | {
    type: "thinking_start";
    contentIndex: number;
    partial: AssistantMessage;
} | {
    type: "thinking_delta";
    contentIndex: number;
    delta: string;
    partial: AssistantMessage;
} | {
    type: "thinking_end";
    contentIndex: number;
    content: string;
    partial: AssistantMessage;
} | {
    type: "toolcall_start";
    contentIndex: number;
    partial: AssistantMessage;
} | {
    type: "toolcall_delta";
    contentIndex: number;
    delta: string;
    partial: AssistantMessage;
} | {
    type: "toolcall_end";
    contentIndex: number;
    toolCall: ToolCall;
    partial: AssistantMessage;
} | {
    type: "done";
    reason: Extract<StopReason, "stop" | "length" | "toolUse">;
    message: AssistantMessage;
} | {
    type: "error";
    reason: Extract<StopReason, "aborted" | "error">;
    error: AssistantMessage;
};
```

#### CacheRetention

Kind: type

```ts
export type CacheRetention = "none" | "short" | "long";
```

#### Context

Kind: interface

```ts
export interface Context {
    systemPrompt?: string | SystemPromptSection[];
    messages: Message[];
    tools?: Tool[];
}
```

#### ImageContent

Kind: interface

```ts
export interface ImageContent {
    type: "image";
    data: string;
    mimeType: string;
}
```

#### KnownApi

Kind: type

```ts
export type KnownApi = "openai-completions" | "mistral-conversations" | "openai-responses" | "azure-openai-responses" | "openai-codex-responses" | "anthropic-messages" | "bedrock-converse-stream" | "google-generative-ai" | "google-vertex";
```

#### KnownProvider

Kind: type

```ts
export type KnownProvider = "amazon-bedrock" | "anthropic" | "google" | "google-vertex" | "openai" | "azure-openai-responses" | "openai-codex" | "deepseek" | "github-copilot" | "xai" | "groq" | "cerebras" | "openrouter" | "vercel-ai-gateway" | "zai" | "mistral" | "minimax" | "minimax-cn" | "moonshotai" | "moonshotai-cn" | "huggingface" | "fireworks" | "opencode" | "opencode-go" | "kimi-coding" | "cloudflare-workers-ai" | "cloudflare-ai-gateway" | "xiaomi" | "xiaomi-token-plan-cn" | "xiaomi-token-plan-ams" | "xiaomi-token-plan-sgp";
```

#### Message

Kind: type

```ts
export type Message = UserMessage | AssistantMessage | ToolResultMessage;
```

#### Model

Kind: interface

```ts
export interface Model<TApi extends Api> {
    id: string;
    name: string;
    api: TApi;
    provider: Provider;
    baseUrl: string;
    reasoning: boolean;
    /**
     * Maps pi thinking levels to provider/model-specific values.
     * Missing keys use provider defaults. null marks a level as unsupported.
     */
    thinkingLevelMap?: ThinkingLevelMap;
    input: ("text" | "image")[];
    cost: {
        input: number;
        output: number;
        cacheRead: number;
        cacheWrite: number;
    };
    contextWindow: number;
    maxTokens: number;
    headers?: Record<string, string>;
    /** Compatibility overrides for OpenAI-compatible APIs. If not set, auto-detected from baseUrl. */
    compat?: TApi extends "openai-completions" ? OpenAICompletionsCompat : TApi extends "openai-responses" ? OpenAIResponsesCompat : TApi extends "anthropic-messages" ? AnthropicMessagesCompat : never;
}
```

#### ModelThinkingLevel

Kind: type

```ts
export type ModelThinkingLevel = "off" | ThinkingLevel;
```

#### OpenAICompletionsCompat

Kind: interface

```ts
/**
 * Compatibility settings for OpenAI-compatible completions APIs.
 * Use this to override URL-based auto-detection for custom providers.
 */
export interface OpenAICompletionsCompat {
    /** Whether the provider supports the `store` field. Default: auto-detected from URL. */
    supportsStore?: boolean;
    /** Whether the provider supports the `developer` role (vs `system`). Default: auto-detected from URL. */
    supportsDeveloperRole?: boolean;
    /** Whether the provider supports `reasoning_effort`. Default: auto-detected from URL. */
    supportsReasoningEffort?: boolean;
    /** Whether the provider supports `stream_options: { include_usage: true }` for token usage in streaming responses. Default: true. */
    supportsUsageInStreaming?: boolean;
    /** Which field to use for max tokens. Default: auto-detected from URL. */
    maxTokensField?: "max_completion_tokens" | "max_tokens";
    /** Whether tool results require the `name` field. Default: auto-detected from URL. */
    requiresToolResultName?: boolean;
    /** Whether a user message after tool results requires an assistant message in between. Default: auto-detected from URL. */
    requiresAssistantAfterToolResult?: boolean;
    /** Whether thinking blocks must be converted to text blocks with <thinking> delimiters. Default: auto-detected from URL. */
    requiresThinkingAsText?: boolean;
    /** Whether all replayed assistant messages must include an empty reasoning_content field when reasoning is enabled. Default: auto-detected from URL. */
    requiresReasoningContentOnAssistantMessages?: boolean;
    /** Format for reasoning/thinking parameter. "openai" uses reasoning_effort, "openrouter" uses reasoning: { effort }, "deepseek" uses thinking: { type } plus reasoning_effort, "zai" uses top-level enable_thinking: boolean, "qwen" uses top-level enable_thinking: boolean, and "qwen-chat-template" uses chat_template_kwargs.enable_thinking. Default: "openai". */
    thinkingFormat?: "openai" | "openrouter" | "deepseek" | "zai" | "qwen" | "qwen-chat-template";
    /** OpenRouter-specific routing preferences. Only used when baseUrl points to OpenRouter. */
    openRouterRouting?: OpenRouterRouting;
    /** Vercel AI Gateway routing preferences. Only used when baseUrl points to Vercel AI Gateway. */
    vercelGatewayRouting?: VercelGatewayRouting;
    /** Whether z.ai supports top-level `tool_stream: true` for streaming tool call deltas. Default: false. */
    zaiToolStream?: boolean;
    /** Whether the provider supports the `strict` field in tool definitions. Default: true. */
    supportsStrictMode?: boolean;
    /** Cache control convention for prompt caching. "anthropic" applies Anthropic-style `cache_control` markers to the system prompt, last tool definition, and last user/assistant text content. */
    cacheControlFormat?: "anthropic";
    /** Whether to send known session-affinity headers (`session_id`, `x-client-request-id`, `x-session-affinity`) from `options.sessionId` when caching is enabled. Default: false. */
    sendSessionAffinityHeaders?: boolean;
    /** Whether the provider supports long prompt cache retention (`prompt_cache_retention: "24h"` or Anthropic-style `cache_control.ttl: "1h"`, depending on format). Default: true. */
    supportsLongCacheRetention?: boolean;
}
```

#### OpenAIResponsesCompat

Kind: interface

```ts
/** Compatibility settings for OpenAI Responses APIs. */
export interface OpenAIResponsesCompat {
    /** Whether to send the OpenAI `session_id` cache-affinity header from `options.sessionId` when caching is enabled. Default: true. */
    sendSessionIdHeader?: boolean;
    /** Whether the provider supports `prompt_cache_retention: "24h"`. Default: true. */
    supportsLongCacheRetention?: boolean;
}
```

#### OpenRouterRouting

Kind: interface

```ts
/**
 * OpenRouter provider routing preferences.
 * Controls which upstream providers OpenRouter routes requests to.
 * Sent as the `provider` field in the OpenRouter API request body.
 * @see https://openrouter.ai/docs/guides/routing/provider-selection
 */
export interface OpenRouterRouting {
    /** Whether to allow backup providers to serve requests. Default: true. */
    allow_fallbacks?: boolean;
    /** Whether to filter providers to only those that support all parameters in the request. Default: false. */
    require_parameters?: boolean;
    /** Data collection setting. "allow" (default): allow providers that may store/train on data. "deny": only use providers that don't collect user data. */
    data_collection?: "deny" | "allow";
    /** Whether to restrict routing to only ZDR (Zero Data Retention) endpoints. */
    zdr?: boolean;
    /** Whether to restrict routing to only models that allow text distillation. */
    enforce_distillable_text?: boolean;
    /** An ordered list of provider names/slugs to try in sequence, falling back to the next if unavailable. */
    order?: string[];
    /** List of provider names/slugs to exclusively allow for this request. */
    only?: string[];
    /** List of provider names/slugs to skip for this request. */
    ignore?: string[];
    /** A list of quantization levels to filter providers by (e.g., ["fp16", "bf16", "fp8", "fp6", "int8", "int4", "fp4", "fp32"]). */
    quantizations?: string[];
    /** Sorting strategy. Can be a string (e.g., "price", "throughput", "latency") or an object with `by` and `partition`. */
    sort?: string | {
        /** The sorting metric: "price", "throughput", "latency". */
        by?: string;
        /** Partitioning strategy: "model" (default) or "none". */
        partition?: string | null;
    };
    /** Maximum price per million tokens (USD). */
    max_price?: {
        /** Price per million prompt tokens. */
        prompt?: number | string;
        /** Price per million completion tokens. */
        completion?: number | string;
        /** Price per image. */
        image?: number | string;
        /** Price per audio unit. */
        audio?: number | string;
        /** Price per request. */
        request?: number | string;
    };
    /** Preferred minimum throughput (tokens/second). Can be a number (applies to p50) or an object with percentile-specific cutoffs. */
    preferred_min_throughput?: number | {
        /** Minimum tokens/second at the 50th percentile. */
        p50?: number;
        /** Minimum tokens/second at the 75th percentile. */
        p75?: number;
        /** Minimum tokens/second at the 90th percentile. */
        p90?: number;
        /** Minimum tokens/second at the 99th percentile. */
        p99?: number;
    };
    /** Preferred maximum latency (seconds). Can be a number (applies to p50) or an object with percentile-specific cutoffs. */
    preferred_max_latency?: number | {
        /** Maximum latency in seconds at the 50th percentile. */
        p50?: number;
        /** Maximum latency in seconds at the 75th percentile. */
        p75?: number;
        /** Maximum latency in seconds at the 90th percentile. */
        p90?: number;
        /** Maximum latency in seconds at the 99th percentile. */
        p99?: number;
    };
}
```

#### Provider

Kind: type

```ts
export type Provider = KnownProvider | string;
```

#### ProviderResponse

Kind: interface

```ts
export interface ProviderResponse {
    status: number;
    headers: Record<string, string>;
}
```

#### ProviderStreamOptions

Kind: type

```ts
export type ProviderStreamOptions = StreamOptions & Record<string, unknown>;
```

#### SimpleStreamOptions

Kind: interface

```ts
export interface SimpleStreamOptions extends StreamOptions {
    reasoning?: ThinkingLevel;
    /** Custom token budgets for thinking levels (token-based providers only) */
    thinkingBudgets?: ThinkingBudgets;
}
```

#### StopReason

Kind: type

```ts
export type StopReason = "stop" | "length" | "toolUse" | "error" | "aborted";
```

#### StreamFunction

Kind: type

```ts
export type StreamFunction<TApi extends Api = Api, TOptions extends StreamOptions = StreamOptions> = (model: Model<TApi>, context: Context, options?: TOptions) => AssistantMessageEventStream;
```

#### StreamOptions

Kind: interface

```ts
export interface StreamOptions {
    temperature?: number;
    maxTokens?: number;
    signal?: AbortSignal;
    apiKey?: string;
    /**
     * Preferred transport for providers that support multiple transports.
     * Providers that do not support this option ignore it.
     */
    transport?: Transport;
    /**
     * Prompt cache retention preference. Providers map this to their supported values.
     * Default: "short".
     */
    cacheRetention?: CacheRetention;
    /**
     * Optional session identifier for providers that support session-based caching.
     * Providers can use this to enable prompt caching, request routing, or other
     * session-aware features. Ignored by providers that don't support it.
     */
    sessionId?: string;
    /**
     * Optional callback for inspecting or replacing provider payloads before sending.
     * Return undefined to keep the payload unchanged.
     */
    onPayload?: (payload: unknown, model: Model<Api>) => unknown | undefined | Promise<unknown | undefined>;
    /**
     * Optional callback invoked after an HTTP response is received and before
     * its body stream is consumed.
     */
    onResponse?: (response: ProviderResponse, model: Model<Api>) => void | Promise<void>;
    /**
     * Optional custom HTTP headers to include in API requests.
     * Merged with provider defaults; can override default headers.
     * Not supported by all providers (e.g., AWS Bedrock uses SDK auth).
     */
    headers?: Record<string, string>;
    /**
     * HTTP request timeout in milliseconds for providers/SDKs that support it.
     * For example, OpenAI and Anthropic SDK clients default to 10 minutes.
     */
    timeoutMs?: number;
    /**
     * Maximum retry attempts for providers/SDKs that support client-side retries.
     * For example, OpenAI and Anthropic SDK clients default to 2.
     */
    maxRetries?: number;
    /**
     * Maximum delay in milliseconds to wait for a retry when the server requests a long wait.
     * If the server's requested delay exceeds this value, the request fails immediately
     * with an error containing the requested delay, allowing higher-level retry logic
     * to handle it with user visibility.
     * Default: 60000 (60 seconds). Set to 0 to disable the cap.
     */
    maxRetryDelayMs?: number;
    /**
     * Optional metadata to include in API requests.
     * Providers extract the fields they understand and ignore the rest.
     * For example, Anthropic uses `user_id` for abuse tracking and rate limiting.
     */
    metadata?: Record<string, unknown>;
}
```

#### SystemPromptSection

Kind: interface

```ts
/**
 * One ordered section of a structured system prompt.
 *
 * Each section's `text` must include its own leading separator, so that
 * concatenating all sections produces the equivalent single-string prompt.
 *
 * Fields are `readonly` because section objects are shared across the
 * session/extension boundary; build a new section instead of mutating one.
 */
export interface SystemPromptSection {
    /** Informational label for diagnostics and display. Ids are not deduplicated or looked up. */
    readonly id: string;
    readonly text: string;
    /**
     * "none" marks volatile content that providers should exclude from the
     * stable cached prefix. Omit for stable sections — per-section TTLs are
     * not supported (the request-level cache retention applies).
     */
    readonly cacheRetention?: "none";
}
```

#### TextContent

Kind: interface

```ts
export interface TextContent {
    type: "text";
    text: string;
    textSignature?: string;
}
```

#### TextSignatureV1

Kind: interface

```ts
export interface TextSignatureV1 {
    v: 1;
    id: string;
    phase?: "commentary" | "final_answer";
}
```

#### ThinkingBudgets

Kind: interface

```ts
/** Token budgets for each thinking level (token-based providers only) */
export interface ThinkingBudgets {
    minimal?: number;
    low?: number;
    medium?: number;
    high?: number;
}
```

#### ThinkingContent

Kind: interface

```ts
export interface ThinkingContent {
    type: "thinking";
    thinking: string;
    thinkingSignature?: string;
    /** When true, the thinking content was redacted by safety filters. The opaque
     *  encrypted payload is stored in `thinkingSignature` so it can be passed back
     *  to the API for multi-turn continuity. */
    redacted?: boolean;
}
```

#### ThinkingLevel

Kind: type

```ts
export type ThinkingLevel = "minimal" | "low" | "medium" | "high" | "xhigh";
```

#### ThinkingLevelMap

Kind: type

```ts
export type ThinkingLevelMap = Partial<Record<ModelThinkingLevel, string | null>>;
```

#### Tool

Kind: interface

```ts
export interface Tool<TParameters extends TSchema = TSchema> {
    name: string;
    description: string;
    parameters: TParameters;
}
```

#### ToolCall

Kind: interface

```ts
export interface ToolCall {
    type: "toolCall";
    id: string;
    name: string;
    arguments: Record<string, any>;
    thoughtSignature?: string;
}
```

#### ToolResultMessage

Kind: interface

```ts
export interface ToolResultMessage<TDetails = any> {
    role: "toolResult";
    toolCallId: string;
    toolName: string;
    content: (TextContent | ImageContent)[];
    details?: TDetails;
    isError: boolean;
    timestamp: number;
}
```

#### Transport

Kind: type

```ts
export type Transport = "sse" | "websocket" | "websocket-cached" | "auto";
```

#### Usage

Kind: interface

```ts
export interface Usage {
    input: number;
    output: number;
    cacheRead: number;
    cacheWrite: number;
    totalTokens: number;
    cost: {
        input: number;
        output: number;
        cacheRead: number;
        cacheWrite: number;
        total: number;
    };
}
```

#### UserMessage

Kind: interface

```ts
export interface UserMessage {
    role: "user";
    content: string | (TextContent | ImageContent)[];
    timestamp: number;
}
```

#### VercelGatewayRouting

Kind: interface

```ts
/**
 * Vercel AI Gateway routing preferences.
 * Controls which upstream providers the gateway routes requests to.
 * @see https://vercel.com/docs/ai-gateway/models-and-providers/provider-options
 */
export interface VercelGatewayRouting {
    /** List of provider slugs to exclusively use for this request (e.g., ["bedrock", "anthropic"]). */
    only?: string[];
    /** List of provider slugs to try in order (e.g., ["anthropic", "openai"]). */
    order?: string[];
}
```

### utils/diagnostics

#### appendAssistantMessageDiagnostic

Kind: function

```ts
export declare function appendAssistantMessageDiagnostic<T extends {
    diagnostics?: AssistantMessageDiagnostic[];
}>(message: T, diagnostic: AssistantMessageDiagnostic): void;
```

#### AssistantMessageDiagnostic

Kind: interface

```ts
export interface AssistantMessageDiagnostic {
    type: string;
    timestamp: number;
    error?: DiagnosticErrorInfo;
    details?: Record<string, unknown>;
}
```

#### createAssistantMessageDiagnostic

Kind: function

```ts
export declare function createAssistantMessageDiagnostic(type: string, error: unknown, details?: Record<string, unknown>): AssistantMessageDiagnostic;
```

#### DiagnosticErrorInfo

Kind: interface

```ts
export interface DiagnosticErrorInfo {
    name?: string;
    message: string;
    stack?: string;
    code?: string | number;
}
```

#### extractDiagnosticError

Kind: function

```ts
export declare function extractDiagnosticError(error: unknown): DiagnosticErrorInfo;
```

#### formatThrownValue

Kind: function

```ts
export declare function formatThrownValue(value: unknown): string;
```

### utils/event-stream

#### AssistantMessageEventStream

Kind: class

```ts
export declare class AssistantMessageEventStream extends EventStream<AssistantMessageEvent, AssistantMessage> {
    constructor();
}
```

#### createAssistantMessageEventStream

Kind: function

```ts
/** Factory function for AssistantMessageEventStream (for use in extensions) */
export declare function createAssistantMessageEventStream(): AssistantMessageEventStream;
```

#### EventStream

Kind: class

```ts
export declare class EventStream<T, R = T> implements AsyncIterable<T> {
    private isComplete;
    private extractResult;
    private queue;
    private waiting;
    private done;
    private finalResultPromise;
    private resolveFinalResult;
    constructor(isComplete: (event: T) => boolean, extractResult: (event: T) => R);
    push(event: T): void;
    end(result?: R): void;
    [Symbol.asyncIterator](): AsyncIterator<T>;
    result(): Promise<R>;
}
```

### utils/json-parse

#### parseJsonWithRepair

Kind: function

```ts
export declare function parseJsonWithRepair<T>(json: string): T;
```

#### parseStreamingJson

Kind: function

```ts
/**
 * Attempts to parse potentially incomplete JSON during streaming.
 * Always returns a valid object, even if the JSON is incomplete.
 *
 * @param partialJson The partial JSON string from streaming
 * @returns Parsed object or empty object if parsing fails
 */
export declare function parseStreamingJson<T = Record<string, unknown>>(partialJson: string | undefined): T;
```

#### repairJson

Kind: function

```ts
/**
 * Repairs malformed JSON string literals by:
 * - escaping raw control characters inside strings
 * - doubling backslashes before invalid escape characters
 */
export declare function repairJson(json: string): string;
```

### utils/oauth/types

#### OAuthAuthInfo

Kind: type

```ts
export type OAuthAuthInfo = {
    url: string;
    instructions?: string;
};
```

#### OAuthCredentials

Kind: type

```ts
export type OAuthCredentials = {
    refresh: string;
    access: string;
    expires: number;
    [key: string]: unknown;
};
```

#### OAuthLoginCallbacks

Kind: interface

```ts
export interface OAuthLoginCallbacks {
    onAuth: (info: OAuthAuthInfo) => void;
    onPrompt: (prompt: OAuthPrompt) => Promise<string>;
    onProgress?: (message: string) => void;
    onManualCodeInput?: () => Promise<string>;
    /** Show an interactive selector and return the selected option id, or undefined on cancel. */
    onSelect?: (prompt: OAuthSelectPrompt) => Promise<string | undefined>;
    signal?: AbortSignal;
}
```

#### OAuthPrompt

Kind: type

```ts
export type OAuthPrompt = {
    message: string;
    placeholder?: string;
    allowEmpty?: boolean;
};
```

#### OAuthProvider

Kind: type

```ts
/** @deprecated Use OAuthProviderId instead */
export type OAuthProvider = OAuthProviderId;
```

#### OAuthProviderId

Kind: type

```ts
export type OAuthProviderId = string;
```

#### OAuthProviderInfo

Kind: interface

```ts
/** @deprecated Use OAuthProviderInterface instead */
export interface OAuthProviderInfo {
    id: OAuthProviderId;
    name: string;
    available: boolean;
}
```

#### OAuthProviderInterface

Kind: interface

```ts
export interface OAuthProviderInterface {
    readonly id: OAuthProviderId;
    readonly name: string;
    /** Run the login flow, return credentials to persist */
    login(callbacks: OAuthLoginCallbacks): Promise<OAuthCredentials>;
    /** Whether login uses a local callback server and supports manual code input. */
    usesCallbackServer?: boolean;
    /** Refresh expired credentials, return updated credentials to persist */
    refreshToken(credentials: OAuthCredentials): Promise<OAuthCredentials>;
    /** Convert credentials to API key string for the provider */
    getApiKey(credentials: OAuthCredentials): string;
    /** Optional: modify models for this provider (e.g., update baseUrl) */
    modifyModels?(models: Model<Api>[], credentials: OAuthCredentials): Model<Api>[];
}
```

#### OAuthSelectOption

Kind: type

```ts
export type OAuthSelectOption = {
    id: string;
    label: string;
};
```

#### OAuthSelectPrompt

Kind: type

```ts
export type OAuthSelectPrompt = {
    message: string;
    options: OAuthSelectOption[];
};
```

### utils/overflow

#### getOverflowPatterns

Kind: function

```ts
/**
 * Get the overflow patterns for testing purposes.
 */
export declare function getOverflowPatterns(): RegExp[];
```

#### isContextOverflow

Kind: function

```ts
/**
 * Check if an assistant message represents a context overflow error.
 *
 * This handles two cases:
 * 1. Error-based overflow: Most providers return stopReason "error" with a
 *    specific error message pattern.
 * 2. Silent overflow: Some providers accept overflow requests and return
 *    successfully. For these, we check if usage.input exceeds the context window.
 *
 * ## Reliability by Provider
 *
 * **Reliable detection (returns error with detectable message):**
 * - Anthropic: "prompt is too long: X tokens > Y maximum" or "request_too_large"
 * - OpenAI (Completions & Responses): "exceeds the context window"
 * - Google Gemini: "input token count exceeds the maximum"
 * - xAI (Grok): "maximum prompt length is X but request contains Y"
 * - Groq: "reduce the length of the messages"
 * - Cerebras: 400/413 status code (no body)
 * - Mistral: "Prompt contains X tokens ... too large for model with Y maximum context length"
 * - OpenRouter (all backends): "maximum context length is X tokens"
 * - llama.cpp: "exceeds the available context size"
 * - LM Studio: "greater than the context length"
 * - Kimi For Coding: "exceeded model token limit: X (requested: Y)"
 *
 * **Unreliable detection:**
 * - z.ai: Sometimes accepts overflow silently (detectable via usage.input > contextWindow),
 *   sometimes returns rate limit errors. Pass contextWindow param to detect silent overflow.
 * - Xiaomi MiMo: Truncates input to fit contextWindow then returns stopReason "length" with
 *   output=0. Pass contextWindow param to detect via the "filled context + zero output" signal.
 * - Ollama: May truncate input silently for some setups, but may also return explicit
 *   overflow errors that match the patterns above. Silent truncation still cannot be
 *   detected here because we do not know the expected token count.
 *
 * ## Custom Providers
 *
 * If you've added custom models via settings.json, this function may not detect
 * overflow errors from those providers. To add support:
 *
 * 1. Send a request that exceeds the model's context window
 * 2. Check the errorMessage in the response
 * 3. Create a regex pattern that matches the error
 * 4. The pattern should be added to OVERFLOW_PATTERNS in this file, or
 *    check the errorMessage yourself before calling this function
 *
 * @param message - The assistant message to check
 * @param contextWindow - Optional context window size for detecting silent overflow (z.ai)
 * @returns true if the message indicates a context overflow
 */
export declare function isContextOverflow(message: AssistantMessage, contextWindow?: number): boolean;
```

### utils/system-prompt

#### flattenSystemPrompt

Kind: function

```ts
/**
 * Flattens a system prompt to a single string.
 *
 * Sections are joined with no inserted separator: each section's `text`
 * carries its own leading separator, so flattening an array of sections is
 * byte-identical to the equivalent single-string prompt. String prompts pass
 * through unchanged.
 */
export declare function flattenSystemPrompt(prompt: string | SystemPromptSection[]): string;

export declare function flattenSystemPrompt(prompt: string | SystemPromptSection[] | undefined): string | undefined;
```

### utils/typebox-helpers

#### StringEnum

Kind: function

```ts
/**
 * Creates a string enum schema compatible with Google's API and other providers
 * that don't support anyOf/const patterns.
 *
 * @example
 * const OperationSchema = StringEnum(["add", "subtract", "multiply", "divide"], {
 *   description: "The operation to perform"
 * });
 *
 * type Operation = Static<typeof OperationSchema>; // "add" | "subtract" | "multiply" | "divide"
 */
export declare function StringEnum<T extends readonly string[]>(values: T, options?: {
    description?: string;
    default?: T[number];
}): TUnsafe<T[number]>;
```

### utils/validation

#### validateToolArguments

Kind: function

```ts
/**
 * Validates tool call arguments against the tool's TypeBox schema
 * @param tool The tool definition with TypeBox schema
 * @param toolCall The tool call from the LLM
 * @returns The validated (and potentially coerced) arguments
 * @throws Error with formatted message if validation fails
 */
export declare function validateToolArguments(tool: Tool, toolCall: ToolCall): any;
```

#### validateToolCall

Kind: function

```ts
/**
 * Finds a tool by name and validates the tool call arguments against its TypeBox schema
 * @param tools Array of tool definitions
 * @param toolCall The tool call from the LLM
 * @returns The validated arguments
 * @throws Error if tool is not found or validation fails
 */
export declare function validateToolCall(tools: Tool[], toolCall: ToolCall): any;
```

## @earendil-works/pi-coding-agent

### config

#### getAgentDir

Kind: function

```ts
/** Get the agent config directory (e.g., ~/.pi/agent/) */
export declare function getAgentDir(): string;
```

#### VERSION

Kind: const

```ts
export declare const VERSION: string;
```

### core/agent-session

#### AgentSession

Kind: class

```ts
export declare class AgentSession {
    readonly agent: Agent;
    readonly sessionManager: SessionManager;
    readonly settingsManager: SettingsManager;
    private _scopedModels;
    private _unsubscribeAgent?;
    private _eventListeners;
    private _agentEventQueue;
    /** Tracks pending steering messages for UI display. Removed when delivered. */
    private _steeringMessages;
    /** Tracks pending follow-up messages for UI display. Removed when delivered. */
    private _followUpMessages;
    /** Messages queued to be included with the next user prompt as context ("asides"). */
    private _pendingNextTurnMessages;
    private _compactionAbortController;
    private _autoCompactionAbortController;
    private _overflowRecoveryAttempted;
    private _branchSummaryAbortController;
    private _retryAbortController;
    private _retryAttempt;
    private _retryPromise;
    private _retryResolve;
    private _bashAbortController;
    private _pendingBashMessages;
    private _extensionRunner;
    private _turnIndex;
    private _resourceLoader;
    private _customTools;
    private _baseToolDefinitions;
    private _cwd;
    private _extensionRunnerRef?;
    private _initialActiveToolNames?;
    private _allowedToolNames?;
    private _baseToolsOverride?;
    private _sessionStartEvent;
    private _extensionUIContext?;
    private _extensionCommandContextActions?;
    private _extensionShutdownHandler?;
    private _extensionErrorListener?;
    private _extensionErrorUnsubscriber?;
    private _modelRegistry;
    private _toolRegistry;
    private _toolDefinitions;
    private _toolPromptSnippets;
    private _toolPromptGuidelines;
    private _baseSystemPromptSections;
    private _baseSystemPromptOptions;
    constructor(config: AgentSessionConfig);
    /** Model registry for API key resolution and model discovery */
    get modelRegistry(): ModelRegistry;
    private _getRequiredRequestAuth;
    /**
     * Install tool hooks once on the Agent instance.
     *
     * The callbacks read `this._extensionRunner` at execution time, so extension reload swaps in the
     * new runner without reinstalling hooks. Extension-specific tool wrappers are still used to adapt
     * registered tool execution to the extension context. Tool call and tool result interception now
     * happens here instead of in wrappers.
     */
    private _installAgentToolHooks;
    /** Emit an event to all listeners */
    private _emit;
    private _emitQueueUpdate;
    private _lastAssistantMessage;
    /** Internal handler for agent events - shared by subscribe and reconnect */
    private _handleAgentEvent;
    private _createRetryPromiseForAgentEnd;
    private _findLastAssistantInMessages;
    private _processAgentEvent;
    /** Resolve the pending retry promise */
    private _resolveRetry;
    /** Extract text content from a message */
    private _getUserMessageText;
    /** Find the last assistant message in agent state (including aborted ones) */
    private _findLastAssistantMessage;
    private _replaceMessageInPlace;
    private _emitExtensionEvent;
    /**
     * Subscribe to agent events.
     * Session persistence is handled internally (saves messages on message_end).
     * Multiple listeners can be added. Returns unsubscribe function for this listener.
     */
    subscribe(listener: AgentSessionEventListener): () => void;
    /**
     * Temporarily disconnect from agent events.
     * User listeners are preserved and will receive events again after resubscribe().
     * Used internally during operations that need to pause event processing.
     */
    private _disconnectFromAgent;
    /**
     * Reconnect to agent events after _disconnectFromAgent().
     * Preserves all existing listeners.
     */
    private _reconnectToAgent;
    /**
     * Remove all listeners and disconnect from agent.
     * Call this when completely done with the session.
     */
    dispose(): void;
    /** Full agent state */
    get state(): AgentState;
    /** Current model (may be undefined if not yet selected) */
    get model(): Model<any> | undefined;
    /** Current thinking level */
    get thinkingLevel(): ThinkingLevel;
    /** Whether agent is currently streaming a response */
    get isStreaming(): boolean;
    /** Current effective system prompt (includes any per-turn extension modifications), flattened to a string */
    get systemPrompt(): string;
    /** Current retry attempt (0 if not retrying) */
    get retryAttempt(): number;
    /**
     * Get the names of currently active tools.
     * Returns the names of tools currently set on the agent.
     */
    getActiveToolNames(): string[];
    /**
     * Get all configured tools with name, description, parameter schema, and source metadata.
     */
    getAllTools(): ToolInfo[];
    getToolDefinition(name: string): ToolDefinition | undefined;
    /**
     * Set active tools by name.
     * Only tools in the registry can be enabled. Unknown tool names are ignored.
     * Also rebuilds the system prompt to reflect the new tool set.
     * Changes take effect on the next agent turn.
     */
    setActiveToolsByName(toolNames: string[]): void;
    /** Whether compaction or branch summarization is currently running */
    get isCompacting(): boolean;
    /** All messages including custom types like BashExecutionMessage */
    get messages(): AgentMessage[];
    /** Current steering mode */
    get steeringMode(): "all" | "one-at-a-time";
    /** Current follow-up mode */
    get followUpMode(): "all" | "one-at-a-time";
    /** Current session file path, or undefined if sessions are disabled */
    get sessionFile(): string | undefined;
    /** Current session ID */
    get sessionId(): string;
    /** Current session display name, if set */
    get sessionName(): string | undefined;
    /** Scoped models for cycling (from --models flag) */
    get scopedModels(): ReadonlyArray<{
        model: Model<any>;
        thinkingLevel?: ThinkingLevel;
    }>;
    /** Update scoped models for cycling */
    setScopedModels(scopedModels: Array<{
        model: Model<any>;
        thinkingLevel?: ThinkingLevel;
    }>): void;
    /** File-based prompt templates */
    get promptTemplates(): ReadonlyArray<PromptTemplate>;
    private _normalizePromptSnippet;
    private _normalizePromptGuidelines;
    /** Rebuild `_baseSystemPromptSections` from the current resources and tool set and apply it to agent state. */
    private _rebuildSystemPrompt;
    /**
     * Send a prompt to the agent.
     * - Handles extension commands (registered via pi.registerCommand) immediately, even during streaming
     * - Expands file-based prompt templates by default
     * - During streaming, queues via steer() or followUp() based on streamingBehavior option
     * - Validates model and API key before sending (when not streaming)
     * @throws Error if streaming and no streamingBehavior specified
     * @throws Error if no model selected or no API key available (when not streaming)
     */
    prompt(text: string, options?: PromptOptions): Promise<void>;
    private _tryExecuteExtensionCommand;
    /**
     * Expand skill commands (/skill:name args) to their full content.
     * Returns the expanded text, or the original text if not a skill command or skill not found.
     * Emits errors via extension runner if file read fails.
     */
    private _expandSkillCommand;
    /**
     * Queue a steering message while the agent is running.
     * Delivered after the current assistant turn finishes executing its tool calls,
     * before the next LLM call.
     * Expands skill commands and prompt templates. Errors on extension commands.
     * @param images Optional image attachments to include with the message
     * @throws Error if text is an extension command
     */
    steer(text: string, images?: ImageContent[]): Promise<void>;
    /**
     * Queue a follow-up message to be processed after the agent finishes.
     * Delivered only when agent has no more tool calls or steering messages.
     * Expands skill commands and prompt templates. Errors on extension commands.
     * @param images Optional image attachments to include with the message
     * @throws Error if text is an extension command
     */
    followUp(text: string, images?: ImageContent[]): Promise<void>;
    private _queueSteer;
    private _queueFollowUp;
    /**
     * Throw an error if the text is an extension command.
     */
    private _throwIfExtensionCommand;
    /**
     * Send a custom message to the session. Creates a CustomMessageEntry.
     *
     * Handles three cases:
     * - Streaming: queues message, processed when loop pulls from queue
     * - Not streaming + triggerTurn: appends to state/session, starts new turn
     * - Not streaming + no trigger: appends to state/session, no turn
     *
     * @param message Custom message with customType, content, display, details
     * @param options.triggerTurn If true and not streaming, triggers a new LLM turn
     * @param options.deliverAs Delivery mode: "steer", "followUp", or "nextTurn"
     */
    sendCustomMessage<T = unknown>(message: Pick<CustomMessage<T>, "customType" | "content" | "display" | "details">, options?: {
        triggerTurn?: boolean;
        deliverAs?: SendMessageDeliverAs;
    }): Promise<void>;
    /**
     * Send a user message to the agent. Always triggers a turn.
     * When the agent is streaming, use deliverAs to specify how to queue the message.
     *
     * @param content User message content (string or content array)
     * @param options.deliverAs Delivery mode when streaming: "steer" or "followUp"
     */
    sendUserMessage(content: string | (TextContent | ImageContent)[], options?: {
        deliverAs?: DeliverAs;
    }): Promise<void>;
    /**
     * Dispatch input through the same pipeline as typed editor input:
     * extension-registered slash commands, prompt templates, and skills all
     * resolve as if the user had typed the text (source: "extension").
     *
     * @param options.deliverAs Delivery mode when streaming: "steer" or "followUp"
     * @throws Error if streaming and no deliverAs specified, unless the input is
     *   handled before reaching the prompt queue (extension commands execute
     *   immediately; input consumed by an `input` handler returns normally)
     */
    dispatchUserInput(input: string, options?: DispatchUserInputOptions): Promise<void>;
    /**
     * Clear all queued messages and return them.
     * Useful for restoring to editor when user aborts.
     * @returns Object with steering and followUp arrays
     */
    clearQueue(): {
        steering: string[];
        followUp: string[];
    };
    /** Number of pending messages (includes both steering and follow-up) */
    get pendingMessageCount(): number;
    /** Get pending steering messages (read-only) */
    getSteeringMessages(): readonly string[];
    /** Get pending follow-up messages (read-only) */
    getFollowUpMessages(): readonly string[];
    get resourceLoader(): ResourceLoader;
    /**
     * Abort current operation and wait for agent to become idle.
     */
    abort(): Promise<void>;
    private _emitModelSelect;
    /**
     * Set model directly.
     * Validates that auth is configured, saves to session and settings.
     * @throws Error if no auth is configured for the model
     */
    setModel(model: Model<any>): Promise<void>;
    /**
     * Cycle to next/previous model.
     * Uses scoped models (from --models flag) if available, otherwise all available models.
     * @param direction - "forward" (default) or "backward"
     * @returns The new model info, or undefined if only one model available
     */
    cycleModel(direction?: "forward" | "backward"): Promise<ModelCycleResult | undefined>;
    private _cycleScopedModel;
    private _cycleAvailableModel;
    /**
     * Set thinking level.
     * Clamps to model capabilities based on available thinking levels.
     * Saves to session and settings only if the level actually changes.
     */
    setThinkingLevel(level: ThinkingLevel): void;
    /**
     * Cycle to next thinking level.
     * @returns New level, or undefined if model doesn't support thinking
     */
    cycleThinkingLevel(): ThinkingLevel | undefined;
    /**
     * Get available thinking levels for current model.
     * The provider will clamp to what the specific model supports internally.
     */
    getAvailableThinkingLevels(): ThinkingLevel[];
    /**
     * Check if current model supports thinking/reasoning.
     */
    supportsThinking(): boolean;
    private _getThinkingLevelForModelSwitch;
    private _clampThinkingLevel;
    /**
     * Set steering message mode.
     * Saves to settings.
     */
    setSteeringMode(mode: "all" | "one-at-a-time"): void;
    /**
     * Set follow-up message mode.
     * Saves to settings.
     */
    setFollowUpMode(mode: "all" | "one-at-a-time"): void;
    /**
     * Manually compact the session context.
     * Aborts current agent operation first.
     * @param customInstructions Optional instructions for the compaction summary
     */
    compact(customInstructions?: string): Promise<CompactionResult>;
    /**
     * Cancel in-progress compaction (manual or auto).
     */
    abortCompaction(): void;
    /**
     * Cancel in-progress branch summarization.
     */
    abortBranchSummary(): void;
    private _checkCompaction;
    private _runAutoCompaction;
    /**
     * Toggle auto-compaction setting.
     */
    setAutoCompactionEnabled(enabled: boolean): void;
    /** Whether auto-compaction is enabled */
    get autoCompactionEnabled(): boolean;
    bindExtensions(bindings: ExtensionBindings): Promise<void>;
    private extendResourcesFromExtensions;
    private buildExtensionResourcePaths;
    private getExtensionSourceLabel;
    private _applyExtensionBindings;
    private _refreshCurrentModelFromRegistry;
    private _bindExtensionCore;
    private _refreshToolRegistry;
    private _buildRuntime;
    reload(): Promise<void>;
    /**
     * Check if an error is retryable (overloaded, rate limit, server errors).
     * Context overflow errors are NOT retryable (handled by compaction instead).
     */
    private _isRetryableError;
    private _handleRetryableError;
    /**
     * Cancel in-progress retry.
     */
    abortRetry(): void;
    private waitForRetry;
    /** Whether auto-retry is currently in progress */
    get isRetrying(): boolean;
    /** Whether auto-retry is enabled */
    get autoRetryEnabled(): boolean;
    /**
     * Toggle auto-retry setting.
     */
    setAutoRetryEnabled(enabled: boolean): void;
    /**
     * Execute a bash command.
     * Adds result to agent context and session.
     * @param command The bash command to execute
     * @param onChunk Optional streaming callback for output
     * @param options.excludeFromContext If true, command output won't be sent to LLM (!! prefix)
     * @param options.operations Custom BashOperations for remote execution
     */
    executeBash(command: string, onChunk?: (chunk: string) => void, options?: {
        excludeFromContext?: boolean;
        operations?: BashOperations;
    }): Promise<BashResult>;
    /**
     * Record a bash execution result in session history.
     * Used by executeBash and by extensions that handle bash execution themselves.
     */
    recordBashResult(command: string, result: BashResult, options?: {
        excludeFromContext?: boolean;
    }): void;
    /**
     * Cancel running bash command.
     */
    abortBash(): void;
    /** Whether a bash command is currently running */
    get isBashRunning(): boolean;
    /** Whether there are pending bash messages waiting to be flushed */
    get hasPendingBashMessages(): boolean;
    /**
     * Flush pending bash messages to agent state and session.
     * Called after agent turn completes to maintain proper message ordering.
     */
    private _flushPendingBashMessages;
    /**
     * Set a display name for the current session.
     */
    setSessionName(name: string): void;
    /**
     * Navigate to a different node in the session tree.
     * Unlike fork() which creates a new session file, this stays in the same file.
     *
     * @param targetId The entry ID to navigate to
     * @param options.summarize Whether user wants to summarize abandoned branch
     * @param options.customInstructions Custom instructions for summarizer
     * @param options.replaceInstructions If true, customInstructions replaces the default prompt
     * @param options.label Label to attach to the branch summary entry
     * @returns Result with editorText (if user message) and cancelled status
     */
    navigateTree(targetId: string, options?: {
        summarize?: boolean;
        customInstructions?: string;
        replaceInstructions?: boolean;
        label?: string;
    }): Promise<{
        editorText?: string;
        cancelled: boolean;
        aborted?: boolean;
        summaryEntry?: BranchSummaryEntry;
    }>;
    /**
     * Get all user messages from session for fork selector.
     */
    getUserMessagesForForking(): Array<{
        entryId: string;
        text: string;
    }>;
    private _extractUserMessageText;
    /**
     * Get session statistics.
     */
    getSessionStats(): SessionStats;
    getContextUsage(): ContextUsage | undefined;
    /**
     * Export session to HTML.
     * @param outputPath Optional output path (defaults to session directory)
     * @returns Path to exported file
     */
    exportToHtml(outputPath?: string): Promise<string>;
    /**
     * Export the current session branch to a JSONL file.
     * Writes the session header followed by all entries on the current branch path.
     * @param outputPath Target file path. If omitted, generates a timestamped file in cwd.
     * @returns The resolved output file path.
     */
    exportToJsonl(outputPath?: string): string;
    /**
     * Get text content of last assistant message.
     * Useful for /copy command.
     * @returns Text content, or undefined if no assistant message exists
     */
    getLastAssistantText(): string | undefined;
    createReplacedSessionContext(): ReplacedSessionContext;
    /**
     * Check if extensions have handlers for a specific event type.
     */
    hasExtensionHandlers(eventType: string): boolean;
    /**
     * Get the extension runner (for setting UI context and error handlers).
     */
    get extensionRunner(): ExtensionRunner;
}
```

#### AgentSessionConfig

Kind: interface

```ts
export interface AgentSessionConfig {
    agent: Agent;
    sessionManager: SessionManager;
    settingsManager: SettingsManager;
    cwd: string;
    /** Models to cycle through with Ctrl+P (from --models flag) */
    scopedModels?: Array<{
        model: Model<any>;
        thinkingLevel?: ThinkingLevel;
    }>;
    /** Resource loader for skills, prompts, themes, context files, system prompt */
    resourceLoader: ResourceLoader;
    /** SDK custom tools registered outside extensions */
    customTools?: ToolDefinition[];
    /** Model registry for API key resolution and model discovery */
    modelRegistry: ModelRegistry;
    /** Initial active built-in tool names. Default: [read, bash, edit, write] */
    initialActiveToolNames?: string[];
    /** Optional allowlist of tool names. When provided, only these tool names are exposed. */
    allowedToolNames?: string[];
    /**
     * Override base tools (useful for custom runtimes).
     *
     * These are synthesized into minimal ToolDefinitions internally so AgentSession can keep
     * a definition-first registry even when callers provide plain AgentTool instances.
     */
    baseToolsOverride?: Record<string, AgentTool>;
    /** Mutable ref used by Agent to access the current ExtensionRunner */
    extensionRunnerRef?: {
        current?: ExtensionRunner;
    };
    /** Session start event metadata emitted when extensions bind to this runtime. */
    sessionStartEvent?: SessionStartEvent;
}
```

#### AgentSessionEvent

Kind: type

```ts
/** Session-specific events that extend the core AgentEvent */
export type AgentSessionEvent = AgentEvent | {
    type: "queue_update";
    steering: readonly string[];
    followUp: readonly string[];
} | {
    type: "compaction_start";
    reason: "manual" | "threshold" | "overflow";
} | {
    type: "session_info_changed";
    name: string | undefined;
} | {
    type: "thinking_level_changed";
    level: ThinkingLevel;
} | {
    type: "compaction_end";
    reason: "manual" | "threshold" | "overflow";
    result: CompactionResult | undefined;
    aborted: boolean;
    willRetry: boolean;
    errorMessage?: string;
} | {
    type: "auto_retry_start";
    attempt: number;
    maxAttempts: number;
    delayMs: number;
    errorMessage: string;
} | {
    type: "auto_retry_end";
    success: boolean;
    attempt: number;
    finalError?: string;
};
```

#### AgentSessionEventListener

Kind: type

```ts
/** Listener function for agent session events */
export type AgentSessionEventListener = (event: AgentSessionEvent) => void;
```

#### ModelCycleResult

Kind: interface

```ts
/** Result from cycleModel() */
export interface ModelCycleResult {
    model: Model<any>;
    thinkingLevel: ThinkingLevel;
    /** Whether cycling through scoped models (--models flag) or all available */
    isScoped: boolean;
}
```

#### ParsedSkillBlock

Kind: interface

```ts
/** Parsed skill block from a user message */
export interface ParsedSkillBlock {
    name: string;
    location: string;
    content: string;
    userMessage: string | undefined;
}
```

#### parseSkillBlock

Kind: function

```ts
/**
 * Parse a skill block from message text.
 * Returns null if the text doesn't contain a skill block.
 */
export declare function parseSkillBlock(text: string): ParsedSkillBlock | null;
```

#### PromptOptions

Kind: interface

```ts
/** Options for AgentSession.prompt() */
export interface PromptOptions {
    /** Whether to expand file-based prompt templates (default: true) */
    expandPromptTemplates?: boolean;
    /** Image attachments */
    images?: ImageContent[];
    /**
     * When streaming, how to queue the message: "steer" (interrupt) or
     * "followUp" (wait). Prompting while streaming without it rejects, unless
     * the text is handled before it reaches the prompt queue (extension
     * commands execute immediately; input consumed by an `input` handler
     * returns without rejecting).
     */
    streamingBehavior?: DeliverAs;
    /** Source of input for extension input event handlers. Defaults to "interactive". */
    source?: InputSource;
    /** Internal hook used by RPC mode to observe prompt preflight acceptance or rejection. */
    preflightResult?: (success: boolean) => void;
}
```

#### SessionStats

Kind: interface

```ts
/** Session statistics for /session command */
export interface SessionStats {
    sessionFile: string | undefined;
    sessionId: string;
    userMessages: number;
    assistantMessages: number;
    toolCalls: number;
    toolResults: number;
    totalMessages: number;
    tokens: {
        input: number;
        output: number;
        cacheRead: number;
        cacheWrite: number;
        total: number;
    };
    cost: number;
    contextUsage?: ContextUsage;
}
```

### core/agent-session-runtime

#### AgentSessionRuntime

Kind: class

```ts
/**
 * Owns the current AgentSession plus its cwd-bound services.
 *
 * Session replacement methods tear down the current runtime first, then create
 * and apply the next runtime. If creation fails, the error is propagated to the
 * caller. The caller is responsible for user-facing error handling.
 */
export declare class AgentSessionRuntime {
    private _session;
    private _services;
    private readonly createRuntime;
    private _diagnostics;
    private _modelFallbackMessage?;
    private rebindSession?;
    private beforeSessionInvalidate?;
    constructor(_session: AgentSession, _services: AgentSessionServices, createRuntime: CreateAgentSessionRuntimeFactory, _diagnostics?: AgentSessionRuntimeDiagnostic[], _modelFallbackMessage?: string | undefined);
    get services(): AgentSessionServices;
    get session(): AgentSession;
    get cwd(): string;
    get diagnostics(): readonly AgentSessionRuntimeDiagnostic[];
    get modelFallbackMessage(): string | undefined;
    setRebindSession(rebindSession?: (session: AgentSession) => Promise<void>): void;
    /**
     * Set a synchronous callback that runs after `session_shutdown` handlers finish
     * but before the current session is invalidated.
     *
     * This is for host-owned UI teardown that must not yield to the event loop,
     * such as detaching extension-provided TUI components before the old extension
     * context becomes stale.
     */
    setBeforeSessionInvalidate(beforeSessionInvalidate?: () => void): void;
    private emitBeforeSwitch;
    private emitBeforeFork;
    private teardownCurrent;
    private apply;
    private finishSessionReplacement;
    switchSession(sessionPath: string, options?: {
        cwdOverride?: string;
        withSession?: (ctx: ReplacedSessionContext) => Promise<void>;
    }): Promise<{
        cancelled: boolean;
    }>;
    newSession(options?: {
        parentSession?: string;
        setup?: (sessionManager: SessionManager) => Promise<void>;
        withSession?: (ctx: ReplacedSessionContext) => Promise<void>;
    }): Promise<{
        cancelled: boolean;
    }>;
    fork(entryId: string, options?: {
        position?: "before" | "at";
        withSession?: (ctx: ReplacedSessionContext) => Promise<void>;
    }): Promise<{
        cancelled: boolean;
        selectedText?: string;
    }>;
    /**
     * Import a session JSONL file and switch runtime state to the imported session.
     *
     * @returns `{ cancelled: true }` when cancelled by `session_before_switch`, otherwise `{ cancelled: false }`.
     * @throws {SessionImportFileNotFoundError} When the input path does not exist.
     * @throws {MissingSessionCwdError} When the imported session cwd cannot be resolved and no override is provided.
     */
    importFromJsonl(inputPath: string, cwdOverride?: string): Promise<{
        cancelled: boolean;
    }>;
    dispose(): Promise<void>;
}
```

#### createAgentSessionRuntime

Kind: function

```ts
/**
 * Create the initial runtime from a runtime factory and initial session target.
 *
 * The same factory is stored on the returned AgentSessionRuntime and reused for
 * later /new, /resume, /fork, and import flows.
 */
export declare function createAgentSessionRuntime(createRuntime: CreateAgentSessionRuntimeFactory, options: {
    cwd: string;
    agentDir: string;
    sessionManager: SessionManager;
    sessionStartEvent?: SessionStartEvent;
}): Promise<AgentSessionRuntime>;
```

#### CreateAgentSessionRuntimeFactory

Kind: type

```ts
/**
 * Creates a full runtime for a target cwd and session manager.
 *
 * The factory closes over process-global fixed inputs, recreates cwd-bound
 * services for the effective cwd, resolves session options against those
 * services, and finally creates the AgentSession.
 */
export type CreateAgentSessionRuntimeFactory = (options: {
    cwd: string;
    agentDir: string;
    sessionManager: SessionManager;
    sessionStartEvent?: SessionStartEvent;
}) => Promise<CreateAgentSessionRuntimeResult>;
```

#### CreateAgentSessionRuntimeResult

Kind: interface

```ts
/**
 * Result returned by runtime creation.
 *
 * The caller gets the created session, its cwd-bound services, and all
 * diagnostics collected during setup.
 */
export interface CreateAgentSessionRuntimeResult extends CreateAgentSessionResult {
    services: AgentSessionServices;
    diagnostics: AgentSessionRuntimeDiagnostic[];
}
```

### core/agent-session-services

#### AgentSessionRuntimeDiagnostic

Kind: interface

```ts
/**
 * Non-fatal issues collected while creating services or sessions.
 *
 * Runtime creation returns diagnostics to the caller instead of printing or
 * exiting. The app layer decides whether warnings should be shown and whether
 * errors should abort startup.
 */
export interface AgentSessionRuntimeDiagnostic {
    type: "info" | "warning" | "error";
    message: string;
}
```

#### AgentSessionServices

Kind: interface

```ts
/**
 * Coherent cwd-bound runtime services for one effective session cwd.
 *
 * This is infrastructure only. The AgentSession itself is created separately so
 * session options can be resolved against these services first.
 */
export interface AgentSessionServices {
    cwd: string;
    agentDir: string;
    authStorage: AuthStorage;
    settingsManager: SettingsManager;
    modelRegistry: ModelRegistry;
    resourceLoader: ResourceLoader;
    diagnostics: AgentSessionRuntimeDiagnostic[];
}
```

#### createAgentSessionFromServices

Kind: function

```ts
/**
 * Create an AgentSession from previously created services.
 *
 * This keeps session creation separate from service creation so callers can
 * resolve model, thinking, tools, and other session inputs against the target
 * cwd before constructing the session.
 */
export declare function createAgentSessionFromServices(options: CreateAgentSessionFromServicesOptions): Promise<CreateAgentSessionResult>;
```

#### CreateAgentSessionFromServicesOptions

Kind: interface

```ts
/**
 * Inputs for creating an AgentSession from already-created services.
 *
 * Use this after services exist and any cwd-bound model/tool/session options
 * have been resolved against those services.
 */
export interface CreateAgentSessionFromServicesOptions {
    services: AgentSessionServices;
    sessionManager: SessionManager;
    sessionStartEvent?: SessionStartEvent;
    model?: Model<any>;
    thinkingLevel?: ThinkingLevel;
    scopedModels?: Array<{
        model: Model<any>;
        thinkingLevel?: ThinkingLevel;
    }>;
    cacheRetention?: CacheRetention;
    tools?: string[];
    noTools?: CreateAgentSessionOptions["noTools"];
    customTools?: ToolDefinition[];
}
```

#### createAgentSessionServices

Kind: function

```ts
/**
 * Create cwd-bound runtime services.
 *
 * Returns services plus diagnostics. It does not create an AgentSession.
 */
export declare function createAgentSessionServices(options: CreateAgentSessionServicesOptions): Promise<AgentSessionServices>;
```

#### CreateAgentSessionServicesOptions

Kind: interface

```ts
/**
 * Inputs for creating cwd-bound runtime services.
 *
 * These services are recreated whenever the effective session cwd changes.
 * CLI-provided resource paths should be resolved to absolute paths before they
 * reach this function, so later cwd switches do not reinterpret them.
 */
export interface CreateAgentSessionServicesOptions {
    cwd: string;
    agentDir?: string;
    authStorage?: AuthStorage;
    settingsManager?: SettingsManager;
    modelRegistry?: ModelRegistry;
    extensionFlagValues?: Map<string, boolean | string>;
    resourceLoaderOptions?: Omit<DefaultResourceLoaderOptions, "cwd" | "agentDir" | "settingsManager">;
}
```

### core/auth-storage

#### ApiKeyCredential

Kind: type

```ts
export type ApiKeyCredential = {
    type: "api_key";
    key: string;
};
```

#### AuthCredential

Kind: type

```ts
export type AuthCredential = ApiKeyCredential | OAuthCredential;
```

#### AuthStatus

Kind: type

```ts
export type AuthStatus = {
    configured: boolean;
    source?: "stored" | "runtime" | "environment" | "fallback" | "models_json_key" | "models_json_command";
    label?: string;
};
```

#### AuthStorage

Kind: class

```ts
/**
 * Credential storage backed by a JSON file.
 */
export declare class AuthStorage {
    private storage;
    private data;
    private runtimeOverrides;
    private fallbackResolver?;
    private loadError;
    private errors;
    private constructor();
    static create(authPath?: string): AuthStorage;
    static fromStorage(storage: AuthStorageBackend): AuthStorage;
    static inMemory(data?: AuthStorageData): AuthStorage;
    /**
     * Set a runtime API key override (not persisted to disk).
     * Used for CLI --api-key flag.
     */
    setRuntimeApiKey(provider: string, apiKey: string): void;
    /**
     * Remove a runtime API key override.
     */
    removeRuntimeApiKey(provider: string): void;
    /**
     * Set a fallback resolver for API keys not found in auth.json or env vars.
     * Used for custom provider keys from models.json.
     */
    setFallbackResolver(resolver: (provider: string) => string | undefined): void;
    private recordError;
    private parseStorageData;
    /**
     * Reload credentials from storage.
     */
    reload(): void;
    private persistProviderChange;
    /**
     * Get credential for a provider.
     */
    get(provider: string): AuthCredential | undefined;
    /**
     * Set credential for a provider.
     */
    set(provider: string, credential: AuthCredential): void;
    /**
     * Remove credential for a provider.
     */
    remove(provider: string): void;
    /**
     * List all providers with credentials.
     */
    list(): string[];
    /**
     * Check if credentials exist for a provider in auth.json.
     */
    has(provider: string): boolean;
    /**
     * Check if any form of auth is configured for a provider.
     * Unlike getApiKey(), this doesn't refresh OAuth tokens.
     */
    hasAuth(provider: string): boolean;
    /**
     * Return auth status without exposing credential values or refreshing tokens.
     */
    getAuthStatus(provider: string): AuthStatus;
    /**
     * Get all credentials (for passing to getOAuthApiKey).
     */
    getAll(): AuthStorageData;
    drainErrors(): Error[];
    /**
     * Login to an OAuth provider.
     */
    login(providerId: OAuthProviderId, callbacks: OAuthLoginCallbacks): Promise<void>;
    /**
     * Logout from a provider.
     */
    logout(provider: string): void;
    private refreshOAuthTokenWithLock;
    /**
     * Get API key for a provider.
     * Priority:
     * 1. Runtime override (CLI --api-key)
     * 2. API key from auth.json
     * 3. OAuth token from auth.json (auto-refreshed with locking)
     * 4. Environment variable
     * 5. Fallback resolver (models.json custom providers)
     */
    getApiKey(providerId: string, options?: {
        includeFallback?: boolean;
    }): Promise<string | undefined>;
    /**
     * Get all registered OAuth providers
     */
    getOAuthProviders(): import("@earendil-works/pi-ai").OAuthProviderInterface[];
}
```

#### AuthStorageBackend

Kind: interface

```ts
export interface AuthStorageBackend {
    withLock<T>(fn: (current: string | undefined) => LockResult<T>): T;
    withLockAsync<T>(fn: (current: string | undefined) => Promise<LockResult<T>>): Promise<T>;
}
```

#### FileAuthStorageBackend

Kind: class

```ts
export declare class FileAuthStorageBackend implements AuthStorageBackend {
    private authPath;
    constructor(authPath?: string);
    private ensureParentDir;
    private ensureFileExists;
    private acquireLockSyncWithRetry;
    withLock<T>(fn: (current: string | undefined) => LockResult<T>): T;
    withLockAsync<T>(fn: (current: string | undefined) => Promise<LockResult<T>>): Promise<T>;
}
```

#### InMemoryAuthStorageBackend

Kind: class

```ts
export declare class InMemoryAuthStorageBackend implements AuthStorageBackend {
    private value;
    withLock<T>(fn: (current: string | undefined) => LockResult<T>): T;
    withLockAsync<T>(fn: (current: string | undefined) => Promise<LockResult<T>>): Promise<T>;
}
```

#### OAuthCredential

Kind: type

```ts
export type OAuthCredential = {
    type: "oauth";
} & OAuthCredentials;
```

### core/compaction/branch-summarization

#### BranchPreparation

Kind: interface

```ts
export interface BranchPreparation {
    /** Messages extracted for summarization, in chronological order */
    messages: AgentMessage[];
    /** File operations extracted from tool calls */
    fileOps: FileOperations;
    /** Total estimated tokens in messages */
    totalTokens: number;
}
```

#### BranchSummaryResult

Kind: interface

```ts
export interface BranchSummaryResult {
    summary?: string;
    readFiles?: string[];
    modifiedFiles?: string[];
    aborted?: boolean;
    error?: string;
}
```

#### collectEntriesForBranchSummary

Kind: function

```ts
/**
 * Collect entries that should be summarized when navigating from one position to another.
 *
 * Walks from oldLeafId back to the common ancestor with targetId, collecting entries
 * along the way. Does NOT stop at compaction boundaries - those are included and their
 * summaries become context.
 *
 * @param session - Session manager (read-only access)
 * @param oldLeafId - Current position (where we're navigating from)
 * @param targetId - Target position (where we're navigating to)
 * @returns Entries to summarize and the common ancestor
 */
export declare function collectEntriesForBranchSummary(session: ReadonlySessionManager, oldLeafId: string | null, targetId: string): CollectEntriesResult;
```

#### CollectEntriesResult

Kind: interface

```ts
export interface CollectEntriesResult {
    /** Entries to summarize, in chronological order */
    entries: SessionEntry[];
    /** Common ancestor between old and new position, if any */
    commonAncestorId: string | null;
}
```

#### generateBranchSummary

Kind: function

```ts
/**
 * Generate a summary of abandoned branch entries.
 *
 * @param entries - Session entries to summarize (chronological order)
 * @param options - Generation options
 */
export declare function generateBranchSummary(entries: SessionEntry[], options: GenerateBranchSummaryOptions): Promise<BranchSummaryResult>;
```

#### GenerateBranchSummaryOptions

Kind: interface

```ts
export interface GenerateBranchSummaryOptions {
    /** Model to use for summarization */
    model: Model<any>;
    /** API key for the model */
    apiKey: string;
    /** Request headers for the model */
    headers?: Record<string, string>;
    /** Abort signal for cancellation */
    signal: AbortSignal;
    /** Optional custom instructions for summarization */
    customInstructions?: string;
    /** If true, customInstructions replaces the default prompt instead of being appended */
    replaceInstructions?: boolean;
    /** Tokens reserved for prompt + LLM response (default 16384) */
    reserveTokens?: number;
}
```

#### prepareBranchEntries

Kind: function

```ts
/**
 * Prepare entries for summarization with token budget.
 *
 * Walks entries from NEWEST to OLDEST, adding messages until we hit the token budget.
 * This ensures we keep the most recent context when the branch is too long.
 *
 * Also collects file operations from:
 * - Tool calls in assistant messages
 * - Existing branch_summary entries' details (for cumulative tracking)
 *
 * @param entries - Entries in chronological order
 * @param tokenBudget - Maximum tokens to include (0 = no limit)
 */
export declare function prepareBranchEntries(entries: SessionEntry[], tokenBudget?: number): BranchPreparation;
```

### core/compaction/compaction

#### calculateContextTokens

Kind: function

```ts
/**
 * Calculate total context tokens from usage.
 * Uses the native totalTokens field when available, falls back to computing from components.
 */
export declare function calculateContextTokens(usage: Usage): number;
```

#### compact

Kind: function

```ts
/**
 * Generate summaries for compaction using prepared data.
 * Returns CompactionResult - SessionManager adds uuid/parentUuid when saving.
 *
 * @param preparation - Pre-calculated preparation from prepareCompaction()
 * @param customInstructions - Optional custom focus for the summary
 */
export declare function compact(preparation: CompactionPreparation, model: Model<any>, apiKey: string, headers?: Record<string, string>, customInstructions?: string, signal?: AbortSignal, thinkingLevel?: ThinkingLevel): Promise<CompactionResult>;
```

#### CompactionResult

Kind: interface

```ts
/** Result from compact() - SessionManager adds uuid/parentUuid when saving */
export interface CompactionResult<T = unknown> {
    summary: string;
    firstKeptEntryId: string;
    tokensBefore: number;
    /** Extension-specific data (e.g., ArtifactIndex, version markers for structured compaction) */
    details?: T;
}
```

#### CutPointResult

Kind: interface

```ts
export interface CutPointResult {
    /** Index of first entry to keep */
    firstKeptEntryIndex: number;
    /** Index of user message that starts the turn being split, or -1 if not splitting */
    turnStartIndex: number;
    /** Whether this cut splits a turn (cut point is not a user message) */
    isSplitTurn: boolean;
}
```

#### DEFAULT_COMPACTION_SETTINGS

Kind: const

```ts
export declare const DEFAULT_COMPACTION_SETTINGS: CompactionSettings;
```

#### estimateTokens

Kind: function

```ts
/**
 * Estimate token count for a message using chars/4 heuristic.
 * This is conservative (overestimates tokens).
 */
export declare function estimateTokens(message: AgentMessage): number;
```

#### findCutPoint

Kind: function

```ts
/**
 * Find the cut point in session entries that keeps approximately `keepRecentTokens`.
 *
 * Algorithm: Walk backwards from newest, accumulating estimated message sizes.
 * Stop when we've accumulated >= keepRecentTokens. Cut at that point.
 *
 * Can cut at user OR assistant messages (never tool results). When cutting at an
 * assistant message with tool calls, its tool results come after and will be kept.
 *
 * Returns CutPointResult with:
 * - firstKeptEntryIndex: the entry index to start keeping from
 * - turnStartIndex: if cutting mid-turn, the user message that started that turn
 * - isSplitTurn: whether we're cutting in the middle of a turn
 *
 * Only considers entries between `startIndex` and `endIndex` (exclusive).
 */
export declare function findCutPoint(entries: SessionEntry[], startIndex: number, endIndex: number, keepRecentTokens: number): CutPointResult;
```

#### findTurnStartIndex

Kind: function

```ts
/**
 * Find the user message (or bashExecution) that starts the turn containing the given entry index.
 * Returns -1 if no turn start found before the index.
 * BashExecutionMessage is treated like a user message for turn boundaries.
 */
export declare function findTurnStartIndex(entries: SessionEntry[], entryIndex: number, startIndex: number): number;
```

#### generateSummary

Kind: function

```ts
/**
 * Generate a summary of the conversation using the LLM.
 * If previousSummary is provided, uses the update prompt to merge.
 */
export declare function generateSummary(currentMessages: AgentMessage[], model: Model<any>, reserveTokens: number, apiKey: string, headers?: Record<string, string>, signal?: AbortSignal, customInstructions?: string, previousSummary?: string, thinkingLevel?: ThinkingLevel): Promise<string>;
```

#### getLastAssistantUsage

Kind: function

```ts
/**
 * Find the last non-aborted assistant message usage from session entries.
 */
export declare function getLastAssistantUsage(entries: SessionEntry[]): Usage | undefined;
```

#### shouldCompact

Kind: function

```ts
/**
 * Check if compaction should trigger based on context usage.
 */
export declare function shouldCompact(contextTokens: number, contextWindow: number, settings: CompactionSettings): boolean;
```

### core/compaction/utils

#### FileOperations

Kind: interface

```ts
export interface FileOperations {
    read: Set<string>;
    written: Set<string>;
    edited: Set<string>;
}
```

#### serializeConversation

Kind: function

```ts
/**
 * Serialize LLM messages to text for summarization.
 * This prevents the model from treating it as a conversation to continue.
 * Call convertToLlm() first to handle custom message types.
 *
 * Tool results are truncated to keep the summarization request within
 * reasonable token budgets. Full content is not needed for summarization.
 */
export declare function serializeConversation(messages: Message[]): string;
```

### core/diagnostics

#### ResourceCollision

Kind: interface

```ts
export interface ResourceCollision {
    resourceType: "extension" | "skill" | "prompt" | "theme";
    name: string;
    winnerPath: string;
    loserPath: string;
    winnerSource?: string;
    loserSource?: string;
}
```

#### ResourceDiagnostic

Kind: interface

```ts
export interface ResourceDiagnostic {
    type: "warning" | "error" | "collision";
    message: string;
    path?: string;
    collision?: ResourceCollision;
}
```

### core/event-bus

#### createEventBus

Kind: function

```ts
export declare function createEventBus(): EventBusController;
```

#### EventBus

Kind: interface

```ts
export interface EventBus {
    emit(channel: string, data: unknown): void;
    on(channel: string, handler: (data: unknown) => void): () => void;
}
```

#### EventBusController

Kind: interface

```ts
export interface EventBusController extends EventBus {
    clear(): void;
}
```

### core/exec

#### ExecOptions

Kind: interface

```ts
/**
 * Shared command execution utilities for extensions and custom tools.
 */
/**
 * Options for executing shell commands.
 */
export interface ExecOptions {
    /** AbortSignal to cancel the command */
    signal?: AbortSignal;
    /** Timeout in milliseconds */
    timeout?: number;
    /** Working directory */
    cwd?: string;
}
```

#### ExecResult

Kind: interface

```ts
/**
 * Result of executing a shell command.
 */
export interface ExecResult {
    stdout: string;
    stderr: string;
    code: number;
    killed: boolean;
}
```

### core/extensions/loader

#### createExtensionRuntime

Kind: function

```ts
/**
 * Create a runtime with throwing stubs for action methods.
 * Runner.bindCore() replaces these with real implementations.
 */
export declare function createExtensionRuntime(): ExtensionRuntime;
```

#### discoverAndLoadExtensions

Kind: function

```ts
/**
 * Discover and load extensions from standard locations.
 */
export declare function discoverAndLoadExtensions(configuredPaths: string[], cwd: string, agentDir?: string, eventBus?: EventBus): Promise<LoadExtensionsResult>;
```

### core/extensions/runner

#### ExtensionRunner

Kind: class

```ts
export declare class ExtensionRunner {
    private extensions;
    private runtime;
    private uiContext;
    private cwd;
    private sessionManager;
    private modelRegistry;
    private errorListeners;
    private getModel;
    private isIdleFn;
    private getSignalFn;
    private waitForIdleFn;
    private abortFn;
    private hasPendingMessagesFn;
    private getContextUsageFn;
    private compactFn;
    private getSystemPromptFn;
    private dispatchUserInputFn;
    private newSessionHandler;
    private forkHandler;
    private navigateTreeHandler;
    private switchSessionHandler;
    private reloadHandler;
    private shutdownHandler;
    private shortcutDiagnostics;
    private commandDiagnostics;
    private staleMessage;
    constructor(extensions: Extension[], runtime: ExtensionRuntime, cwd: string, sessionManager: SessionManager, modelRegistry: ModelRegistry);
    bindCore(actions: ExtensionActions, contextActions: ExtensionContextActions, providerActions?: {
        registerProvider?: (name: string, config: ProviderConfig) => void;
        unregisterProvider?: (name: string) => void;
    }): void;
    bindCommandContext(actions?: ExtensionCommandContextActions): void;
    setUIContext(uiContext?: ExtensionUIContext): void;
    getUIContext(): ExtensionUIContext;
    hasUI(): boolean;
    getExtensionPaths(): string[];
    /** Get all registered tools from all extensions (first registration per name wins). */
    getAllRegisteredTools(): RegisteredTool[];
    /** Get a tool definition by name. Returns undefined if not found. */
    getToolDefinition(toolName: string): RegisteredTool["definition"] | undefined;
    getFlags(): Map<string, ExtensionFlag>;
    setFlagValue(name: string, value: boolean | string): void;
    getFlagValues(): Map<string, boolean | string>;
    getShortcuts(resolvedKeybindings: KeybindingsConfig): Map<KeyId, ExtensionShortcut>;
    getShortcutDiagnostics(): ResourceDiagnostic[];
    invalidate(message?: string): void;
    private assertActive;
    onError(listener: ExtensionErrorListener): () => void;
    emitError(error: ExtensionError): void;
    hasHandlers(eventType: string): boolean;
    getMessageRenderer(customType: string): MessageRenderer | undefined;
    private resolveRegisteredCommands;
    getRegisteredCommands(): ResolvedCommand[];
    getCommandDiagnostics(): ResourceDiagnostic[];
    getCommand(name: string): ResolvedCommand | undefined;
    /**
     * Request a graceful shutdown. Called by extension tools and event handlers.
     * The actual shutdown behavior is provided by the mode via bindExtensions().
     */
    shutdown(): void;
    /**
     * Create an ExtensionContext for use in event handlers and tool execution.
     * Context values are resolved at call time, so changes via bindCore/bindUI are reflected.
     */
    createContext(): ExtensionContext;
    createCommandContext(): ExtensionCommandContext;
    private isSessionBeforeEvent;
    emit<TEvent extends RunnerEmitEvent>(event: TEvent): Promise<RunnerEmitResult<TEvent>>;
    emitMessageEnd(event: MessageEndEvent): Promise<AgentMessage | undefined>;
    emitToolResult(event: ToolResultEvent): Promise<ToolResultEventResult | undefined>;
    emitToolCall(event: ToolCallEvent): Promise<ToolCallEventResult | undefined>;
    emitUserBash(event: UserBashEvent): Promise<UserBashEventResult | undefined>;
    emitContext(messages: AgentMessage[]): Promise<AgentMessage[]>;
    emitBeforeProviderRequest(payload: unknown): Promise<unknown>;
    emitBeforeAgentStart(prompt: string, images: ImageContent[] | undefined, systemPromptSections: SystemPromptSection[], systemPromptOptions: BuildSystemPromptOptions): Promise<BeforeAgentStartCombinedResult | undefined>;
    emitResourcesDiscover(cwd: string, reason: ResourcesDiscoverEvent["reason"]): Promise<{
        skillPaths: Array<{
            path: string;
            extensionPath: string;
        }>;
        promptPaths: Array<{
            path: string;
            extensionPath: string;
        }>;
        themePaths: Array<{
            path: string;
            extensionPath: string;
        }>;
    }>;
    /** Emit input event. Transforms chain, "handled" short-circuits. */
    emitInput(text: string, images: ImageContent[] | undefined, source: InputSource): Promise<InputEventResult>;
}
```

### core/extensions/types

#### AgentEndEvent

Kind: interface

```ts
/** Fired when an agent loop ends */
export interface AgentEndEvent {
    type: "agent_end";
    messages: AgentMessage[];
}
```

#### AgentStartEvent

Kind: interface

```ts
/** Fired when an agent loop starts */
export interface AgentStartEvent {
    type: "agent_start";
}
```

#### AutocompleteProviderFactory

Kind: type

```ts
/** Wrap the current autocomplete provider with additional behavior. */
export type AutocompleteProviderFactory = (current: AutocompleteProvider) => AutocompleteProvider;
```

#### BashToolCallEvent

Kind: interface

```ts
export interface BashToolCallEvent extends ToolCallEventBase {
    toolName: "bash";
    input: BashToolInput;
}
```

#### BeforeAgentStartEvent

Kind: interface

```ts
/** Fired after user submits prompt but before agent loop. */
export interface BeforeAgentStartEvent {
    type: "before_agent_start";
    /** The raw user prompt text (after expansion). */
    prompt: string;
    /** Images attached to the user prompt, if any. */
    images?: ImageContent[];
    /** The fully assembled system prompt string. */
    systemPrompt: string;
    /** Read-only view of the base system prompt sections for this turn, before extension contributions. The volatile environment tail is the section with `cacheRetention: "none"`. */
    systemPromptSections: readonly SystemPromptSection[];
    /** Structured options used to build the system prompt. Extensions can inspect this to understand what Pi loaded without re-discovering resources. */
    systemPromptOptions: BuildSystemPromptOptions;
}
```

#### BeforeAgentStartEventResult

Kind: interface

```ts
export interface BeforeAgentStartEventResult {
    message?: Pick<CustomMessage, "customType" | "content" | "display" | "details">;
    /** Replace the system prompt for this turn. If multiple extensions return this, they are chained. If any extension returns this, the final string wins for the whole prompt and all contributed `systemPromptSection`s are dropped for the turn. */
    systemPrompt?: string;
    /** Contribute a system prompt section for this turn. Sections accumulate in extension load order and are inserted before the volatile environment tail. Section texts are joined without separators, so `text` should start with its own separator (typically `\n\n`). */
    systemPromptSection?: SystemPromptSection;
}
```

#### BeforeProviderRequestEvent

Kind: interface

```ts
/** Fired before a provider request is sent. Can replace the payload. */
export interface BeforeProviderRequestEvent {
    type: "before_provider_request";
    payload: unknown;
}
```

#### BeforeProviderRequestEventResult

Kind: type

```ts
export type BeforeProviderRequestEventResult = unknown;
```

#### CompactOptions

Kind: interface

```ts
export interface CompactOptions {
    customInstructions?: string;
    onComplete?: (result: CompactionResult) => void;
    onError?: (error: Error) => void;
}
```

#### ContextEvent

Kind: interface

```ts
/** Fired before each LLM call. Can modify messages. */
export interface ContextEvent {
    type: "context";
    messages: AgentMessage[];
}
```

#### ContextUsage

Kind: interface

```ts
export interface ContextUsage {
    /** Estimated context tokens, or null if unknown (e.g. right after compaction, before next LLM response). */
    tokens: number | null;
    contextWindow: number;
    /** Context usage as percentage of context window, or null if tokens is unknown. */
    percent: number | null;
}
```

#### CustomToolCallEvent

Kind: interface

```ts
export interface CustomToolCallEvent extends ToolCallEventBase {
    toolName: string;
    input: Record<string, unknown>;
}
```

#### defineTool

Kind: function

```ts
/**
 * Preserve parameter inference for standalone tool definitions.
 *
 * Use this when assigning a tool to a variable or passing it through arrays such
 * as `customTools`, where contextual typing would otherwise widen params to
 * `unknown`.
 */
export declare function defineTool<TParams extends TSchema, TDetails = unknown, TState = any>(tool: ToolDefinition<TParams, TDetails, TState>): ToolDefinition<TParams, TDetails, TState> & AnyToolDefinition;
```

#### EditToolCallEvent

Kind: interface

```ts
export interface EditToolCallEvent extends ToolCallEventBase {
    toolName: "edit";
    input: EditToolInput;
}
```

#### Extension

Kind: interface

```ts
/** Loaded extension with all registered items. */
export interface Extension {
    path: string;
    resolvedPath: string;
    sourceInfo: SourceInfo;
    handlers: Map<string, HandlerFn[]>;
    tools: Map<string, RegisteredTool>;
    messageRenderers: Map<string, MessageRenderer>;
    commands: Map<string, RegisteredCommand>;
    flags: Map<string, ExtensionFlag>;
    shortcuts: Map<KeyId, ExtensionShortcut>;
}
```

#### ExtensionActions

Kind: interface

```ts
/**
 * Action implementations for pi.* API methods.
 * Provided to runner.initialize(), copied into the shared runtime.
 */
export interface ExtensionActions {
    sendMessage: SendMessageHandler;
    sendUserMessage: SendUserMessageHandler;
    appendEntry: AppendEntryHandler;
    setSessionName: SetSessionNameHandler;
    getSessionName: GetSessionNameHandler;
    setLabel: SetLabelHandler;
    getActiveTools: GetActiveToolsHandler;
    getAllTools: GetAllToolsHandler;
    setActiveTools: SetActiveToolsHandler;
    refreshTools: RefreshToolsHandler;
    getCommands: GetCommandsHandler;
    setModel: SetModelHandler;
    getThinkingLevel: GetThinkingLevelHandler;
    setThinkingLevel: SetThinkingLevelHandler;
}
```

#### ExtensionAPI

Kind: interface

```ts
/**
 * ExtensionAPI passed to extension factory functions.
 */
export interface ExtensionAPI {
    on(event: "resources_discover", handler: ExtensionHandler<ResourcesDiscoverEvent, ResourcesDiscoverResult>): void;
    on(event: "session_start", handler: ExtensionHandler<SessionStartEvent>): void;
    on(event: "session_before_switch", handler: ExtensionHandler<SessionBeforeSwitchEvent, SessionBeforeSwitchResult>): void;
    on(event: "session_before_fork", handler: ExtensionHandler<SessionBeforeForkEvent, SessionBeforeForkResult>): void;
    on(event: "session_before_compact", handler: ExtensionHandler<SessionBeforeCompactEvent, SessionBeforeCompactResult>): void;
    on(event: "session_compact", handler: ExtensionHandler<SessionCompactEvent>): void;
    on(event: "session_shutdown", handler: ExtensionHandler<SessionShutdownEvent>): void;
    on(event: "session_before_tree", handler: ExtensionHandler<SessionBeforeTreeEvent, SessionBeforeTreeResult>): void;
    on(event: "session_tree", handler: ExtensionHandler<SessionTreeEvent>): void;
    on(event: "context", handler: ExtensionHandler<ContextEvent, ContextEventResult>): void;
    on(event: "before_provider_request", handler: ExtensionHandler<BeforeProviderRequestEvent, BeforeProviderRequestEventResult>): void;
    on(event: "after_provider_response", handler: ExtensionHandler<AfterProviderResponseEvent>): void;
    on(event: "before_agent_start", handler: ExtensionHandler<BeforeAgentStartEvent, BeforeAgentStartEventResult>): void;
    on(event: "agent_start", handler: ExtensionHandler<AgentStartEvent>): void;
    on(event: "agent_end", handler: ExtensionHandler<AgentEndEvent>): void;
    on(event: "turn_start", handler: ExtensionHandler<TurnStartEvent>): void;
    on(event: "turn_end", handler: ExtensionHandler<TurnEndEvent>): void;
    on(event: "message_start", handler: ExtensionHandler<MessageStartEvent>): void;
    on(event: "message_update", handler: ExtensionHandler<MessageUpdateEvent>): void;
    on(event: "message_end", handler: ExtensionHandler<MessageEndEvent, MessageEndEventResult>): void;
    on(event: "tool_execution_start", handler: ExtensionHandler<ToolExecutionStartEvent>): void;
    on(event: "tool_execution_update", handler: ExtensionHandler<ToolExecutionUpdateEvent>): void;
    on(event: "tool_execution_end", handler: ExtensionHandler<ToolExecutionEndEvent>): void;
    on(event: "model_select", handler: ExtensionHandler<ModelSelectEvent>): void;
    on(event: "thinking_level_select", handler: ExtensionHandler<ThinkingLevelSelectEvent>): void;
    on(event: "tool_call", handler: ExtensionHandler<ToolCallEvent, ToolCallEventResult>): void;
    on(event: "tool_result", handler: ExtensionHandler<ToolResultEvent, ToolResultEventResult>): void;
    on(event: "user_bash", handler: ExtensionHandler<UserBashEvent, UserBashEventResult>): void;
    on(event: "input", handler: ExtensionHandler<InputEvent, InputEventResult>): void;
    /** Register a tool that the LLM can call. */
    registerTool<TParams extends TSchema = TSchema, TDetails = unknown, TState = any>(tool: ToolDefinition<TParams, TDetails, TState>): void;
    /** Register a custom command. */
    registerCommand(name: string, options: Omit<RegisteredCommand, "name" | "sourceInfo">): void;
    /** Register a keyboard shortcut. */
    registerShortcut(shortcut: KeyId, options: {
        description?: string;
        handler: (ctx: ExtensionContext) => Promise<void> | void;
    }): void;
    /** Register a CLI flag. */
    registerFlag(name: string, options: {
        description?: string;
        type: "boolean" | "string";
        default?: boolean | string;
    }): void;
    /** Get the value of a registered CLI flag. */
    getFlag(name: string): boolean | string | undefined;
    /** Register a custom renderer for CustomMessageEntry. */
    registerMessageRenderer<T = unknown>(customType: string, renderer: MessageRenderer<T>): void;
    /** Send a custom message to the session. */
    sendMessage<T = unknown>(message: Pick<CustomMessage<T>, "customType" | "content" | "display" | "details">, options?: {
        triggerTurn?: boolean;
        deliverAs?: SendMessageDeliverAs;
    }): void;
    /**
     * Send a user message to the agent. Always triggers a turn.
     * When the agent is streaming, use deliverAs to specify how to queue the message.
     */
    sendUserMessage(content: string | (TextContent | ImageContent)[], options?: {
        deliverAs?: DeliverAs;
    }): void;
    /** Append a custom entry to the session for state persistence (not sent to LLM). */
    appendEntry<T = unknown>(customType: string, data?: T): void;
    /** Set the session display name (shown in session selector). */
    setSessionName(name: string): void;
    /** Get the current session name, if set. */
    getSessionName(): string | undefined;
    /** Set or clear a label on an entry. Labels are user-defined markers for bookmarking/navigation. */
    setLabel(entryId: string, label: string | undefined): void;
    /** Execute a shell command. */
    exec(command: string, args: string[], options?: ExecOptions): Promise<ExecResult>;
    /** Get the list of currently active tool names. */
    getActiveTools(): string[];
    /** Get all configured tools with parameter schema and source metadata. */
    getAllTools(): ToolInfo[];
    /** Set the active tools by name. */
    setActiveTools(toolNames: string[]): void;
    /** Get available slash commands in the current session. */
    getCommands(): SlashCommandInfo[];
    /** Set the current model. Returns false if no API key available. */
    setModel(model: Model<any>): Promise<boolean>;
    /** Get current thinking level. */
    getThinkingLevel(): ThinkingLevel;
    /** Set thinking level (clamped to model capabilities). */
    setThinkingLevel(level: ThinkingLevel): void;
    /**
     * Register or override a model provider.
     *
     * If `models` is provided: replaces all existing models for this provider.
     * If only `baseUrl` is provided: overrides the URL for existing models.
     * If `oauth` is provided: registers OAuth provider for /login support.
     * If `streamSimple` is provided: registers a custom API stream handler.
     *
     * During initial extension load this call is queued and applied once the
     * runner has bound its context. After that it takes effect immediately, so
     * it is safe to call from command handlers or event callbacks without
     * requiring a `/reload`.
     *
     * @example
     * // Register a new provider with custom models
     * pi.registerProvider("my-proxy", {
     *   baseUrl: "https://proxy.example.com",
     *   apiKey: "PROXY_API_KEY",
     *   api: "anthropic-messages",
     *   models: [
     *     {
     *       id: "claude-sonnet-4-20250514",
     *       name: "Claude 4 Sonnet (proxy)",
     *       reasoning: false,
     *       input: ["text", "image"],
     *       cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
     *       contextWindow: 200000,
     *       maxTokens: 16384
     *     }
     *   ]
     * });
     *
     * @example
     * // Override baseUrl for an existing provider
     * pi.registerProvider("anthropic", {
     *   baseUrl: "https://proxy.example.com"
     * });
     *
     * @example
     * // Register provider with OAuth support
     * pi.registerProvider("corporate-ai", {
     *   baseUrl: "https://ai.corp.com",
     *   api: "openai-responses",
     *   models: [...],
     *   oauth: {
     *     name: "Corporate AI (SSO)",
     *     async login(callbacks) { ... },
     *     async refreshToken(credentials) { ... },
     *     getApiKey(credentials) { return credentials.access; }
     *   }
     * });
     */
    registerProvider(name: string, config: ProviderConfig): void;
    /**
     * Unregister a previously registered provider.
     *
     * Removes all models belonging to the named provider and restores any
     * built-in models that were overridden by it. Has no effect if the provider
     * is not currently registered.
     *
     * Like `registerProvider`, this takes effect immediately when called after
     * the initial load phase.
     *
     * @example
     * pi.unregisterProvider("my-proxy");
     */
    unregisterProvider(name: string): void;
    /** Shared event bus for extension communication. */
    events: EventBus;
}
```

#### ExtensionCommandContext

Kind: interface

```ts
/**
 * Extended context for command handlers.
 * Includes session control methods only safe in user-initiated commands.
 */
export interface ExtensionCommandContext extends ExtensionContext {
    /** Wait for the agent to finish streaming */
    waitForIdle(): Promise<void>;
    /** Fork from a specific entry, creating a new session file. */
    fork(entryId: string, options?: {
        position?: "before" | "at";
        withSession?: (ctx: ReplacedSessionContext) => Promise<void>;
    }): Promise<{
        cancelled: boolean;
    }>;
    /** Navigate to a different point in the session tree. */
    navigateTree(targetId: string, options?: {
        summarize?: boolean;
        customInstructions?: string;
        replaceInstructions?: boolean;
        label?: string;
    }): Promise<{
        cancelled: boolean;
    }>;
    /** Switch to a different session file. */
    switchSession(sessionPath: string, options?: {
        withSession?: (ctx: ReplacedSessionContext) => Promise<void>;
    }): Promise<{
        cancelled: boolean;
    }>;
    /** Reload extensions, skills, prompts, and themes. */
    reload(): Promise<void>;
}
```

#### ExtensionCommandContextActions

Kind: interface

```ts
/**
 * Actions for ExtensionCommandContext (ctx.* in command handlers).
 * Only needed for interactive mode where extension commands are invokable.
 */
export interface ExtensionCommandContextActions {
    waitForIdle: () => Promise<void>;
    newSession: (options?: {
        parentSession?: string;
        setup?: (sessionManager: SessionManager) => Promise<void>;
        withSession?: (ctx: ReplacedSessionContext) => Promise<void>;
    }) => Promise<{
        cancelled: boolean;
    }>;
    fork: (entryId: string, options?: {
        position?: "before" | "at";
        withSession?: (ctx: ReplacedSessionContext) => Promise<void>;
    }) => Promise<{
        cancelled: boolean;
    }>;
    navigateTree: (targetId: string, options?: {
        summarize?: boolean;
        customInstructions?: string;
        replaceInstructions?: boolean;
        label?: string;
    }) => Promise<{
        cancelled: boolean;
    }>;
    switchSession: (sessionPath: string, options?: {
        withSession?: (ctx: ReplacedSessionContext) => Promise<void>;
    }) => Promise<{
        cancelled: boolean;
    }>;
    reload: () => Promise<void>;
}
```

#### ExtensionContext

Kind: interface

```ts
/**
 * Context passed to extension event handlers.
 */
export interface ExtensionContext {
    /** UI methods for user interaction */
    ui: ExtensionUIContext;
    /** Whether UI is available (false in print/RPC mode) */
    hasUI: boolean;
    /** Current working directory */
    cwd: string;
    /** Session manager (read-only) */
    sessionManager: ReadonlySessionManager;
    /** Model registry for API key resolution */
    modelRegistry: ModelRegistry;
    /** Current model (may be undefined) */
    model: Model<any> | undefined;
    /** Whether the agent is idle (not streaming) */
    isIdle(): boolean;
    /** The current abort signal, or undefined when the agent is not streaming. */
    signal: AbortSignal | undefined;
    /** Abort the current agent operation */
    abort(): void;
    /** Whether there are queued messages waiting */
    hasPendingMessages(): boolean;
    /** Gracefully shutdown pi and exit. Available in all contexts. */
    shutdown(): void;
    /** Get current context usage for the active model. */
    getContextUsage(): ContextUsage | undefined;
    /** Trigger compaction without awaiting completion. */
    compact(options?: CompactOptions): void;
    /** Get the current effective system prompt. */
    getSystemPrompt(): string;
    /**
     * Dispatch input through the same pipeline as typed editor input:
     * extension-registered slash commands, prompt templates, and skills all
     * resolve as if the user had typed the text. Built-in interactive commands
     * (`/model`, `/login`, ...) are not part of this pipeline — dispatching one
     * sends the text to the LLM as a literal user message.
     */
    dispatchUserInput(input: string, options?: DispatchUserInputOptions): Promise<void>;
    /**
     * Start a new session, optionally with initialization. Available under the
     * built-in modes (interactive, print, RPC); in SDK embeddings that never
     * bind a command context, calling it rejects with an error.
     */
    newSession(options?: {
        parentSession?: string;
        setup?: (sessionManager: SessionManager) => Promise<void>;
        withSession?: (ctx: ReplacedSessionContext) => Promise<void>;
    }): Promise<{
        cancelled: boolean;
    }>;
}
```

#### ExtensionContextActions

Kind: interface

```ts
/**
 * Actions for ExtensionContext (ctx.* in event handlers).
 * Required by all modes.
 */
export interface ExtensionContextActions {
    getModel: () => Model<any> | undefined;
    isIdle: () => boolean;
    getSignal: () => AbortSignal | undefined;
    abort: () => void;
    hasPendingMessages: () => boolean;
    shutdown: () => void;
    getContextUsage: () => ContextUsage | undefined;
    compact: (options?: CompactOptions) => void;
    getSystemPrompt: () => string;
    dispatchUserInput: DispatchUserInputHandler;
}
```

#### ExtensionError

Kind: interface

```ts
export interface ExtensionError {
    extensionPath: string;
    event: string;
    error: string;
    stack?: string;
}
```

#### ExtensionEvent

Kind: type

```ts
/** Union of all event types */
export type ExtensionEvent = ResourcesDiscoverEvent | SessionEvent | ContextEvent | BeforeProviderRequestEvent | AfterProviderResponseEvent | BeforeAgentStartEvent | AgentStartEvent | AgentEndEvent | TurnStartEvent | TurnEndEvent | MessageStartEvent | MessageUpdateEvent | MessageEndEvent | ToolExecutionStartEvent | ToolExecutionUpdateEvent | ToolExecutionEndEvent | ModelSelectEvent | ThinkingLevelSelectEvent | UserBashEvent | InputEvent | ToolCallEvent | ToolResultEvent;
```

#### ExtensionFactory

Kind: type

```ts
/** Extension factory function type. Supports both sync and async initialization. */
export type ExtensionFactory = (pi: ExtensionAPI) => void | Promise<void>;
```

#### ExtensionFlag

Kind: interface

```ts
export interface ExtensionFlag {
    name: string;
    description?: string;
    type: "boolean" | "string";
    default?: boolean | string;
    extensionPath: string;
}
```

#### ExtensionHandler

Kind: type

```ts
/** Handler function type for events */
export type ExtensionHandler<E, R = undefined> = (event: E, ctx: ExtensionContext) => Promise<R | void> | R | void;
```

#### ExtensionRuntime

Kind: interface

```ts
/**
 * Full runtime = state + actions.
 * Created by loader with throwing action stubs, completed by runner.initialize().
 */
export interface ExtensionRuntime extends ExtensionRuntimeState, ExtensionActions {
}
```

#### ExtensionShortcut

Kind: interface

```ts
export interface ExtensionShortcut {
    shortcut: KeyId;
    description?: string;
    handler: (ctx: ExtensionContext) => Promise<void> | void;
    extensionPath: string;
}
```

#### ExtensionUIContext

Kind: interface

```ts
/**
 * UI context for extensions to request interactive UI.
 * Each mode (interactive, RPC, print) provides its own implementation.
 */
export interface ExtensionUIContext {
    /** Show a selector and return the user's choice. */
    select(title: string, options: string[], opts?: ExtensionUIDialogOptions): Promise<string | undefined>;
    /** Show a confirmation dialog. */
    confirm(title: string, message: string, opts?: ExtensionUIDialogOptions): Promise<boolean>;
    /** Show a text input dialog. */
    input(title: string, placeholder?: string, opts?: ExtensionUIDialogOptions): Promise<string | undefined>;
    /** Show a notification to the user. */
    notify(message: string, type?: "info" | "warning" | "error"): void;
    /** Listen to raw terminal input (interactive mode only). Returns an unsubscribe function. */
    onTerminalInput(handler: TerminalInputHandler): () => void;
    /** Set status text in the footer/status bar. Pass undefined to clear. */
    setStatus(key: string, text: string | undefined): void;
    /** Set the working/loading message shown during streaming. Call with no argument to restore default. */
    setWorkingMessage(message?: string): void;
    /** Show or hide the built-in interactive working loader row during streaming. */
    setWorkingVisible(visible: boolean): void;
    /**
     * Configure the interactive working indicator shown during streaming.
     *
     * - Omit the argument to restore the default animated spinner.
     * - Use `frames: ["●"]` for a static indicator.
     * - Use `frames: []` to hide the indicator entirely.
     * - Custom frames are rendered as provided, so extensions must add their own colors.
     */
    setWorkingIndicator(options?: WorkingIndicatorOptions): void;
    /** Set the label shown for hidden thinking blocks. Call with no argument to restore default. */
    setHiddenThinkingLabel(label?: string): void;
    /** Set a widget to display above or below the editor. Accepts string array or component factory. */
    setWidget(key: string, content: string[] | undefined, options?: ExtensionWidgetOptions): void;
    setWidget(key: string, content: ((tui: TUI, theme: Theme) => Component & {
        dispose?(): void;
    }) | undefined, options?: ExtensionWidgetOptions): void;
    /** Set a custom footer component, or undefined to restore the built-in footer.
     *
     * The factory receives a FooterDataProvider for data not otherwise accessible:
     * git branch and extension statuses from setStatus(). Token stats, model info,
     * etc. are available via ctx.sessionManager and ctx.model.
     */
    setFooter(factory: ((tui: TUI, theme: Theme, footerData: ReadonlyFooterDataProvider) => Component & {
        dispose?(): void;
    }) | undefined): void;
    /** Set a custom header component (shown at startup, above chat), or undefined to restore the built-in header. */
    setHeader(factory: ((tui: TUI, theme: Theme) => Component & {
        dispose?(): void;
    }) | undefined): void;
    /** Set the terminal window/tab title. */
    setTitle(title: string): void;
    /** Show a custom component with keyboard focus. */
    custom<T>(factory: (tui: TUI, theme: Theme, keybindings: KeybindingsManager, done: (result: T) => void) => (Component & {
        dispose?(): void;
    }) | Promise<Component & {
        dispose?(): void;
    }>, options?: {
        overlay?: boolean;
        /** Overlay positioning/sizing options. Can be static or a function for dynamic updates. */
        overlayOptions?: OverlayOptions | (() => OverlayOptions);
        /** Called with the overlay handle after the overlay is shown. Use to control visibility. */
        onHandle?: (handle: OverlayHandle) => void;
    }): Promise<T>;
    /** Paste text into the editor, triggering paste handling (collapse for large content). */
    pasteToEditor(text: string): void;
    /** Set the text in the core input editor. */
    setEditorText(text: string): void;
    /** Get the current text from the core input editor. */
    getEditorText(): string;
    /** Show a multi-line editor for text editing. */
    editor(title: string, prefill?: string): Promise<string | undefined>;
    /** Stack additional autocomplete behavior on top of the built-in provider. */
    addAutocompleteProvider(factory: AutocompleteProviderFactory): void;
    /**
     * Set a custom editor component via factory function.
     * Pass undefined to restore the default editor.
     *
     * The factory receives:
     * - `theme`: EditorTheme for styling borders and autocomplete
     * - `keybindings`: KeybindingsManager for app-level keybindings
     *
     * For full app keybinding support (escape, ctrl+d, model switching, etc.),
     * extend `CustomEditor` from `@leanandmean/pi-coding-agent` and call
     * `super.handleInput(data)` for keys you don't handle.
     *
     * @example
     * ```ts
     * import { CustomEditor } from "@leanandmean/pi-coding-agent";
     *
     * class VimEditor extends CustomEditor {
     *   private mode: "normal" | "insert" = "insert";
     *
     *   handleInput(data: string): void {
     *     if (this.mode === "normal") {
     *       // Handle vim normal mode keys...
     *       if (data === "i") { this.mode = "insert"; return; }
     *     }
     *     super.handleInput(data);  // App keybindings + text editing
     *   }
     * }
     *
     * ctx.ui.setEditorComponent((tui, theme, keybindings) =>
     *   new VimEditor(tui, theme, keybindings)
     * );
     * ```
     */
    setEditorComponent(factory: EditorFactory | undefined): void;
    /** Get the currently configured custom editor factory, or undefined when using the default editor. */
    getEditorComponent(): EditorFactory | undefined;
    /** Get the current theme for styling. */
    readonly theme: Theme;
    /** Get all available themes with their names and file paths. */
    getAllThemes(): {
        name: string;
        path: string | undefined;
    }[];
    /** Load a theme by name without switching to it. Returns undefined if not found. */
    getTheme(name: string): Theme | undefined;
    /** Set the current theme by name or Theme object. */
    setTheme(theme: string | Theme): {
        success: boolean;
        error?: string;
    };
    /** Get current tool output expansion state. */
    getToolsExpanded(): boolean;
    /** Set tool output expansion state. */
    setToolsExpanded(expanded: boolean): void;
}
```

#### ExtensionUIDialogOptions

Kind: interface

```ts
/** Options for extension UI dialogs. */
export interface ExtensionUIDialogOptions {
    /** AbortSignal to programmatically dismiss the dialog. */
    signal?: AbortSignal;
    /** Timeout in milliseconds. Dialog auto-dismisses with live countdown display. */
    timeout?: number;
}
```

#### ExtensionWidgetOptions

Kind: interface

```ts
/** Options for extension widgets. */
export interface ExtensionWidgetOptions {
    /** Where the widget is rendered. Defaults to "aboveEditor". */
    placement?: WidgetPlacement;
}
```

#### FindToolCallEvent

Kind: interface

```ts
export interface FindToolCallEvent extends ToolCallEventBase {
    toolName: "find";
    input: FindToolInput;
}
```

#### GrepToolCallEvent

Kind: interface

```ts
export interface GrepToolCallEvent extends ToolCallEventBase {
    toolName: "grep";
    input: GrepToolInput;
}
```

#### InputEvent

Kind: interface

```ts
/** Fired when user input is received, before agent processing */
export interface InputEvent {
    type: "input";
    /** The input text */
    text: string;
    /** Attached images, if any */
    images?: ImageContent[];
    /** Where the input came from */
    source: InputSource;
}
```

#### InputEventResult

Kind: type

```ts
/** Result from input event handler */
export type InputEventResult = {
    action: "continue";
} | {
    action: "transform";
    text: string;
    images?: ImageContent[];
} | {
    action: "handled";
};
```

#### InputSource

Kind: type

```ts
/** Source of user input */
export type InputSource = "interactive" | "rpc" | "extension";
```

#### isBashToolResult

Kind: function

```ts
export declare function isBashToolResult(e: ToolResultEvent): e is BashToolResultEvent;
```

#### isEditToolResult

Kind: function

```ts
export declare function isEditToolResult(e: ToolResultEvent): e is EditToolResultEvent;
```

#### isFindToolResult

Kind: function

```ts
export declare function isFindToolResult(e: ToolResultEvent): e is FindToolResultEvent;
```

#### isGrepToolResult

Kind: function

```ts
export declare function isGrepToolResult(e: ToolResultEvent): e is GrepToolResultEvent;
```

#### isLsToolResult

Kind: function

```ts
export declare function isLsToolResult(e: ToolResultEvent): e is LsToolResultEvent;
```

#### isReadToolResult

Kind: function

```ts
export declare function isReadToolResult(e: ToolResultEvent): e is ReadToolResultEvent;
```

#### isToolCallEventType

Kind: function

```ts
/**
 * Type guard for narrowing ToolCallEvent by tool name.
 *
 * Built-in tools narrow automatically (no type params needed):
 * ```ts
 * if (isToolCallEventType("bash", event)) {
 *   event.input.command;  // string
 * }
 * ```
 *
 * Custom tools require explicit type parameters:
 * ```ts
 * if (isToolCallEventType<"my_tool", MyToolInput>("my_tool", event)) {
 *   event.input.action;  // typed
 * }
 * ```
 *
 * Note: Direct narrowing via `event.toolName === "bash"` doesn't work because
 * CustomToolCallEvent.toolName is `string` which overlaps with all literals.
 */
export declare function isToolCallEventType(toolName: "bash", event: ToolCallEvent): event is BashToolCallEvent;

export declare function isToolCallEventType(toolName: "read", event: ToolCallEvent): event is ReadToolCallEvent;

export declare function isToolCallEventType(toolName: "edit", event: ToolCallEvent): event is EditToolCallEvent;

export declare function isToolCallEventType(toolName: "write", event: ToolCallEvent): event is WriteToolCallEvent;

export declare function isToolCallEventType(toolName: "grep", event: ToolCallEvent): event is GrepToolCallEvent;

export declare function isToolCallEventType(toolName: "find", event: ToolCallEvent): event is FindToolCallEvent;

export declare function isToolCallEventType(toolName: "ls", event: ToolCallEvent): event is LsToolCallEvent;

export declare function isToolCallEventType<TName extends string, TInput extends Record<string, unknown>>(toolName: TName, event: ToolCallEvent): event is ToolCallEvent & {
    toolName: TName;
    input: TInput;
};
```

#### isWriteToolResult

Kind: function

```ts
export declare function isWriteToolResult(e: ToolResultEvent): e is WriteToolResultEvent;
```

#### LoadExtensionsResult

Kind: interface

```ts
/** Result of loading extensions. */
export interface LoadExtensionsResult {
    extensions: Extension[];
    errors: Array<{
        path: string;
        error: string;
    }>;
    /** Shared runtime - actions are throwing stubs until runner.initialize() */
    runtime: ExtensionRuntime;
}
```

#### LsToolCallEvent

Kind: interface

```ts
export interface LsToolCallEvent extends ToolCallEventBase {
    toolName: "ls";
    input: LsToolInput;
}
```

#### MessageRenderer

Kind: type

```ts
export type MessageRenderer<T = unknown> = (message: CustomMessage<T>, options: MessageRenderOptions, theme: Theme) => Component | undefined;
```

#### MessageRenderOptions

Kind: interface

```ts
export interface MessageRenderOptions {
    expanded: boolean;
}
```

#### ProviderConfig

Kind: interface

```ts
/** Configuration for registering a provider via pi.registerProvider(). */
export interface ProviderConfig {
    /** Display name for the provider in UI. */
    name?: string;
    /** Base URL for the API endpoint. Required when defining models. */
    baseUrl?: string;
    /** API key or environment variable name. Required when defining models (unless oauth provided). */
    apiKey?: string;
    /** API type. Required at provider or model level when defining models. */
    api?: Api;
    /** Optional streamSimple handler for custom APIs. */
    streamSimple?: (model: Model<Api>, context: Context, options?: SimpleStreamOptions) => AssistantMessageEventStream;
    /**
     * Declares that `streamSimple` accepts `SystemPromptSection[]` in
     * `Context.systemPrompt`. When omitted, a sections array is flattened to
     * the equivalent single string before dispatch, so handlers written
     * against the legacy `string` contract keep working unchanged.
     */
    handlesSystemPromptSections?: boolean;
    /** Custom headers to include in requests. */
    headers?: Record<string, string>;
    /** If true, adds Authorization: Bearer header with the resolved API key. */
    authHeader?: boolean;
    /** Models to register. If provided, replaces all existing models for this provider. */
    models?: ProviderModelConfig[];
    /** OAuth provider for /login support. The `id` is set automatically from the provider name. */
    oauth?: {
        /** Display name for the provider in login UI. */
        name: string;
        /** Run the login flow, return credentials to persist. */
        login(callbacks: OAuthLoginCallbacks): Promise<OAuthCredentials>;
        /** Refresh expired credentials, return updated credentials to persist. */
        refreshToken(credentials: OAuthCredentials): Promise<OAuthCredentials>;
        /** Convert credentials to API key string for the provider. */
        getApiKey(credentials: OAuthCredentials): string;
        /** Optional: modify models for this provider (e.g., update baseUrl based on credentials). */
        modifyModels?(models: Model<Api>[], credentials: OAuthCredentials): Model<Api>[];
    };
}
```

#### ProviderModelConfig

Kind: interface

```ts
/** Configuration for a model within a provider. */
export interface ProviderModelConfig {
    /** Model ID (e.g., "claude-sonnet-4-20250514"). */
    id: string;
    /** Display name (e.g., "Claude 4 Sonnet"). */
    name: string;
    /** API type override for this model. */
    api?: Api;
    /** API endpoint URL override for this model. */
    baseUrl?: string;
    /** Whether the model supports extended thinking. */
    reasoning: boolean;
    /** Maps pi thinking levels to provider/model-specific values; null marks a level unsupported. */
    thinkingLevelMap?: Model<Api>["thinkingLevelMap"];
    /** Supported input types. */
    input: ("text" | "image")[];
    /** Cost per token (for tracking, can be 0). */
    cost: {
        input: number;
        output: number;
        cacheRead: number;
        cacheWrite: number;
    };
    /** Maximum context window size in tokens. */
    contextWindow: number;
    /** Maximum output tokens. */
    maxTokens: number;
    /** Custom headers for this model. */
    headers?: Record<string, string>;
    /** OpenAI compatibility settings. */
    compat?: Model<Api>["compat"];
}
```

#### ReadToolCallEvent

Kind: interface

```ts
export interface ReadToolCallEvent extends ToolCallEventBase {
    toolName: "read";
    input: ReadToolInput;
}
```

#### RegisteredCommand

Kind: interface

```ts
export interface RegisteredCommand {
    name: string;
    sourceInfo: SourceInfo;
    description?: string;
    getArgumentCompletions?: (argumentPrefix: string) => AutocompleteItem[] | null | Promise<AutocompleteItem[] | null>;
    handler: (args: string, ctx: ExtensionCommandContext) => Promise<void>;
}
```

#### RegisteredTool

Kind: interface

```ts
export interface RegisteredTool {
    definition: ToolDefinition;
    sourceInfo: SourceInfo;
}
```

#### ResolvedCommand

Kind: interface

```ts
export interface ResolvedCommand extends RegisteredCommand {
    invocationName: string;
}
```

#### SessionBeforeCompactEvent

Kind: interface

```ts
/** Fired before context compaction (can be cancelled or customized) */
export interface SessionBeforeCompactEvent {
    type: "session_before_compact";
    preparation: CompactionPreparation;
    branchEntries: SessionEntry[];
    customInstructions?: string;
    signal: AbortSignal;
}
```

#### SessionBeforeForkEvent

Kind: interface

```ts
/** Fired before forking a session (can be cancelled) */
export interface SessionBeforeForkEvent {
    type: "session_before_fork";
    entryId: string;
    position: "before" | "at";
}
```

#### SessionBeforeSwitchEvent

Kind: interface

```ts
/** Fired before switching to another session (can be cancelled) */
export interface SessionBeforeSwitchEvent {
    type: "session_before_switch";
    reason: "new" | "resume";
    targetSessionFile?: string;
}
```

#### SessionBeforeTreeEvent

Kind: interface

```ts
/** Fired before navigating in the session tree (can be cancelled) */
export interface SessionBeforeTreeEvent {
    type: "session_before_tree";
    preparation: TreePreparation;
    signal: AbortSignal;
}
```

#### SessionCompactEvent

Kind: interface

```ts
/** Fired after context compaction */
export interface SessionCompactEvent {
    type: "session_compact";
    compactionEntry: CompactionEntry;
    fromExtension: boolean;
}
```

#### SessionShutdownEvent

Kind: interface

```ts
/** Fired before an extension runtime is torn down due to quit, reload, or session replacement. */
export interface SessionShutdownEvent {
    type: "session_shutdown";
    reason: "quit" | "reload" | "new" | "resume" | "fork";
    /** Destination session file when shutting down due to session replacement. */
    targetSessionFile?: string;
}
```

#### SessionStartEvent

Kind: interface

```ts
/** Fired when a session is started, loaded, or reloaded */
export interface SessionStartEvent {
    type: "session_start";
    /** Why this session start happened. */
    reason: "startup" | "reload" | "new" | "resume" | "fork";
    /** Previously active session file. Present for "new", "resume", and "fork". */
    previousSessionFile?: string;
}
```

#### SessionTreeEvent

Kind: interface

```ts
/** Fired after navigating in the session tree */
export interface SessionTreeEvent {
    type: "session_tree";
    newLeafId: string | null;
    oldLeafId: string | null;
    summaryEntry?: BranchSummaryEntry;
    fromExtension?: boolean;
}
```

#### TerminalInputHandler

Kind: type

```ts
/** Raw terminal input listener for extensions. */
export type TerminalInputHandler = (data: string) => {
    consume?: boolean;
    data?: string;
} | undefined;
```

#### ToolCallEvent

Kind: type

```ts
/**
 * Fired before a tool executes. Can block.
 *
 * `event.input` is mutable. Mutate it in place to patch tool arguments before execution.
 * Later `tool_call` handlers see earlier mutations. No re-validation is performed after mutation.
 */
export type ToolCallEvent = BashToolCallEvent | ReadToolCallEvent | EditToolCallEvent | WriteToolCallEvent | GrepToolCallEvent | FindToolCallEvent | LsToolCallEvent | CustomToolCallEvent;
```

#### ToolCallEventResult

Kind: interface

```ts
export interface ToolCallEventResult {
    /** Block tool execution. To modify arguments, mutate `event.input` in place instead. */
    block?: boolean;
    reason?: string;
}
```

#### ToolDefinition

Kind: interface

```ts
/**
 * Tool definition for registerTool().
 */
export interface ToolDefinition<TParams extends TSchema = TSchema, TDetails = unknown, TState = any> {
    /** Tool name (used in LLM tool calls) */
    name: string;
    /** Human-readable label for UI */
    label: string;
    /** Description for LLM */
    description: string;
    /** Optional one-line snippet for the Available tools section in the default system prompt. Custom tools are omitted from that section when this is not provided. */
    promptSnippet?: string;
    /** Optional guideline bullets appended to the default system prompt Guidelines section when this tool is active. */
    promptGuidelines?: string[];
    /** Parameter schema (TypeBox) */
    parameters: TParams;
    /** Controls whether ToolExecutionComponent renders the standard colored shell or the tool renders its own framing. */
    renderShell?: "default" | "self";
    /** Optional compatibility shim to prepare raw tool call arguments before schema validation. Must return an object conforming to TParams. */
    prepareArguments?: (args: unknown) => Static<TParams>;
    /**
     * Per-tool execution mode override.
     * - "sequential": this tool must execute one at a time with other tool calls.
     * - "parallel": this tool can execute concurrently with other tool calls.
     *
     * If omitted, the default execution mode applies.
     */
    executionMode?: ToolExecutionMode;
    /** Execute the tool. */
    execute(toolCallId: string, params: Static<TParams>, signal: AbortSignal | undefined, onUpdate: AgentToolUpdateCallback<TDetails> | undefined, ctx: ExtensionContext): Promise<AgentToolResult<TDetails>>;
    /** Custom rendering for tool call display */
    renderCall?: (args: Static<TParams>, theme: Theme, context: ToolRenderContext<TState, Static<TParams>>) => Component;
    /** Custom rendering for tool result display */
    renderResult?: (result: AgentToolResult<TDetails>, options: ToolRenderResultOptions, theme: Theme, context: ToolRenderContext<TState, Static<TParams>>) => Component;
}
```

#### ToolInfo

Kind: type

```ts
/** Tool info with name, description, parameter schema, and source metadata */
export type ToolInfo = Pick<ToolDefinition, "name" | "description" | "parameters"> & {
    sourceInfo: SourceInfo;
};
```

#### ToolRenderResultOptions

Kind: interface

```ts
/** Rendering options for tool results */
export interface ToolRenderResultOptions {
    /** Whether the result view is expanded */
    expanded: boolean;
    /** Whether this is a partial/streaming result */
    isPartial: boolean;
}
```

#### ToolResultEvent

Kind: type

```ts
/** Fired after a tool executes. Can modify result. */
export type ToolResultEvent = BashToolResultEvent | ReadToolResultEvent | EditToolResultEvent | WriteToolResultEvent | GrepToolResultEvent | FindToolResultEvent | LsToolResultEvent | CustomToolResultEvent;
```

#### TurnEndEvent

Kind: interface

```ts
/** Fired at the end of each turn */
export interface TurnEndEvent {
    type: "turn_end";
    turnIndex: number;
    message: AgentMessage;
    toolResults: ToolResultMessage[];
}
```

#### TurnStartEvent

Kind: interface

```ts
/** Fired at the start of each turn */
export interface TurnStartEvent {
    type: "turn_start";
    turnIndex: number;
    timestamp: number;
}
```

#### UserBashEvent

Kind: interface

```ts
/** Fired when user executes a bash command via ! or !! prefix */
export interface UserBashEvent {
    type: "user_bash";
    /** The command to execute */
    command: string;
    /** True if !! prefix was used (excluded from LLM context) */
    excludeFromContext: boolean;
    /** Current working directory */
    cwd: string;
}
```

#### UserBashEventResult

Kind: interface

```ts
/** Result from user_bash event handler */
export interface UserBashEventResult {
    /** Custom operations to use for execution */
    operations?: BashOperations;
    /** Full replacement: extension handled execution, use this result */
    result?: BashResult;
}
```

#### WidgetPlacement

Kind: type

```ts
/** Placement for extension widgets. */
export type WidgetPlacement = "aboveEditor" | "belowEditor";
```

#### WorkingIndicatorOptions

Kind: interface

```ts
/** Working indicator configuration for the interactive streaming loader. */
export interface WorkingIndicatorOptions {
    /** Animation frames. Use an empty array to hide the indicator entirely. Custom frames are rendered verbatim. */
    frames?: string[];
    /** Frame interval in milliseconds for animated indicators. */
    intervalMs?: number;
}
```

#### WriteToolCallEvent

Kind: interface

```ts
export interface WriteToolCallEvent extends ToolCallEventBase {
    toolName: "write";
    input: WriteToolInput;
}
```

### core/extensions/wrapper

#### wrapRegisteredTool

Kind: function

```ts
/**
 * Wrap a RegisteredTool into an AgentTool.
 * Uses the runner's createContext() for consistent context across tools and event handlers.
 */
export declare function wrapRegisteredTool(registeredTool: RegisteredTool, runner: ExtensionRunner): AgentTool;
```

#### wrapRegisteredTools

Kind: function

```ts
/**
 * Wrap all registered tools into AgentTools.
 * Uses the runner's createContext() for consistent context across tools and event handlers.
 */
export declare function wrapRegisteredTools(registeredTools: RegisteredTool[], runner: ExtensionRunner): AgentTool[];
```

### core/footer-data-provider

#### ReadonlyFooterDataProvider

Kind: type

```ts
/** Read-only view for extensions - excludes setExtensionStatus, setAvailableProviderCount and dispose */
export type ReadonlyFooterDataProvider = Pick<FooterDataProvider, "getGitBranch" | "getExtensionStatuses" | "getAvailableProviderCount" | "onBranchChange">;
```

### core/keybindings

#### AppKeybinding

Kind: type

```ts
export type AppKeybinding = keyof AppKeybindings;
```

#### KeybindingsManager

Kind: class

```ts
export declare class KeybindingsManager extends TuiKeybindingsManager {
    private configPath;
    constructor(userBindings?: KeybindingsConfig, configPath?: string);
    static create(agentDir?: string): KeybindingsManager;
    reload(): void;
    getEffectiveConfig(): KeybindingsConfig;
    private static loadFromFile;
}
```

### core/messages

#### convertToLlm

Kind: function

```ts
/**
 * Transform AgentMessages (including custom types) to LLM-compatible Messages.
 *
 * This is used by:
 * - Agent's transormToLlm option (for prompt calls and queued messages)
 * - Compaction's generateSummary (for summarization)
 * - Custom extensions and tools
 */
export declare function convertToLlm(messages: AgentMessage[]): Message[];
```

### core/model-registry

#### ModelRegistry

Kind: class

```ts
/**
 * Model registry - loads and manages models, resolves API keys via AuthStorage.
 */
export declare class ModelRegistry {
    readonly authStorage: AuthStorage;
    private modelsJsonPath;
    private models;
    private providerRequestConfigs;
    private modelRequestHeaders;
    private registeredProviders;
    private loadError;
    private constructor();
    static create(authStorage: AuthStorage, modelsJsonPath?: string): ModelRegistry;
    static inMemory(authStorage: AuthStorage): ModelRegistry;
    /**
     * Reload models from disk (built-in + custom from models.json).
     */
    refresh(): void;
    /**
     * Get any error from loading models.json (undefined if no error).
     */
    getError(): string | undefined;
    private loadModels;
    /** Load built-in models and apply provider/model overrides */
    private loadBuiltInModels;
    /** Merge custom models into built-in list by provider+id (custom wins on conflicts). */
    private mergeCustomModels;
    private loadCustomModels;
    private validateConfig;
    private parseModels;
    /**
     * Get all models (built-in + custom).
     * If models.json had errors, returns only built-in models.
     */
    getAll(): Model<Api>[];
    /**
     * Get only models that have auth configured.
     * This is a fast check that doesn't refresh OAuth tokens.
     */
    getAvailable(): Model<Api>[];
    /**
     * Find a model by provider and ID.
     */
    find(provider: string, modelId: string): Model<Api> | undefined;
    /**
     * Get API key for a model.
     */
    hasConfiguredAuth(model: Model<Api>): boolean;
    private getModelRequestKey;
    private storeProviderRequestConfig;
    private storeModelHeaders;
    /**
     * Get API key and request headers for a model.
     */
    getApiKeyAndHeaders(model: Model<Api>): Promise<ResolvedRequestAuth>;
    /**
     * Return auth status for a provider, including request auth configured in models.json.
     * This intentionally does not execute command-backed config values.
     */
    getProviderAuthStatus(provider: string): AuthStatus;
    /**
     * Get display name for a provider.
     */
    getProviderDisplayName(provider: string): string;
    /**
     * Get API key for a provider.
     */
    getApiKeyForProvider(provider: string): Promise<string | undefined>;
    /**
     * Check if a model is using OAuth credentials (subscription).
     */
    isUsingOAuth(model: Model<Api>): boolean;
    /**
     * Register a provider dynamically (from extensions).
     *
     * If provider has models: replaces all existing models for this provider.
     * If provider has only baseUrl/headers: overrides existing models' URLs.
     * If provider has oauth: registers OAuth provider for /login support.
     */
    registerProvider(providerName: string, config: ProviderConfigInput): void;
    /**
     * Unregister a previously registered provider.
     *
     * Removes the provider from the registry and reloads models from disk so that
     * built-in models overridden by this provider are restored to their original state.
     * Also resets dynamic OAuth and API stream registrations before reapplying
     * remaining dynamic providers.
     * Has no effect if the provider was never registered.
     */
    unregisterProvider(providerName: string): void;
    /**
     * Upsert a provider config into registeredProviders.
     * If the provider is already registered, defined values in the incoming config
     * override existing ones; undefined values are preserved from the stored config.
     * If the provider is not registered, the incoming config is stored as-is.
     */
    private upsertRegisteredProvider;
    private validateProviderConfig;
    private applyProviderConfig;
}
```

### core/package-manager

#### DefaultPackageManager

Kind: class

```ts
export declare class DefaultPackageManager implements PackageManager {
    private cwd;
    private agentDir;
    private settingsManager;
    private globalNpmRoot;
    private globalNpmRootCommandKey;
    private progressCallback;
    constructor(options: PackageManagerOptions);
    setProgressCallback(callback: ProgressCallback | undefined): void;
    addSourceToSettings(source: string, options?: {
        local?: boolean;
    }): boolean;
    removeSourceFromSettings(source: string, options?: {
        local?: boolean;
    }): boolean;
    getInstalledPath(source: string, scope: "user" | "project"): string | undefined;
    private emitProgress;
    private withProgress;
    resolve(onMissing?: (source: string) => Promise<MissingSourceAction>): Promise<ResolvedPaths>;
    resolveExtensionSources(sources: string[], options?: {
        local?: boolean;
        temporary?: boolean;
    }): Promise<ResolvedPaths>;
    listConfiguredPackages(): ConfiguredPackage[];
    install(source: string, options?: {
        local?: boolean;
    }): Promise<void>;
    installAndPersist(source: string, options?: {
        local?: boolean;
    }): Promise<void>;
    remove(source: string, options?: {
        local?: boolean;
    }): Promise<void>;
    removeAndPersist(source: string, options?: {
        local?: boolean;
    }): Promise<boolean>;
    update(source?: string): Promise<void>;
    private updateConfiguredSources;
    private shouldUpdateNpmSource;
    private updateNpmBatch;
    private installNpmBatch;
    checkForAvailableUpdates(): Promise<PackageUpdate[]>;
    private resolvePackageSources;
    private resolveLocalExtensionSource;
    private installParsedSource;
    private getPackageSourceString;
    private getSourceMatchKeyForInput;
    private getSourceMatchKeyForSettings;
    private buildNoMatchingPackageMessage;
    private findSuggestedConfiguredSource;
    private packageSourcesMatch;
    private normalizePackageSourceForSettings;
    private parseSource;
    private installedNpmMatchesPinnedVersion;
    private npmHasAvailableUpdate;
    private getInstalledNpmVersion;
    private getLatestNpmVersion;
    private gitHasAvailableUpdate;
    private getRemoteGitHead;
    private getLocalGitUpdateTarget;
    private getGitUpstreamRef;
    private runGitRemoteCommand;
    private runWithConcurrency;
    /**
     * Get a unique identity for a package, ignoring version/ref.
     * Used to detect when the same package is in both global and project settings.
     * For git packages, uses normalized host/path to ensure SSH and HTTPS URLs
     * for the same repository are treated as identical.
     */
    private getPackageIdentity;
    /**
     * Dedupe packages: if same package identity appears in both global and project,
     * keep only the project one (project wins).
     */
    private dedupePackages;
    private parseNpmSpec;
    private getNpmCommand;
    private runNpmCommand;
    private getGitDependencyInstallArgs;
    private runNpmCommandSync;
    private installNpm;
    private uninstallNpm;
    private installGit;
    private updateGit;
    private refreshTemporaryGitSource;
    private removeGit;
    private pruneEmptyGitParents;
    private ensureNpmProject;
    private ensureGitIgnore;
    private getNpmInstallRoot;
    private getGlobalNpmRoot;
    private getNpmInstallPath;
    private getGitInstallPath;
    private getGitInstallRoot;
    private getTemporaryDir;
    private getBaseDirForScope;
    private resolvePath;
    private resolvePathFromBase;
    private collectPackageResources;
    private collectDefaultResources;
    private applyPackageFilter;
    /**
     * Collect all files from a package for a resource type, applying manifest patterns.
     * Returns { allFiles, enabledByManifest } where enabledByManifest is the set of files
     * that pass the manifest's own patterns.
     */
    private collectManifestFiles;
    private readPiManifest;
    private addManifestEntries;
    private collectFilesFromManifestEntries;
    private resolveLocalEntries;
    private addAutoDiscoveredResources;
    private collectFilesFromPaths;
    private getTargetMap;
    private addResource;
    private createAccumulator;
    private toResolvedPaths;
    private spawnCommand;
    private spawnCaptureCommand;
    private runCommandCapture;
    private runCommand;
    private runCommandSync;
}
```

#### PackageManager

Kind: interface

```ts
export interface PackageManager {
    resolve(onMissing?: (source: string) => Promise<MissingSourceAction>): Promise<ResolvedPaths>;
    install(source: string, options?: {
        local?: boolean;
    }): Promise<void>;
    installAndPersist(source: string, options?: {
        local?: boolean;
    }): Promise<void>;
    remove(source: string, options?: {
        local?: boolean;
    }): Promise<void>;
    removeAndPersist(source: string, options?: {
        local?: boolean;
    }): Promise<boolean>;
    update(source?: string): Promise<void>;
    listConfiguredPackages(): ConfiguredPackage[];
    resolveExtensionSources(sources: string[], options?: {
        local?: boolean;
        temporary?: boolean;
    }): Promise<ResolvedPaths>;
    addSourceToSettings(source: string, options?: {
        local?: boolean;
    }): boolean;
    removeSourceFromSettings(source: string, options?: {
        local?: boolean;
    }): boolean;
    setProgressCallback(callback: ProgressCallback | undefined): void;
    getInstalledPath(source: string, scope: "user" | "project"): string | undefined;
}
```

#### PathMetadata

Kind: interface

```ts
export interface PathMetadata {
    source: string;
    scope: SourceScope;
    origin: "package" | "top-level";
    baseDir?: string;
}
```

#### ProgressCallback

Kind: type

```ts
export type ProgressCallback = (event: ProgressEvent) => void;
```

#### ProgressEvent

Kind: interface

```ts
export interface ProgressEvent {
    type: "start" | "progress" | "complete" | "error";
    action: "install" | "remove" | "update" | "clone" | "pull";
    source: string;
    message?: string;
}
```

#### ResolvedPaths

Kind: interface

```ts
export interface ResolvedPaths {
    extensions: ResolvedResource[];
    skills: ResolvedResource[];
    prompts: ResolvedResource[];
    themes: ResolvedResource[];
}
```

#### ResolvedResource

Kind: interface

```ts
export interface ResolvedResource {
    path: string;
    enabled: boolean;
    metadata: PathMetadata;
}
```

### core/prompt-templates

#### PromptTemplate

Kind: interface

```ts
/**
 * Represents a prompt template loaded from a markdown file
 */
export interface PromptTemplate {
    name: string;
    description: string;
    argumentHint?: string;
    content: string;
    sourceInfo: SourceInfo;
    filePath: string;
}
```

### core/resource-loader

#### DefaultResourceLoader

Kind: class

```ts
export declare class DefaultResourceLoader implements ResourceLoader {
    private cwd;
    private agentDir;
    private settingsManager;
    private eventBus;
    private packageManager;
    private additionalExtensionPaths;
    private additionalSkillPaths;
    private additionalPromptTemplatePaths;
    private additionalThemePaths;
    private extensionFactories;
    private noExtensions;
    private noSkills;
    private noPromptTemplates;
    private noThemes;
    private noContextFiles;
    private systemPromptSource?;
    private appendSystemPromptSource?;
    private extensionsOverride?;
    private skillsOverride?;
    private promptsOverride?;
    private themesOverride?;
    private agentsFilesOverride?;
    private systemPromptOverride?;
    private appendSystemPromptOverride?;
    private extensionsResult;
    private skills;
    private skillDiagnostics;
    private prompts;
    private promptDiagnostics;
    private themes;
    private themeDiagnostics;
    private agentsFiles;
    private systemPrompt?;
    private appendSystemPrompt;
    private lastSkillPaths;
    private extensionSkillSourceInfos;
    private extensionPromptSourceInfos;
    private extensionThemeSourceInfos;
    private lastPromptPaths;
    private lastThemePaths;
    constructor(options: DefaultResourceLoaderOptions);
    getExtensions(): LoadExtensionsResult;
    getSkills(): {
        skills: Skill[];
        diagnostics: ResourceDiagnostic[];
    };
    getPrompts(): {
        prompts: PromptTemplate[];
        diagnostics: ResourceDiagnostic[];
    };
    getThemes(): {
        themes: Theme[];
        diagnostics: ResourceDiagnostic[];
    };
    getAgentsFiles(): {
        agentsFiles: Array<{
            path: string;
            content: string;
        }>;
    };
    getSystemPrompt(): string | undefined;
    getAppendSystemPrompt(): string[];
    extendResources(paths: ResourceExtensionPaths): void;
    reload(): Promise<void>;
    private normalizeExtensionPaths;
    private updateSkillsFromPaths;
    private updatePromptsFromPaths;
    private updateThemesFromPaths;
    private applyExtensionSourceInfo;
    private findSourceInfoForPath;
    private getDefaultSourceInfoForPath;
    private mergePaths;
    private resolveResourcePath;
    private loadThemes;
    private loadThemesFromDir;
    private loadThemeFromFile;
    private loadExtensionFactories;
    private dedupePrompts;
    private dedupeThemes;
    private discoverSystemPromptFile;
    private discoverAppendSystemPromptFile;
    private isUnderPath;
    private detectExtensionConflicts;
}
```

#### loadProjectContextFiles

Kind: function

```ts
export declare function loadProjectContextFiles(options: {
    cwd: string;
    agentDir: string;
}): Array<{
    path: string;
    content: string;
}>;
```

#### ResourceLoader

Kind: interface

```ts
export interface ResourceLoader {
    getExtensions(): LoadExtensionsResult;
    getSkills(): {
        skills: Skill[];
        diagnostics: ResourceDiagnostic[];
    };
    getPrompts(): {
        prompts: PromptTemplate[];
        diagnostics: ResourceDiagnostic[];
    };
    getThemes(): {
        themes: Theme[];
        diagnostics: ResourceDiagnostic[];
    };
    getAgentsFiles(): {
        agentsFiles: Array<{
            path: string;
            content: string;
        }>;
    };
    getSystemPrompt(): string | undefined;
    getAppendSystemPrompt(): string[];
    extendResources(paths: ResourceExtensionPaths): void;
    reload(): Promise<void>;
}
```

### core/sdk

#### createAgentSession

Kind: function

```ts
/**
 * Create an AgentSession with the specified options.
 *
 * @example
 * ```typescript
 * // Minimal - uses defaults
 * const { session } = await createAgentSession();
 *
 * // With explicit model
 * import { getModel } from '@earendil-works/pi-ai';
 * const { session } = await createAgentSession({
 *   model: getModel('anthropic', 'claude-opus-4-5'),
 *   thinkingLevel: 'high',
 * });
 *
 * // Continue previous session
 * const { session, modelFallbackMessage } = await createAgentSession({
 *   continueSession: true,
 * });
 *
 * // Full control
 * const loader = new DefaultResourceLoader({
 *   cwd: process.cwd(),
 *   agentDir: getAgentDir(),
 *   settingsManager: SettingsManager.create(),
 * });
 * await loader.reload();
 * const { session } = await createAgentSession({
 *   model: myModel,
 *   tools: [readTool, bashTool],
 *   resourceLoader: loader,
 *   sessionManager: SessionManager.inMemory(),
 * });
 * ```
 */
export declare function createAgentSession(options?: CreateAgentSessionOptions): Promise<CreateAgentSessionResult>;
```

#### CreateAgentSessionOptions

Kind: interface

```ts
export interface CreateAgentSessionOptions {
    /** Working directory for project-local discovery. Default: process.cwd() */
    cwd?: string;
    /** Global config directory. Default: ~/.pi/agent */
    agentDir?: string;
    /** Auth storage for credentials. Default: AuthStorage.create(agentDir/auth.json) */
    authStorage?: AuthStorage;
    /** Model registry. Default: ModelRegistry.create(authStorage, agentDir/models.json) */
    modelRegistry?: ModelRegistry;
    /** Model to use. Default: from settings, else first available */
    model?: Model<any>;
    /** Thinking level. Default: from settings, else 'medium' (clamped to model capabilities) */
    thinkingLevel?: ThinkingLevel;
    /** Models available for cycling (Ctrl+P in interactive mode) */
    scopedModels?: Array<{
        model: Model<any>;
        thinkingLevel?: ThinkingLevel;
    }>;
    /**
     * Prompt-cache retention for provider requests.
     *
     * Precedence: this option, then the PI_CACHE_RETENTION environment variable,
     * then "long". Pass "short" for short-lived sessions such as subagents.
     */
    cacheRetention?: CacheRetention;
    /**
     * Optional default tool suppression mode when no explicit allowlist is provided.
     *
     * - "all": start with no tools enabled
     * - "builtin": disable the default built-in tools (read, bash, edit, write)
     *   but keep extension/custom tools enabled
     */
    noTools?: "all" | "builtin";
    /**
     * Optional allowlist of tool names.
     *
     * When omitted, pi enables the default built-in tools (read, bash, edit, write)
     * and leaves extension/custom tools enabled unless `noTools` changes that default.
     * When provided, only the listed tool names are enabled.
     */
    tools?: string[];
    /** Custom tools to register (in addition to built-in tools). */
    customTools?: ToolDefinition[];
    /** Resource loader. When omitted, DefaultResourceLoader is used. */
    resourceLoader?: ResourceLoader;
    /** Session manager. Default: SessionManager.create(cwd) */
    sessionManager?: SessionManager;
    /** Settings manager. Default: SettingsManager.create(cwd, agentDir) */
    settingsManager?: SettingsManager;
    /** Session start event metadata for extension runtime startup. */
    sessionStartEvent?: SessionStartEvent;
}
```

#### CreateAgentSessionResult

Kind: interface

```ts
/** Result from createAgentSession */
export interface CreateAgentSessionResult {
    /** The created session */
    session: AgentSession;
    /** Extensions result (for UI context setup in interactive mode) */
    extensionsResult: LoadExtensionsResult;
    /** Warning if session was restored with a different model than saved */
    modelFallbackMessage?: string;
}
```

### core/session-manager

#### BranchSummaryEntry

Kind: interface

```ts
export interface BranchSummaryEntry<T = unknown> extends SessionEntryBase {
    type: "branch_summary";
    fromId: string;
    summary: string;
    /** Extension-specific data (not sent to LLM) */
    details?: T;
    /** True if generated by an extension, false if pi-generated */
    fromHook?: boolean;
}
```

#### buildSessionContext

Kind: function

```ts
/**
 * Build the session context from entries using tree traversal.
 * If leafId is provided, walks from that entry to root.
 * Handles compaction and branch summaries along the path.
 */
export declare function buildSessionContext(entries: SessionEntry[], leafId?: string | null, byId?: Map<string, SessionEntry>): SessionContext;
```

#### CompactionEntry

Kind: interface

```ts
export interface CompactionEntry<T = unknown> extends SessionEntryBase {
    type: "compaction";
    summary: string;
    firstKeptEntryId: string;
    tokensBefore: number;
    /** Extension-specific data (e.g., ArtifactIndex, version markers for structured compaction) */
    details?: T;
    /** True if generated by an extension, undefined/false if pi-generated (backward compatible) */
    fromHook?: boolean;
}
```

#### CURRENT_SESSION_VERSION

Kind: const

```ts
export declare const CURRENT_SESSION_VERSION = 3;
```

#### CustomEntry

Kind: interface

```ts
/**
 * Custom entry for extensions to store extension-specific data in the session.
 * Use customType to identify your extension's entries.
 *
 * Purpose: Persist extension state across session reloads. On reload, extensions can
 * scan entries for their customType and reconstruct internal state.
 *
 * Does NOT participate in LLM context (ignored by buildSessionContext).
 * For injecting content into context, see CustomMessageEntry.
 */
export interface CustomEntry<T = unknown> extends SessionEntryBase {
    type: "custom";
    customType: string;
    data?: T;
}
```

#### CustomMessageEntry

Kind: interface

```ts
/**
 * Custom message entry for extensions to inject messages into LLM context.
 * Use customType to identify your extension's entries.
 *
 * Unlike CustomEntry, this DOES participate in LLM context.
 * The content is converted to a user message in buildSessionContext().
 * Use details for extension-specific metadata (not sent to LLM).
 *
 * display controls TUI rendering:
 * - false: hidden entirely
 * - true: rendered with distinct styling (different from user messages)
 */
export interface CustomMessageEntry<T = unknown> extends SessionEntryBase {
    type: "custom_message";
    customType: string;
    content: string | (TextContent | ImageContent)[];
    details?: T;
    display: boolean;
}
```

#### FileEntry

Kind: type

```ts
/** Raw file entry (includes header) */
export type FileEntry = SessionHeader | SessionEntry;
```

#### getLatestCompactionEntry

Kind: function

```ts
export declare function getLatestCompactionEntry(entries: SessionEntry[]): CompactionEntry | null;
```

#### migrateSessionEntries

Kind: function

```ts
/** Exported for testing */
export declare function migrateSessionEntries(entries: FileEntry[]): void;
```

#### ModelChangeEntry

Kind: interface

```ts
export interface ModelChangeEntry extends SessionEntryBase {
    type: "model_change";
    provider: string;
    modelId: string;
}
```

#### NewSessionOptions

Kind: interface

```ts
export interface NewSessionOptions {
    id?: string;
    parentSession?: string;
}
```

#### parseSessionEntries

Kind: function

```ts
/** Exported for compaction.test.ts */
export declare function parseSessionEntries(content: string): FileEntry[];
```

#### SessionContext

Kind: interface

```ts
export interface SessionContext {
    messages: AgentMessage[];
    thinkingLevel: string;
    model: {
        provider: string;
        modelId: string;
    } | null;
}
```

#### SessionEntry

Kind: type

```ts
/** Session entry - has id/parentId for tree structure (returned by "read" methods in SessionManager) */
export type SessionEntry = SessionMessageEntry | ThinkingLevelChangeEntry | ModelChangeEntry | CompactionEntry | BranchSummaryEntry | CustomEntry | CustomMessageEntry | LabelEntry | SessionInfoEntry;
```

#### SessionEntryBase

Kind: interface

```ts
export interface SessionEntryBase {
    type: string;
    id: string;
    parentId: string | null;
    timestamp: string;
}
```

#### SessionHeader

Kind: interface

```ts
export interface SessionHeader {
    type: "session";
    version?: number;
    id: string;
    timestamp: string;
    cwd: string;
    parentSession?: string;
}
```

#### SessionInfo

Kind: interface

```ts
export interface SessionInfo {
    path: string;
    id: string;
    /** Working directory where the session was started. Empty string for old sessions. */
    cwd: string;
    /** User-defined display name from session_info entries. */
    name?: string;
    /** Path to the parent session (if this session was forked). */
    parentSessionPath?: string;
    created: Date;
    modified: Date;
    messageCount: number;
    firstMessage: string;
    allMessagesText: string;
}
```

#### SessionInfoEntry

Kind: interface

```ts
/** Session metadata entry (e.g., user-defined display name). */
export interface SessionInfoEntry extends SessionEntryBase {
    type: "session_info";
    name?: string;
}
```

#### SessionManager

Kind: class

```ts
/**
 * Manages conversation sessions as append-only trees stored in JSONL files.
 *
 * Each session entry has an id and parentId forming a tree structure. The "leaf"
 * pointer tracks the current position. Appending creates a child of the current leaf.
 * Branching moves the leaf to an earlier entry, allowing new branches without
 * modifying history.
 *
 * Use buildSessionContext() to get the resolved message list for the LLM, which
 * handles compaction summaries and follows the path from root to current leaf.
 */
export declare class SessionManager {
    private sessionId;
    private sessionFile;
    private sessionDir;
    private cwd;
    private persist;
    private flushed;
    private fileEntries;
    private byId;
    private labelsById;
    private labelTimestampsById;
    private leafId;
    private constructor();
    /** Switch to a different session file (used for resume and branching) */
    setSessionFile(sessionFile: string): void;
    newSession(options?: NewSessionOptions): string | undefined;
    private _buildIndex;
    private _rewriteFile;
    isPersisted(): boolean;
    getCwd(): string;
    getSessionDir(): string;
    getSessionId(): string;
    getSessionFile(): string | undefined;
    _persist(entry: SessionEntry): void;
    private _appendEntry;
    /** Append a message as child of current leaf, then advance leaf. Returns entry id.
     * Does not allow writing CompactionSummaryMessage and BranchSummaryMessage directly.
     * Reason: we want these to be top-level entries in the session, not message session entries,
     * so it is easier to find them.
     * These need to be appended via appendCompaction() and appendBranchSummary() methods.
     */
    appendMessage(message: Message | CustomMessage | BashExecutionMessage): string;
    /** Append a thinking level change as child of current leaf, then advance leaf. Returns entry id. */
    appendThinkingLevelChange(thinkingLevel: string): string;
    /** Append a model change as child of current leaf, then advance leaf. Returns entry id. */
    appendModelChange(provider: string, modelId: string): string;
    /** Append a compaction summary as child of current leaf, then advance leaf. Returns entry id. */
    appendCompaction<T = unknown>(summary: string, firstKeptEntryId: string, tokensBefore: number, details?: T, fromHook?: boolean): string;
    /** Append a custom entry (for extensions) as child of current leaf, then advance leaf. Returns entry id. */
    appendCustomEntry(customType: string, data?: unknown): string;
    /** Append a session info entry (e.g., display name). Returns entry id. */
    appendSessionInfo(name: string): string;
    /** Get the current session name from the latest session_info entry, if any. */
    getSessionName(): string | undefined;
    /**
     * Append a custom message entry (for extensions) that participates in LLM context.
     * @param customType Extension identifier for filtering on reload
     * @param content Message content (string or TextContent/ImageContent array)
     * @param display Whether to show in TUI (true = styled display, false = hidden)
     * @param details Optional extension-specific metadata (not sent to LLM)
     * @returns Entry id
     */
    appendCustomMessageEntry<T = unknown>(customType: string, content: string | (TextContent | ImageContent)[], display: boolean, details?: T): string;
    getLeafId(): string | null;
    getLeafEntry(): SessionEntry | undefined;
    getEntry(id: string): SessionEntry | undefined;
    /**
     * Get all direct children of an entry.
     */
    getChildren(parentId: string): SessionEntry[];
    /**
     * Get the label for an entry, if any.
     */
    getLabel(id: string): string | undefined;
    /**
     * Set or clear a label on an entry.
     * Labels are user-defined markers for bookmarking/navigation.
     * Pass undefined or empty string to clear the label.
     */
    appendLabelChange(targetId: string, label: string | undefined): string;
    /**
     * Walk from entry to root, returning all entries in path order.
     * Includes all entry types (messages, compaction, model changes, etc.).
     * Use buildSessionContext() to get the resolved messages for the LLM.
     */
    getBranch(fromId?: string): SessionEntry[];
    /**
     * Build the session context (what gets sent to the LLM).
     * Uses tree traversal from current leaf.
     */
    buildSessionContext(): SessionContext;
    /**
     * Get session header.
     */
    getHeader(): SessionHeader | null;
    /**
     * Get all session entries (excludes header). Returns a shallow copy.
     * The session is append-only: use appendXXX() to add entries, branch() to
     * change the leaf pointer. Entries cannot be modified or deleted.
     */
    getEntries(): SessionEntry[];
    /**
     * Get the session as a tree structure. Returns a shallow defensive copy of all entries.
     * A well-formed session has exactly one root (first entry with parentId === null).
     * Orphaned entries (broken parent chain) are also returned as roots.
     */
    getTree(): SessionTreeNode[];
    /**
     * Start a new branch from an earlier entry.
     * Moves the leaf pointer to the specified entry. The next appendXXX() call
     * will create a child of that entry, forming a new branch. Existing entries
     * are not modified or deleted.
     */
    branch(branchFromId: string): void;
    /**
     * Reset the leaf pointer to null (before any entries).
     * The next appendXXX() call will create a new root entry (parentId = null).
     * Use this when navigating to re-edit the first user message.
     */
    resetLeaf(): void;
    /**
     * Start a new branch with a summary of the abandoned path.
     * Same as branch(), but also appends a branch_summary entry that captures
     * context from the abandoned conversation path.
     */
    branchWithSummary(branchFromId: string | null, summary: string, details?: unknown, fromHook?: boolean): string;
    /**
     * Create a new session file containing only the path from root to the specified leaf.
     * Useful for extracting a single conversation path from a branched session.
     * Returns the new session file path, or undefined if not persisting.
     */
    createBranchedSession(leafId: string): string | undefined;
    /**
     * Create a new session.
     * @param cwd Working directory (stored in session header)
     * @param sessionDir Optional session directory. If omitted, uses default (~/.pi/agent/sessions/<encoded-cwd>/).
     */
    static create(cwd: string, sessionDir?: string): SessionManager;
    /**
     * Open a specific session file.
     * @param path Path to session file
     * @param sessionDir Optional session directory for /new or /branch. If omitted, derives from file's parent.
     * @param cwdOverride Optional cwd override instead of the session header cwd.
     */
    static open(path: string, sessionDir?: string, cwdOverride?: string): SessionManager;
    /**
     * Continue the most recent session, or create new if none.
     * @param cwd Working directory
     * @param sessionDir Optional session directory. If omitted, uses default (~/.pi/agent/sessions/<encoded-cwd>/).
     */
    static continueRecent(cwd: string, sessionDir?: string): SessionManager;
    /** Create an in-memory session (no file persistence) */
    static inMemory(cwd?: string): SessionManager;
    /**
     * Fork a session from another project directory into the current project.
     * Creates a new session in the target cwd with the full history from the source session.
     * @param sourcePath Path to the source session file
     * @param targetCwd Target working directory (where the new session will be stored)
     * @param sessionDir Optional session directory. If omitted, uses default for targetCwd.
     */
    static forkFrom(sourcePath: string, targetCwd: string, sessionDir?: string): SessionManager;
    /**
     * List all sessions for a directory.
     * @param cwd Working directory (used to compute default session directory)
     * @param sessionDir Optional session directory. If omitted, uses default (~/.pi/agent/sessions/<encoded-cwd>/).
     * @param onProgress Optional callback for progress updates (loaded, total)
     */
    static list(cwd: string, sessionDir?: string, onProgress?: SessionListProgress): Promise<SessionInfo[]>;
    /**
     * List all sessions across all project directories.
     * @param onProgress Optional callback for progress updates (loaded, total)
     */
    static listAll(onProgress?: SessionListProgress): Promise<SessionInfo[]>;
}
```

#### SessionMessageEntry

Kind: interface

```ts
export interface SessionMessageEntry extends SessionEntryBase {
    type: "message";
    message: AgentMessage;
}
```

#### ThinkingLevelChangeEntry

Kind: interface

```ts
export interface ThinkingLevelChangeEntry extends SessionEntryBase {
    type: "thinking_level_change";
    thinkingLevel: string;
}
```

### core/settings-manager

#### CompactionSettings

Kind: interface

```ts
export interface CompactionSettings {
    enabled?: boolean;
    reserveTokens?: number;
    keepRecentTokens?: number;
}
```

#### ImageSettings

Kind: interface

```ts
export interface ImageSettings {
    autoResize?: boolean;
    blockImages?: boolean;
}
```

#### PackageSource

Kind: type

```ts
/**
 * Package source for npm/git packages.
 * - String form: load all resources from the package
 * - Object form: filter which resources to load
 */
export type PackageSource = string | {
    source: string;
    extensions?: string[];
    skills?: string[];
    prompts?: string[];
    themes?: string[];
};
```

#### RetrySettings

Kind: interface

```ts
export interface RetrySettings {
    enabled?: boolean;
    maxRetries?: number;
    baseDelayMs?: number;
    provider?: ProviderRetrySettings;
}
```

#### SettingsManager

Kind: class

```ts
export declare class SettingsManager {
    private storage;
    private globalSettings;
    private projectSettings;
    private settings;
    private modifiedFields;
    private modifiedNestedFields;
    private modifiedProjectFields;
    private modifiedProjectNestedFields;
    private globalSettingsLoadError;
    private projectSettingsLoadError;
    private writeQueue;
    private errors;
    private constructor();
    /** Create a SettingsManager that loads from files */
    static create(cwd: string, agentDir?: string): SettingsManager;
    /** Create a SettingsManager from an arbitrary storage backend */
    static fromStorage(storage: SettingsStorage): SettingsManager;
    /** Create an in-memory SettingsManager (no file I/O) */
    static inMemory(settings?: Partial<Settings>): SettingsManager;
    private static loadFromStorage;
    private static tryLoadFromStorage;
    /** Migrate old settings format to new format */
    private static migrateSettings;
    getGlobalSettings(): Settings;
    getProjectSettings(): Settings;
    reload(): Promise<void>;
    /** Apply additional overrides on top of current settings */
    applyOverrides(overrides: Partial<Settings>): void;
    /** Mark a global field as modified during this session */
    private markModified;
    /** Mark a project field as modified during this session */
    private markProjectModified;
    private recordError;
    private clearModifiedScope;
    private enqueueWrite;
    private cloneModifiedNestedFields;
    private persistScopedSettings;
    private save;
    private saveProjectSettings;
    flush(): Promise<void>;
    drainErrors(): SettingsError[];
    getLastChangelogVersion(): string | undefined;
    setLastChangelogVersion(version: string): void;
    getSessionDir(): string | undefined;
    getDefaultProvider(): string | undefined;
    getDefaultModel(): string | undefined;
    setDefaultProvider(provider: string): void;
    setDefaultModel(modelId: string): void;
    setDefaultModelAndProvider(provider: string, modelId: string): void;
    getSteeringMode(): "all" | "one-at-a-time";
    setSteeringMode(mode: "all" | "one-at-a-time"): void;
    getFollowUpMode(): "all" | "one-at-a-time";
    setFollowUpMode(mode: "all" | "one-at-a-time"): void;
    getTheme(): string | undefined;
    setTheme(theme: string): void;
    getDefaultThinkingLevel(): "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | undefined;
    setDefaultThinkingLevel(level: "off" | "minimal" | "low" | "medium" | "high" | "xhigh"): void;
    getTransport(): TransportSetting;
    setTransport(transport: TransportSetting): void;
    getCompactionEnabled(): boolean;
    setCompactionEnabled(enabled: boolean): void;
    getCompactionReserveTokens(): number;
    getCompactionKeepRecentTokens(): number;
    getCompactionSettings(): {
        enabled: boolean;
        reserveTokens: number;
        keepRecentTokens: number;
    };
    getBranchSummarySettings(): {
        reserveTokens: number;
        skipPrompt: boolean;
    };
    getBranchSummarySkipPrompt(): boolean;
    getRetryEnabled(): boolean;
    setRetryEnabled(enabled: boolean): void;
    getRetrySettings(): {
        enabled: boolean;
        maxRetries: number;
        baseDelayMs: number;
    };
    getProviderRetrySettings(): {
        timeoutMs?: number;
        maxRetries?: number;
        maxRetryDelayMs: number;
    };
    getHideThinkingBlock(): boolean;
    setHideThinkingBlock(hide: boolean): void;
    getShellPath(): string | undefined;
    setShellPath(path: string | undefined): void;
    getQuietStartup(): boolean;
    setQuietStartup(quiet: boolean): void;
    getShellCommandPrefix(): string | undefined;
    setShellCommandPrefix(prefix: string | undefined): void;
    getNpmCommand(): string[] | undefined;
    setNpmCommand(command: string[] | undefined): void;
    getCollapseChangelog(): boolean;
    setCollapseChangelog(collapse: boolean): void;
    getEnableInstallTelemetry(): boolean;
    setEnableInstallTelemetry(enabled: boolean): void;
    getPackages(): PackageSource[];
    setPackages(packages: PackageSource[]): void;
    setProjectPackages(packages: PackageSource[]): void;
    getExtensionPaths(): string[];
    setExtensionPaths(paths: string[]): void;
    setProjectExtensionPaths(paths: string[]): void;
    getSkillPaths(): string[];
    setSkillPaths(paths: string[]): void;
    setProjectSkillPaths(paths: string[]): void;
    getPromptTemplatePaths(): string[];
    setPromptTemplatePaths(paths: string[]): void;
    setProjectPromptTemplatePaths(paths: string[]): void;
    getThemePaths(): string[];
    setThemePaths(paths: string[]): void;
    setProjectThemePaths(paths: string[]): void;
    getEnableSkillCommands(): boolean;
    setEnableSkillCommands(enabled: boolean): void;
    getThinkingBudgets(): ThinkingBudgetsSettings | undefined;
    getShowImages(): boolean;
    setShowImages(show: boolean): void;
    getImageWidthCells(): number;
    setImageWidthCells(width: number): void;
    getClearOnShrink(): boolean;
    setClearOnShrink(enabled: boolean): void;
    getShowTerminalProgress(): boolean;
    setShowTerminalProgress(enabled: boolean): void;
    getImageAutoResize(): boolean;
    setImageAutoResize(enabled: boolean): void;
    getBlockImages(): boolean;
    setBlockImages(blocked: boolean): void;
    getEnabledModels(): string[] | undefined;
    setEnabledModels(patterns: string[] | undefined): void;
    getDoubleEscapeAction(): "fork" | "tree" | "none";
    setDoubleEscapeAction(action: "fork" | "tree" | "none"): void;
    getTreeFilterMode(): "default" | "no-tools" | "user-only" | "labeled-only" | "all";
    setTreeFilterMode(mode: "default" | "no-tools" | "user-only" | "labeled-only" | "all"): void;
    getShowHardwareCursor(): boolean;
    setShowHardwareCursor(enabled: boolean): void;
    getEditorPaddingX(): number;
    setEditorPaddingX(padding: number): void;
    getAutocompleteMaxVisible(): number;
    setAutocompleteMaxVisible(maxVisible: number): void;
    getCodeBlockIndent(): string;
    getWarnings(): WarningSettings;
    setWarnings(warnings: WarningSettings): void;
}
```

### core/skills

#### formatSkillsForPrompt

Kind: function

```ts
/**
 * Format skills for inclusion in a system prompt.
 * Uses XML format per Agent Skills standard.
 * See: https://agentskills.io/integrate-skills
 *
 * Skills with disableModelInvocation=true are excluded from the prompt
 * (they can only be invoked explicitly via /skill:name commands).
 */
export declare function formatSkillsForPrompt(skills: Skill[]): string;
```

#### loadSkills

Kind: function

```ts
/**
 * Load skills from all configured locations.
 * Returns skills and any validation diagnostics.
 */
export declare function loadSkills(options: LoadSkillsOptions): LoadSkillsResult;
```

#### loadSkillsFromDir

Kind: function

```ts
/**
 * Load skills from a directory.
 *
 * Discovery rules:
 * - if a directory contains SKILL.md, treat it as a skill root and do not recurse further
 * - otherwise, load direct .md children in the root
 * - recurse into subdirectories to find SKILL.md
 */
export declare function loadSkillsFromDir(options: LoadSkillsFromDirOptions): LoadSkillsResult;
```

#### LoadSkillsFromDirOptions

Kind: interface

```ts
export interface LoadSkillsFromDirOptions {
    /** Directory to scan for skills */
    dir: string;
    /** Source identifier for these skills */
    source: string;
}
```

#### LoadSkillsResult

Kind: interface

```ts
export interface LoadSkillsResult {
    skills: Skill[];
    diagnostics: ResourceDiagnostic[];
}
```

#### Skill

Kind: interface

```ts
export interface Skill {
    name: string;
    description: string;
    filePath: string;
    baseDir: string;
    sourceInfo: SourceInfo;
    disableModelInvocation: boolean;
}
```

#### SkillFrontmatter

Kind: interface

```ts
export interface SkillFrontmatter {
    name?: string;
    description?: string;
    "disable-model-invocation"?: boolean;
    [key: string]: unknown;
}
```

### core/slash-commands

#### SlashCommandInfo

Kind: interface

```ts
export interface SlashCommandInfo {
    name: string;
    description?: string;
    source: SlashCommandSource;
    sourceInfo: SourceInfo;
}
```

#### SlashCommandSource

Kind: type

```ts
export type SlashCommandSource = "extension" | "prompt" | "skill";
```

### core/source-info

#### createSyntheticSourceInfo

Kind: function

```ts
export declare function createSyntheticSourceInfo(path: string, options: {
    source: string;
    scope?: SourceScope;
    origin?: SourceOrigin;
    baseDir?: string;
}): SourceInfo;
```

#### SourceInfo

Kind: interface

```ts
export interface SourceInfo {
    path: string;
    source: string;
    scope: SourceScope;
    origin: SourceOrigin;
    baseDir?: string;
}
```

### core/system-prompt

#### BuildSystemPromptOptions

Kind: interface

```ts
export interface BuildSystemPromptOptions {
    /** Custom system prompt (replaces default). */
    customPrompt?: string;
    /** Tools to include in prompt. Default: [read, bash, edit, write] */
    selectedTools?: string[];
    /** Optional one-line tool snippets keyed by tool name. */
    toolSnippets?: Record<string, string>;
    /** Additional guideline bullets appended to the default system prompt guidelines. */
    promptGuidelines?: string[];
    /** Text to append to system prompt. */
    appendSystemPrompt?: string;
    /** Working directory. */
    cwd: string;
    /** Pre-loaded context files. */
    contextFiles?: Array<{
        path: string;
        content: string;
    }>;
    /** Pre-loaded skills. */
    skills?: Skill[];
}
```

### core/tools/bash

#### BashOperations

Kind: interface

```ts
/**
 * Pluggable operations for the bash tool.
 * Override these to delegate command execution to remote systems (for example SSH).
 */
export interface BashOperations {
    /**
     * Execute a command and stream output.
     * @param command The command to execute
     * @param cwd Working directory
     * @param options Execution options
     * @returns Promise resolving to exit code (null if killed)
     */
    exec: (command: string, cwd: string, options: {
        onData: (data: Buffer) => void;
        signal?: AbortSignal;
        timeout?: number;
        env?: NodeJS.ProcessEnv;
    }) => Promise<{
        exitCode: number | null;
    }>;
}
```

#### BashSpawnContext

Kind: interface

```ts
export interface BashSpawnContext {
    command: string;
    cwd: string;
    env: NodeJS.ProcessEnv;
}
```

#### BashSpawnHook

Kind: type

```ts
export type BashSpawnHook = (context: BashSpawnContext) => BashSpawnContext;
```

#### BashToolDetails

Kind: interface

```ts
export interface BashToolDetails {
    truncation?: TruncationResult;
    fullOutputPath?: string;
}
```

#### BashToolInput

Kind: type

```ts
export type BashToolInput = Static<typeof bashSchema>;
```

#### BashToolOptions

Kind: interface

```ts
export interface BashToolOptions {
    /** Custom operations for command execution. Default: local shell */
    operations?: BashOperations;
    /** Command prefix prepended to every command (for example shell setup commands) */
    commandPrefix?: string;
    /** Optional explicit shell path from settings */
    shellPath?: string;
    /** Hook to adjust command, cwd, or env before execution */
    spawnHook?: BashSpawnHook;
}
```

#### createBashTool

Kind: function

```ts
export declare function createBashTool(cwd: string, options?: BashToolOptions): AgentTool<typeof bashSchema>;
```

#### createBashToolDefinition

Kind: function

```ts
export declare function createBashToolDefinition(cwd: string, options?: BashToolOptions): ToolDefinition<typeof bashSchema, BashToolDetails | undefined, BashRenderState>;
```

#### createLocalBashOperations

Kind: function

```ts
/**
 * Create bash operations using pi's built-in local shell execution backend.
 *
 * This is useful for extensions that intercept user_bash and still want pi's
 * standard local shell behavior while wrapping or rewriting commands.
 */
export declare function createLocalBashOperations(options?: {
    shellPath?: string;
}): BashOperations;
```

### core/tools/edit

#### createEditTool

Kind: function

```ts
export declare function createEditTool(cwd: string, options?: EditToolOptions): AgentTool<typeof editSchema>;
```

#### createEditToolDefinition

Kind: function

```ts
export declare function createEditToolDefinition(cwd: string, options?: EditToolOptions): ToolDefinition<typeof editSchema, EditToolDetails | undefined, EditRenderState>;
```

#### EditOperations

Kind: interface

```ts
/**
 * Pluggable operations for the edit tool.
 * Override these to delegate file editing to remote systems (for example SSH).
 */
export interface EditOperations {
    /** Read file contents as a Buffer */
    readFile: (absolutePath: string) => Promise<Buffer>;
    /** Write content to a file */
    writeFile: (absolutePath: string, content: string) => Promise<void>;
    /** Check if file is readable and writable (throw if not) */
    access: (absolutePath: string) => Promise<void>;
}
```

#### EditToolDetails

Kind: interface

```ts
export interface EditToolDetails {
    /** Unified diff of the changes made */
    diff: string;
    /** Line number of the first change in the new file (for editor navigation) */
    firstChangedLine?: number;
}
```

#### EditToolInput

Kind: type

```ts
export type EditToolInput = Static<typeof editSchema>;
```

#### EditToolOptions

Kind: interface

```ts
export interface EditToolOptions {
    /** Custom operations for file editing. Default: local filesystem */
    operations?: EditOperations;
}
```

### core/tools/file-mutation-queue

#### withFileMutationQueue

Kind: function

```ts
/**
 * Serialize file mutation operations targeting the same file.
 * Operations for different files still run in parallel.
 */
export declare function withFileMutationQueue<T>(filePath: string, fn: () => Promise<T>): Promise<T>;
```

### core/tools/find

#### createFindTool

Kind: function

```ts
export declare function createFindTool(cwd: string, options?: FindToolOptions): AgentTool<typeof findSchema>;
```

#### createFindToolDefinition

Kind: function

```ts
export declare function createFindToolDefinition(cwd: string, options?: FindToolOptions): ToolDefinition<typeof findSchema, FindToolDetails | undefined>;
```

#### FindOperations

Kind: interface

```ts
/**
 * Pluggable operations for the find tool.
 * Override these to delegate file search to remote systems (for example SSH).
 */
export interface FindOperations {
    /** Check if path exists */
    exists: (absolutePath: string) => Promise<boolean> | boolean;
    /** Find files matching glob pattern. Returns relative or absolute paths. */
    glob: (pattern: string, cwd: string, options: {
        ignore: string[];
        limit: number;
    }) => Promise<string[]> | string[];
}
```

#### FindToolDetails

Kind: interface

```ts
export interface FindToolDetails {
    truncation?: TruncationResult;
    resultLimitReached?: number;
}
```

#### FindToolInput

Kind: type

```ts
export type FindToolInput = Static<typeof findSchema>;
```

#### FindToolOptions

Kind: interface

```ts
export interface FindToolOptions {
    /** Custom operations for find. Default: local filesystem plus fd */
    operations?: FindOperations;
}
```

### core/tools/grep

#### createGrepTool

Kind: function

```ts
export declare function createGrepTool(cwd: string, options?: GrepToolOptions): AgentTool<typeof grepSchema>;
```

#### createGrepToolDefinition

Kind: function

```ts
export declare function createGrepToolDefinition(cwd: string, options?: GrepToolOptions): ToolDefinition<typeof grepSchema, GrepToolDetails | undefined>;
```

#### GrepOperations

Kind: interface

```ts
/**
 * Pluggable operations for the grep tool.
 * Override these to delegate search to remote systems (for example SSH).
 */
export interface GrepOperations {
    /** Check if path is a directory. Throws if path does not exist. */
    isDirectory: (absolutePath: string) => Promise<boolean> | boolean;
    /** Read file contents for context lines */
    readFile: (absolutePath: string) => Promise<string> | string;
}
```

#### GrepToolDetails

Kind: interface

```ts
export interface GrepToolDetails {
    truncation?: TruncationResult;
    matchLimitReached?: number;
    linesTruncated?: boolean;
}
```

#### GrepToolInput

Kind: type

```ts
export type GrepToolInput = Static<typeof grepSchema>;
```

#### GrepToolOptions

Kind: interface

```ts
export interface GrepToolOptions {
    /** Custom operations for grep. Default: local filesystem plus ripgrep */
    operations?: GrepOperations;
}
```

### core/tools/index

#### createCodingTools

Kind: function

```ts
export declare function createCodingTools(cwd: string, options?: ToolsOptions): Tool[];
```

#### createReadOnlyTools

Kind: function

```ts
export declare function createReadOnlyTools(cwd: string, options?: ToolsOptions): Tool[];
```

#### ToolsOptions

Kind: interface

```ts
export interface ToolsOptions {
    read?: ReadToolOptions;
    bash?: BashToolOptions;
    write?: WriteToolOptions;
    edit?: EditToolOptions;
    grep?: GrepToolOptions;
    find?: FindToolOptions;
    ls?: LsToolOptions;
}
```

### core/tools/ls

#### createLsTool

Kind: function

```ts
export declare function createLsTool(cwd: string, options?: LsToolOptions): AgentTool<typeof lsSchema>;
```

#### createLsToolDefinition

Kind: function

```ts
export declare function createLsToolDefinition(cwd: string, options?: LsToolOptions): ToolDefinition<typeof lsSchema, LsToolDetails | undefined>;
```

#### LsOperations

Kind: interface

```ts
/**
 * Pluggable operations for the ls tool.
 * Override these to delegate directory listing to remote systems (for example SSH).
 */
export interface LsOperations {
    /** Check if path exists */
    exists: (absolutePath: string) => Promise<boolean> | boolean;
    /** Get file or directory stats. Throws if not found. */
    stat: (absolutePath: string) => Promise<{
        isDirectory: () => boolean;
    }> | {
        isDirectory: () => boolean;
    };
    /** Read directory entries */
    readdir: (absolutePath: string) => Promise<string[]> | string[];
}
```

#### LsToolDetails

Kind: interface

```ts
export interface LsToolDetails {
    truncation?: TruncationResult;
    entryLimitReached?: number;
}
```

#### LsToolInput

Kind: type

```ts
export type LsToolInput = Static<typeof lsSchema>;
```

#### LsToolOptions

Kind: interface

```ts
export interface LsToolOptions {
    /** Custom operations for directory listing. Default: local filesystem */
    operations?: LsOperations;
}
```

### core/tools/read

#### createReadTool

Kind: function

```ts
export declare function createReadTool(cwd: string, options?: ReadToolOptions): AgentTool<typeof readSchema>;
```

#### createReadToolDefinition

Kind: function

```ts
export declare function createReadToolDefinition(cwd: string, options?: ReadToolOptions): ToolDefinition<typeof readSchema, ReadToolDetails | undefined>;
```

#### ReadOperations

Kind: interface

```ts
/**
 * Pluggable operations for the read tool.
 * Override these to delegate file reading to remote systems (for example SSH).
 */
export interface ReadOperations {
    /** Read file contents as a Buffer */
    readFile: (absolutePath: string) => Promise<Buffer>;
    /** Check if file is readable (throw if not) */
    access: (absolutePath: string) => Promise<void>;
    /** Detect image MIME type, return null or undefined for non-images */
    detectImageMimeType?: (absolutePath: string) => Promise<string | null | undefined>;
}
```

#### ReadToolDetails

Kind: interface

```ts
export interface ReadToolDetails {
    truncation?: TruncationResult;
}
```

#### ReadToolInput

Kind: type

```ts
export type ReadToolInput = Static<typeof readSchema>;
```

#### ReadToolOptions

Kind: interface

```ts
export interface ReadToolOptions {
    /** Whether to auto-resize images to 2000x2000 max. Default: true */
    autoResizeImages?: boolean;
    /** Custom operations for file reading. Default: local filesystem */
    operations?: ReadOperations;
}
```

### core/tools/truncate

#### DEFAULT_MAX_BYTES

Kind: const

```ts
export declare const DEFAULT_MAX_BYTES: number;
```

#### DEFAULT_MAX_LINES

Kind: const

```ts
/**
 * Shared truncation utilities for tool outputs.
 *
 * Truncation is based on two independent limits - whichever is hit first wins:
 * - Line limit (default: 2000 lines)
 * - Byte limit (default: 50KB)
 *
 * Never returns partial lines (except bash tail truncation edge case).
 */
export declare const DEFAULT_MAX_LINES = 2000;
```

#### formatSize

Kind: function

```ts
/**
 * Format bytes as human-readable size.
 */
export declare function formatSize(bytes: number): string;
```

#### truncateHead

Kind: function

```ts
/**
 * Truncate content from the head (keep first N lines/bytes).
 * Suitable for file reads where you want to see the beginning.
 *
 * Never returns partial lines. If first line exceeds byte limit,
 * returns empty content with firstLineExceedsLimit=true.
 */
export declare function truncateHead(content: string, options?: TruncationOptions): TruncationResult;
```

#### truncateLine

Kind: function

```ts
/**
 * Truncate a single line to max characters, adding [truncated] suffix.
 * Used for grep match lines.
 */
export declare function truncateLine(line: string, maxChars?: number): {
    text: string;
    wasTruncated: boolean;
};
```

#### truncateTail

Kind: function

```ts
/**
 * Truncate content from the tail (keep last N lines/bytes).
 * Suitable for bash output where you want to see the end (errors, final results).
 *
 * May return partial first line if the last line of original content exceeds byte limit.
 */
export declare function truncateTail(content: string, options?: TruncationOptions): TruncationResult;
```

#### TruncationOptions

Kind: interface

```ts
export interface TruncationOptions {
    /** Maximum number of lines (default: 2000) */
    maxLines?: number;
    /** Maximum number of bytes (default: 50KB) */
    maxBytes?: number;
}
```

#### TruncationResult

Kind: interface

```ts
export interface TruncationResult {
    /** The truncated content */
    content: string;
    /** Whether truncation occurred */
    truncated: boolean;
    /** Which limit was hit: "lines", "bytes", or null if not truncated */
    truncatedBy: "lines" | "bytes" | null;
    /** Total number of lines in the original content */
    totalLines: number;
    /** Total number of bytes in the original content */
    totalBytes: number;
    /** Number of complete lines in the truncated output */
    outputLines: number;
    /** Number of bytes in the truncated output */
    outputBytes: number;
    /** Whether the last line was partially truncated (only for tail truncation edge case) */
    lastLinePartial: boolean;
    /** Whether the first line exceeded the byte limit (for head truncation) */
    firstLineExceedsLimit: boolean;
    /** The max lines limit that was applied */
    maxLines: number;
    /** The max bytes limit that was applied */
    maxBytes: number;
}
```

### core/tools/write

#### createWriteTool

Kind: function

```ts
export declare function createWriteTool(cwd: string, options?: WriteToolOptions): AgentTool<typeof writeSchema>;
```

#### createWriteToolDefinition

Kind: function

```ts
export declare function createWriteToolDefinition(cwd: string, options?: WriteToolOptions): ToolDefinition<typeof writeSchema, undefined>;
```

#### WriteOperations

Kind: interface

```ts
/**
 * Pluggable operations for the write tool.
 * Override these to delegate file writing to remote systems (for example SSH).
 */
export interface WriteOperations {
    /** Write content to a file */
    writeFile: (absolutePath: string, content: string) => Promise<void>;
    /** Create directory recursively */
    mkdir: (dir: string) => Promise<void>;
}
```

#### WriteToolInput

Kind: type

```ts
export type WriteToolInput = Static<typeof writeSchema>;
```

#### WriteToolOptions

Kind: interface

```ts
export interface WriteToolOptions {
    /** Custom operations for file writing. Default: local filesystem */
    operations?: WriteOperations;
}
```

### external/@earendil-works/pi-agent-core/dist/types

#### AgentToolResult

Kind: re-export

```ts
export type { AgentEndEvent, AgentStartEvent, AgentToolResult, AgentToolUpdateCallback, AppKeybinding, AutocompleteProviderFactory, BashToolCallEvent, BeforeAgentStartEvent, BeforeAgentStartEventResult, BeforeProviderRequestEvent, BeforeProviderRequestEventResult, BuildSystemPromptOptions, CompactOptions, ContextEvent, ContextUsage, CustomToolCallEvent, EditToolCallEvent, ExecOptions, ExecResult, Extension, ExtensionActions, ExtensionAPI, ExtensionCommandContext, ExtensionCommandContextActions, ExtensionContext, ExtensionContextActions, ExtensionError, ExtensionEvent, ExtensionFactory, ExtensionFlag, ExtensionHandler, ExtensionRuntime, ExtensionShortcut, ExtensionUIContext, ExtensionUIDialogOptions, ExtensionWidgetOptions, FindToolCallEvent, GrepToolCallEvent, InputEvent, InputEventResult, InputSource, KeybindingsManager, LoadExtensionsResult, LsToolCallEvent, MessageRenderer, MessageRenderOptions, ProviderConfig, ProviderModelConfig, ReadToolCallEvent, RegisteredCommand, RegisteredTool, ResolvedCommand, SessionBeforeCompactEvent, SessionBeforeForkEvent, SessionBeforeSwitchEvent, SessionBeforeTreeEvent, SessionCompactEvent, SessionShutdownEvent, SessionStartEvent, SessionTreeEvent, SlashCommandInfo, SlashCommandSource, SourceInfo, TerminalInputHandler, ToolCallEvent, ToolCallEventResult, ToolDefinition, ToolExecutionMode, ToolInfo, ToolRenderResultOptions, ToolResultEvent, TurnEndEvent, TurnStartEvent, UserBashEvent, UserBashEventResult, WidgetPlacement, WorkingIndicatorOptions, WriteToolCallEvent, } from "./core/extensions/index.js";
```

#### AgentToolUpdateCallback

Kind: re-export

```ts
export type { AgentEndEvent, AgentStartEvent, AgentToolResult, AgentToolUpdateCallback, AppKeybinding, AutocompleteProviderFactory, BashToolCallEvent, BeforeAgentStartEvent, BeforeAgentStartEventResult, BeforeProviderRequestEvent, BeforeProviderRequestEventResult, BuildSystemPromptOptions, CompactOptions, ContextEvent, ContextUsage, CustomToolCallEvent, EditToolCallEvent, ExecOptions, ExecResult, Extension, ExtensionActions, ExtensionAPI, ExtensionCommandContext, ExtensionCommandContextActions, ExtensionContext, ExtensionContextActions, ExtensionError, ExtensionEvent, ExtensionFactory, ExtensionFlag, ExtensionHandler, ExtensionRuntime, ExtensionShortcut, ExtensionUIContext, ExtensionUIDialogOptions, ExtensionWidgetOptions, FindToolCallEvent, GrepToolCallEvent, InputEvent, InputEventResult, InputSource, KeybindingsManager, LoadExtensionsResult, LsToolCallEvent, MessageRenderer, MessageRenderOptions, ProviderConfig, ProviderModelConfig, ReadToolCallEvent, RegisteredCommand, RegisteredTool, ResolvedCommand, SessionBeforeCompactEvent, SessionBeforeForkEvent, SessionBeforeSwitchEvent, SessionBeforeTreeEvent, SessionCompactEvent, SessionShutdownEvent, SessionStartEvent, SessionTreeEvent, SlashCommandInfo, SlashCommandSource, SourceInfo, TerminalInputHandler, ToolCallEvent, ToolCallEventResult, ToolDefinition, ToolExecutionMode, ToolInfo, ToolRenderResultOptions, ToolResultEvent, TurnEndEvent, TurnStartEvent, UserBashEvent, UserBashEventResult, WidgetPlacement, WorkingIndicatorOptions, WriteToolCallEvent, } from "./core/extensions/index.js";
```

#### ToolExecutionMode

Kind: re-export

```ts
export type { AgentEndEvent, AgentStartEvent, AgentToolResult, AgentToolUpdateCallback, AppKeybinding, AutocompleteProviderFactory, BashToolCallEvent, BeforeAgentStartEvent, BeforeAgentStartEventResult, BeforeProviderRequestEvent, BeforeProviderRequestEventResult, BuildSystemPromptOptions, CompactOptions, ContextEvent, ContextUsage, CustomToolCallEvent, EditToolCallEvent, ExecOptions, ExecResult, Extension, ExtensionActions, ExtensionAPI, ExtensionCommandContext, ExtensionCommandContextActions, ExtensionContext, ExtensionContextActions, ExtensionError, ExtensionEvent, ExtensionFactory, ExtensionFlag, ExtensionHandler, ExtensionRuntime, ExtensionShortcut, ExtensionUIContext, ExtensionUIDialogOptions, ExtensionWidgetOptions, FindToolCallEvent, GrepToolCallEvent, InputEvent, InputEventResult, InputSource, KeybindingsManager, LoadExtensionsResult, LsToolCallEvent, MessageRenderer, MessageRenderOptions, ProviderConfig, ProviderModelConfig, ReadToolCallEvent, RegisteredCommand, RegisteredTool, ResolvedCommand, SessionBeforeCompactEvent, SessionBeforeForkEvent, SessionBeforeSwitchEvent, SessionBeforeTreeEvent, SessionCompactEvent, SessionShutdownEvent, SessionStartEvent, SessionTreeEvent, SlashCommandInfo, SlashCommandSource, SourceInfo, TerminalInputHandler, ToolCallEvent, ToolCallEventResult, ToolDefinition, ToolExecutionMode, ToolInfo, ToolRenderResultOptions, ToolResultEvent, TurnEndEvent, TurnStartEvent, UserBashEvent, UserBashEventResult, WidgetPlacement, WorkingIndicatorOptions, WriteToolCallEvent, } from "./core/extensions/index.js";
```

### main

#### main

Kind: function

```ts
export declare function main(args: string[], options?: MainOptions): Promise<void>;
```

#### MainOptions

Kind: interface

```ts
export interface MainOptions {
    extensionFactories?: ExtensionFactory[];
}
```

### modes/interactive/components/armin

#### ArminComponent

Kind: class

```ts
export declare class ArminComponent implements Component {
    private ui;
    private interval;
    private effect;
    private finalGrid;
    private currentGrid;
    private effectState;
    private cachedLines;
    private cachedWidth;
    private gridVersion;
    private cachedVersion;
    constructor(ui: TUI);
    invalidate(): void;
    render(width: number): string[];
    private createEmptyGrid;
    private initEffect;
    private startAnimation;
    private stopAnimation;
    private tickEffect;
    private tickTypewriter;
    private tickScanline;
    private tickRain;
    private tickFade;
    private tickCrt;
    private tickGlitch;
    private tickDissolve;
    private updateDisplay;
    dispose(): void;
}
```

### modes/interactive/components/assistant-message

#### AssistantMessageComponent

Kind: class

```ts
/**
 * Component that renders a complete assistant message
 */
export declare class AssistantMessageComponent extends Container {
    private contentContainer;
    private hideThinkingBlock;
    private markdownTheme;
    private hiddenThinkingLabel;
    private lastMessage?;
    private hasToolCalls;
    constructor(message?: AssistantMessage, hideThinkingBlock?: boolean, markdownTheme?: MarkdownTheme, hiddenThinkingLabel?: string);
    invalidate(): void;
    setHideThinkingBlock(hide: boolean): void;
    setHiddenThinkingLabel(label: string): void;
    render(width: number): string[];
    updateContent(message: AssistantMessage): void;
}
```

### modes/interactive/components/bash-execution

#### BashExecutionComponent

Kind: class

```ts
export declare class BashExecutionComponent extends Container {
    private command;
    private outputLines;
    private status;
    private exitCode;
    private loader;
    private truncationResult?;
    private fullOutputPath?;
    private expanded;
    private contentContainer;
    constructor(command: string, ui: TUI, excludeFromContext?: boolean);
    /**
     * Set whether the output is expanded (shows full output) or collapsed (preview only).
     */
    setExpanded(expanded: boolean): void;
    invalidate(): void;
    appendOutput(chunk: string): void;
    setComplete(exitCode: number | undefined, cancelled: boolean, truncationResult?: TruncationResult, fullOutputPath?: string): void;
    private updateDisplay;
    /**
     * Get the raw output for creating BashExecutionMessage.
     */
    getOutput(): string;
    /**
     * Get the command that was executed.
     */
    getCommand(): string;
}
```

### modes/interactive/components/bordered-loader

#### BorderedLoader

Kind: class

```ts
/** Loader wrapped with borders for extension UI */
export declare class BorderedLoader extends Container {
    private loader;
    private cancellable;
    private signalController?;
    constructor(tui: TUI, theme: Theme, message: string, options?: {
        cancellable?: boolean;
    });
    get signal(): AbortSignal;
    set onAbort(fn: (() => void) | undefined);
    handleInput(data: string): void;
    dispose(): void;
}
```

### modes/interactive/components/branch-summary-message

#### BranchSummaryMessageComponent

Kind: class

```ts
/**
 * Component that renders a branch summary message with collapsed/expanded state.
 * Uses same background color as custom messages for visual consistency.
 */
export declare class BranchSummaryMessageComponent extends Box {
    private expanded;
    private message;
    private markdownTheme;
    constructor(message: BranchSummaryMessage, markdownTheme?: MarkdownTheme);
    setExpanded(expanded: boolean): void;
    invalidate(): void;
    private updateDisplay;
}
```

### modes/interactive/components/compaction-summary-message

#### CompactionSummaryMessageComponent

Kind: class

```ts
/**
 * Component that renders a compaction message with collapsed/expanded state.
 * Uses same background color as custom messages for visual consistency.
 */
export declare class CompactionSummaryMessageComponent extends Box {
    private expanded;
    private message;
    private markdownTheme;
    constructor(message: CompactionSummaryMessage, markdownTheme?: MarkdownTheme);
    setExpanded(expanded: boolean): void;
    invalidate(): void;
    private updateDisplay;
}
```

### modes/interactive/components/custom-editor

#### CustomEditor

Kind: class

```ts
/**
 * Custom editor that handles app-level keybindings for coding-agent.
 */
export declare class CustomEditor extends Editor {
    private keybindings;
    actionHandlers: Map<AppKeybinding, () => void>;
    onEscape?: () => void;
    onCtrlD?: () => void;
    onPasteImage?: () => void;
    /** Handler for extension-registered shortcuts. Returns true if handled. */
    onExtensionShortcut?: (data: string) => boolean;
    constructor(tui: TUI, theme: EditorTheme, keybindings: KeybindingsManager, options?: EditorOptions);
    /**
     * Register a handler for an app action.
     */
    onAction(action: AppKeybinding, handler: () => void): void;
    handleInput(data: string): void;
}
```

### modes/interactive/components/custom-message

#### CustomMessageComponent

Kind: class

```ts
/**
 * Component that renders a custom message entry from extensions.
 * Uses distinct styling to differentiate from user messages.
 */
export declare class CustomMessageComponent extends Container {
    private message;
    private customRenderer?;
    private box;
    private customComponent?;
    private markdownTheme;
    private _expanded;
    constructor(message: CustomMessage<unknown>, customRenderer?: MessageRenderer, markdownTheme?: MarkdownTheme);
    setExpanded(expanded: boolean): void;
    invalidate(): void;
    private rebuild;
}
```

### modes/interactive/components/diff

#### renderDiff

Kind: function

```ts
/**
 * Render a diff string with colored lines and intra-line change highlighting.
 * - Context lines: dim/gray
 * - Removed lines: red, with inverse on changed tokens
 * - Added lines: green, with inverse on changed tokens
 */
export declare function renderDiff(diffText: string, _options?: RenderDiffOptions): string;
```

#### RenderDiffOptions

Kind: interface

```ts
export interface RenderDiffOptions {
    /** File path (unused, kept for API compatibility) */
    filePath?: string;
}
```

### modes/interactive/components/dynamic-border

#### DynamicBorder

Kind: class

```ts
/**
 * Dynamic border component that adjusts to viewport width.
 *
 * Note: When used from extensions loaded via jiti, the global `theme` may be undefined
 * because jiti creates a separate module cache. Always pass an explicit color
 * function when using DynamicBorder in components exported for extension use.
 */
export declare class DynamicBorder implements Component {
    private color;
    constructor(color?: (str: string) => string);
    invalidate(): void;
    render(width: number): string[];
}
```

### modes/interactive/components/extension-editor

#### ExtensionEditorComponent

Kind: class

```ts
export declare class ExtensionEditorComponent extends Container implements Focusable {
    private editor;
    private onSubmitCallback;
    private onCancelCallback;
    private tui;
    private keybindings;
    private _focused;
    get focused(): boolean;
    set focused(value: boolean);
    constructor(tui: TUI, keybindings: KeybindingsManager, title: string, prefill: string | undefined, onSubmit: (value: string) => void, onCancel: () => void, options?: EditorOptions);
    handleInput(keyData: string): void;
    private openExternalEditor;
}
```

### modes/interactive/components/extension-input

#### ExtensionInputComponent

Kind: class

```ts
export declare class ExtensionInputComponent extends Container implements Focusable {
    private input;
    private onSubmitCallback;
    private onCancelCallback;
    private titleText;
    private baseTitle;
    private countdown;
    private _focused;
    get focused(): boolean;
    set focused(value: boolean);
    constructor(title: string, _placeholder: string | undefined, onSubmit: (value: string) => void, onCancel: () => void, opts?: ExtensionInputOptions);
    handleInput(keyData: string): void;
    dispose(): void;
}
```

### modes/interactive/components/extension-selector

#### ExtensionSelectorComponent

Kind: class

```ts
export declare class ExtensionSelectorComponent extends Container {
    private options;
    private selectedIndex;
    private listContainer;
    private onSelectCallback;
    private onCancelCallback;
    private titleText;
    private baseTitle;
    private countdown;
    constructor(title: string, options: string[], onSelect: (option: string) => void, onCancel: () => void, opts?: ExtensionSelectorOptions);
    private updateList;
    handleInput(keyData: string): void;
    dispose(): void;
}
```

### modes/interactive/components/footer

#### FooterComponent

Kind: class

```ts
/**
 * Footer component that shows pwd, token stats, and context usage.
 * Computes token/context stats from session, gets git branch and extension statuses from provider.
 */
export declare class FooterComponent implements Component {
    private session;
    private footerData;
    private autoCompactEnabled;
    constructor(session: AgentSession, footerData: ReadonlyFooterDataProvider);
    setSession(session: AgentSession): void;
    setAutoCompactEnabled(enabled: boolean): void;
    /**
     * No-op: git branch caching now handled by provider.
     * Kept for compatibility with existing call sites in interactive-mode.
     */
    invalidate(): void;
    /**
     * Clean up resources.
     * Git watcher cleanup now handled by provider.
     */
    dispose(): void;
    render(width: number): string[];
}
```

### modes/interactive/components/keybinding-hints

#### keyHint

Kind: function

```ts
export declare function keyHint(keybinding: Keybinding, description: string): string;
```

#### keyText

Kind: function

```ts
export declare function keyText(keybinding: Keybinding): string;
```

#### rawKeyHint

Kind: function

```ts
export declare function rawKeyHint(key: string, description: string): string;
```

### modes/interactive/components/login-dialog

#### LoginDialogComponent

Kind: class

```ts
/**
 * Login dialog component - replaces editor during OAuth login flow
 */
export declare class LoginDialogComponent extends Container implements Focusable {
    private onComplete;
    private contentContainer;
    private input;
    private tui;
    private abortController;
    private inputResolver?;
    private inputRejecter?;
    private _focused;
    get focused(): boolean;
    set focused(value: boolean);
    constructor(tui: TUI, providerId: string, onComplete: (success: boolean, message?: string) => void, providerNameOverride?: string, titleOverride?: string);
    get signal(): AbortSignal;
    private cancel;
    /**
     * Called by onAuth callback - show URL and optional instructions
     */
    showAuth(url: string, instructions?: string): void;
    /**
     * Show input for manual code/URL entry (for callback server providers)
     */
    showManualInput(prompt: string): Promise<string>;
    /**
     * Called by onPrompt callback - show prompt and wait for input
     * Note: Does NOT clear content, appends to existing (preserves URL from showAuth)
     */
    showPrompt(message: string, placeholder?: string): Promise<string>;
    /**
     * Show informational text without prompting for input.
     */
    showInfo(lines: string[]): void;
    /**
     * Show waiting message (for polling flows like GitHub Copilot)
     */
    showWaiting(message: string): void;
    /**
     * Called by onProgress callback
     */
    showProgress(message: string): void;
    handleInput(data: string): void;
}
```

### modes/interactive/components/model-selector

#### ModelSelectorComponent

Kind: class

```ts
/**
 * Component that renders a model selector with search
 */
export declare class ModelSelectorComponent extends Container implements Focusable {
    private searchInput;
    private _focused;
    get focused(): boolean;
    set focused(value: boolean);
    private listContainer;
    private allModels;
    private scopedModelItems;
    private activeModels;
    private filteredModels;
    private selectedIndex;
    private currentModel?;
    private settingsManager;
    private modelRegistry;
    private onSelectCallback;
    private onCancelCallback;
    private errorMessage?;
    private tui;
    private scopedModels;
    private scope;
    private scopeText?;
    private scopeHintText?;
    constructor(tui: TUI, currentModel: Model<any> | undefined, settingsManager: SettingsManager, modelRegistry: ModelRegistry, scopedModels: ReadonlyArray<ScopedModelItem>, onSelect: (model: Model<any>) => void, onCancel: () => void, initialSearchInput?: string);
    private loadModels;
    private sortModels;
    private getScopeText;
    private getScopeHintText;
    private setScope;
    private filterModels;
    private updateList;
    handleInput(keyData: string): void;
    private handleSelect;
    getSearchInput(): Input;
}
```

### modes/interactive/components/oauth-selector

#### OAuthSelectorComponent

Kind: class

```ts
/**
 * Component that renders an auth provider selector
 */
export declare class OAuthSelectorComponent extends Container implements Focusable {
    private searchInput;
    private _focused;
    get focused(): boolean;
    set focused(value: boolean);
    private listContainer;
    private allProviders;
    private filteredProviders;
    private selectedIndex;
    private mode;
    private authStorage;
    private getAuthStatus;
    private onSelectCallback;
    private onCancelCallback;
    constructor(mode: "login" | "logout", authStorage: AuthStorage, providers: AuthSelectorProvider[], onSelect: (providerId: string) => void, onCancel: () => void, getAuthStatus?: (providerId: string) => AuthStatus);
    private filterProviders;
    private updateList;
    private formatStatusIndicator;
    handleInput(keyData: string): void;
}
```

### modes/interactive/components/session-selector

#### SessionSelectorComponent

Kind: class

```ts
/**
 * Component that renders a session selector
 */
export declare class SessionSelectorComponent extends Container implements Focusable {
    handleInput(data: string): void;
    private canRename;
    private sessionList;
    private header;
    private keybindings;
    private scope;
    private sortMode;
    private nameFilter;
    private currentSessions;
    private allSessions;
    private currentSessionsLoader;
    private allSessionsLoader;
    private onCancel;
    private requestRender;
    private renameSession?;
    private currentLoading;
    private allLoading;
    private allLoadSeq;
    private mode;
    private renameInput;
    private renameTargetPath;
    private _focused;
    get focused(): boolean;
    set focused(value: boolean);
    private buildBaseLayout;
    constructor(currentSessionsLoader: SessionsLoader, allSessionsLoader: SessionsLoader, onSelect: (sessionPath: string) => void, onCancel: () => void, onExit: () => void, requestRender: () => void, options?: {
        renameSession?: (sessionPath: string, currentName: string | undefined) => Promise<void>;
        showRenameHint?: boolean;
        keybindings?: KeybindingsManager;
    }, currentSessionFilePath?: string);
    private loadCurrentSessions;
    private enterRenameMode;
    private exitRenameMode;
    private confirmRename;
    private loadScope;
    private toggleSortMode;
    private toggleNameFilter;
    private refreshSessionsAfterMutation;
    private toggleScope;
    getSessionList(): SessionList;
}
```

### modes/interactive/components/settings-selector

#### SettingsCallbacks

Kind: interface

```ts
export interface SettingsCallbacks {
    onAutoCompactChange: (enabled: boolean) => void;
    onShowImagesChange: (enabled: boolean) => void;
    onImageWidthCellsChange: (width: number) => void;
    onAutoResizeImagesChange: (enabled: boolean) => void;
    onBlockImagesChange: (blocked: boolean) => void;
    onEnableSkillCommandsChange: (enabled: boolean) => void;
    onSteeringModeChange: (mode: "all" | "one-at-a-time") => void;
    onFollowUpModeChange: (mode: "all" | "one-at-a-time") => void;
    onTransportChange: (transport: Transport) => void;
    onThinkingLevelChange: (level: ThinkingLevel) => void;
    onThemeChange: (theme: string) => void;
    onThemePreview?: (theme: string) => void;
    onHideThinkingBlockChange: (hidden: boolean) => void;
    onCollapseChangelogChange: (collapsed: boolean) => void;
    onEnableInstallTelemetryChange: (enabled: boolean) => void;
    onDoubleEscapeActionChange: (action: "fork" | "tree" | "none") => void;
    onTreeFilterModeChange: (mode: "default" | "no-tools" | "user-only" | "labeled-only" | "all") => void;
    onShowHardwareCursorChange: (enabled: boolean) => void;
    onEditorPaddingXChange: (padding: number) => void;
    onAutocompleteMaxVisibleChange: (maxVisible: number) => void;
    onQuietStartupChange: (enabled: boolean) => void;
    onClearOnShrinkChange: (enabled: boolean) => void;
    onShowTerminalProgressChange: (enabled: boolean) => void;
    onWarningsChange: (warnings: WarningSettings) => void;
    onCancel: () => void;
}
```

#### SettingsConfig

Kind: interface

```ts
export interface SettingsConfig {
    autoCompact: boolean;
    showImages: boolean;
    imageWidthCells: number;
    autoResizeImages: boolean;
    blockImages: boolean;
    enableSkillCommands: boolean;
    steeringMode: "all" | "one-at-a-time";
    followUpMode: "all" | "one-at-a-time";
    transport: Transport;
    thinkingLevel: ThinkingLevel;
    availableThinkingLevels: ThinkingLevel[];
    currentTheme: string;
    availableThemes: string[];
    hideThinkingBlock: boolean;
    collapseChangelog: boolean;
    enableInstallTelemetry: boolean;
    doubleEscapeAction: "fork" | "tree" | "none";
    treeFilterMode: "default" | "no-tools" | "user-only" | "labeled-only" | "all";
    showHardwareCursor: boolean;
    editorPaddingX: number;
    autocompleteMaxVisible: number;
    quietStartup: boolean;
    clearOnShrink: boolean;
    showTerminalProgress: boolean;
    warnings: WarningSettings;
}
```

#### SettingsSelectorComponent

Kind: class

```ts
/**
 * Main settings selector component.
 */
export declare class SettingsSelectorComponent extends Container {
    private settingsList;
    constructor(config: SettingsConfig, callbacks: SettingsCallbacks);
    getSettingsList(): SettingsList;
}
```

### modes/interactive/components/show-images-selector

#### ShowImagesSelectorComponent

Kind: class

```ts
/**
 * Component that renders a show images selector with borders
 */
export declare class ShowImagesSelectorComponent extends Container {
    private selectList;
    constructor(currentValue: boolean, onSelect: (show: boolean) => void, onCancel: () => void);
    getSelectList(): SelectList;
}
```

### modes/interactive/components/skill-invocation-message

#### SkillInvocationMessageComponent

Kind: class

```ts
/**
 * Component that renders a skill invocation message with collapsed/expanded state.
 * Uses same background color as custom messages for visual consistency.
 * Only renders the skill block itself - user message is rendered separately.
 */
export declare class SkillInvocationMessageComponent extends Box {
    private expanded;
    private skillBlock;
    private markdownTheme;
    constructor(skillBlock: ParsedSkillBlock, markdownTheme?: MarkdownTheme);
    setExpanded(expanded: boolean): void;
    invalidate(): void;
    private updateDisplay;
}
```

### modes/interactive/components/theme-selector

#### ThemeSelectorComponent

Kind: class

```ts
/**
 * Component that renders a theme selector
 */
export declare class ThemeSelectorComponent extends Container {
    private selectList;
    private onPreview;
    constructor(currentTheme: string, onSelect: (themeName: string) => void, onCancel: () => void, onPreview: (themeName: string) => void);
    getSelectList(): SelectList;
}
```

### modes/interactive/components/thinking-selector

#### ThinkingSelectorComponent

Kind: class

```ts
/**
 * Component that renders a thinking level selector with borders
 */
export declare class ThinkingSelectorComponent extends Container {
    private selectList;
    constructor(currentLevel: ThinkingLevel, availableLevels: ThinkingLevel[], onSelect: (level: ThinkingLevel) => void, onCancel: () => void);
    getSelectList(): SelectList;
}
```

### modes/interactive/components/tool-execution

#### ToolExecutionComponent

Kind: class

```ts
export declare class ToolExecutionComponent extends Container {
    private contentBox;
    private contentText;
    private selfRenderContainer;
    private callRendererComponent?;
    private resultRendererComponent?;
    private rendererState;
    private imageComponents;
    private imageSpacers;
    private toolName;
    private toolCallId;
    private args;
    private expanded;
    private showImages;
    private imageWidthCells;
    private isPartial;
    private toolDefinition?;
    private builtInToolDefinition?;
    private ui;
    private cwd;
    private executionStarted;
    private argsComplete;
    private result?;
    private convertedImages;
    private hideComponent;
    constructor(toolName: string, toolCallId: string, args: any, options: ToolExecutionOptions | undefined, toolDefinition: ToolDefinition<any, any> | undefined, ui: TUI, cwd: string);
    private getCallRenderer;
    private getResultRenderer;
    private hasRendererDefinition;
    private getRenderShell;
    private getRenderContext;
    private createCallFallback;
    private createResultFallback;
    updateArgs(args: any): void;
    markExecutionStarted(): void;
    setArgsComplete(): void;
    updateResult(result: {
        content: Array<{
            type: string;
            text?: string;
            data?: string;
            mimeType?: string;
        }>;
        details?: any;
        isError: boolean;
    }, isPartial?: boolean): void;
    private maybeConvertImagesForKitty;
    setExpanded(expanded: boolean): void;
    setShowImages(show: boolean): void;
    setImageWidthCells(width: number): void;
    invalidate(): void;
    render(width: number): string[];
    private updateDisplay;
    private getTextOutput;
    private formatToolExecution;
}
```

#### ToolExecutionOptions

Kind: interface

```ts
export interface ToolExecutionOptions {
    showImages?: boolean;
    imageWidthCells?: number;
}
```

### modes/interactive/components/tree-selector

#### TreeSelectorComponent

Kind: class

```ts
/**
 * Component that renders a session tree selector for navigation
 */
export declare class TreeSelectorComponent extends Container implements Focusable {
    private treeList;
    private labelInput;
    private labelInputContainer;
    private treeContainer;
    private onLabelChangeCallback?;
    private _focused;
    get focused(): boolean;
    set focused(value: boolean);
    constructor(tree: SessionTreeNode[], currentLeafId: string | null, terminalHeight: number, onSelect: (entryId: string) => void, onCancel: () => void, onLabelChange?: (entryId: string, label: string | undefined) => void, initialSelectedId?: string, initialFilterMode?: FilterMode);
    private showLabelInput;
    private hideLabelInput;
    handleInput(keyData: string): void;
    getTreeList(): TreeList;
}
```

### modes/interactive/components/user-message

#### UserMessageComponent

Kind: class

```ts
/**
 * Component that renders a user message
 */
export declare class UserMessageComponent extends Container {
    private contentBox;
    constructor(text: string, markdownTheme?: MarkdownTheme);
    render(width: number): string[];
}
```

### modes/interactive/components/user-message-selector

#### UserMessageSelectorComponent

Kind: class

```ts
/**
 * Component that renders a user message selector for branching
 */
export declare class UserMessageSelectorComponent extends Container {
    private messageList;
    constructor(messages: UserMessageItem[], onSelect: (entryId: string) => void, onCancel: () => void, initialSelectedId?: string);
    getMessageList(): UserMessageList;
}
```

### modes/interactive/components/visual-truncate

#### truncateToVisualLines

Kind: function

```ts
/**
 * Truncate text to a maximum number of visual lines (from the end).
 * This accounts for line wrapping based on terminal width.
 *
 * @param text - The text content (may contain newlines)
 * @param maxVisualLines - Maximum number of visual lines to show
 * @param width - Terminal/render width
 * @param paddingX - Horizontal padding for Text component (default 0).
 *                   Use 0 when result will be placed in a Box (Box adds its own padding).
 *                   Use 1 when result will be placed in a plain Container.
 * @returns The truncated visual lines and count of skipped lines
 */
export declare function truncateToVisualLines(text: string, maxVisualLines: number, width: number, paddingX?: number): VisualTruncateResult;
```

#### VisualTruncateResult

Kind: interface

```ts
/**
 * Shared utility for truncating text to visual lines (accounting for line wrapping).
 * Used by both tool-execution.ts and bash-execution.ts for consistent behavior.
 */
export interface VisualTruncateResult {
    /** The visual lines to display */
    visualLines: string[];
    /** Number of visual lines that were skipped (hidden) */
    skippedCount: number;
}
```

### modes/interactive/interactive-mode

#### InteractiveMode

Kind: class

```ts
export declare class InteractiveMode {
    private options;
    private runtimeHost;
    private ui;
    private chatContainer;
    private pendingMessagesContainer;
    private statusContainer;
    private defaultEditor;
    private editor;
    private editorComponentFactory;
    private autocompleteProvider;
    private autocompleteProviderWrappers;
    private fdPath;
    private editorContainer;
    private footer;
    private footerDataProvider;
    private keybindings;
    private version;
    private isInitialized;
    private onInputCallback?;
    private loadingAnimation;
    private workingMessage;
    private workingVisible;
    private workingIndicatorOptions;
    private readonly defaultWorkingMessage;
    private readonly defaultHiddenThinkingLabel;
    private hiddenThinkingLabel;
    private lastSigintTime;
    private lastEscapeTime;
    private changelogMarkdown;
    private startupNoticesShown;
    private anthropicSubscriptionWarningShown;
    private lastStatusSpacer;
    private lastStatusText;
    private streamingComponent;
    private streamingMessage;
    private pendingTools;
    private toolOutputExpanded;
    private hideThinkingBlock;
    private skillCommands;
    private unsubscribe?;
    private signalCleanupHandlers;
    private isBashMode;
    private bashComponent;
    private pendingBashComponents;
    private autoCompactionLoader;
    private autoCompactionEscapeHandler?;
    private retryLoader;
    private retryCountdown;
    private retryEscapeHandler?;
    private compactionQueuedMessages;
    private shutdownRequested;
    private extensionSelector;
    private extensionInput;
    private extensionEditor;
    private extensionTerminalInputUnsubscribers;
    private extensionWidgetsAbove;
    private extensionWidgetsBelow;
    private widgetContainerAbove;
    private widgetContainerBelow;
    private customFooter;
    private headerContainer;
    private builtInHeader;
    private customHeader;
    private get session();
    private get agent();
    private get sessionManager();
    private get settingsManager();
    constructor(runtimeHost: AgentSessionRuntime, options?: InteractiveModeOptions);
    private getAutocompleteSourceTag;
    private prefixAutocompleteDescription;
    private getBuiltInCommandConflictDiagnostics;
    private createBaseAutocompleteProvider;
    private setupAutocompleteProvider;
    private showStartupNoticesIfNeeded;
    init(): Promise<void>;
    /**
     * Update terminal title with session name and cwd.
     */
    private updateTerminalTitle;
    /**
     * Run the interactive mode. This is the main entry point.
     * Initializes the UI, shows warnings, processes initial messages, and starts the interactive loop.
     */
    run(): Promise<void>;
    private checkForPackageUpdates;
    private checkTmuxKeyboardSetup;
    /**
     * Get changelog entries to display on startup.
     * Only shows new entries since last seen version, skips for resumed sessions.
     */
    private getChangelogForDisplay;
    private reportInstallTelemetry;
    private getMarkdownThemeWithSettings;
    private formatDisplayPath;
    private formatExtensionDisplayPath;
    private formatContextPath;
    private getStartupExpansionState;
    /**
     * Get a short path relative to the package root for display.
     */
    private getShortPath;
    private getCompactPathLabel;
    private getCompactPackageSourceLabel;
    private getCompactExtensionLabel;
    private getCompactDisplayPathSegments;
    private getCompactNonPackageExtensionLabel;
    private getCompactExtensionLabels;
    private getDisplaySourceInfo;
    private getScopeGroup;
    private isPackageSource;
    private buildScopeGroups;
    private formatScopeGroups;
    private findSourceInfoForPath;
    private formatPathWithSource;
    private formatDiagnostics;
    private showLoadedResources;
    private bindCurrentSessionExtensions;
    private applyRuntimeSettings;
    private rebindCurrentSession;
    private handleExtensionNewSession;
    private handleFatalRuntimeError;
    private renderCurrentSessionState;
    /**
     * Get a registered tool definition by name (for custom rendering).
     */
    private getRegisteredToolDefinition;
    /**
     * Set up keyboard shortcuts registered by extensions.
     */
    private setupExtensionShortcuts;
    /**
     * Set extension status text in the footer.
     */
    private setExtensionStatus;
    private getWorkingLoaderMessage;
    private createWorkingLoader;
    private stopWorkingLoader;
    private setWorkingVisible;
    private setWorkingIndicator;
    private setHiddenThinkingLabel;
    /**
     * Set an extension widget (string array or custom component).
     */
    private setExtensionWidget;
    private clearExtensionWidgets;
    private resetExtensionUI;
    private static readonly MAX_WIDGET_LINES;
    /**
     * Render all extension widgets to the widget container.
     */
    private renderWidgets;
    private renderWidgetContainer;
    /**
     * Set a custom footer component, or restore the built-in footer.
     */
    private setExtensionFooter;
    /**
     * Set a custom header component, or restore the built-in header.
     */
    private setExtensionHeader;
    private addExtensionTerminalInputListener;
    private clearExtensionTerminalInputListeners;
    /**
     * Create the ExtensionUIContext for extensions.
     */
    private createExtensionUIContext;
    /**
     * Show a selector for extensions.
     */
    private showExtensionSelector;
    /**
     * Hide the extension selector.
     */
    private hideExtensionSelector;
    private showExtensionConfirm;
    private promptForMissingSessionCwd;
    /**
     * Show a text input for extensions.
     */
    private showExtensionInput;
    /**
     * Hide the extension input.
     */
    private hideExtensionInput;
    /**
     * Show a multi-line editor for extensions (with Ctrl+G support).
     */
    private showExtensionEditor;
    /**
     * Hide the extension editor.
     */
    private hideExtensionEditor;
    /**
     * Set a custom editor component from an extension.
     * Pass undefined to restore the default editor.
     */
    private setCustomEditorComponent;
    /**
     * Show a notification for extensions.
     */
    private showExtensionNotify;
    private showExtensionCustom;
    /**
     * Show an extension error in the UI.
     */
    private showExtensionError;
    private setupKeyHandlers;
    private handleClipboardImagePaste;
    private setupEditorSubmitHandler;
    private subscribeToAgent;
    private handleEvent;
    /** Extract text content from a user message */
    private getUserMessageText;
    /**
     * Show a status message in the chat.
     *
     * If multiple status messages are emitted back-to-back (without anything else being added to the chat),
     * we update the previous status line instead of appending new ones to avoid log spam.
     */
    private showStatus;
    private addMessageToChat;
    /**
     * Render session context to chat. Used for initial load and rebuild after compaction.
     * @param sessionContext Session context to render
     * @param options.updateFooter Update footer state
     * @param options.populateHistory Add user messages to editor history
     */
    private renderSessionContext;
    renderInitialMessages(): void;
    getUserInput(): Promise<string>;
    private rebuildChatFromMessages;
    private handleCtrlC;
    private handleCtrlD;
    /**
     * Gracefully shutdown the agent.
     * Stops the TUI before emitting shutdown events so extension UI cleanup cannot
     * repaint the final frame while the process is exiting.
     */
    private isShuttingDown;
    private shutdown;
    private emergencyTerminalExit;
    private checkShutdownRequested;
    private registerSignalHandlers;
    private unregisterSignalHandlers;
    private handleCtrlZ;
    private handleFollowUp;
    private handleDequeue;
    private updateEditorBorderColor;
    private cycleThinkingLevel;
    private cycleModel;
    private toggleToolOutputExpansion;
    private setToolsExpanded;
    private toggleThinkingBlockVisibility;
    private openExternalEditor;
    clearEditor(): void;
    showError(errorMessage: string): void;
    showWarning(warningMessage: string): void;
    showNewVersionNotification(newVersion: string): void;
    showPackageUpdateNotification(packages: string[]): void;
    /**
     * Get all queued messages (read-only).
     * Combines session queue and compaction queue.
     */
    private getAllQueuedMessages;
    /**
     * Clear all queued messages and return their contents.
     * Clears both session queue and compaction queue.
     */
    private clearAllQueues;
    private updatePendingMessagesDisplay;
    private restoreQueuedMessagesToEditor;
    private queueCompactionMessage;
    private isExtensionCommand;
    private flushCompactionQueue;
    /** Move pending bash components from pending area to chat */
    private flushPendingBashComponents;
    /**
     * Shows a selector component in place of the editor.
     * @param create Factory that receives a `done` callback and returns the component and focus target
     */
    private showSelector;
    private showSettingsSelector;
    private handleModelCommand;
    private findExactModelMatch;
    private getModelCandidates;
    private updateAvailableProviderCount;
    private maybeWarnAboutAnthropicSubscriptionAuth;
    private showModelSelector;
    private showModelsSelector;
    private showUserMessageSelector;
    private handleCloneCommand;
    private showTreeSelector;
    private showSessionSelector;
    private handleResumeSession;
    private getLoginProviderOptions;
    private getLogoutProviderOptions;
    private showLoginAuthTypeSelector;
    private showLoginProviderSelector;
    private showOAuthSelector;
    private completeProviderAuthentication;
    private showBedrockSetupDialog;
    private showApiKeyLoginDialog;
    private showOAuthLoginSelect;
    private showLoginDialog;
    private handleReloadCommand;
    private handleExportCommand;
    private getPathCommandArgument;
    private handleImportCommand;
    private handleShareCommand;
    private handleCopyCommand;
    private handleNameCommand;
    private handleSessionCommand;
    private handleChangelogCommand;
    /**
     * Capitalize keybinding for display (e.g., "ctrl+c" -> "Ctrl+C").
     */
    private capitalizeKey;
    /**
     * Get capitalized display string for an app keybinding action.
     */
    private getAppKeyDisplay;
    /**
     * Get capitalized display string for an editor keybinding action.
     */
    private getEditorKeyDisplay;
    private handleHotkeysCommand;
    private handleClearCommand;
    private handleDebugCommand;
    private handleArminSaysHi;
    private handleDementedDelves;
    private handleDaxnuts;
    private checkDaxnutsEasterEgg;
    private handleBashCommand;
    private handleCompactCommand;
    stop(): void;
}
```

#### InteractiveModeOptions

Kind: interface

```ts
/**
 * Options for InteractiveMode initialization.
 */
export interface InteractiveModeOptions {
    /** Providers that were migrated to auth.json (shows warning) */
    migratedProviders?: string[];
    /** Warning message if session model couldn't be restored */
    modelFallbackMessage?: string;
    /** Initial message to send on startup (can include @file content) */
    initialMessage?: string;
    /** Images to attach to the initial message */
    initialImages?: ImageContent[];
    /** Additional messages to send after the initial message */
    initialMessages?: string[];
    /** Force verbose startup (overrides quietStartup setting) */
    verbose?: boolean;
}
```

### modes/interactive/theme/theme

#### getLanguageFromPath

Kind: function

```ts
/**
 * Get language identifier from file path extension.
 */
export declare function getLanguageFromPath(filePath: string): string | undefined;
```

#### getMarkdownTheme

Kind: function

```ts
export declare function getMarkdownTheme(): MarkdownTheme;
```

#### getSelectListTheme

Kind: function

```ts
export declare function getSelectListTheme(): SelectListTheme;
```

#### getSettingsListTheme

Kind: function

```ts
export declare function getSettingsListTheme(): import("@earendil-works/pi-tui").SettingsListTheme;
```

#### highlightCode

Kind: function

```ts
/**
 * Highlight code with syntax coloring based on file extension or language.
 * Returns array of highlighted lines.
 */
export declare function highlightCode(code: string, lang?: string): string[];
```

#### initTheme

Kind: function

```ts
export declare function initTheme(themeName?: string, enableWatcher?: boolean): void;
```

#### Theme

Kind: class

```ts
export declare class Theme {
    readonly name?: string;
    readonly sourcePath?: string;
    sourceInfo?: SourceInfo;
    private fgColors;
    private bgColors;
    private mode;
    constructor(fgColors: Record<ThemeColor, string | number>, bgColors: Record<ThemeBg, string | number>, mode: ColorMode, options?: {
        name?: string;
        sourcePath?: string;
        sourceInfo?: SourceInfo;
    });
    fg(color: ThemeColor, text: string): string;
    bg(color: ThemeBg, text: string): string;
    bold(text: string): string;
    italic(text: string): string;
    underline(text: string): string;
    inverse(text: string): string;
    strikethrough(text: string): string;
    getFgAnsi(color: ThemeColor): string;
    getBgAnsi(color: ThemeBg): string;
    getColorMode(): ColorMode;
    getThinkingBorderColor(level: "off" | "minimal" | "low" | "medium" | "high" | "xhigh"): (str: string) => string;
    getBashModeBorderColor(): (str: string) => string;
}
```

#### ThemeColor

Kind: type

```ts
export type ThemeColor = "accent" | "border" | "borderAccent" | "borderMuted" | "success" | "error" | "warning" | "muted" | "dim" | "text" | "thinkingText" | "userMessageText" | "customMessageText" | "customMessageLabel" | "toolTitle" | "toolOutput" | "mdHeading" | "mdLink" | "mdLinkUrl" | "mdCode" | "mdCodeBlock" | "mdCodeBlockBorder" | "mdQuote" | "mdQuoteBorder" | "mdHr" | "mdListBullet" | "toolDiffAdded" | "toolDiffRemoved" | "toolDiffContext" | "syntaxComment" | "syntaxKeyword" | "syntaxFunction" | "syntaxVariable" | "syntaxString" | "syntaxNumber" | "syntaxType" | "syntaxOperator" | "syntaxPunctuation" | "thinkingOff" | "thinkingMinimal" | "thinkingLow" | "thinkingMedium" | "thinkingHigh" | "thinkingXhigh" | "bashMode";
```

### modes/print-mode

#### PrintModeOptions

Kind: interface

```ts
/**
 * Options for print mode.
 */
export interface PrintModeOptions {
    /** Output mode: "text" for final response only, "json" for all events */
    mode: "text" | "json";
    /** Array of additional prompts to send after initialMessage */
    messages?: string[];
    /** First message to send (may contain @file content) */
    initialMessage?: string;
    /** Images to attach to the initial message */
    initialImages?: ImageContent[];
}
```

#### runPrintMode

Kind: function

```ts
/**
 * Run in print (single-shot) mode.
 * Sends prompts to the agent and outputs the result.
 */
export declare function runPrintMode(runtimeHost: AgentSessionRuntime, options: PrintModeOptions): Promise<number>;
```

### modes/rpc/rpc-client

#### ModelInfo

Kind: interface

```ts
export interface ModelInfo {
    provider: string;
    id: string;
    contextWindow: number;
    reasoning: boolean;
}
```

#### RpcClient

Kind: class

```ts
export declare class RpcClient {
    private options;
    private process;
    private stopReadingStdout;
    private eventListeners;
    private pendingRequests;
    private requestId;
    private stderr;
    constructor(options?: RpcClientOptions);
    /**
     * Start the RPC agent process.
     */
    start(): Promise<void>;
    /**
     * Stop the RPC agent process.
     */
    stop(): Promise<void>;
    /**
     * Subscribe to agent events.
     */
    onEvent(listener: RpcEventListener): () => void;
    /**
     * Get collected stderr output (useful for debugging).
     */
    getStderr(): string;
    /**
     * Send a prompt to the agent.
     * Returns immediately after sending; use onEvent() to receive streaming events.
     * Use waitForIdle() to wait for completion.
     */
    prompt(message: string, images?: ImageContent[]): Promise<void>;
    /**
     * Queue a steering message to interrupt the agent mid-run.
     */
    steer(message: string, images?: ImageContent[]): Promise<void>;
    /**
     * Queue a follow-up message to be processed after the agent finishes.
     */
    followUp(message: string, images?: ImageContent[]): Promise<void>;
    /**
     * Abort current operation.
     */
    abort(): Promise<void>;
    /**
     * Start a new session, optionally with parent tracking.
     * @param parentSession - Optional parent session path for lineage tracking
     * @returns Object with `cancelled: true` if an extension cancelled the new session
     */
    newSession(parentSession?: string): Promise<{
        cancelled: boolean;
    }>;
    /**
     * Get current session state.
     */
    getState(): Promise<RpcSessionState>;
    /**
     * Set model by provider and ID.
     */
    setModel(provider: string, modelId: string): Promise<{
        provider: string;
        id: string;
    }>;
    /**
     * Cycle to next model.
     */
    cycleModel(): Promise<{
        model: {
            provider: string;
            id: string;
        };
        thinkingLevel: ThinkingLevel;
        isScoped: boolean;
    } | null>;
    /**
     * Get list of available models.
     */
    getAvailableModels(): Promise<ModelInfo[]>;
    /**
     * Set thinking level.
     */
    setThinkingLevel(level: ThinkingLevel): Promise<void>;
    /**
     * Cycle thinking level.
     */
    cycleThinkingLevel(): Promise<{
        level: ThinkingLevel;
    } | null>;
    /**
     * Set steering mode.
     */
    setSteeringMode(mode: "all" | "one-at-a-time"): Promise<void>;
    /**
     * Set follow-up mode.
     */
    setFollowUpMode(mode: "all" | "one-at-a-time"): Promise<void>;
    /**
     * Compact session context.
     */
    compact(customInstructions?: string): Promise<CompactionResult>;
    /**
     * Set auto-compaction enabled/disabled.
     */
    setAutoCompaction(enabled: boolean): Promise<void>;
    /**
     * Set auto-retry enabled/disabled.
     */
    setAutoRetry(enabled: boolean): Promise<void>;
    /**
     * Abort in-progress retry.
     */
    abortRetry(): Promise<void>;
    /**
     * Execute a bash command.
     */
    bash(command: string): Promise<BashResult>;
    /**
     * Abort running bash command.
     */
    abortBash(): Promise<void>;
    /**
     * Get session statistics.
     */
    getSessionStats(): Promise<SessionStats>;
    /**
     * Export session to HTML.
     */
    exportHtml(outputPath?: string): Promise<{
        path: string;
    }>;
    /**
     * Switch to a different session file.
     * @returns Object with `cancelled: true` if an extension cancelled the switch
     */
    switchSession(sessionPath: string): Promise<{
        cancelled: boolean;
    }>;
    /**
     * Fork from a specific message.
     * @returns Object with `text` (the message text) and `cancelled` (if extension cancelled)
     */
    fork(entryId: string): Promise<{
        text: string;
        cancelled: boolean;
    }>;
    /**
     * Clone the current active branch into a new session.
     * @returns Object with `cancelled: true` if an extension cancelled the clone
     */
    clone(): Promise<{
        cancelled: boolean;
    }>;
    /**
     * Get messages available for forking.
     */
    getForkMessages(): Promise<Array<{
        entryId: string;
        text: string;
    }>>;
    /**
     * Get text of last assistant message.
     */
    getLastAssistantText(): Promise<string | null>;
    /**
     * Set the session display name.
     */
    setSessionName(name: string): Promise<void>;
    /**
     * Get all messages in the session.
     */
    getMessages(): Promise<AgentMessage[]>;
    /**
     * Get available commands (extension commands, prompt templates, skills).
     */
    getCommands(): Promise<RpcSlashCommand[]>;
    /**
     * Wait for agent to become idle (no streaming).
     * Resolves when agent_end event is received.
     */
    waitForIdle(timeout?: number): Promise<void>;
    /**
     * Collect events until agent becomes idle.
     */
    collectEvents(timeout?: number): Promise<AgentEvent[]>;
    /**
     * Send prompt and wait for completion, returning all events.
     */
    promptAndWait(message: string, images?: ImageContent[], timeout?: number): Promise<AgentEvent[]>;
    private handleLine;
    private send;
    private getData;
}
```

#### RpcClientOptions

Kind: interface

```ts
export interface RpcClientOptions {
    /** Path to the CLI entry point (default: searches for dist/cli.js) */
    cliPath?: string;
    /** Working directory for the agent */
    cwd?: string;
    /** Environment variables */
    env?: Record<string, string>;
    /** Provider to use */
    provider?: string;
    /** Model ID to use */
    model?: string;
    /** Additional CLI arguments */
    args?: string[];
}
```

#### RpcEventListener

Kind: type

```ts
export type RpcEventListener = (event: AgentEvent) => void;
```

### modes/rpc/rpc-mode

#### runRpcMode

Kind: function

```ts
/**
 * Run in RPC mode.
 * Listens for JSON commands on stdin, outputs events and responses on stdout.
 */
export declare function runRpcMode(runtimeHost: AgentSessionRuntime): Promise<never>;
```

### modes/rpc/rpc-types

#### RpcCommand

Kind: type

```ts
export type RpcCommand = {
    id?: string;
    type: "prompt";
    message: string;
    images?: ImageContent[];
    streamingBehavior?: "steer" | "followUp";
} | {
    id?: string;
    type: "steer";
    message: string;
    images?: ImageContent[];
} | {
    id?: string;
    type: "follow_up";
    message: string;
    images?: ImageContent[];
} | {
    id?: string;
    type: "abort";
} | {
    id?: string;
    type: "new_session";
    parentSession?: string;
} | {
    id?: string;
    type: "get_state";
} | {
    id?: string;
    type: "set_model";
    provider: string;
    modelId: string;
} | {
    id?: string;
    type: "cycle_model";
} | {
    id?: string;
    type: "get_available_models";
} | {
    id?: string;
    type: "set_thinking_level";
    level: ThinkingLevel;
} | {
    id?: string;
    type: "cycle_thinking_level";
} | {
    id?: string;
    type: "set_steering_mode";
    mode: "all" | "one-at-a-time";
} | {
    id?: string;
    type: "set_follow_up_mode";
    mode: "all" | "one-at-a-time";
} | {
    id?: string;
    type: "compact";
    customInstructions?: string;
} | {
    id?: string;
    type: "set_auto_compaction";
    enabled: boolean;
} | {
    id?: string;
    type: "set_auto_retry";
    enabled: boolean;
} | {
    id?: string;
    type: "abort_retry";
} | {
    id?: string;
    type: "bash";
    command: string;
} | {
    id?: string;
    type: "abort_bash";
} | {
    id?: string;
    type: "get_session_stats";
} | {
    id?: string;
    type: "export_html";
    outputPath?: string;
} | {
    id?: string;
    type: "switch_session";
    sessionPath: string;
} | {
    id?: string;
    type: "fork";
    entryId: string;
} | {
    id?: string;
    type: "clone";
} | {
    id?: string;
    type: "get_fork_messages";
} | {
    id?: string;
    type: "get_last_assistant_text";
} | {
    id?: string;
    type: "set_session_name";
    name: string;
} | {
    id?: string;
    type: "get_messages";
} | {
    id?: string;
    type: "get_commands";
};
```

#### RpcResponse

Kind: type

```ts
export type RpcResponse = {
    id?: string;
    type: "response";
    command: "prompt";
    success: true;
} | {
    id?: string;
    type: "response";
    command: "steer";
    success: true;
} | {
    id?: string;
    type: "response";
    command: "follow_up";
    success: true;
} | {
    id?: string;
    type: "response";
    command: "abort";
    success: true;
} | {
    id?: string;
    type: "response";
    command: "new_session";
    success: true;
    data: {
        cancelled: boolean;
    };
} | {
    id?: string;
    type: "response";
    command: "get_state";
    success: true;
    data: RpcSessionState;
} | {
    id?: string;
    type: "response";
    command: "set_model";
    success: true;
    data: Model<any>;
} | {
    id?: string;
    type: "response";
    command: "cycle_model";
    success: true;
    data: {
        model: Model<any>;
        thinkingLevel: ThinkingLevel;
        isScoped: boolean;
    } | null;
} | {
    id?: string;
    type: "response";
    command: "get_available_models";
    success: true;
    data: {
        models: Model<any>[];
    };
} | {
    id?: string;
    type: "response";
    command: "set_thinking_level";
    success: true;
} | {
    id?: string;
    type: "response";
    command: "cycle_thinking_level";
    success: true;
    data: {
        level: ThinkingLevel;
    } | null;
} | {
    id?: string;
    type: "response";
    command: "set_steering_mode";
    success: true;
} | {
    id?: string;
    type: "response";
    command: "set_follow_up_mode";
    success: true;
} | {
    id?: string;
    type: "response";
    command: "compact";
    success: true;
    data: CompactionResult;
} | {
    id?: string;
    type: "response";
    command: "set_auto_compaction";
    success: true;
} | {
    id?: string;
    type: "response";
    command: "set_auto_retry";
    success: true;
} | {
    id?: string;
    type: "response";
    command: "abort_retry";
    success: true;
} | {
    id?: string;
    type: "response";
    command: "bash";
    success: true;
    data: BashResult;
} | {
    id?: string;
    type: "response";
    command: "abort_bash";
    success: true;
} | {
    id?: string;
    type: "response";
    command: "get_session_stats";
    success: true;
    data: SessionStats;
} | {
    id?: string;
    type: "response";
    command: "export_html";
    success: true;
    data: {
        path: string;
    };
} | {
    id?: string;
    type: "response";
    command: "switch_session";
    success: true;
    data: {
        cancelled: boolean;
    };
} | {
    id?: string;
    type: "response";
    command: "fork";
    success: true;
    data: {
        text: string;
        cancelled: boolean;
    };
} | {
    id?: string;
    type: "response";
    command: "clone";
    success: true;
    data: {
        cancelled: boolean;
    };
} | {
    id?: string;
    type: "response";
    command: "get_fork_messages";
    success: true;
    data: {
        messages: Array<{
            entryId: string;
            text: string;
        }>;
    };
} | {
    id?: string;
    type: "response";
    command: "get_last_assistant_text";
    success: true;
    data: {
        text: string | null;
    };
} | {
    id?: string;
    type: "response";
    command: "set_session_name";
    success: true;
} | {
    id?: string;
    type: "response";
    command: "get_messages";
    success: true;
    data: {
        messages: AgentMessage[];
    };
} | {
    id?: string;
    type: "response";
    command: "get_commands";
    success: true;
    data: {
        commands: RpcSlashCommand[];
    };
} | {
    id?: string;
    type: "response";
    command: string;
    success: false;
    error: string;
};
```

#### RpcSessionState

Kind: interface

```ts
export interface RpcSessionState {
    model?: Model<any>;
    thinkingLevel: ThinkingLevel;
    isStreaming: boolean;
    isCompacting: boolean;
    steeringMode: "all" | "one-at-a-time";
    followUpMode: "all" | "one-at-a-time";
    sessionFile?: string;
    sessionId: string;
    sessionName?: string;
    autoCompactionEnabled: boolean;
    messageCount: number;
    pendingMessageCount: number;
}
```

### utils/clipboard

#### copyToClipboard

Kind: function

```ts
export declare function copyToClipboard(text: string): Promise<void>;
```

### utils/frontmatter

#### parseFrontmatter

Kind: const

```ts
export declare const parseFrontmatter: <T extends Record<string, unknown> = Record<string, unknown>>(content: string) => ParsedFrontmatter<T>;
```

#### stripFrontmatter

Kind: const

```ts
export declare const stripFrontmatter: (content: string) => string;
```

### utils/shell

#### getShellConfig

Kind: function

```ts
/**
 * Resolve shell configuration based on platform and an optional explicit shell path.
 * Resolution order:
 * 1. User-specified shellPath
 * 2. On Windows: Git Bash in known locations, then bash on PATH
 * 3. On Unix: /bin/bash, then bash on PATH, then fallback to sh
 */
export declare function getShellConfig(customShellPath?: string): ShellConfig;
```

## @earendil-works/pi-tui

### autocomplete

#### AutocompleteItem

Kind: interface

```ts
export interface AutocompleteItem {
    value: string;
    label: string;
    description?: string;
}
```

#### AutocompleteProvider

Kind: interface

```ts
export interface AutocompleteProvider {
    getSuggestions(lines: string[], cursorLine: number, cursorCol: number, options: {
        signal: AbortSignal;
        force?: boolean;
    }): Promise<AutocompleteSuggestions | null>;
    applyCompletion(lines: string[], cursorLine: number, cursorCol: number, item: AutocompleteItem, prefix: string): {
        lines: string[];
        cursorLine: number;
        cursorCol: number;
    };
    shouldTriggerFileCompletion?(lines: string[], cursorLine: number, cursorCol: number): boolean;
}
```

#### AutocompleteSuggestions

Kind: interface

```ts
export interface AutocompleteSuggestions {
    items: AutocompleteItem[];
    prefix: string;
}
```

#### CombinedAutocompleteProvider

Kind: class

```ts
export declare class CombinedAutocompleteProvider implements AutocompleteProvider {
    private commands;
    private basePath;
    private fdPath;
    constructor(commands: (AutocompleteItem | SlashCommand)[] | undefined, basePath: string, fdPath?: string | null);
    getSuggestions(lines: string[], cursorLine: number, cursorCol: number, options: {
        signal: AbortSignal;
        force?: boolean;
    }): Promise<AutocompleteSuggestions | null>;
    applyCompletion(lines: string[], cursorLine: number, cursorCol: number, item: AutocompleteItem, prefix: string): {
        lines: string[];
        cursorLine: number;
        cursorCol: number;
    };
    private extractAtPrefix;
    private extractPathPrefix;
    private expandHomePath;
    private resolveScopedFuzzyQuery;
    private scopedPathForDisplay;
    private getFileSuggestions;
    private scoreEntry;
    private getFuzzyFileSuggestions;
    shouldTriggerFileCompletion(lines: string[], cursorLine: number, cursorCol: number): boolean;
}
```

#### SlashCommand

Kind: interface

```ts
export interface SlashCommand {
    name: string;
    description?: string;
    argumentHint?: string;
    getArgumentCompletions?(argumentPrefix: string): Awaitable<AutocompleteItem[] | null>;
}
```

### components/box

#### Box

Kind: class

```ts
/**
 * Box component - a container that applies padding and background to all children
 */
export declare class Box implements Component {
    children: Component[];
    private paddingX;
    private paddingY;
    private bgFn?;
    private cache?;
    constructor(paddingX?: number, paddingY?: number, bgFn?: (text: string) => string);
    addChild(component: Component): void;
    removeChild(component: Component): void;
    clear(): void;
    setBgFn(bgFn?: (text: string) => string): void;
    private invalidateCache;
    private matchCache;
    invalidate(): void;
    render(width: number): string[];
    private applyBg;
}
```

### components/cancellable-loader

#### CancellableLoader

Kind: class

```ts
/**
 * Loader that can be cancelled with Escape.
 * Extends Loader with an AbortSignal for cancelling async operations.
 *
 * @example
 * const loader = new CancellableLoader(tui, cyan, dim, "Working...");
 * loader.onAbort = () => done(null);
 * doWork(loader.signal).then(done);
 */
export declare class CancellableLoader extends Loader {
    private abortController;
    /** Called when user presses Escape */
    onAbort?: () => void;
    /** AbortSignal that is aborted when user presses Escape */
    get signal(): AbortSignal;
    /** Whether the loader was aborted */
    get aborted(): boolean;
    handleInput(data: string): void;
    dispose(): void;
}
```

### components/editor

#### Editor

Kind: class

```ts
export declare class Editor implements Component, Focusable {
    private state;
    /** Focusable interface - set by TUI when focus changes */
    focused: boolean;
    protected tui: TUI;
    private theme;
    private paddingX;
    private lastWidth;
    private scrollOffset;
    borderColor: (str: string) => string;
    private autocompleteProvider?;
    private autocompleteList?;
    private autocompleteState;
    private autocompletePrefix;
    private autocompleteMaxVisible;
    private autocompleteAbort?;
    private autocompleteDebounceTimer?;
    private autocompleteRequestTask;
    private autocompleteStartToken;
    private autocompleteRequestId;
    private pastes;
    private pasteCounter;
    private pasteBuffer;
    private isInPaste;
    private history;
    private historyIndex;
    private killRing;
    private lastAction;
    private jumpMode;
    private preferredVisualCol;
    private snappedFromCursorCol;
    private undoStack;
    onSubmit?: (text: string) => void;
    onChange?: (text: string) => void;
    disableSubmit: boolean;
    constructor(tui: TUI, theme: EditorTheme, options?: EditorOptions);
    /** Set of currently valid paste IDs, for marker-aware segmentation. */
    private validPasteIds;
    /** Segment text with paste-marker awareness, only merging markers with valid IDs. */
    private segment;
    getPaddingX(): number;
    setPaddingX(padding: number): void;
    getAutocompleteMaxVisible(): number;
    setAutocompleteMaxVisible(maxVisible: number): void;
    setAutocompleteProvider(provider: AutocompleteProvider): void;
    /**
     * Add a prompt to history for up/down arrow navigation.
     * Called after successful submission.
     */
    addToHistory(text: string): void;
    private isEditorEmpty;
    private isOnFirstVisualLine;
    private isOnLastVisualLine;
    private navigateHistory;
    /** Internal setText that doesn't reset history state - used by navigateHistory */
    private setTextInternal;
    invalidate(): void;
    render(width: number): string[];
    handleInput(data: string): void;
    private layoutText;
    getText(): string;
    private expandPasteMarkers;
    /**
     * Get text with paste markers expanded to their actual content.
     * Use this when you need the full content (e.g., for external editor).
     */
    getExpandedText(): string;
    getLines(): string[];
    getCursor(): {
        line: number;
        col: number;
    };
    setText(text: string): void;
    /**
     * Insert text at the current cursor position.
     * Used for programmatic insertion (e.g., clipboard image markers).
     * This is atomic for undo - single undo restores entire pre-insert state.
     */
    insertTextAtCursor(text: string): void;
    /**
     * Normalize text for editor storage:
     * - Normalize line endings (\r\n and \r -> \n)
     * - Expand tabs to 4 spaces
     */
    private normalizeText;
    /**
     * Internal text insertion at cursor. Handles single and multi-line text.
     * Does not push undo snapshots or trigger autocomplete - caller is responsible.
     * Normalizes line endings and calls onChange once at the end.
     */
    private insertTextAtCursorInternal;
    private insertCharacter;
    private handlePaste;
    private addNewLine;
    private shouldSubmitOnBackslashEnter;
    private submitValue;
    private handleBackspace;
    /**
     * Set cursor column and clear preferredVisualCol.
     * Use this for all non-vertical cursor movements to reset sticky column behavior.
     */
    private setCursorCol;
    /**
     * Move cursor to a target visual line, applying sticky column logic.
     * Shared by moveCursor() and pageScroll().
     */
    private moveToVisualLine;
    /**
     * Compute the target visual column for vertical cursor movement.
     * Implements the sticky column decision table:
     *
     * | P | S | T | U | Scenario                                             | Set Preferred | Move To     |
     * |---|---|---|---| ---------------------------------------------------- |---------------|-------------|
     * | 0 | * | 0 | - | Start nav, target fits                               | null          | current     |
     * | 0 | * | 1 | - | Start nav, target shorter                            | current       | target end  |
     * | 1 | 0 | 0 | 0 | Clamped, target fits preferred                       | null          | preferred   |
     * | 1 | 0 | 0 | 1 | Clamped, target longer but still can't fit preferred | keep          | target end  |
     * | 1 | 0 | 1 | - | Clamped, target even shorter                         | keep          | target end  |
     * | 1 | 1 | 0 | - | Rewrapped, target fits current                       | null          | current     |
     * | 1 | 1 | 1 | - | Rewrapped, target shorter than current               | current       | target end  |
     *
     * Where:
     * - P = preferred col is set
     * - S = cursor in middle of source line (not clamped to end)
     * - T = target line shorter than current visual col
     * - U = target line shorter than preferred col
     */
    private computeVerticalMoveColumn;
    private moveToLineStart;
    private moveToLineEnd;
    private deleteToStartOfLine;
    private deleteToEndOfLine;
    private deleteWordBackwards;
    private deleteWordForward;
    private handleForwardDelete;
    /**
     * Build a mapping from visual lines to logical positions.
     * Returns an array where each element represents a visual line with:
     * - logicalLine: index into this.state.lines
     * - startCol: starting column in the logical line
     * - length: length of this visual line segment
     */
    private buildVisualLineMap;
    /**
     * Find the visual line index that contains the given logical position.
     */
    private findVisualLineAt;
    /**
     * Find the visual line index for the current cursor position.
     */
    private findCurrentVisualLine;
    private moveCursor;
    /**
     * Scroll by a page (direction: -1 for up, 1 for down).
     * Moves cursor by the page size while keeping it in bounds.
     */
    private pageScroll;
    private moveWordBackwards;
    /**
     * Yank (paste) the most recent kill ring entry at cursor position.
     */
    private yank;
    /**
     * Cycle through kill ring (only works immediately after yank or yank-pop).
     * Replaces the last yanked text with the previous entry in the ring.
     */
    private yankPop;
    /**
     * Insert text at cursor position (used by yank operations).
     */
    private insertYankedText;
    /**
     * Delete the previously yanked text (used by yank-pop).
     * The yanked text is derived from killRing[end] since it hasn't been rotated yet.
     */
    private deleteYankedText;
    private pushUndoSnapshot;
    private undo;
    /**
     * Jump to the first occurrence of a character in the specified direction.
     * Multi-line search. Case-sensitive. Skips the current cursor position.
     */
    private jumpToChar;
    private moveWordForwards;
    private isSlashMenuAllowed;
    private isAtStartOfMessage;
    private isInSlashCommandContext;
    /**
     * Find the best autocomplete item index for the given prefix.
     * Returns -1 if no match is found.
     *
     * Match priority:
     * 1. Exact match (prefix === item.value) -> always selected
     * 2. Prefix match -> first item whose value starts with prefix
     * 3. No match -> -1 (keep default highlight)
     *
     * Matching is case-sensitive and checks item.value only.
     */
    private getBestAutocompleteMatchIndex;
    private createAutocompleteList;
    private tryTriggerAutocomplete;
    private handleTabCompletion;
    private handleSlashCommandCompletion;
    private forceFileAutocomplete;
    private requestAutocomplete;
    private startAutocompleteRequest;
    private getAutocompleteDebounceMs;
    private runAutocompleteRequest;
    private isAutocompleteRequestCurrent;
    private applyAutocompleteSuggestions;
    private cancelAutocompleteRequest;
    private clearAutocompleteUi;
    private cancelAutocomplete;
    isShowingAutocomplete(): boolean;
    private updateAutocomplete;
}
```

#### EditorOptions

Kind: interface

```ts
export interface EditorOptions {
    paddingX?: number;
    autocompleteMaxVisible?: number;
}
```

#### EditorTheme

Kind: interface

```ts
export interface EditorTheme {
    borderColor: (str: string) => string;
    selectList: SelectListTheme;
}
```

### components/image

#### Image

Kind: class

```ts
export declare class Image implements Component {
    private base64Data;
    private mimeType;
    private dimensions;
    private theme;
    private options;
    private imageId?;
    private cachedLines?;
    private cachedWidth?;
    constructor(base64Data: string, mimeType: string, theme: ImageTheme, options?: ImageOptions, dimensions?: ImageDimensions);
    /** Get the Kitty image ID used by this image (if any). */
    getImageId(): number | undefined;
    invalidate(): void;
    render(width: number): string[];
}
```

#### ImageOptions

Kind: interface

```ts
export interface ImageOptions {
    maxWidthCells?: number;
    maxHeightCells?: number;
    filename?: string;
    /** Kitty image ID. If provided, reuses this ID (for animations/updates). */
    imageId?: number;
}
```

#### ImageTheme

Kind: interface

```ts
export interface ImageTheme {
    fallbackColor: (str: string) => string;
}
```

### components/input

#### Input

Kind: class

```ts
/**
 * Input component - single-line text input with horizontal scrolling
 */
export declare class Input implements Component, Focusable {
    private value;
    private cursor;
    onSubmit?: (value: string) => void;
    onEscape?: () => void;
    /** Focusable interface - set by TUI when focus changes */
    focused: boolean;
    private pasteBuffer;
    private isInPaste;
    private killRing;
    private lastAction;
    private undoStack;
    getValue(): string;
    setValue(value: string): void;
    handleInput(data: string): void;
    private insertCharacter;
    private handleBackspace;
    private handleForwardDelete;
    private deleteToLineStart;
    private deleteToLineEnd;
    private deleteWordBackwards;
    private deleteWordForward;
    private yank;
    private yankPop;
    private pushUndo;
    private undo;
    private moveWordBackwards;
    private moveWordForwards;
    private handlePaste;
    invalidate(): void;
    render(width: number): string[];
}
```

### components/loader

#### Loader

Kind: class

```ts
/**
 * Loader component that updates with an optional spinning animation.
 */
export declare class Loader extends Text {
    private spinnerColorFn;
    private messageColorFn;
    private message;
    private frames;
    private intervalMs;
    private currentFrame;
    private intervalId;
    private ui;
    private renderIndicatorVerbatim;
    constructor(ui: TUI, spinnerColorFn: (str: string) => string, messageColorFn: (str: string) => string, message?: string, indicator?: LoaderIndicatorOptions);
    render(width: number): string[];
    start(): void;
    stop(): void;
    setMessage(message: string): void;
    setIndicator(indicator?: LoaderIndicatorOptions): void;
    private restartAnimation;
    private updateDisplay;
}
```

#### LoaderIndicatorOptions

Kind: interface

```ts
export interface LoaderIndicatorOptions {
    /** Animation frames. Use an empty array to hide the indicator. */
    frames?: string[];
    /** Frame interval in milliseconds for animated indicators. */
    intervalMs?: number;
}
```

### components/markdown

#### DefaultTextStyle

Kind: interface

```ts
/**
 * Default text styling for markdown content.
 * Applied to all text unless overridden by markdown formatting.
 */
export interface DefaultTextStyle {
    /** Foreground color function */
    color?: (text: string) => string;
    /** Background color function */
    bgColor?: (text: string) => string;
    /** Bold text */
    bold?: boolean;
    /** Italic text */
    italic?: boolean;
    /** Strikethrough text */
    strikethrough?: boolean;
    /** Underline text */
    underline?: boolean;
}
```

#### Markdown

Kind: class

```ts
export declare class Markdown implements Component {
    private text;
    private paddingX;
    private paddingY;
    private defaultTextStyle?;
    private theme;
    private defaultStylePrefix?;
    private cachedText?;
    private cachedWidth?;
    private cachedLines?;
    constructor(text: string, paddingX: number, paddingY: number, theme: MarkdownTheme, defaultTextStyle?: DefaultTextStyle);
    setText(text: string): void;
    invalidate(): void;
    render(width: number): string[];
    /**
     * Apply default text style to a string.
     * This is the base styling applied to all text content.
     * NOTE: Background color is NOT applied here - it's applied at the padding stage
     * to ensure it extends to the full line width.
     */
    private applyDefaultStyle;
    private getDefaultStylePrefix;
    private getStylePrefix;
    private getDefaultInlineStyleContext;
    private renderToken;
    private renderInlineTokens;
    /**
     * Render a list with proper nesting support
     */
    private renderList;
    /**
     * Render list item tokens, handling nested lists
     * Returns lines WITHOUT the parent indent (renderList will add it)
     */
    private renderListItem;
    /**
     * Get the visible width of the longest word in a string.
     */
    private getLongestWordWidth;
    /**
     * Wrap a table cell to fit into a column.
     *
     * Delegates to wrapTextWithAnsi() so ANSI codes + long tokens are handled
     * consistently with the rest of the renderer.
     */
    private wrapCellText;
    /**
     * Render a table with width-aware cell wrapping.
     * Cells that don't fit are wrapped to multiple lines.
     */
    private renderTable;
}
```

#### MarkdownTheme

Kind: interface

```ts
/**
 * Theme functions for markdown elements.
 * Each function takes text and returns styled text with ANSI codes.
 */
export interface MarkdownTheme {
    heading: (text: string) => string;
    link: (text: string) => string;
    linkUrl: (text: string) => string;
    code: (text: string) => string;
    codeBlock: (text: string) => string;
    codeBlockBorder: (text: string) => string;
    quote: (text: string) => string;
    quoteBorder: (text: string) => string;
    hr: (text: string) => string;
    listBullet: (text: string) => string;
    bold: (text: string) => string;
    italic: (text: string) => string;
    strikethrough: (text: string) => string;
    underline: (text: string) => string;
    highlightCode?: (code: string, lang?: string) => string[];
    /** Prefix applied to each rendered code block line (default: "  ") */
    codeBlockIndent?: string;
}
```

### components/select-list

#### SelectItem

Kind: interface

```ts
export interface SelectItem {
    value: string;
    label: string;
    description?: string;
}
```

#### SelectList

Kind: class

```ts
export declare class SelectList implements Component {
    private items;
    private filteredItems;
    private selectedIndex;
    private maxVisible;
    private theme;
    private layout;
    onSelect?: (item: SelectItem) => void;
    onCancel?: () => void;
    onSelectionChange?: (item: SelectItem) => void;
    constructor(items: SelectItem[], maxVisible: number, theme: SelectListTheme, layout?: SelectListLayoutOptions);
    setFilter(filter: string): void;
    setSelectedIndex(index: number): void;
    invalidate(): void;
    render(width: number): string[];
    handleInput(keyData: string): void;
    private renderItem;
    private getPrimaryColumnWidth;
    private getPrimaryColumnBounds;
    private truncatePrimary;
    private getDisplayValue;
    private notifySelectionChange;
    getSelectedItem(): SelectItem | null;
}
```

#### SelectListLayoutOptions

Kind: interface

```ts
export interface SelectListLayoutOptions {
    minPrimaryColumnWidth?: number;
    maxPrimaryColumnWidth?: number;
    truncatePrimary?: (context: SelectListTruncatePrimaryContext) => string;
}
```

#### SelectListTheme

Kind: interface

```ts
export interface SelectListTheme {
    selectedPrefix: (text: string) => string;
    selectedText: (text: string) => string;
    description: (text: string) => string;
    scrollInfo: (text: string) => string;
    noMatch: (text: string) => string;
}
```

#### SelectListTruncatePrimaryContext

Kind: interface

```ts
export interface SelectListTruncatePrimaryContext {
    text: string;
    maxWidth: number;
    columnWidth: number;
    item: SelectItem;
    isSelected: boolean;
}
```

### components/settings-list

#### SettingItem

Kind: interface

```ts
export interface SettingItem {
    /** Unique identifier for this setting */
    id: string;
    /** Display label (left side) */
    label: string;
    /** Optional description shown when selected */
    description?: string;
    /** Current value to display (right side) */
    currentValue: string;
    /** If provided, Enter/Space cycles through these values */
    values?: string[];
    /** If provided, Enter opens this submenu. Receives current value and done callback. */
    submenu?: (currentValue: string, done: (selectedValue?: string) => void) => Component;
}
```

#### SettingsList

Kind: class

```ts
export declare class SettingsList implements Component {
    private items;
    private filteredItems;
    private theme;
    private selectedIndex;
    private maxVisible;
    private onChange;
    private onCancel;
    private searchInput?;
    private searchEnabled;
    private submenuComponent;
    private submenuItemIndex;
    constructor(items: SettingItem[], maxVisible: number, theme: SettingsListTheme, onChange: (id: string, newValue: string) => void, onCancel: () => void, options?: SettingsListOptions);
    /** Update an item's currentValue */
    updateValue(id: string, newValue: string): void;
    invalidate(): void;
    render(width: number): string[];
    private renderMainList;
    handleInput(data: string): void;
    private activateItem;
    private closeSubmenu;
    private applyFilter;
    private addHintLine;
}
```

#### SettingsListTheme

Kind: interface

```ts
export interface SettingsListTheme {
    label: (text: string, selected: boolean) => string;
    value: (text: string, selected: boolean) => string;
    description: (text: string) => string;
    cursor: string;
    hint: (text: string) => string;
}
```

### components/spacer

#### Spacer

Kind: class

```ts
/**
 * Spacer component that renders empty lines
 */
export declare class Spacer implements Component {
    private lines;
    constructor(lines?: number);
    setLines(lines: number): void;
    invalidate(): void;
    render(_width: number): string[];
}
```

### components/text

#### Text

Kind: class

```ts
/**
 * Text component - displays multi-line text with word wrapping
 */
export declare class Text implements Component {
    private text;
    private paddingX;
    private paddingY;
    private customBgFn?;
    private cachedText?;
    private cachedWidth?;
    private cachedLines?;
    constructor(text?: string, paddingX?: number, paddingY?: number, customBgFn?: (text: string) => string);
    setText(text: string): void;
    setCustomBgFn(customBgFn?: (text: string) => string): void;
    invalidate(): void;
    render(width: number): string[];
}
```

### components/truncated-text

#### TruncatedText

Kind: class

```ts
/**
 * Text component that truncates to fit viewport width
 */
export declare class TruncatedText implements Component {
    private text;
    private paddingX;
    private paddingY;
    constructor(text: string, paddingX?: number, paddingY?: number);
    invalidate(): void;
    render(width: number): string[];
}
```

### editor-component

#### EditorComponent

Kind: interface

```ts
/**
 * Interface for custom editor components.
 *
 * This allows extensions to provide their own editor implementation
 * (e.g., vim mode, emacs mode, custom keybindings) while maintaining
 * compatibility with the core application.
 */
export interface EditorComponent extends Component {
    /** Get the current text content */
    getText(): string;
    /** Set the text content */
    setText(text: string): void;
    /** Handle raw terminal input (key presses, paste sequences, etc.) */
    handleInput(data: string): void;
    /** Called when user submits (e.g., Enter key) */
    onSubmit?: (text: string) => void;
    /** Called when text changes */
    onChange?: (text: string) => void;
    /** Add text to history for up/down navigation */
    addToHistory?(text: string): void;
    /** Insert text at current cursor position */
    insertTextAtCursor?(text: string): void;
    /**
     * Get text with any markers expanded (e.g., paste markers).
     * Falls back to getText() if not implemented.
     */
    getExpandedText?(): string;
    /** Set the autocomplete provider */
    setAutocompleteProvider?(provider: AutocompleteProvider): void;
    /** Border color function */
    borderColor?: (str: string) => string;
    /** Set horizontal padding */
    setPaddingX?(padding: number): void;
    /** Set max visible items in autocomplete dropdown */
    setAutocompleteMaxVisible?(maxVisible: number): void;
}
```

### fuzzy

#### fuzzyFilter

Kind: function

```ts
/**
 * Filter and sort items by fuzzy match quality (best matches first).
 * Supports space-separated tokens: all tokens must match.
 */
export declare function fuzzyFilter<T>(items: T[], query: string, getText: (item: T) => string): T[];
```

#### fuzzyMatch

Kind: function

```ts
export declare function fuzzyMatch(query: string, text: string): FuzzyMatch;
```

#### FuzzyMatch

Kind: interface

```ts
/**
 * Fuzzy matching utilities.
 * Matches if all query characters appear in order (not necessarily consecutive).
 * Lower score = better match.
 */
export interface FuzzyMatch {
    matches: boolean;
    score: number;
}
```

### keybindings

#### getKeybindings

Kind: function

```ts
export declare function getKeybindings(): KeybindingsManager;
```

#### Keybinding

Kind: type

```ts
export type Keybinding = keyof Keybindings;
```

#### KeybindingConflict

Kind: interface

```ts
export interface KeybindingConflict {
    key: KeyId;
    keybindings: string[];
}
```

#### KeybindingDefinition

Kind: interface

```ts
export interface KeybindingDefinition {
    defaultKeys: KeyId | KeyId[];
    description?: string;
}
```

#### KeybindingDefinitions

Kind: type

```ts
export type KeybindingDefinitions = Record<string, KeybindingDefinition>;
```

#### Keybindings

Kind: interface

```ts
/**
 * Global keybinding registry.
 * Downstream packages can add keybindings via declaration merging.
 */
export interface Keybindings {
    "tui.editor.cursorUp": true;
    "tui.editor.cursorDown": true;
    "tui.editor.cursorLeft": true;
    "tui.editor.cursorRight": true;
    "tui.editor.cursorWordLeft": true;
    "tui.editor.cursorWordRight": true;
    "tui.editor.cursorLineStart": true;
    "tui.editor.cursorLineEnd": true;
    "tui.editor.jumpForward": true;
    "tui.editor.jumpBackward": true;
    "tui.editor.pageUp": true;
    "tui.editor.pageDown": true;
    "tui.editor.deleteCharBackward": true;
    "tui.editor.deleteCharForward": true;
    "tui.editor.deleteWordBackward": true;
    "tui.editor.deleteWordForward": true;
    "tui.editor.deleteToLineStart": true;
    "tui.editor.deleteToLineEnd": true;
    "tui.editor.yank": true;
    "tui.editor.yankPop": true;
    "tui.editor.undo": true;
    "tui.input.newLine": true;
    "tui.input.submit": true;
    "tui.input.tab": true;
    "tui.input.copy": true;
    "tui.select.up": true;
    "tui.select.down": true;
    "tui.select.pageUp": true;
    "tui.select.pageDown": true;
    "tui.select.confirm": true;
    "tui.select.cancel": true;
}
```

#### KeybindingsConfig

Kind: type

```ts
export type KeybindingsConfig = Record<string, KeyId | KeyId[] | undefined>;
```

#### KeybindingsManager

Kind: class

```ts
export declare class KeybindingsManager {
    private definitions;
    private userBindings;
    private keysById;
    private conflicts;
    constructor(definitions: KeybindingDefinitions, userBindings?: KeybindingsConfig);
    private rebuild;
    matches(data: string, keybinding: Keybinding): boolean;
    getKeys(keybinding: Keybinding): KeyId[];
    getDefinition(keybinding: Keybinding): KeybindingDefinition;
    getConflicts(): KeybindingConflict[];
    setUserBindings(userBindings: KeybindingsConfig): void;
    getUserBindings(): KeybindingsConfig;
    getResolvedBindings(): KeybindingsConfig;
}
```

#### setKeybindings

Kind: function

```ts
export declare function setKeybindings(keybindings: KeybindingsManager): void;
```

#### TUI_KEYBINDINGS

Kind: const

```ts
export declare const TUI_KEYBINDINGS: {
    readonly "tui.editor.cursorUp": {
        readonly defaultKeys: "up";
        readonly description: "Move cursor up";
    };
    readonly "tui.editor.cursorDown": {
        readonly defaultKeys: "down";
        readonly description: "Move cursor down";
    };
    readonly "tui.editor.cursorLeft": {
        readonly defaultKeys: ["left", "ctrl+b"];
        readonly description: "Move cursor left";
    };
    readonly "tui.editor.cursorRight": {
        readonly defaultKeys: ["right", "ctrl+f"];
        readonly description: "Move cursor right";
    };
    readonly "tui.editor.cursorWordLeft": {
        readonly defaultKeys: ["alt+left", "ctrl+left", "alt+b"];
        readonly description: "Move cursor word left";
    };
    readonly "tui.editor.cursorWordRight": {
        readonly defaultKeys: ["alt+right", "ctrl+right", "alt+f"];
        readonly description: "Move cursor word right";
    };
    readonly "tui.editor.cursorLineStart": {
        readonly defaultKeys: ["home", "ctrl+a"];
        readonly description: "Move to line start";
    };
    readonly "tui.editor.cursorLineEnd": {
        readonly defaultKeys: ["end", "ctrl+e"];
        readonly description: "Move to line end";
    };
    readonly "tui.editor.jumpForward": {
        readonly defaultKeys: "ctrl+]";
        readonly description: "Jump forward to character";
    };
    readonly "tui.editor.jumpBackward": {
        readonly defaultKeys: "ctrl+alt+]";
        readonly description: "Jump backward to character";
    };
    readonly "tui.editor.pageUp": {
        readonly defaultKeys: "pageUp";
        readonly description: "Page up";
    };
    readonly "tui.editor.pageDown": {
        readonly defaultKeys: "pageDown";
        readonly description: "Page down";
    };
    readonly "tui.editor.deleteCharBackward": {
        readonly defaultKeys: "backspace";
        readonly description: "Delete character backward";
    };
    readonly "tui.editor.deleteCharForward": {
        readonly defaultKeys: ["delete", "ctrl+d"];
        readonly description: "Delete character forward";
    };
    readonly "tui.editor.deleteWordBackward": {
        readonly defaultKeys: ["ctrl+w", "alt+backspace"];
        readonly description: "Delete word backward";
    };
    readonly "tui.editor.deleteWordForward": {
        readonly defaultKeys: ["alt+d", "alt+delete"];
        readonly description: "Delete word forward";
    };
    readonly "tui.editor.deleteToLineStart": {
        readonly defaultKeys: "ctrl+u";
        readonly description: "Delete to line start";
    };
    readonly "tui.editor.deleteToLineEnd": {
        readonly defaultKeys: "ctrl+k";
        readonly description: "Delete to line end";
    };
    readonly "tui.editor.yank": {
        readonly defaultKeys: "ctrl+y";
        readonly description: "Yank";
    };
    readonly "tui.editor.yankPop": {
        readonly defaultKeys: "alt+y";
        readonly description: "Yank pop";
    };
    readonly "tui.editor.undo": {
        readonly defaultKeys: "ctrl+-";
        readonly description: "Undo";
    };
    readonly "tui.input.newLine": {
        readonly defaultKeys: "shift+enter";
        readonly description: "Insert newline";
    };
    readonly "tui.input.submit": {
        readonly defaultKeys: "enter";
        readonly description: "Submit input";
    };
    readonly "tui.input.tab": {
        readonly defaultKeys: "tab";
        readonly description: "Tab / autocomplete";
    };
    readonly "tui.input.copy": {
        readonly defaultKeys: "ctrl+c";
        readonly description: "Copy selection";
    };
    readonly "tui.select.up": {
        readonly defaultKeys: "up";
        readonly description: "Move selection up";
    };
    readonly "tui.select.down": {
        readonly defaultKeys: "down";
        readonly description: "Move selection down";
    };
    readonly "tui.select.pageUp": {
        readonly defaultKeys: "pageUp";
        readonly description: "Selection page up";
    };
    readonly "tui.select.pageDown": {
        readonly defaultKeys: "pageDown";
        readonly description: "Selection page down";
    };
    readonly "tui.select.confirm": {
        readonly defaultKeys: "enter";
        readonly description: "Confirm selection";
    };
    readonly "tui.select.cancel": {
        readonly defaultKeys: ["escape", "ctrl+c"];
        readonly description: "Cancel selection";
    };
};
```

### keys

#### decodeKittyPrintable

Kind: function

```ts
/**
 * Decode a Kitty CSI-u sequence into a printable character, if applicable.
 *
 * When Kitty keyboard protocol flag 1 (disambiguate) is active, terminals send
 * CSI-u sequences for all keys, including plain printable characters. This
 * function extracts the printable character from such sequences.
 *
 * Only accepts plain or Shift-modified keys. Rejects Ctrl, Alt, and unsupported
 * modifier combinations (those are handled by keybinding matching instead).
 * Prefers the shifted keycode when Shift is held and a shifted key is reported.
 *
 * @param data - Raw input data from terminal
 * @returns The printable character, or undefined if not a printable CSI-u sequence
 */
export declare function decodeKittyPrintable(data: string): string | undefined;
```

#### isKeyRelease

Kind: function

```ts
/**
 * Check if the last parsed key event was a key release.
 * Only meaningful when Kitty keyboard protocol with flag 2 is active.
 */
export declare function isKeyRelease(data: string): boolean;
```

#### isKeyRepeat

Kind: function

```ts
/**
 * Check if the last parsed key event was a key repeat.
 * Only meaningful when Kitty keyboard protocol with flag 2 is active.
 */
export declare function isKeyRepeat(data: string): boolean;
```

#### isKittyProtocolActive

Kind: function

```ts
/**
 * Query whether Kitty keyboard protocol is currently active.
 */
export declare function isKittyProtocolActive(): boolean;
```

#### Key

Kind: const

```ts
/**
 * Helper object for creating typed key identifiers with autocomplete.
 *
 * Usage:
 * - Key.escape, Key.enter, Key.tab, etc. for special keys
 * - Key.backtick, Key.comma, Key.period, etc. for symbol keys
 * - Key.ctrl("c"), Key.alt("x"), Key.super("k") for single modifiers
 * - Key.ctrlShift("p"), Key.ctrlAlt("x"), Key.ctrlSuper("k") for combined modifiers
 */
export declare const Key: {
    readonly escape: "escape";
    readonly esc: "esc";
    readonly enter: "enter";
    readonly return: "return";
    readonly tab: "tab";
    readonly space: "space";
    readonly backspace: "backspace";
    readonly delete: "delete";
    readonly insert: "insert";
    readonly clear: "clear";
    readonly home: "home";
    readonly end: "end";
    readonly pageUp: "pageUp";
    readonly pageDown: "pageDown";
    readonly up: "up";
    readonly down: "down";
    readonly left: "left";
    readonly right: "right";
    readonly f1: "f1";
    readonly f2: "f2";
    readonly f3: "f3";
    readonly f4: "f4";
    readonly f5: "f5";
    readonly f6: "f6";
    readonly f7: "f7";
    readonly f8: "f8";
    readonly f9: "f9";
    readonly f10: "f10";
    readonly f11: "f11";
    readonly f12: "f12";
    readonly backtick: "`";
    readonly hyphen: "-";
    readonly equals: "=";
    readonly leftbracket: "[";
    readonly rightbracket: "]";
    readonly backslash: "\\";
    readonly semicolon: ";";
    readonly quote: "'";
    readonly comma: ",";
    readonly period: ".";
    readonly slash: "/";
    readonly exclamation: "!";
    readonly at: "@";
    readonly hash: "#";
    readonly dollar: "$";
    readonly percent: "%";
    readonly caret: "^";
    readonly ampersand: "&";
    readonly asterisk: "*";
    readonly leftparen: "(";
    readonly rightparen: ")";
    readonly underscore: "_";
    readonly plus: "+";
    readonly pipe: "|";
    readonly tilde: "~";
    readonly leftbrace: "{";
    readonly rightbrace: "}";
    readonly colon: ":";
    readonly lessthan: "<";
    readonly greaterthan: ">";
    readonly question: "?";
    readonly ctrl: <K extends BaseKey>(key: K) => `ctrl+${K}`;
    readonly shift: <K extends BaseKey>(key: K) => `shift+${K}`;
    readonly alt: <K extends BaseKey>(key: K) => `alt+${K}`;
    readonly super: <K extends BaseKey>(key: K) => `super+${K}`;
    readonly ctrlShift: <K extends BaseKey>(key: K) => `ctrl+shift+${K}`;
    readonly shiftCtrl: <K extends BaseKey>(key: K) => `shift+ctrl+${K}`;
    readonly ctrlAlt: <K extends BaseKey>(key: K) => `ctrl+alt+${K}`;
    readonly altCtrl: <K extends BaseKey>(key: K) => `alt+ctrl+${K}`;
    readonly shiftAlt: <K extends BaseKey>(key: K) => `shift+alt+${K}`;
    readonly altShift: <K extends BaseKey>(key: K) => `alt+shift+${K}`;
    readonly ctrlSuper: <K extends BaseKey>(key: K) => `ctrl+super+${K}`;
    readonly superCtrl: <K extends BaseKey>(key: K) => `super+ctrl+${K}`;
    readonly shiftSuper: <K extends BaseKey>(key: K) => `shift+super+${K}`;
    readonly superShift: <K extends BaseKey>(key: K) => `super+shift+${K}`;
    readonly altSuper: <K extends BaseKey>(key: K) => `alt+super+${K}`;
    readonly superAlt: <K extends BaseKey>(key: K) => `super+alt+${K}`;
    readonly ctrlShiftAlt: <K extends BaseKey>(key: K) => `ctrl+shift+alt+${K}`;
    readonly ctrlShiftSuper: <K extends BaseKey>(key: K) => `ctrl+shift+super+${K}`;
};
```

#### KeyEventType

Kind: type

```ts
/**
 * Event types from Kitty keyboard protocol (flag 2)
 * 1 = key press, 2 = key repeat, 3 = key release
 */
export type KeyEventType = "press" | "repeat" | "release";
```

#### KeyId

Kind: type

```ts
/**
 * Union type of all valid key identifiers.
 * Provides autocomplete and catches typos at compile time.
 */
export type KeyId = BaseKey | ModifiedKeyId<BaseKey>;
```

#### matchesKey

Kind: function

```ts
/**
 * Match input data against a key identifier string.
 *
 * Supported key identifiers:
 * - Single keys: "escape", "tab", "enter", "backspace", "delete", "home", "end", "space"
 * - Arrow keys: "up", "down", "left", "right"
 * - Ctrl combinations: "ctrl+c", "ctrl+z", etc.
 * - Shift combinations: "shift+tab", "shift+enter"
 * - Alt combinations: "alt+enter", "alt+backspace"
 * - Super combinations: "super+k", "super+enter"
 * - Combined modifiers: "shift+ctrl+p", "ctrl+alt+x", "ctrl+super+k"
 *
 * Use the Key helper for autocomplete: Key.ctrl("c"), Key.escape, Key.ctrlShift("p"), Key.super("k")
 *
 * @param data - Raw input data from terminal
 * @param keyId - Key identifier (e.g., "ctrl+c", "escape", Key.ctrl("c"))
 */
export declare function matchesKey(data: string, keyId: KeyId): boolean;
```

#### parseKey

Kind: function

```ts
export declare function parseKey(data: string): string | undefined;
```

#### setKittyProtocolActive

Kind: function

```ts
/**
 * Keyboard input handling for terminal applications.
 *
 * Supports both legacy terminal sequences and Kitty keyboard protocol.
 * See: https://sw.kovidgoyal.net/kitty/keyboard-protocol/
 * Reference: https://github.com/sst/opentui/blob/7da92b4088aebfe27b9f691c04163a48821e49fd/packages/core/src/lib/parse.keypress.ts
 *
 * Symbol keys are also supported, however some ctrl+symbol combos
 * overlap with ASCII codes, e.g. ctrl+[ = ESC.
 * See: https://sw.kovidgoyal.net/kitty/keyboard-protocol/#legacy-ctrl-mapping-of-ascii-keys
 * Those can still be * used for ctrl+shift combos
 *
 * API:
 * - matchesKey(data, keyId) - Check if input matches a key identifier
 * - parseKey(data) - Parse input and return the key identifier
 * - Key - Helper object for creating typed key identifiers
 * - setKittyProtocolActive(active) - Set global Kitty protocol state
 * - isKittyProtocolActive() - Query global Kitty protocol state
 */
/**
 * Set the global Kitty keyboard protocol state.
 * Called by ProcessTerminal after detecting protocol support.
 */
export declare function setKittyProtocolActive(active: boolean): void;
```

### stdin-buffer

#### StdinBuffer

Kind: class

```ts
/**
 * Buffers stdin input and emits complete sequences via the 'data' event.
 * Handles partial escape sequences that arrive across multiple chunks.
 */
export declare class StdinBuffer extends EventEmitter<StdinBufferEventMap> {
    private buffer;
    private timeout;
    private readonly timeoutMs;
    private pasteMode;
    private pasteBuffer;
    private pendingKittyPrintableCodepoint;
    constructor(options?: StdinBufferOptions);
    process(data: string | Buffer): void;
    private emitDataSequence;
    flush(): string[];
    clear(): void;
    getBuffer(): string;
    destroy(): void;
}
```

#### StdinBufferEventMap

Kind: type

```ts
export type StdinBufferEventMap = {
    data: [string];
    paste: [string];
};
```

#### StdinBufferOptions

Kind: type

```ts
export type StdinBufferOptions = {
    /**
     * Maximum time to wait for sequence completion (default: 10ms)
     * After this time, the buffer is flushed even if incomplete
     */
    timeout?: number;
};
```

### terminal

#### ProcessTerminal

Kind: class

```ts
/**
 * Real terminal using process.stdin/stdout
 */
export declare class ProcessTerminal implements Terminal {
    private wasRaw;
    private inputHandler?;
    private resizeHandler?;
    private _kittyProtocolActive;
    private _modifyOtherKeysActive;
    private stdinBuffer?;
    private stdinDataHandler?;
    private progressInterval?;
    private writeLogPath;
    get kittyProtocolActive(): boolean;
    start(onInput: (data: string) => void, onResize: () => void): void;
    /**
     * Set up StdinBuffer to split batched input into individual sequences.
     * This ensures components receive single events, making matchesKey/isKeyRelease work correctly.
     *
     * Also watches for Kitty protocol response and enables it when detected.
     * This is done here (after stdinBuffer parsing) rather than on raw stdin
     * to handle the case where the response arrives split across multiple events.
     */
    private setupStdinBuffer;
    /**
     * Query terminal for Kitty keyboard protocol support and enable if available.
     *
     * Sends CSI ? u to query current flags. If terminal responds with CSI ? <flags> u,
     * it supports the protocol and we enable it with CSI > 1 u.
     *
     * If no Kitty response arrives shortly after startup, fall back to enabling
     * xterm modifyOtherKeys mode 2. This is needed for tmux, which can forward
     * modified enter keys as CSI-u when extended-keys is enabled, but may not
     * answer the Kitty protocol query.
     *
     * The response is detected in setupStdinBuffer's data handler, which properly
     * handles the case where the response arrives split across multiple stdin events.
     */
    private queryAndEnableKittyProtocol;
    /**
     * On Windows, add ENABLE_VIRTUAL_TERMINAL_INPUT (0x0200) to the stdin
     * console handle so the terminal sends VT sequences for modified keys
     * (e.g. \x1b[Z for Shift+Tab). Without this, libuv's ReadConsoleInputW
     * discards modifier state and Shift+Tab arrives as plain \t.
     */
    private enableWindowsVTInput;
    drainInput(maxMs?: number, idleMs?: number): Promise<void>;
    stop(): void;
    write(data: string): void;
    get columns(): number;
    get rows(): number;
    moveBy(lines: number): void;
    hideCursor(): void;
    showCursor(): void;
    clearLine(): void;
    clearFromCursor(): void;
    clearScreen(): void;
    setTitle(title: string): void;
    setProgress(active: boolean): void;
    private clearProgressInterval;
}
```

#### Terminal

Kind: interface

```ts
/**
 * Minimal terminal interface for TUI
 */
export interface Terminal {
    start(onInput: (data: string) => void, onResize: () => void): void;
    stop(): void;
    /**
     * Drain stdin before exiting to prevent Kitty key release events from
     * leaking to the parent shell over slow SSH connections.
     * @param maxMs - Maximum time to drain (default: 1000ms)
     * @param idleMs - Exit early if no input arrives within this time (default: 50ms)
     */
    drainInput(maxMs?: number, idleMs?: number): Promise<void>;
    write(data: string): void;
    get columns(): number;
    get rows(): number;
    get kittyProtocolActive(): boolean;
    moveBy(lines: number): void;
    hideCursor(): void;
    showCursor(): void;
    clearLine(): void;
    clearFromCursor(): void;
    clearScreen(): void;
    setTitle(title: string): void;
    setProgress(active: boolean): void;
}
```

### terminal-image

#### allocateImageId

Kind: function

```ts
/**
 * Generate a random image ID for Kitty graphics protocol.
 * Uses random IDs to avoid collisions between different module instances
 * (e.g., main app vs extensions).
 */
export declare function allocateImageId(): number;
```

#### calculateImageRows

Kind: function

```ts
export declare function calculateImageRows(imageDimensions: ImageDimensions, targetWidthCells: number, cellDimensions?: CellDimensions): number;
```

#### CellDimensions

Kind: interface

```ts
export interface CellDimensions {
    widthPx: number;
    heightPx: number;
}
```

#### deleteAllKittyImages

Kind: function

```ts
/**
 * Delete all visible Kitty graphics images.
 * Uses uppercase 'A' to also free the image data.
 */
export declare function deleteAllKittyImages(): string;
```

#### deleteKittyImage

Kind: function

```ts
/**
 * Delete a Kitty graphics image by ID.
 * Uses uppercase 'I' to also free the image data.
 */
export declare function deleteKittyImage(imageId: number): string;
```

#### detectCapabilities

Kind: function

```ts
export declare function detectCapabilities(): TerminalCapabilities;
```

#### encodeITerm2

Kind: function

```ts
export declare function encodeITerm2(base64Data: string, options?: {
    width?: number | string;
    height?: number | string;
    name?: string;
    preserveAspectRatio?: boolean;
    inline?: boolean;
}): string;
```

#### encodeKitty

Kind: function

```ts
export declare function encodeKitty(base64Data: string, options?: {
    columns?: number;
    rows?: number;
    imageId?: number;
    /** Whether Kitty should apply its default cursor movement after placement. Default: true. */
    moveCursor?: boolean;
}): string;
```

#### getCapabilities

Kind: function

```ts
export declare function getCapabilities(): TerminalCapabilities;
```

#### getCellDimensions

Kind: function

```ts
export declare function getCellDimensions(): CellDimensions;
```

#### getGifDimensions

Kind: function

```ts
export declare function getGifDimensions(base64Data: string): ImageDimensions | null;
```

#### getImageDimensions

Kind: function

```ts
export declare function getImageDimensions(base64Data: string, mimeType: string): ImageDimensions | null;
```

#### getJpegDimensions

Kind: function

```ts
export declare function getJpegDimensions(base64Data: string): ImageDimensions | null;
```

#### getPngDimensions

Kind: function

```ts
export declare function getPngDimensions(base64Data: string): ImageDimensions | null;
```

#### getWebpDimensions

Kind: function

```ts
export declare function getWebpDimensions(base64Data: string): ImageDimensions | null;
```

#### hyperlink

Kind: function

```ts
/**
 * Wrap text in an OSC 8 hyperlink sequence.
 * The text is rendered as a clickable hyperlink in terminals that support OSC 8
 * (Ghostty, Kitty, WezTerm, iTerm2, VSCode, and others).
 * In terminals that do not support OSC 8, the escape sequences are ignored
 * and only the plain text is displayed.
 *
 * @param text - The visible text to display
 * @param url - The URL to link to
 */
export declare function hyperlink(text: string, url: string): string;
```

#### ImageDimensions

Kind: interface

```ts
export interface ImageDimensions {
    widthPx: number;
    heightPx: number;
}
```

#### imageFallback

Kind: function

```ts
export declare function imageFallback(mimeType: string, dimensions?: ImageDimensions, filename?: string): string;
```

#### ImageProtocol

Kind: type

```ts
export type ImageProtocol = "kitty" | "iterm2" | null;
```

#### ImageRenderOptions

Kind: interface

```ts
export interface ImageRenderOptions {
    maxWidthCells?: number;
    maxHeightCells?: number;
    preserveAspectRatio?: boolean;
    /** Kitty image ID. If provided, reuses/replaces existing image with this ID. */
    imageId?: number;
    /** Whether Kitty should apply its default cursor movement after placement. */
    moveCursor?: boolean;
}
```

#### renderImage

Kind: function

```ts
export declare function renderImage(base64Data: string, imageDimensions: ImageDimensions, options?: ImageRenderOptions): {
    sequence: string;
    rows: number;
    imageId?: number;
} | null;
```

#### resetCapabilitiesCache

Kind: function

```ts
export declare function resetCapabilitiesCache(): void;
```

#### setCapabilities

Kind: function

```ts
/** Override the cached capabilities. Useful in tests to exercise both code paths. */
export declare function setCapabilities(caps: TerminalCapabilities): void;
```

#### setCellDimensions

Kind: function

```ts
export declare function setCellDimensions(dims: CellDimensions): void;
```

#### TerminalCapabilities

Kind: interface

```ts
export interface TerminalCapabilities {
    images: ImageProtocol;
    trueColor: boolean;
    hyperlinks: boolean;
}
```

### tui

#### Component

Kind: interface

```ts
/**
 * Component interface - all components must implement this
 */
export interface Component {
    /**
     * Render the component to lines for the given viewport width
     * @param width - Current viewport width
     * @returns Array of strings, each representing a line
     */
    render(width: number): string[];
    /**
     * Optional handler for keyboard input when component has focus
     */
    handleInput?(data: string): void;
    /**
     * If true, component receives key release events (Kitty protocol).
     * Default is false - release events are filtered out.
     */
    wantsKeyRelease?: boolean;
    /**
     * Invalidate any cached rendering state.
     * Called when theme changes or when component needs to re-render from scratch.
     */
    invalidate(): void;
}
```

#### Container

Kind: class

```ts
/**
 * Container - a component that contains other components
 */
export declare class Container implements Component {
    children: Component[];
    addChild(component: Component): void;
    removeChild(component: Component): void;
    clear(): void;
    invalidate(): void;
    render(width: number): string[];
}
```

#### CURSOR_MARKER

Kind: const

```ts
/**
 * Cursor position marker - APC (Application Program Command) sequence.
 * This is a zero-width escape sequence that terminals ignore.
 * Components emit this at the cursor position when focused.
 * TUI finds and strips this marker, then positions the hardware cursor there.
 */
export declare const CURSOR_MARKER = "\u001B_pi:c\u0007";
```

#### Focusable

Kind: interface

```ts
/**
 * Interface for components that can receive focus and display a hardware cursor.
 * When focused, the component should emit CURSOR_MARKER at the cursor position
 * in its render output. TUI will find this marker and position the hardware
 * cursor there for proper IME candidate window positioning.
 */
export interface Focusable {
    /** Set by TUI when focus changes. Component should emit CURSOR_MARKER when true. */
    focused: boolean;
}
```

#### isFocusable

Kind: function

```ts
/** Type guard to check if a component implements Focusable */
export declare function isFocusable(component: Component | null): component is Component & Focusable;
```

#### OverlayAnchor

Kind: type

```ts
/**
 * Anchor position for overlays
 */
export type OverlayAnchor = "center" | "top-left" | "top-right" | "bottom-left" | "bottom-right" | "top-center" | "bottom-center" | "left-center" | "right-center";
```

#### OverlayHandle

Kind: interface

```ts
/**
 * Handle returned by showOverlay for controlling the overlay
 */
export interface OverlayHandle {
    /** Permanently remove the overlay (cannot be shown again) */
    hide(): void;
    /** Temporarily hide or show the overlay */
    setHidden(hidden: boolean): void;
    /** Check if overlay is temporarily hidden */
    isHidden(): boolean;
    /** Focus this overlay and bring it to the visual front */
    focus(): void;
    /** Release focus to the previous target */
    unfocus(): void;
    /** Check if this overlay currently has focus */
    isFocused(): boolean;
}
```

#### OverlayMargin

Kind: interface

```ts
/**
 * Margin configuration for overlays
 */
export interface OverlayMargin {
    top?: number;
    right?: number;
    bottom?: number;
    left?: number;
}
```

#### OverlayOptions

Kind: interface

```ts
/**
 * Options for overlay positioning and sizing.
 * Values can be absolute numbers or percentage strings (e.g., "50%").
 */
export interface OverlayOptions {
    /** Width in columns, or percentage of terminal width (e.g., "50%") */
    width?: SizeValue;
    /** Minimum width in columns */
    minWidth?: number;
    /** Maximum height in rows, or percentage of terminal height (e.g., "50%") */
    maxHeight?: SizeValue;
    /** Anchor point for positioning (default: 'center') */
    anchor?: OverlayAnchor;
    /** Horizontal offset from anchor position (positive = right) */
    offsetX?: number;
    /** Vertical offset from anchor position (positive = down) */
    offsetY?: number;
    /** Row position: absolute number, or percentage (e.g., "25%" = 25% from top) */
    row?: SizeValue;
    /** Column position: absolute number, or percentage (e.g., "50%" = centered horizontally) */
    col?: SizeValue;
    /** Margin from terminal edges. Number applies to all sides. */
    margin?: OverlayMargin | number;
    /**
     * Control overlay visibility based on terminal dimensions.
     * If provided, overlay is only rendered when this returns true.
     * Called each render cycle with current terminal dimensions.
     */
    visible?: (termWidth: number, termHeight: number) => boolean;
    /** If true, don't capture keyboard focus when shown */
    nonCapturing?: boolean;
}
```

#### SizeValue

Kind: type

```ts
/** Value that can be absolute (number) or percentage (string like "50%") */
export type SizeValue = number | `${number}%`;
```

#### TUI

Kind: class

```ts
/**
 * TUI - Main class for managing terminal UI with differential rendering
 */
export declare class TUI extends Container {
    terminal: Terminal;
    private previousLines;
    private previousKittyImageIds;
    private previousWidth;
    private previousHeight;
    private focusedComponent;
    private inputListeners;
    /** Global callback for debug key (Shift+Ctrl+D). Called before input is forwarded to focused component. */
    onDebug?: () => void;
    private renderRequested;
    private renderTimer;
    private lastRenderAt;
    private static readonly MIN_RENDER_INTERVAL_MS;
    private cursorRow;
    private hardwareCursorRow;
    private showHardwareCursor;
    private clearOnShrink;
    private maxLinesRendered;
    private previousViewportTop;
    private fullRedrawCount;
    private stopped;
    private focusOrderCounter;
    private overlayStack;
    constructor(terminal: Terminal, showHardwareCursor?: boolean);
    get fullRedraws(): number;
    getShowHardwareCursor(): boolean;
    setShowHardwareCursor(enabled: boolean): void;
    getClearOnShrink(): boolean;
    /**
     * Set whether to trigger full re-render when content shrinks.
     * When true (default), empty rows are cleared when content shrinks.
     * When false, empty rows remain (reduces redraws on slower terminals).
     */
    setClearOnShrink(enabled: boolean): void;
    setFocus(component: Component | null): void;
    /**
     * Show an overlay component with configurable positioning and sizing.
     * Returns a handle to control the overlay's visibility.
     */
    showOverlay(component: Component, options?: OverlayOptions): OverlayHandle;
    /** Hide the topmost overlay and restore previous focus. */
    hideOverlay(): void;
    /** Check if there are any visible overlays */
    hasOverlay(): boolean;
    /** Check if an overlay entry is currently visible */
    private isOverlayVisible;
    /** Find the topmost visible capturing overlay, if any */
    private getTopmostVisibleOverlay;
    invalidate(): void;
    start(): void;
    addInputListener(listener: InputListener): () => void;
    removeInputListener(listener: InputListener): void;
    private queryCellSize;
    stop(): void;
    requestRender(force?: boolean): void;
    private scheduleRender;
    private handleInput;
    private consumeCellSizeResponse;
    /**
     * Resolve overlay layout from options.
     * Returns { width, row, col, maxHeight } for rendering.
     */
    private resolveOverlayLayout;
    private resolveAnchorRow;
    private resolveAnchorCol;
    /** Composite all overlays into content lines (sorted by focusOrder, higher = on top). */
    private compositeOverlays;
    private static readonly SEGMENT_RESET;
    private applyLineResets;
    private collectKittyImageIds;
    private deleteKittyImages;
    private expandLastChangedForKittyImages;
    private deleteChangedKittyImages;
    /** Splice overlay content into a base line at a specific column. Single-pass optimized. */
    private compositeLineAt;
    /**
     * Find and extract cursor position from rendered lines.
     * Searches for CURSOR_MARKER, calculates its position, and strips it from the output.
     * Only scans the bottom terminal height lines (visible viewport).
     * @param lines - Rendered lines to search
     * @param height - Terminal height (visible viewport size)
     * @returns Cursor position { row, col } or null if no marker found
     */
    private extractCursorPosition;
    private doRender;
    /**
     * Position the hardware cursor for IME candidate window.
     * @param cursorPos The cursor position extracted from rendered output, or null
     * @param totalLines Total number of rendered lines
     */
    private positionHardwareCursor;
}
```

### utils

#### truncateToWidth

Kind: function

```ts
/**
 * Truncate text to fit within a maximum visible width, adding ellipsis if needed.
 * Optionally pad with spaces to reach exactly maxWidth.
 * Properly handles ANSI escape codes (they don't count toward width).
 *
 * @param text - Text to truncate (may contain ANSI codes)
 * @param maxWidth - Maximum visible width
 * @param ellipsis - Ellipsis string to append when truncating (default: "...")
 * @param pad - If true, pad result with spaces to exactly maxWidth (default: false)
 * @returns Truncated text, optionally padded to exactly maxWidth
 */
export declare function truncateToWidth(text: string, maxWidth: number, ellipsis?: string, pad?: boolean): string;
```

#### visibleWidth

Kind: function

```ts
/**
 * Calculate the visible width of a string in terminal columns.
 */
export declare function visibleWidth(str: string): number;
```

#### wrapTextWithAnsi

Kind: function

```ts
/**
 * Wrap text with ANSI codes preserved.
 *
 * ONLY does word wrapping - NO padding, NO background colors.
 * Returns lines where each line is <= width visible chars.
 * Active ANSI codes are preserved across line breaks.
 *
 * @param text - Text to wrap (may contain ANSI codes and newlines)
 * @param width - Maximum visible width per line
 * @returns Array of wrapped lines (NOT padded to width)
 */
export declare function wrapTextWithAnsi(text: string, width: number): string[];
```
