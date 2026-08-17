import type { Context, Model, SystemPromptSection } from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
import { ExtensionRunner, spliceContributedSections } from "../src/core/extensions/runner.js";
import type { Extension, ExtensionError, HandlerFn } from "../src/core/extensions/types.js";

function makeExtension(path: string, handler: HandlerFn): Extension {
	return {
		path,
		resolvedPath: path,
		sourceInfo: { path, source: "user", scope: "global", origin: "file" },
		handlers: new Map([["before_agent_start", [handler]]]),
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

const baseSections: SystemPromptSection[] = [
	{ id: "base", text: "base prompt" },
	{ id: "volatile", text: "\nvolatile", cacheRetention: "none" },
];

const model = { provider: "test", id: "model-b", headers: { token: "original" } } as Model<any>;

describe("ExtensionRunner before_provider_call", () => {
	it("chains request-local prompt replacements without mutating shared context", async () => {
		const observed: Array<{ model: Model<any>; systemPrompt: Context["systemPrompt"] }> = [];
		const first = makeExtension("first", (event: any) => {
			observed.push({ model: event.model, systemPrompt: event.systemPrompt });
			return { systemPrompt: `${event.systemPrompt}\nfirst` };
		});
		first.handlers = new Map([["before_provider_call", [...first.handlers.get("before_agent_start")!]]]);
		const second = makeExtension("second", (event: any) => {
			observed.push({ model: event.model, systemPrompt: event.systemPrompt });
			return { systemPrompt: `${event.systemPrompt}\nsecond` };
		});
		second.handlers = new Map([["before_provider_call", [...second.handlers.get("before_agent_start")!]]]);
		const { runner, errors } = makeRunner([first, second]);
		const context = { systemPrompt: "base", messages: [], tools: [] } satisfies Context;

		const result = await runner.emitBeforeProviderCall(context, model);

		expect(errors).toEqual([]);
		expect(observed).toEqual([
			{ model, systemPrompt: "base" },
			{ model, systemPrompt: "base\nfirst" },
		]);
		expect(result.systemPrompt).toBe("base\nfirst\nsecond");
		expect(context.systemPrompt).toBe("base");
		expect(result).not.toBe(context);
	});

	it("isolates the routed model snapshot from handlers and transport", async () => {
		const observed: string[] = [];
		const mutating = makeExtension("mutating", (event: any) => {
			expect(Object.isFrozen(event.model)).toBe(true);
			expect(Object.isFrozen(event.model.headers)).toBe(true);
			Reflect.set(event.model.headers, "token", "mutated");
		});
		mutating.handlers = new Map([["before_provider_call", [...mutating.handlers.get("before_agent_start")!]]]);
		const observing = makeExtension("observing", (event: any) => {
			observed.push(event.model.headers.token);
		});
		observing.handlers = new Map([["before_provider_call", [...observing.handlers.get("before_agent_start")!]]]);
		const { runner, errors } = makeRunner([mutating, observing]);

		await runner.emitBeforeProviderCall({ systemPrompt: "base", messages: [], tools: [] }, model);

		expect(errors).toEqual([]);
		expect(observed).toEqual(["original"]);
		expect(model.headers?.token).toBe("original");
	});

	it("isolates handler errors and continues chaining", async () => {
		const failing = makeExtension("failing", () => {
			throw new Error("broken");
		});
		failing.handlers = new Map([["before_provider_call", [...failing.handlers.get("before_agent_start")!]]]);
		const succeeding = makeExtension("succeeding", (event: any) => ({ systemPrompt: `${event.systemPrompt}\nok` }));
		succeeding.handlers = new Map([["before_provider_call", [...succeeding.handlers.get("before_agent_start")!]]]);
		const { runner, errors } = makeRunner([failing, succeeding]);

		const result = await runner.emitBeforeProviderCall({ systemPrompt: "base", messages: [], tools: [] }, model);

		expect(result.systemPrompt).toBe("base\nok");
		expect(errors).toMatchObject([{ extensionPath: "failing", event: "before_provider_call", error: "broken" }]);
	});
});

describe("ExtensionRunner before_provider_request", () => {
	it("exposes the exact model while preserving payload chaining", async () => {
		const observed: Model<any>[] = [];
		const first = makeExtension("first", (event: any) => {
			observed.push(event.model);
			return { ...event.payload, first: true };
		});
		first.handlers = new Map([["before_provider_request", [...first.handlers.get("before_agent_start")!]]]);
		const second = makeExtension("second", (event: any) => {
			observed.push(event.model);
			return { ...event.payload, second: true };
		});
		second.handlers = new Map([["before_provider_request", [...second.handlers.get("before_agent_start")!]]]);
		const { runner, errors } = makeRunner([first, second]);

		const result = await runner.emitBeforeProviderRequest({ base: true }, model);

		expect(errors).toEqual([]);
		expect(observed).toEqual([model, model]);
		expect(result).toEqual({ base: true, first: true, second: true });
	});

	it("isolates the routed model snapshot from handlers and transport", async () => {
		const observed: string[] = [];
		const mutating = makeExtension("mutating", (event: any) => {
			expect(Object.isFrozen(event.model)).toBe(true);
			expect(Object.isFrozen(event.model.headers)).toBe(true);
			Reflect.set(event.model.headers, "token", "mutated");
		});
		mutating.handlers = new Map([["before_provider_request", [...mutating.handlers.get("before_agent_start")!]]]);
		const observing = makeExtension("observing", (event: any) => {
			observed.push(event.model.headers.token);
		});
		observing.handlers = new Map([["before_provider_request", [...observing.handlers.get("before_agent_start")!]]]);
		const { runner, errors } = makeRunner([mutating, observing]);

		await runner.emitBeforeProviderRequest({}, model);

		expect(errors).toEqual([]);
		expect(observed).toEqual(["original"]);
		expect(model.headers?.token).toBe("original");
	});
});

describe("ExtensionRunner systemPromptSection validation", () => {
	it("accepts valid section with id, text, and cacheRetention 'none'", async () => {
		const ext = makeExtension("test-ext", () => ({
			systemPromptSection: { id: "scramjet:test", text: "\n\nTest section", cacheRetention: "none" },
		}));
		const { runner, errors } = makeRunner([ext]);

		const result = await runner.emitBeforeAgentStart("hello", undefined, baseSections, {});

		expect(errors).toHaveLength(0);
		expect(result?.systemPromptSections).toHaveLength(1);
		expect(result!.systemPromptSections![0].id).toBe("scramjet:test");
	});

	it("accepts valid section with id and text, no cacheRetention", async () => {
		const ext = makeExtension("test-ext", () => ({
			systemPromptSection: { id: "scramjet:stable", text: "\n\nStable section" },
		}));
		const { runner, errors } = makeRunner([ext]);

		const result = await runner.emitBeforeAgentStart("hello", undefined, baseSections, {});

		expect(errors).toHaveLength(0);
		expect(result?.systemPromptSections).toHaveLength(1);
	});

	it("emits error for section missing text", async () => {
		const ext = makeExtension("bad-ext", () => ({
			systemPromptSection: { id: "scramjet:bad" },
		}));
		const { runner, errors } = makeRunner([ext]);

		const result = await runner.emitBeforeAgentStart("hello", undefined, baseSections, {});

		expect(errors).toHaveLength(1);
		expect(errors[0].extensionPath).toBe("bad-ext");
		expect(errors[0].error).toContain("`id` and `text` must be strings");
		expect(result?.systemPromptSections).toBeUndefined();
	});

	it("emits error for section missing id", async () => {
		const ext = makeExtension("bad-ext", () => ({
			systemPromptSection: { text: "no id here" },
		}));
		const { runner, errors } = makeRunner([ext]);

		const result = await runner.emitBeforeAgentStart("hello", undefined, baseSections, {});

		expect(errors).toHaveLength(1);
		expect(errors[0].error).toContain("`id` and `text` must be strings");
		expect(result?.systemPromptSections).toBeUndefined();
	});

	it("emits error for invalid cacheRetention value", async () => {
		const ext = makeExtension("bad-ext", () => ({
			systemPromptSection: { id: "scramjet:cached", text: "cached", cacheRetention: "long" },
		}));
		const { runner, errors } = makeRunner([ext]);

		const result = await runner.emitBeforeAgentStart("hello", undefined, baseSections, {});

		expect(errors).toHaveLength(1);
		expect(errors[0].error).toContain('`cacheRetention` must be "none" or omitted');
		expect(errors[0].error).toContain('"long"');
		expect(result?.systemPromptSections).toBeUndefined();
	});

	it("emits ordering warning when stable section follows volatile", async () => {
		const volatileExt = makeExtension("volatile-ext", () => ({
			systemPromptSection: { id: "scramjet:volatile", text: "\n\nVolatile", cacheRetention: "none" },
		}));
		const stableExt = makeExtension("stable-ext", () => ({
			systemPromptSection: { id: "scramjet:stable-after", text: "\n\nStable after volatile" },
		}));
		const { runner, errors } = makeRunner([volatileExt, stableExt]);

		const result = await runner.emitBeforeAgentStart("hello", undefined, baseSections, {});

		expect(result?.systemPromptSections).toHaveLength(2);
		const orderWarning = errors.find((e) => e.error.includes("after a volatile one"));
		expect(orderWarning).toBeDefined();
		expect(orderWarning!.extensionPath).toBe("stable-ext");
	});
});

describe("spliceContributedSections", () => {
	it("inserts before the first volatile section", () => {
		const contributed: SystemPromptSection[] = [{ id: "ext", text: "\n\next content" }];

		const result = spliceContributedSections(baseSections, contributed);

		expect(result).toHaveLength(3);
		expect(result[0].id).toBe("base");
		expect(result[1].id).toBe("ext");
		expect(result[2].id).toBe("volatile");
	});

	it("appends when no volatile section exists", () => {
		const stableOnly: SystemPromptSection[] = [{ id: "base", text: "base" }];
		const contributed: SystemPromptSection[] = [{ id: "ext", text: "\n\next" }];

		const result = spliceContributedSections(stableOnly, contributed);

		expect(result).toHaveLength(2);
		expect(result[1].id).toBe("ext");
	});

	it("normalizes text without leading newline", () => {
		const contributed: SystemPromptSection[] = [{ id: "ext", text: "no leading newline" }];

		const result = spliceContributedSections(baseSections, contributed);

		expect(result[1].text).toBe("\n\nno leading newline");
	});

	it("preserves text that starts with newline", () => {
		const contributed: SystemPromptSection[] = [{ id: "ext", text: "\nalready has newline" }];

		const result = spliceContributedSections(baseSections, contributed);

		expect(result[1].text).toBe("\nalready has newline");
	});

	it("does not mutate the base array", () => {
		const original = baseSections.slice();
		const contributed: SystemPromptSection[] = [{ id: "ext", text: "\n\next" }];

		spliceContributedSections(baseSections, contributed);

		expect(baseSections).toEqual(original);
	});
});
