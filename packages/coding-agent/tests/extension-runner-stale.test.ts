import { describe, expect, it } from "vitest";
import { ExtensionRunner } from "../src/core/extensions/runner.js";
import type { Extension, ExtensionError, HandlerFn } from "../src/core/extensions/types.js";

function makeExtension(path: string, eventType: string, handlers: HandlerFn[]): Extension {
	return {
		path,
		resolvedPath: path,
		sourceInfo: { path, source: "user", scope: "global", origin: "file" },
		handlers: new Map([[eventType, handlers]]),
		tools: new Map(),
		messageRenderers: new Map(),
		commands: new Map(),
		flags: new Map(),
		shortcuts: new Map(),
	};
}

function makeRunner(extensions: Extension[]): { runner: ExtensionRunner; errors: ExtensionError[] } {
	const errors: ExtensionError[] = [];
	const runtime = {
		flagValues: new Map(),
		pendingProviderRegistrations: [],
		assertActive: () => {},
		invalidate: () => {},
		registerProvider: () => {},
		unregisterProvider: () => {},
		sendMessage: () => {},
		sendUserMessage: async () => {},
		appendEntry: () => {},
		setSessionName: () => {},
		getSessionName: () => undefined,
		setLabel: () => {},
		getActiveTools: () => [],
		getAllTools: () => [],
		setActiveTools: () => {},
		refreshTools: () => {},
		getCommands: () => [],
		setModel: () => {},
		getThinkingLevel: () => undefined,
		setThinkingLevel: () => {},
	} as any;
	const sessionManager = { getBranch: () => [] } as any;
	const modelRegistry = {} as any;
	const runner = new ExtensionRunner(extensions, runtime, "/tmp", sessionManager, modelRegistry);
	runner.onError((e) => errors.push(e));
	return { runner, errors };
}

const agentEndEvent = { type: "agent_end", messages: [] } as any;

describe("ExtensionRunner stale short-circuit", () => {
	it("R1: skips all handlers when the runner is already invalidated", async () => {
		let called = false;
		const ext = makeExtension("ext", "agent_end", [
			() => {
				called = true;
			},
		]);
		const { runner, errors } = makeRunner([ext]);
		runner.invalidate();

		await runner.emit(agentEndEvent);

		expect(called).toBe(false);
		expect(errors).toEqual([]);
	});

	it("R2: mid-emit invalidation skips remaining handlers without errors", async () => {
		let secondCalled = false;
		const first: HandlerFn = async (_event, _ctx) => {
			await new Promise((resolve) => setTimeout(resolve, 0));
			runner.invalidate();
		};
		const second: HandlerFn = (_event, ctx) => {
			secondCalled = true;
			void (ctx as any).hasUI;
		};
		const ext1 = makeExtension("ext1", "agent_end", [first]);
		const ext2 = makeExtension("ext2", "agent_end", [second]);
		const { runner, errors } = makeRunner([ext1, ext2]);

		await runner.emit(agentEndEvent);

		expect(secondCalled).toBe(false);
		expect(errors).toEqual([]);
	});

	it("R3: every emit variant short-circuits after invalidation", async () => {
		const contextMessages: any[] = [{ role: "user", content: [] }];
		const providerPayload = { some: "payload" };
		const cases: Array<{
			eventType: string;
			invoke: (runner: ExtensionRunner) => Promise<unknown>;
			expected?: unknown;
		}> = [
			{ eventType: "agent_end", invoke: (r) => r.emit(agentEndEvent), expected: undefined },
			{
				eventType: "message_end",
				invoke: (r) =>
					r.emitMessageEnd({ type: "message_end", message: { role: "assistant", content: [] } } as any),
				expected: undefined,
			},
			{
				eventType: "tool_result",
				invoke: (r) => r.emitToolResult({ type: "tool_result", toolName: "read", content: [] } as any),
				expected: undefined,
			},
			{
				eventType: "tool_call",
				invoke: (r) => r.emitToolCall({ type: "tool_call", toolName: "read" } as any),
				expected: undefined,
			},
			{
				eventType: "user_bash",
				invoke: (r) => r.emitUserBash({ type: "user_bash", command: "ls" } as any),
				expected: undefined,
			},
			{ eventType: "context", invoke: (r) => r.emitContext(contextMessages), expected: contextMessages },
			{
				eventType: "before_provider_request",
				invoke: (r) => r.emitBeforeProviderRequest(providerPayload),
				expected: providerPayload,
			},
			{
				eventType: "before_agent_start",
				invoke: (r) => r.emitBeforeAgentStart("prompt", undefined, [], {} as any),
				expected: undefined,
			},
			{
				eventType: "resources_discover",
				invoke: (r) => r.emitResourcesDiscover("/tmp", "startup" as any),
				expected: { skillPaths: [], promptPaths: [], themePaths: [] },
			},
			{
				eventType: "input",
				invoke: (r) => r.emitInput("hi", undefined, "interactive" as any),
				expected: { action: "continue" },
			},
		];

		for (const { eventType, invoke, expected } of cases) {
			let called = false;
			const ext = makeExtension("ext", eventType, [
				() => {
					called = true;
				},
			]);
			const { runner, errors } = makeRunner([ext]);
			runner.invalidate();

			const result = await invoke(runner);

			expect(called, `${eventType} handler should be skipped`).toBe(false);
			expect(errors, `${eventType} should emit no errors`).toEqual([]);
			expect(result, `${eventType} short-circuit return shape`).toEqual(expected);
		}
	});

	it("R6: mid-chain invalidation in emitInput returns the partial transform", async () => {
		const first: HandlerFn = async () => {
			runner.invalidate();
			return { action: "transform", text: "transformed" } as any;
		};
		let secondCalled = false;
		const second: HandlerFn = () => {
			secondCalled = true;
		};
		const ext1 = makeExtension("ext1", "input", [first]);
		const ext2 = makeExtension("ext2", "input", [second]);
		const { runner, errors } = makeRunner([ext1, ext2]);

		const result = await runner.emitInput("hi", undefined, "interactive" as any);

		expect(secondCalled).toBe(false);
		expect(errors).toEqual([]);
		expect(result).toEqual({ action: "transform", text: "transformed", images: undefined });
	});

	it("R6: mid-chain invalidation in emitMessageEnd returns the partially-modified message", async () => {
		const replaced = { role: "assistant", content: [{ type: "text", text: "modified" }] };
		const first: HandlerFn = async () => {
			runner.invalidate();
			return { message: replaced } as any;
		};
		let secondCalled = false;
		const second: HandlerFn = () => {
			secondCalled = true;
		};
		const ext1 = makeExtension("ext1", "message_end", [first]);
		const ext2 = makeExtension("ext2", "message_end", [second]);
		const { runner, errors } = makeRunner([ext1, ext2]);

		const result = await runner.emitMessageEnd({
			type: "message_end",
			message: { role: "assistant", content: [] },
		} as any);

		expect(secondCalled).toBe(false);
		expect(errors).toEqual([]);
		expect(result).toEqual(replaced);
	});

	it("R6: mid-chain invalidation in emitContext returns the partially-accumulated messages", async () => {
		const transformed = [{ role: "user", content: [{ type: "text", text: "injected" }] }];
		const first: HandlerFn = async () => {
			runner.invalidate();
			return { messages: transformed } as any;
		};
		let secondCalled = false;
		const second: HandlerFn = () => {
			secondCalled = true;
		};
		const ext1 = makeExtension("ext1", "context", [first]);
		const ext2 = makeExtension("ext2", "context", [second]);
		const { runner, errors } = makeRunner([ext1, ext2]);

		const result = await runner.emitContext([{ role: "user", content: [] }] as any);

		expect(secondCalled).toBe(false);
		expect(errors).toEqual([]);
		expect(result).toEqual(transformed);
	});

	it("R4: a handler that touches a captured ctx after invalidation still throws stale error", async () => {
		const handler: HandlerFn = async (_event, ctx) => {
			await new Promise((resolve) => setTimeout(resolve, 0));
			runner.invalidate();
			void (ctx as any).hasUI;
		};
		const ext = makeExtension("ext", "agent_end", [handler]);
		const { runner, errors } = makeRunner([ext]);

		await runner.emit(agentEndEvent);

		expect(errors).toHaveLength(1);
		expect(errors[0].error).toContain("stale");
	});

	it("R5: a session_shutdown emit on an invalidated runner is reported via emitError, not silently dropped", async () => {
		let called = false;
		const ext = makeExtension("ext", "session_shutdown", [
			() => {
				called = true;
			},
		]);
		const { runner, errors } = makeRunner([ext]);
		runner.invalidate();

		await runner.emit({ type: "session_shutdown" } as any);

		expect(called).toBe(false);
		expect(errors).toHaveLength(1);
		expect(errors[0].event).toBe("session_shutdown");
	});
});
