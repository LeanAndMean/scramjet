import type { SystemPromptSection } from "@leanandmean/ai";
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
