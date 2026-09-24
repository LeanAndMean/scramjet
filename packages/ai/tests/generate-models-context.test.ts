import { runInNewContext } from "node:vm";
import ts from "typescript";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Model } from "../src/types.js";

const initialExitCode = process.exitCode;
const initialCandidatePath = process.env.SCRAMJET_MODEL_CANDIDATE;
const { writeFileSync, committedModels } = vi.hoisted(() => ({
	writeFileSync: vi.fn(),
	committedModels: {} as Record<string, Record<string, unknown>>,
}));
vi.mock("fs", async (importOriginal) => ({ ...(await importOriginal<typeof import("fs")>()), writeFileSync }));
vi.mock("../src/models.generated.js", () => ({ MODELS: committedModels }));

const committedProviderIds = [
	"amazon-bedrock",
	"anthropic",
	"azure-openai-responses",
	"cerebras",
	"cloudflare-ai-gateway",
	"cloudflare-workers-ai",
	"deepseek",
	"fireworks",
	"github-copilot",
	"google",
	"google-vertex",
	"groq",
	"huggingface",
	"kimi-coding",
	"minimax",
	"minimax-cn",
	"mistral",
	"moonshotai",
	"moonshotai-cn",
	"openai",
	"openai-codex",
	"opencode",
	"opencode-go",
	"openrouter",
	"together",
	"vercel-ai-gateway",
	"xai",
	"xiaomi",
	"xiaomi-token-plan-ams",
	"xiaomi-token-plan-cn",
	"xiaomi-token-plan-sgp",
	"zai",
];

function feedModel(id: string, context: number, overrides: Record<string, unknown> = {}) {
	return {
		id,
		name: `Feed ${id}`,
		tool_call: true,
		reasoning: true,
		limit: { context, output: 128000 },
		cost: { input: 0, output: 0, cache_read: 0, cache_write: 0 },
		modalities: { input: ["text", "image"] },
		provider: { npm: id.startsWith("claude") ? "@ai-sdk/anthropic" : "@ai-sdk/openai" },
		...overrides,
	};
}

const copilotAdditions = {
	"claude-fable-5.1": {
		api: "openai-completions",
		contextWindow: 1000000,
		maxInputTokens: 936000,
		maxTokens: 64000,
		cost: { input: 10, output: 50, cacheRead: 0.25, cacheWrite: 12.5 },
		thinkingLevelMap: { off: null, minimal: null, xhigh: "xhigh", max: "max" },
	},
	"claude-opus-5": {
		api: "openai-completions",
		contextWindow: 1000000,
		maxInputTokens: 936000,
		maxTokens: 64000,
		cost: { input: 5, output: 25, cacheRead: 0.5, cacheWrite: 6.25 },
		thinkingLevelMap: { off: null, minimal: null, xhigh: "xhigh", max: "max" },
	},
	"claude-opus-5.5": {
		api: "openai-completions",
		contextWindow: 1000000,
		maxInputTokens: 872000,
		maxTokens: 128000,
		cost: { input: 4, output: 20, cacheRead: 0.2, cacheWrite: 5 },
		thinkingLevelMap: { off: null, minimal: null, xhigh: "xhigh", max: "max" },
	},
	"gpt-6-sol": {
		api: "openai-responses",
		contextWindow: 1000000,
		maxInputTokens: 872000,
		maxTokens: 128000,
		cost: { input: 2, output: 10, cacheRead: 0.2, cacheWrite: 2.5 },
		thinkingLevelMap: { off: "none", minimal: null, xhigh: "xhigh", max: "max" },
	},
	"gpt-6-luna": {
		api: "openai-responses",
		contextWindow: 1000000,
		maxInputTokens: 872000,
		maxTokens: 128000,
		cost: { input: 0.1, output: 0.5, cacheRead: 0.01, cacheWrite: 0.125 },
		thinkingLevelMap: { off: "none", minimal: null, xhigh: "xhigh", max: "max" },
	},
	"kimi-k3": {
		api: "openai-completions",
		contextWindow: 1048576,
		maxInputTokens: 917504,
		maxTokens: 131072,
		cost: { input: 3, output: 15, cacheRead: 0.3, cacheWrite: 0 },
		thinkingLevelMap: { off: null, minimal: null, medium: null, xhigh: null, max: "max" },
	},
	"gemini-3.6-flash": {
		api: "openai-completions",
		contextWindow: 1000000,
		maxInputTokens: 936000,
		maxTokens: 64000,
		cost: { input: 0.75, output: 3.75, cacheRead: 0.07, cacheWrite: 0 },
		thinkingLevelMap: { off: null, xhigh: null, max: null },
	},
	"gemini-3.7-flash": {
		api: "openai-completions",
		contextWindow: 1000000,
		maxInputTokens: 936000,
		maxTokens: 64000,
		cost: { input: 0.75, output: 3.75, cacheRead: 0.07, cacheWrite: 0 },
		thinkingLevelMap: { off: null, minimal: null, xhigh: null, max: null },
	},
	"gemini-3.8-flash": {
		api: "openai-completions",
		contextWindow: 1048576,
		maxInputTokens: 983040,
		maxTokens: 65536,
		cost: { input: 0.75, output: 3.75, cacheRead: 0.07, cacheWrite: 0 },
		thinkingLevelMap: { off: null, minimal: null, xhigh: null, max: null },
	},
	"grok-4.5": {
		api: "openai-responses",
		contextWindow: 500000,
		maxInputTokens: 372000,
		maxTokens: 128000,
		cost: { input: 2, output: 6, cacheRead: 0.5, cacheWrite: 0 },
		thinkingLevelMap: { off: null, minimal: null, xhigh: null, max: null },
	},
	"grok-4.6": {
		api: "openai-responses",
		contextWindow: 500000,
		maxInputTokens: 372000,
		maxTokens: 128000,
		cost: { input: 2, output: 6, cacheRead: 0.5, cacheWrite: 0 },
		thinkingLevelMap: { off: null, minimal: null, xhigh: "xhigh", max: null },
	},
	"mai-code-1.1-flash": {
		api: "openai-responses",
		contextWindow: 256000,
		maxInputTokens: 128000,
		maxTokens: 128000,
		cost: { input: 0.2, output: 1.2, cacheRead: 0.02, cacheWrite: 0.25 },
		thinkingLevelMap: { off: null, minimal: null, xhigh: null, max: null },
	},
} as const;

async function generate(
	present: boolean,
	options: {
		invalidContext?: number;
		omittedCopilotCorrection?: string;
		correctedCopilotOverride?: Record<string, unknown>;
		endpointError?: boolean;
		failedCatalog?: string;
		expectFailure?: boolean;
		openRouterEndpoints?: unknown;
		openRouterEndpointFor?: string;
		openRouterEndpointError?: boolean;
		vercelEndpoints?: unknown;
		unsettledEndpoint?: string;
		unsettledEndpointBody?: string;
		expectedError?: string;
		modelsDevChange?: (data: Record<string, any>) => void;
		openRouterChange?: (data: any[]) => void;
		vercelChange?: (data: any[]) => void;
		candidatePath?: string;
		committedModels?: Record<string, Record<string, unknown>>;
		expectedFetchCount?: number;
	} = {},
) {
	vi.resetModules();
	for (const provider of Object.keys(committedModels)) delete committedModels[provider];
	for (const provider of committedProviderIds) committedModels[provider] = {};
	committedModels["azure-openai-responses"] = Object.fromEntries(
		[
			"gpt-5.1-codex",
			"gpt-5.4",
			"gpt-5.6-luna",
			"gpt-5.6-sol",
			"gpt-5.6-terra",
			...(present ? ["gpt-5.5", "gpt-5-pro"] : []),
		].map((id) => [id, {}]),
	);
	for (const [provider, models] of Object.entries(options.committedModels ?? {})) {
		committedModels[provider] = { ...committedModels[provider], ...models };
	}
	if (options.candidatePath !== undefined) process.env.SCRAMJET_MODEL_CANDIDATE = options.candidatePath;
	else delete process.env.SCRAMJET_MODEL_CANDIDATE;
	writeFileSync.mockClear();
	const errors = vi.spyOn(console, "error").mockImplementation(() => {});
	vi.spyOn(console, "log").mockImplementation(() => {});
	const models = Object.fromEntries(
		[
			"gpt-5.3-codex",
			"gpt-5.4",
			"gpt-5.4-mini",
			"gpt-5.5",
			"gpt-5.6-luna",
			"gpt-5.6-sol",
			"gpt-5.6-terra",
			"gpt-6-astra",
			"gpt-5-pro",
			"mai-code-1-flash-picker",
			"claude-sonnet-4",
			"claude-sonnet-4-5",
		].map((id) => [
			id,
			id === "gpt-5-pro"
				? { ...feedModel(id, 400000), limit: { context: 400000, output: 272000 } }
				: feedModel(
						id,
						id === "gpt-5.4-mini"
							? 400000
							: id === "mai-code-1-flash-picker"
								? 256000
								: id.startsWith("claude")
									? 1000000
									: 1050000,
					),
		]),
	);
	const fetch = vi.fn(async (url: string, init?: RequestInit): Promise<any> => {
		const waitForAbort = () =>
			new Promise((_resolve, reject) => {
				init?.signal?.addEventListener("abort", () => reject(init.signal?.reason), { once: true });
			});
		if (url === options.unsettledEndpoint) return await waitForAbort();
		const endpointJson = (value: unknown) =>
			url === options.unsettledEndpointBody ? waitForAbort : async () => value;
		if (url === options.failedCatalog) return { ok: false, status: 503 };
		if (url.startsWith("https://openrouter.ai/api/v1/models/") && url.endsWith("/endpoints")) {
			if (url.includes("/models/openrouter/") && url.endsWith("/endpoints")) {
				return { ok: true, json: async () => ({ data: { endpoints: [] } }) };
			}
			return {
				ok: !options.openRouterEndpointError,
				status: options.openRouterEndpointError ? 503 : 200,
				json: endpointJson({
					data: {
						endpoints: (options.openRouterEndpoints &&
						(!options.openRouterEndpointFor || url.includes(`/${options.openRouterEndpointFor}/endpoints`))
							? options.openRouterEndpoints
							: undefined) ?? [
							{ context_length: 1500000, max_prompt_tokens: 1200000, supported_parameters: ["tools"] },
							{ context_length: 1400000, max_prompt_tokens: null, supported_parameters: ["tools"] },
						],
					},
				}),
			};
		}
		if (url === "https://ai-gateway.vercel.sh/v1/models/openai/gpt-5.4/endpoints") {
			return {
				ok: !options.endpointError,
				status: options.endpointError ? 503 : 200,
				json: endpointJson({
					data: {
						endpoints: options.vercelEndpoints ?? [
							{ context_length: 1600000, supported_parameters: ["tools"] },
							{ context_length: 2000000, supported_parameters: ["tools"] },
							{ context_length: 3000000, supported_parameters: [] },
						],
					},
				}),
			};
		}
		if (url === "https://models.dev/api.json") {
			const catalog: Record<string, any> = {
				"zai-coding-plan": {
					models: Object.fromEntries(
						["glm-4.7", "glm-5.1", "glm-5-turbo"].map((id) => [id, feedModel(id, 200000)]),
					),
				},
				"cloudflare-ai-gateway": {
					models: {
						"workers-ai/@cf/moonshotai/kimi-k2.6": feedModel("workers-ai/@cf/moonshotai/kimi-k2.6", 256000),
					},
				},
				"fireworks-ai": {
					models: Object.fromEntries(
						["deepseek-v4-flash", "deepseek-v4-pro", "glm-5p1"].map((id) => [
							`accounts/fireworks/models/${id}`,
							feedModel(id, id === "glm-5p1" ? 202800 : 1000000),
						]),
					),
				},
				google: { models: { example: feedModel("example", 500000) } },
				cerebras: { models: { example: feedModel("example", 500000) } },
				groq: {
					models: {
						example: feedModel("example", "invalidContext" in options ? options.invalidContext! : 500000),
					},
				},
				together: { models: { "zai-org/GLM-5.2": feedModel("zai-org/GLM-5.2", 262144) } },
				xai: {
					models: present
						? { "grok-code-fast-1": feedModel("grok-code-fast-1", 32768) }
						: { "fixture-source-model": feedModel("fixture-source-model", 32768) },
				},
				anthropic: { models },
				openai: { models: present ? models : { "other-supported": feedModel("other-supported", 128000) } },
				opencode: { models },
				"opencode-go": { models },
				"github-copilot": {
					models: {
						"gpt-5.2-codex": feedModel("gpt-5.2-codex", 400000),
						...Object.fromEntries(
							Object.keys(copilotAdditions)
								.filter((id) => id !== options.omittedCopilotCorrection)
								.map((id) => [
									id,
									feedModel(
										id,
										id === "gemini-3.8-flash" ? 1000000 : 200000,
										id === "gpt-6-sol" ? options.correctedCopilotOverride : undefined,
									),
								]),
						),
						"deprecated-copilot-candidate": feedModel("deprecated-copilot-candidate", 200000, {
							status: "deprecated",
						}),
						"no-tools-copilot-candidate": feedModel("no-tools-copilot-candidate", 200000, {
							tool_call: false,
						}),
						...(present
							? {
									...models,
									...Object.fromEntries(
										[
											"claude-opus-4.7",
											"claude-opus-4.8",
											"gemini-3.5-flash",
											"claude-fable-5",
											"claude-sonnet-5",
										].map((id) => [id, feedModel(id, 200000)]),
									),
								}
							: {}),
					},
				},
			};
			for (const key of [
				"amazon-bedrock",
				"cloudflare-workers-ai",
				"huggingface",
				"kimi-for-coding",
				"minimax",
				"minimax-cn",
				"mistral",
				"moonshotai",
				"moonshotai-cn",
				"xiaomi",
			]) {
				catalog[key] ??= { models: { "fixture-source-model": feedModel("fixture-source-model", 200000) } };
			}
			options.modelsDevChange?.(catalog);
			return { ok: true, json: endpointJson(catalog) };
		}
		if (url === "https://openrouter.ai/api/v1/models") {
			const freePricing = { prompt: "0", completion: "0", input_cache_read: "0", input_cache_write: "0" };
			const items: any[] = [
				{
					id: "x-ai/grok-code-fast-1",
					context_length: 32768,
					pricing: { ...freePricing },
					supported_parameters: ["tools"],
				},
				{
					id: "openai/gpt-5.4",
					name: "First",
					context_length: 1500000,
					pricing: { ...freePricing },
					supported_parameters: ["tools"],
				},
				{
					id: "openai/gpt-5.5",
					name: "Second",
					context_length: 2000000,
					pricing: { ...freePricing },
					supported_parameters: ["tools"],
				},
			];
			for (const item of items) item.top_provider = { max_completion_tokens: 4096 };
			options.openRouterChange?.(items);
			return { ok: true, json: endpointJson({ data: items }) };
		}
		if (url === "https://ai-gateway.vercel.sh/v1/models") {
			const items: any[] = [
				{
					id: "openai/gpt-5.4",
					context_window: 1600000,
					max_tokens: 4096,
					pricing: { input: "0", output: "0", input_cache_read: "0", input_cache_write: "0" },
					tags: ["tool-use"],
				},
			];
			options.vercelChange?.(items);
			return { ok: true, json: endpointJson({ data: items }) };
		}
		throw new Error(`Unexpected fetch: ${url}`);
	});
	vi.stubGlobal("fetch", fetch);
	await import("../scripts/generate-models.js");
	await vi.waitFor(() => expect(writeFileSync.mock.calls.length + errors.mock.calls.length).toBeGreaterThan(0));
	if (options.expectFailure) {
		expect(errors).toHaveBeenCalled();
		if (options.expectedFetchCount !== undefined) expect(fetch).toHaveBeenCalledTimes(options.expectedFetchCount);
		expect(writeFileSync).not.toHaveBeenCalled();
		expect(process.exitCode).toBe(1);
		if (options.expectedError) {
			const messages: string[] = [];
			let error = errors.mock.calls.at(-1)?.[0];
			while (error instanceof Error) {
				messages.push(error.message);
				error = error.cause;
			}
			expect(messages.join("\n")).toContain(options.expectedError);
		}
		return null;
	}
	if (errors.mock.calls.length) throw errors.mock.calls.at(-1)?.[0];
	expect(writeFileSync).toHaveBeenCalledTimes(1);
	expect(errors).not.toHaveBeenCalled();
	expect(fetch).toHaveBeenCalledTimes(options.expectedFetchCount ?? 7);
	const output = writeFileSync.mock.calls[0][1] as string;
	if (options.candidatePath !== undefined) {
		expect(writeFileSync.mock.calls[0][0]).toBe(options.candidatePath);
		expect(writeFileSync.mock.calls[0][2]).toEqual({ flag: "wx" });
		return JSON.parse(output);
	}
	const compiled = ts.transpileModule(output, { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText;
	const exports: { MODELS?: Record<string, Record<string, Model<any>>> } = {};
	runInNewContext(compiled, { exports });
	return exports.MODELS!;
}

afterEach(() => {
	process.exitCode = initialExitCode;
	if (initialCandidatePath === undefined) delete process.env.SCRAMJET_MODEL_CANDIDATE;
	else process.env.SCRAMJET_MODEL_CANDIDATE = initialCandidatePath;
	vi.restoreAllMocks();
	vi.unstubAllGlobals();
});

describe("real generator context corrections", () => {
	it("emits unresolved cache prices only in a separate review candidate", async () => {
		const candidatePath = "/tmp/scramjet-569-candidate.json";
		const candidate = (await generate(true, {
			candidatePath,
			modelsDevChange: (data) => {
				delete data.google.models.example.cost.cache_write;
			},
			openRouterChange: (items) => {
				delete items[1].pricing.input_cache_read;
			},
			vercelChange: (items) => {
				delete items[0].pricing.input_cache_write;
			},
		}))!;
		expect(candidate.models.google.example.cost.cacheWrite).toBeNull();
		expect(candidate.models.openrouter["openai/gpt-5.4"].cost.cacheRead).toBeNull();
		expect(candidate.models["vercel-ai-gateway"]["openai/gpt-5.4"].cost.cacheWrite).toBeNull();
		expect(candidate.unresolvedCosts).toContainEqual({ provider: "google", id: "example", fields: ["cacheWrite"] });
		expect(candidate.unresolvedCosts).toContainEqual({
			provider: "openrouter",
			id: "openai/gpt-5.4",
			fields: ["cacheRead"],
		});
		expect(candidate.unresolvedCosts).toContainEqual({
			provider: "vercel-ai-gateway",
			id: "openai/gpt-5.4",
			fields: ["cacheWrite"],
		});
		await generate(true, {
			modelsDevChange: (data) => {
				delete data.google.models.example.cost.cache_write;
			},
			expectFailure: true,
			expectedError: "models.dev/google/example cache write",
		});
	});

	it("records missing required source prices without asserting free rates in a review candidate", async () => {
		const candidate = (await generate(true, {
			candidatePath: "/tmp/scramjet-569-missing-input.json",
			modelsDevChange: (data) => {
				delete data.google.models.example.cost.input;
			},
			openRouterChange: (items) => {
				delete items[1].pricing.completion;
			},
			vercelChange: (items) => {
				delete items[0].pricing.input;
			},
		}))!;
		expect(candidate.models.google.example.cost.input).toBeNull();
		expect(candidate.models.openrouter["openai/gpt-5.4"].cost.output).toBeNull();
		expect(candidate.models["vercel-ai-gateway"]["openai/gpt-5.4"].cost.input).toBeNull();
		expect(candidate.unresolvedCosts).toContainEqual({ provider: "google", id: "example", fields: ["input"] });
		await generate(true, {
			modelsDevChange: (data) => {
				delete data.google.models.example.cost.input;
			},
			expectFailure: true,
			expectedError: "models.dev/google/example input",
		});
	});

	it("reports exact committed identities missing from a candidate and blocks canonical replacement", async () => {
		const committedModels = { groq: { example: {} } };
		const modelsDevChange = (data: Record<string, any>) => {
			data.groq.models.kept = feedModel("kept", 500000);
			delete data.groq.models.example;
		};
		const candidate = await generate(true, {
			candidatePath: "/tmp/scramjet-569-truncated.json",
			committedModels,
			modelsDevChange,
		});
		expect(candidate.unexplainedLosses).toEqual(["groq/example"]);
		expect(candidate.models.groq.kept).toBeDefined();
		await generate(true, {
			committedModels,
			modelsDevChange,
			expectFailure: true,
			expectedError: "Unreviewed catalog losses: groq/example",
		});
	});

	it("rejects invalid explicit candidate pricing and endpoint failures without writing", async () => {
		await generate(true, {
			candidatePath: "/tmp/scramjet-569-candidate.json",
			openRouterChange: (items) => {
				items[1].pricing.input_cache_write = "garbage";
			},
			expectFailure: true,
			expectedError: "openrouter/openai/gpt-5.4 cache write",
		});
		await generate(true, {
			candidatePath: "/tmp/scramjet-569-candidate.json",
			openRouterEndpointError: true,
			expectFailure: true,
		});
		await generate(true, {
			candidatePath: "/tmp/scramjet-569-candidate.json",
			openRouterChange: (items) => items.push({ ...items[1] }),
			expectFailure: true,
		});
	});

	it("records unpriced dynamic routers with no endpoints without dropping other records", async () => {
		const dynamic = {
			id: "openrouter/auto",
			name: "Auto Router",
			context_length: 2000000,
			pricing: { prompt: "-1", completion: "-1" },
			supported_parameters: ["tools"],
			top_provider: { max_completion_tokens: null },
		};
		const candidate = (await generate(true, {
			candidatePath: "/tmp/scramjet-569-dynamic.json",
			expectedFetchCount: 9,
			openRouterChange: (items) =>
				items.push(dynamic, {
					...dynamic,
					id: "openrouter/free",
					name: "Free Models Router",
					pricing: { prompt: "0", completion: "0" },
				}),
		}))!;
		expect(candidate.models.openrouter["openrouter/auto"]).toBeUndefined();
		expect(candidate.models.openrouter["openrouter/free"]).toBeUndefined();
		expect(candidate.models.openrouter["openai/gpt-5.4"]).toBeDefined();
		expect(candidate.unresolvedSources).toContainEqual({
			source: "openrouter",
			id: "openrouter/auto",
			reason: "unpriced dynamic route without declared endpoints",
		});
		expect(candidate.unresolvedSources).toContainEqual({
			source: "openrouter",
			id: "openrouter/free",
			reason: "dynamic route without declared output limit",
		});
		await generate(true, {
			openRouterChange: (items) => items.push(dynamic),
			expectFailure: true,
		});
	});

	it("isolates a known zero-limit endpoint only in a review candidate", async () => {
		const candidate = (await generate(true, {
			candidatePath: "/tmp/scramjet-569-zero-limit.json",
			expectedFetchCount: 8,
			openRouterChange: (items) =>
				items.push({
					id: "qwen/qwen3-coder-30b-a3b-instruct",
					context_length: 1500000,
					pricing: { prompt: "0", completion: "0", input_cache_read: "0", input_cache_write: "0" },
					supported_parameters: ["tools"],
					top_provider: { max_completion_tokens: 4096 },
				}),
			openRouterEndpointFor: "qwen/qwen3-coder-30b-a3b-instruct",
			openRouterEndpoints: [
				{ context_length: 1500000, max_prompt_tokens: 1200000, supported_parameters: ["tools"] },
				{
					name: "Amazon Bedrock | qwen/qwen3-coder-30b-a3b-instruct",
					context_length: 0,
					max_completion_tokens: 0,
					supported_parameters: ["tools"],
				},
			],
		}))!;
		expect(candidate.models.openrouter["qwen/qwen3-coder-30b-a3b-instruct"].contextWindow).toBe(1500000);
		expect(candidate.unresolvedSources).toContainEqual({
			source: "openrouter endpoint",
			id: "qwen/qwen3-coder-30b-a3b-instruct/Amazon Bedrock | qwen/qwen3-coder-30b-a3b-instruct",
			reason: "zero declared context and output limits",
		});
		const onlyBadEndpoint = (await generate(true, {
			candidatePath: "/tmp/scramjet-569-only-bad-endpoint.json",
			expectedFetchCount: 8,
			openRouterChange: (items) =>
				items.push({
					id: "qwen/qwen3-coder-30b-a3b-instruct",
					context_length: 1500000,
					pricing: { prompt: "0", completion: "0", input_cache_read: "0", input_cache_write: "0" },
					supported_parameters: ["tools"],
					top_provider: { max_completion_tokens: 4096 },
				}),
			openRouterEndpointFor: "qwen/qwen3-coder-30b-a3b-instruct",
			openRouterEndpoints: [
				{
					name: "Amazon Bedrock | qwen/qwen3-coder-30b-a3b-instruct",
					context_length: 0,
					max_completion_tokens: 0,
					supported_parameters: ["tools"],
				},
			],
		}))!;
		expect(onlyBadEndpoint.models.openrouter["qwen/qwen3-coder-30b-a3b-instruct"]).toBeUndefined();
		expect(onlyBadEndpoint.unresolvedSources).toContainEqual({
			source: "openrouter",
			id: "qwen/qwen3-coder-30b-a3b-instruct",
			reason: "no valid declared endpoints",
		});
		await generate(true, {
			openRouterEndpoints: [
				{
					name: "Amazon Bedrock | qwen/qwen3-coder-30b-a3b-instruct",
					context_length: 0,
					max_completion_tokens: 0,
					supported_parameters: ["tools"],
				},
			],
			expectFailure: true,
		});
	});

	it("separates Vercel non-text records from language records with missing tags", async () => {
		const candidate = (await generate(true, {
			candidatePath: "/tmp/scramjet-569-vercel-tags.json",
			vercelChange: (items) => {
				items.push({ id: "example/embed", type: "embedding", pricing: { input: "1" } });
				items.push({ id: "example/language", type: "language", pricing: { input: "1" } });
			},
		}))!;
		expect(candidate.models["vercel-ai-gateway"]["example/embed"]).toBeUndefined();
		expect(candidate.models["vercel-ai-gateway"]["example/language"]).toBeUndefined();
		expect(candidate.unresolvedSources).toContainEqual({
			source: "vercel-ai-gateway",
			id: "example/language",
			reason: "language model without capability tags",
		});
		await generate(true, {
			vercelChange: (items) => items.push({ id: "example/language", type: "language" }),
			expectFailure: true,
		});
	});

	it("rejects an explicitly empty candidate path before acquisition or writing", async () => {
		await generate(true, {
			candidatePath: "",
			expectFailure: true,
			expectedFetchCount: 0,
			expectedError: "Candidate output must be an absolute path outside the canonical catalog",
		});
	});

	it("rejects candidate output directed at the canonical snapshot", async () => {
		await generate(true, {
			candidatePath: new URL("../src/models.generated.ts", import.meta.url).pathname,
			expectFailure: true,
			expectedError: "Candidate output must be an absolute path outside the canonical catalog",
		});
	});

	it("reports missing previously supported optional models.dev sections in the candidate", async () => {
		const candidate = await generate(true, {
			candidatePath: "/tmp/scramjet-569-missing-section.json",
			modelsDevChange: (data) => {
				delete data["kimi-for-coding"];
				delete data["zai-coding-plan"];
				delete data.together;
			},
		});
		expect(candidate.unresolvedSources).toContainEqual({
			source: "models.dev/kimi-for-coding",
			id: "*",
			reason: "Previously supported source section missing",
		});
		expect(candidate.unresolvedSources).toContainEqual({
			source: "models.dev/zai-coding-plan",
			id: "*",
			reason: "Previously supported source section missing",
		});
		const togetherGaps = candidate.unresolvedSources.filter((entry: { source: string }) =>
			entry.source.startsWith("models.dev/together"),
		);
		expect(togetherGaps).toEqual([
			{ source: "models.dev/together", id: "*", reason: "Previously supported source section missing" },
		]);
	});

	it.each(["absent", "empty", "non-tool"])("guards previously supported optional source when %s", async (state) => {
		const modelsDevChange = (data: Record<string, any>) => {
			if (state === "absent") delete data["kimi-for-coding"];
			else
				data["kimi-for-coding"].models =
					state === "empty"
						? {}
						: { "fixture-source-model": feedModel("fixture-source-model", 200000, { tool_call: false }) };
		};
		const reason =
			state === "absent"
				? "Previously supported source section missing"
				: "Previously supported source section has no usable models";
		const candidate = await generate(true, { candidatePath: "/tmp/scramjet-569-optional-gap.json", modelsDevChange });
		expect(candidate.unresolvedSources).toContainEqual({ source: "models.dev/kimi-for-coding", id: "*", reason });
		await generate(true, {
			modelsDevChange,
			expectFailure: true,
			expectedError: `models.dev/kimi-for-coding: ${reason}`,
		});
	});

	it("allows intentionally empty optional sections without supported built-ins and alternate Together aliases", async () => {
		const models = (await generate(true, {
			modelsDevChange: (data) => {
				data.togetherai = { models: {} };
				data["together-ai"] = { models: {} };
			},
		}))!;
		expect(models.together["zai-org/GLM-5.2"]).toBeDefined();
	});

	it("selects a usable Together alias when the primary section is empty", async () => {
		const modelsDevChange = (data: Record<string, any>) => {
			data.together.models = {};
			data.togetherai = { models: { "zai-org/GLM-5.2": feedModel("zai-org/GLM-5.2", 262144) } };
		};
		const models = (await generate(true, { modelsDevChange }))!;
		expect(models.together["zai-org/GLM-5.2"]).toBeDefined();
		const candidate = await generate(true, {
			candidatePath: "/tmp/scramjet-569-together-alias.json",
			modelsDevChange,
		});
		expect(candidate.unresolvedSources).not.toContainEqual(
			expect.objectContaining({ source: "models.dev/together" }),
		);
	});

	it("rejects loss of required models.dev sections and malformed optional sections", async () => {
		await generate(true, {
			modelsDevChange: (data) => {
				delete data.anthropic;
			},
			expectFailure: true,
			expectedError: "models.dev/anthropic",
		});
		await generate(true, {
			modelsDevChange: (data) => {
				data.google.models = {};
			},
			expectFailure: true,
			expectedError: "models.dev/google: no usable models",
		});
		await generate(true, {
			modelsDevChange: (data) => {
				data.mistral = { models: [] };
			},
			expectFailure: true,
			expectedError: "models.dev/mistral",
		});
	});

	it.each([undefined, "garbage", -1, Infinity])(
		"rejects missing or invalid OpenRouter prompt price %s",
		async (prompt) => {
			await generate(true, {
				openRouterChange: (items) => {
					items[1].pricing = { prompt, completion: "0" };
				},
				expectFailure: true,
				expectedError: "openrouter/openai/gpt-5.4",
			});
		},
	);

	it("rejects whitespace prices instead of interpreting them as free", async () => {
		await generate(true, {
			openRouterChange: (items) => {
				items[1].pricing.prompt = "  ";
			},
			expectFailure: true,
			expectedError: "openrouter/openai/gpt-5.4 prompt",
		});
	});

	it("validates selected Together aliases but ignores intentionally excluded records", async () => {
		await generate(true, {
			modelsDevChange: (data) => {
				data.togetherai = { models: { broken: feedModel("broken", 100000, { cost: undefined }) } };
				delete data.together;
			},
			expectFailure: true,
			expectedError: "models.dev/togetherai/broken",
		});
		const models = (await generate(true, {
			modelsDevChange: (data) => {
				data["github-copilot"].models["deprecated-copilot-candidate"].cost = undefined;
			},
		}))!;
		expect(models["github-copilot"]["deprecated-copilot-candidate"]).toBeUndefined();
	});

	it("accepts explicit zero price and rejects missing Vercel output price", async () => {
		const models = (await generate(true, {
			vercelChange: (items) => {
				items[0].pricing = { input: "0", output: "0", input_cache_read: "0", input_cache_write: "0" };
			},
		}))!;
		expect(models["vercel-ai-gateway"]["openai/gpt-5.4"].cost.input).toBe(0);
		await generate(true, {
			vercelChange: (items) => {
				items[0].pricing = { input: "0", input_cache_read: "0", input_cache_write: "0" };
			},
			expectFailure: true,
			expectedError: "vercel-ai-gateway/openai/gpt-5.4",
		});
	});

	it("rejects absent output ceilings instead of synthesizing 4096", async () => {
		await generate(true, {
			openRouterChange: (items) => {
				delete items[0].top_provider;
			},
			expectFailure: true,
			expectedError: "openrouter/x-ai/grok-code-fast-1",
		});
		await generate(true, {
			vercelChange: (items) => {
				delete items[0].max_tokens;
			},
			expectFailure: true,
			expectedError: "vercel-ai-gateway/openai/gpt-5.4",
		});
	});

	it("rejects duplicate normalized models.dev aliases", async () => {
		await generate(true, {
			modelsDevChange: (data) => {
				data["kimi-for-coding"] = { models: { k2p5: feedModel("k2p5", 256000), k2p6: feedModel("k2p6", 256000) } };
			},
			expectFailure: true,
			expectedError: "Duplicate kimi-coding/kimi-for-coding",
		});
	});

	it("rejects duplicate route identities before writing", async () => {
		await generate(true, {
			openRouterChange: (items) => {
				items.push({ ...items[1] });
			},
			expectFailure: true,
			expectedError: "Duplicate openrouter/openai/gpt-5.4",
		});
	});

	it("round-trips hostile source identifiers and names as literal strings", async () => {
		const id = 'unsafe"\\n\\\\model';
		const name = 'value"\\n}; globalThis.injected = true; //';
		const models = (await generate(true, {
			modelsDevChange: (data) => {
				data.groq.models[id] = feedModel(id, 500000, { name });
			},
		}))!;
		expect(models.groq[id].name).toBe(name);
	});

	it("rejects invalid model output and cost before writing", async () => {
		await generate(true, {
			modelsDevChange: (data) => {
				data.groq.models.example.limit.output = 0;
			},
			expectFailure: true,
			expectedError: "groq/example",
		});
		await generate(true, {
			modelsDevChange: (data) => {
				data.groq.models.example.cost = { input: -1 };
			},
			expectFailure: true,
			expectedError: "groq/example",
		});
	});

	it.each([
		["reasoning", "true", "invalid reasoning capability"],
		["input modalities", "image", "invalid input modalities"],
		["input modality entries", ["text", 1], "invalid input modalities"],
	] as const)("rejects malformed models.dev %s before writing", async (_label, value, expectedError) => {
		await generate(true, {
			modelsDevChange: (data) => {
				if (_label === "reasoning") data.groq.models.example.reasoning = value;
				else data.groq.models.example.modalities.input = value;
			},
			expectFailure: true,
			expectedError,
		});
	});
	it("rejects malformed models.dev tool capability before writing", async () => {
		await generate(true, {
			modelsDevChange: (data) => {
				data.groq.models["invalid-tools"] = feedModel("invalid-tools", 500000, { tool_call: "true" });
			},
			expectFailure: true,
			expectedError: "models.dev/groq/invalid-tools: invalid tool capability",
		});
	});

	it("uses Azure-specific GPT-6 Responses limits without including unsupported routes", async () => {
		const models = (await generate(true, {
			modelsDevChange: (data) => {
				for (const id of ["gpt-6-luna", "gpt-6-sol", "gpt-7-unreviewed"]) {
					data.openai.models[id] = feedModel(id, 1050000);
				}
			},
		}))!;
		for (const id of ["gpt-6-luna", "gpt-6-sol"]) {
			expect(models.openai[id].thinkingLevelMap).toMatchObject({
				off: "none",
				minimal: null,
				xhigh: "xhigh",
				max: "max",
			});
		}
		for (const [id, cost] of Object.entries({
			"gpt-6-astra": { input: 10, output: 50, cacheRead: 1, cacheWrite: 12.5 },
			"gpt-6-luna": { input: 0.1, output: 0.5, cacheRead: 0.01, cacheWrite: 0.125 },
			"gpt-6-sol": { input: 2, output: 10, cacheRead: 0.2, cacheWrite: 2.5 },
		})) {
			expect(models["azure-openai-responses"][id]).toMatchObject({
				cost,
				api: "azure-openai-responses",
				provider: "azure-openai-responses",
				contextWindow: 1050000,
				maxInputTokens: 922000,
				maxTokens: 128000,
			});
		}
		for (const id of ["gpt-6-luna", "gpt-6-sol"]) {
			expect(models["azure-openai-responses"][id].thinkingLevelMap).toBeUndefined();
		}
		expect(models.openai["gpt-6-sol"].cost.input).toBe(0);
		expect(models["azure-openai-responses"]["gpt-6-sol"].cost.input).toBe(2);
		expect(models.openai["gpt-7-unreviewed"]).toBeDefined();
		expect(models["azure-openai-responses"]["gpt-7-unreviewed"]).toBeUndefined();
		expect(models["azure-openai-responses"]["gpt-realtime-2.1"]).toBeUndefined();
	});

	it("does not offer a Realtime-only OpenAI model through Responses", async () => {
		const models = (await generate(true, {
			modelsDevChange: (data) => {
				data.openai.models["gpt-realtime-2.1"] = feedModel("gpt-realtime-2.1", 128000);
			},
		}))!;
		expect(models.openai["gpt-realtime-2.1"]).toBeUndefined();
		expect(models["azure-openai-responses"]["gpt-realtime-2.1"]).toBeUndefined();
	});

	it("reconciles Codex's listed GPT-6 routes and retired GPT-5.4 IDs", async () => {
		const models = (await generate(true))!["openai-codex"];
		for (const id of ["gpt-6-sol", "gpt-6-luna"]) {
			expect(models[id], id).toMatchObject({
				api: "openai-codex-responses",
				baseUrl: "https://chatgpt.com/backend-api",
				contextWindow: 872000,
				maxTokens: 128000,
				thinkingLevelMap: { max: "max", xhigh: "xhigh" },
			});
		}
		for (const id of ["gpt-5.4", "gpt-5.4-mini"]) expect(models[id]).toBeUndefined();
	});

	it("includes documented tool-capable Vertex routes and omits retired or tool-incapable models", async () => {
		const models = (await generate(true))!["google-vertex"];
		for (const id of [
			"gemini-3.1-flash-lite",
			"gemini-3.5-flash",
			"gemini-3.5-flash-lite",
			"gemini-3.6-flash",
			"gemini-3.7-flash",
			"gemini-3.8-flash",
		]) {
			expect(models[id], id).toMatchObject({
				api: "google-vertex",
				contextWindow: 1048576,
				maxTokens: 65536,
			});
		}
		for (const id of ["gemini-2.0-flash", "gemini-2.0-flash-lite", "gemini-3.8-flash-cyber"]) {
			expect(models[id]).toBeUndefined();
		}
	});

	it("preserves provider-documented OpenCode Go Qwen 3.7 Messages routing", async () => {
		const models = (await generate(true, {
			modelsDevChange: (data) => {
				for (const id of ["qwen3.7-plus", "qwen3.7-max"]) {
					data["opencode-go"].models[id] = feedModel(id, 1000000, { provider: undefined });
				}
			},
		}))!["opencode-go"];
		for (const id of ["qwen3.7-plus", "qwen3.7-max"]) {
			expect(models[id]).toMatchObject({
				api: "anthropic-messages",
				baseUrl: "https://opencode.ai/zen/go",
			});
		}
	});

	it("emits exact verified GitHub Copilot additions", async () => {
		const models = (await generate(true))!["github-copilot"];
		for (const [id, expected] of Object.entries(copilotAdditions)) {
			expect(models[id], id).toMatchObject({
				id,
				name: id,
				provider: "github-copilot",
				baseUrl: "https://api.individual.githubcopilot.com",
				reasoning: true,
				input: ["text", "image"],
				headers: {
					"User-Agent": "GitHubCopilotChat/0.35.0",
					"Editor-Version": "vscode/1.107.0",
					"Editor-Plugin-Version": "copilot-chat/0.35.0",
					"Copilot-Integration-Id": "vscode-chat",
					"X-GitHub-Api-Version": "2026-06-01",
				},
				...expected,
			});
			expect(models[id].thinkingLevelMap).toEqual(expected.thinkingLevelMap);
			if (expected.api === "openai-completions") {
				expect(models[id].compat).toEqual({
					supportsStore: false,
					supportsDeveloperRole: false,
					supportsReasoningEffort: true,
				});
			} else {
				expect(models[id]).not.toHaveProperty("compat");
			}
		}
		expect(models["deprecated-copilot-candidate"]).toBeUndefined();
		expect(models["no-tools-copilot-candidate"]).toBeUndefined();
	});

	it("rejects a missing corrected GitHub Copilot candidate before writing", async () => {
		await generate(true, {
			omittedCopilotCorrection: "kimi-k3",
			expectFailure: true,
			expectedError: "Missing corrected GitHub Copilot candidates: kimi-k3",
		});
	});

	it.each([
		["deprecated", { status: "deprecated" }],
		["without tool calls", { tool_call: false }],
	])("rejects corrected GitHub Copilot candidates that are %s before writing", async (_label, override) => {
		await generate(true, {
			correctedCopilotOverride: override,
			expectFailure: true,
			expectedError: "Missing corrected GitHub Copilot candidates: gpt-6-sol",
		});
	});

	it.each([true, false])("preserves route scope with feed-present=%s", async (present) => {
		const models = (await generate(present))!;
		expect(models.together["zai-org/GLM-5.2"].contextWindow).toBe(1000000);
		expect(models["cloudflare-ai-gateway"]["workers-ai/@cf/moonshotai/kimi-k2.6"].contextWindow).toBe(262144);
		for (const id of ["deepseek-v4-flash", "deepseek-v4-pro"]) {
			expect(models.fireworks[`accounts/fireworks/models/${id}`].contextWindow).toBe(1048576);
		}
		expect(models.fireworks["accounts/fireworks/models/glm-5p1"].contextWindow).toBe(202752);
		for (const id of ["glm-4.7", "glm-5.1"]) expect(models.zai[id].contextWindow).toBe(1000000);
		expect(models.zai["glm-5-turbo"].contextWindow).toBe(200000);
		expect(models["amazon-bedrock"]["eu.anthropic.claude-opus-4-6-v1"].contextWindow).toBe(1000000);
		for (const model of Object.values(models["github-copilot"])) {
			expect(model.headers?.["X-GitHub-Api-Version"]).toBe("2026-06-01");
		}
		if (present) {
			for (const id of [
				"claude-opus-4.7",
				"claude-opus-4.8",
				"gemini-3.5-flash",
				"claude-fable-5",
				"claude-sonnet-5",
			]) {
				expect(models["github-copilot"][id].contextWindow).toBe(1000000);
			}
			for (const id of ["claude-fable-5", "claude-sonnet-5"]) {
				expect(models["github-copilot"][id].maxTokens).toBe(64000);
			}
		}
		expect(models.xai["grok-code-fast-1"]).toMatchObject({
			contextWindow: 256000,
			maxTokens: present ? 128000 : 8192,
		});
		expect(models.xai["grok-3"].contextWindow).toBe(131072);
		expect(models.xai["grok-3-fast"].contextWindow).toBe(131072);
		expect(models.openrouter["x-ai/grok-code-fast-1"].contextWindow).toBe(32768);
		for (const provider of ["openai", "azure-openai-responses"]) {
			expect(models[provider]["gpt-5.4"].contextWindow).toBe(1050000);
			if (present) expect(models[provider]["gpt-5.5"].contextWindow).toBe(1050000);
			else expect(models[provider]["gpt-5.5"]).toBeUndefined();
		}
		expect(models.openai["gpt-6-astra"].contextWindow).toBe(1050000);
		expect(models.openai["gpt-6-astra"]).not.toHaveProperty("contextWindowBudget");
		expect(models["azure-openai-responses"]["gpt-6-astra"]).toMatchObject({
			contextWindow: 1050000,
			maxInputTokens: 922000,
			maxTokens: 128000,
		});
		expect(models.opencode["gpt-5.4"].contextWindow).toBe(1050000);
		expect(models.opencode["claude-sonnet-4-5"].contextWindow).toBe(200000);
		expect(models.opencode["claude-sonnet-4"].contextWindow).toBe(200000);
		expect(models.anthropic["claude-sonnet-4-5"].contextWindow).toBe(200000);
		expect(models["opencode-go"]["gpt-5.4"].contextWindow).toBe(1050000);
		for (const id of ["claude-sonnet-4", "claude-sonnet-4-5"]) {
			expect(models["opencode-go"][id].contextWindow).toBe(1000000);
		}
		expect(models["github-copilot"]["gpt-6-astra"]).toMatchObject({
			contextWindow: 1000000,
			maxTokens: 128000,
		});
		expect(models["github-copilot"]["gpt-5.3-codex"].contextWindow).toBe(1000000);
		for (const records of Object.values(models)) {
			for (const model of Object.values(records)) expect(model).not.toHaveProperty("contextWindowBudget");
		}
		const codexContexts: Record<string, number> = {
			"gpt-5.1": 400000,
			"gpt-5.1-codex-max": 400000,
			"gpt-5.1-codex-mini": 400000,
			"gpt-5.2": 400000,
			"gpt-5.2-codex": 400000,
			"gpt-5.3-codex": 272000,
			"gpt-5.3-codex-spark": 128000,
			"gpt-5.5": 272000,
			"gpt-5.6-sol": 872000,
			"gpt-5.6-terra": 872000,
			"gpt-5.6-luna": 872000,
			"gpt-6-astra": 872000,
			"gpt-6-sol": 872000,
			"gpt-6-luna": 872000,
		};
		for (const model of Object.values(models["openai-codex"])) {
			expect(model.contextWindow, model.id).toBe(codexContexts[model.id]);
			expect(model.maxTokens).toBe(128000);
		}
		expect(models.openrouter["openai/gpt-5.4"]).toMatchObject({
			name: "First",
			contextWindow: 1500000,
			maxInputTokens: 1400000,
		});
		expect(models.openai["gpt-5.4"]).not.toHaveProperty("maxInputTokens");
		expect(models["azure-openai-responses"]["gpt-5.4"].maxInputTokens).toBe(922000);
		expect(models["azure-openai-responses"]["gpt-5.1-codex"].maxInputTokens).toBe(272000);
		for (const id of ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]) {
			expect(models["azure-openai-responses"][id].maxInputTokens).toBe(922000);
			expect(models["openai-codex"][id]).not.toHaveProperty("maxInputTokens");
		}
		const copilotInputLimits = {
			"claude-fable-5": 936000,
			"claude-opus-4.7": 936000,
			"claude-opus-4.8": 936000,
			"claude-sonnet-5": 936000,
			"gemini-3.5-flash": 936000,
			"gpt-5.4": 922000,
			"gpt-5.4-mini": 272000,
			"gpt-5.5": 922000,
			"gpt-5.6-luna": 922000,
			"gpt-5.6-sol": 922000,
			"gpt-5.6-terra": 922000,
			"gpt-6-astra": 872000,
			"mai-code-1-flash-picker": 128000,
		};
		if (present) {
			for (const [id, maxInputTokens] of Object.entries(copilotInputLimits)) {
				expect(models["github-copilot"][id].maxInputTokens, id).toBe(maxInputTokens);
			}
			expect(models["azure-openai-responses"]["gpt-5.5"]).toMatchObject({
				maxInputTokens: 922000,
				requestLimits: [
					{ maxTotalTokens: 922000, maxInputTokens: 922000, maxOutputTokens: 128000, supportsTools: true },
				],
			});
			expect(models["azure-openai-responses"]["gpt-5-pro"].maxTokens).toBe(128000);
			expect(models.openai["gpt-5-pro"].maxTokens).toBe(272000);
		}
		expect(models["github-copilot"]["gpt-6-astra"].maxInputTokens).toBe(872000);
		expect(models["github-copilot"]["gpt-5.3-codex"]).not.toHaveProperty("maxInputTokens");
		expect(models["vercel-ai-gateway"]["openai/gpt-5.4"].contextWindow).toBe(2000000);
		expect(models.openai["gpt-5.4"].name).toBe(present ? "Feed gpt-5.4" : "GPT-5.4");
	});

	it("records an empty OpenRouter endpoint aggregate only in candidate output", async () => {
		const openRouterEndpoints: unknown[] = [];
		const candidate = await generate(true, {
			candidatePath: "/tmp/scramjet-569-empty-openrouter-endpoints.json",
			openRouterEndpoints,
			openRouterEndpointFor: "openai/gpt-5.4",
		});
		expect(candidate.models.openrouter["openai/gpt-5.4"]).toBeUndefined();
		expect(candidate.unresolvedSources).toContainEqual({
			source: "openrouter",
			id: "openai/gpt-5.4",
			reason: "no valid declared endpoints",
		});
		await generate(true, {
			openRouterEndpoints,
			openRouterEndpointFor: "openai/gpt-5.4",
			expectFailure: true,
			expectedError: "openrouter/openai/gpt-5.4: no valid declared endpoints",
		});
	});

	it.each([
		{ openRouterEndpoints: [{ context_length: 1500000, supported_parameters: ["tools"] }] },
		{
			openRouterEndpoints: [
				{ context_length: 1500000, max_prompt_tokens: 1600000, supported_parameters: ["tools"] },
			],
		},
	])("does not invent an independent input cap from $openRouterEndpoints", async ({ openRouterEndpoints }) => {
		const models = (await generate(true, { openRouterEndpoints }))!;
		expect(models.openrouter["openai/gpt-5.4"].contextWindow).toBe(1500000);
		expect(models.openrouter["openai/gpt-5.4"]).not.toHaveProperty("maxInputTokens");
		expect(models.openrouter["openai/gpt-5.4"]).not.toHaveProperty("requestLimits");
	});

	it.each([0, -1, Number.NaN, Number.POSITIVE_INFINITY, "98304"])(
		"rejects invalid endpoint input limit %s before writing",
		async (max_prompt_tokens) => {
			await generate(true, {
				openRouterEndpoints: [{ context_length: 131072, max_prompt_tokens, supported_parameters: ["tools"] }],
				expectFailure: true,
			});
		},
	);

	it.each([
		{ openRouterEndpoints: {} },
		{ openRouterEndpoints: [null] },
		{ openRouterEndpoints: [{ max_prompt_tokens: 98304 }] },
	])("rejects malformed endpoint constraints $openRouterEndpoints before writing", async ({ openRouterEndpoints }) => {
		await generate(true, { openRouterEndpoints, expectFailure: true });
	});

	it("records a Vercel tool-capability contradiction only in candidate output", async () => {
		const vercelEndpoints = [{ context_length: 1600000, supported_parameters: [] }];
		const candidate = await generate(true, {
			candidatePath: "/tmp/scramjet-569-no-tools-vercel.json",
			vercelEndpoints,
		});
		expect(candidate.models["vercel-ai-gateway"]?.["openai/gpt-5.4"]).toBeUndefined();
		expect(candidate.unresolvedSources).toContainEqual({
			source: "vercel-ai-gateway",
			id: "openai/gpt-5.4",
			reason: "no tool-capable endpoints",
		});
		await generate(true, {
			vercelEndpoints,
			expectFailure: true,
			expectedError: "Unresolved endpoint context maximum for openai/gpt-5.4",
		});
	});

	it("retains Vercel joint output constraints without selecting a smaller total context", async () => {
		const models = (await generate(true, {
			vercelEndpoints: [
				{ context_length: 1600000, max_completion_tokens: 4096, tags: ["tool-use"] },
				{ context_length: 2000000, max_completion_tokens: 2048, supported_parameters: ["tools"] },
			],
		}))!;
		expect(models["vercel-ai-gateway"]["openai/gpt-5.4"]).toMatchObject({
			contextWindow: 2000000,
			maxTokens: 4096,
			requestLimits: [
				{ maxTotalTokens: 1600000, maxOutputTokens: 4096, supportsTools: true },
				{ maxTotalTokens: 2000000, maxOutputTokens: 2048, supportsTools: true },
			],
		});
	});
	it("deduplicates equivalent constraints and omits non-binding endpoint metadata", async () => {
		const endpoint = { context_length: 1000000, max_completion_tokens: 2048, supported_parameters: ["tools"] };
		const models = (await generate(true, { openRouterEndpoints: [endpoint, endpoint] }))!;
		expect(models.openrouter["openai/gpt-5.4"].requestLimits).toHaveLength(1);
		const unbound = (await generate(true, {
			openRouterEndpoints: [{ ...endpoint, context_length: 1500000, max_completion_tokens: 4096 }, endpoint],
		}))!;
		expect(unbound.openrouter["openai/gpt-5.4"].requestLimits).toBeUndefined();
	});
	it.each([null, 0, Infinity, "8192"])("rejects malformed Vercel endpoint declaration %s", async (value) => {
		await generate(true, {
			vercelEndpoints: [
				value === null
					? null
					: {
							context_length: 1500000,
							max_completion_tokens: value,
							supported_parameters: ["tools"],
						},
			],
			expectFailure: true,
		});
	});
	it.each([undefined, null, "tools", [1]])(
		"rejects unqualified endpoint capabilities %s",
		async (supported_parameters) => {
			await generate(true, {
				openRouterEndpoints: [{ context_length: 131072, supported_parameters }],
				expectFailure: true,
			});
		},
	);
	it("does not publish an OpenRouter aggregate whose endpoints all deny tools", async () => {
		const openRouterEndpoints = [{ context_length: 40960, max_completion_tokens: 2048, supported_parameters: [] }];
		const candidate = await generate(true, {
			candidatePath: "/tmp/scramjet-569-no-tools.json",
			openRouterEndpoints,
			openRouterEndpointFor: "openai/gpt-5.4",
		});
		expect(candidate.models.openrouter["openai/gpt-5.4"]).toBeUndefined();
		expect(candidate.unresolvedSources).toContainEqual({
			source: "openrouter",
			id: "openai/gpt-5.4",
			reason: "no tool-capable endpoints",
		});
		await generate(true, {
			openRouterEndpoints,
			openRouterEndpointFor: "openai/gpt-5.4",
			expectFailure: true,
			expectedError: "openrouter/openai/gpt-5.4: no tool-capable endpoints",
		});
	});

	it("retains joint endpoint constraints and tool capability instead of independent maxima", async () => {
		const models = (await generate(true, {
			openRouterEndpoints: [
				{ context_length: 40960, max_completion_tokens: 36864, supported_parameters: [] },
				{ context_length: 40960, max_completion_tokens: 16384, supported_parameters: ["tools"] },
				{
					context_length: 131072,
					max_prompt_tokens: 98304,
					max_completion_tokens: 8192,
					supported_parameters: ["tools"],
				},
			],
		}))!;
		expect(models.openrouter["openai/gpt-5.4"].requestLimits).toEqual([
			{ maxTotalTokens: 40960, maxOutputTokens: 36864, supportsTools: false },
			{ maxTotalTokens: 40960, maxOutputTokens: 16384, supportsTools: true },
			{ maxTotalTokens: 131072, maxInputTokens: 98304, maxOutputTokens: 8192, supportsTools: true },
		]);
		expect(models.openai["gpt-5.4"].requestLimits).toBeUndefined();
	});
	it.each([0, -1, Infinity, NaN, "8192"])(
		"rejects invalid endpoint output %s before writing",
		async (max_completion_tokens) => {
			await generate(true, {
				openRouterEndpoints: [{ context_length: 131072, max_completion_tokens, supported_parameters: ["tools"] }],
				expectFailure: true,
			});
		},
	);
	it("does not emit a partial catalog when OpenRouter endpoint acquisition fails", async () => {
		await generate(true, { openRouterEndpointError: true, expectFailure: true });
	});

	describe.each([
		{
			endpoint: "https://models.dev/api.json",
			error: "models.dev catalog acquisition timed out after 30000ms",
		},
		{
			endpoint: "https://openrouter.ai/api/v1/models",
			error: "OpenRouter model catalog acquisition timed out after 30000ms",
		},
		{
			endpoint: "https://ai-gateway.vercel.sh/v1/models",
			error: "Vercel AI Gateway model catalog acquisition timed out after 30000ms",
		},
		{
			endpoint: "https://openrouter.ai/api/v1/models/openai/gpt-5.4/endpoints",
			error: "openrouter/openai/gpt-5.4: endpoint discovery timed out after 30000ms",
		},
		{
			endpoint: "https://ai-gateway.vercel.sh/v1/models/openai/gpt-5.4/endpoints",
			error: "vercel-ai-gateway/openai/gpt-5.4: endpoint discovery timed out after 30000ms",
		},
	])("$endpoint timeout", ({ endpoint, error }) => {
		it.each(["fetch", "body"] as const)("times out an unsettled %s without writing", async (phase) => {
			const nativeTimeout = AbortSignal.timeout.bind(AbortSignal);
			const timeout = vi.spyOn(AbortSignal, "timeout").mockImplementation((duration) => {
				expect(duration).toBe(30_000);
				return nativeTimeout(1);
			});
			await generate(true, {
				...(phase === "fetch" ? { unsettledEndpoint: endpoint } : { unsettledEndpointBody: endpoint }),
				expectedError: error,
				expectFailure: true,
			});
			expect(timeout).toHaveBeenCalled();
		});
	});

	it.each([undefined, 0, -1, Number.NaN, Number.POSITIVE_INFINITY])(
		"does not invent a context maximum for invalid source value %s",
		async (invalidContext) => {
			await generate(true, { invalidContext, expectFailure: true });
		},
	);

	it("does not emit a partial catalog when endpoint acquisition fails", async () => {
		await generate(true, { endpointError: true, expectFailure: true });
	});

	it.each([
		"https://models.dev/api.json",
		"https://openrouter.ai/api/v1/models",
		"https://ai-gateway.vercel.sh/v1/models",
	])("does not emit a partial catalog when %s fails", async (failedCatalog) => {
		await generate(true, { failedCatalog, expectFailure: true });
	});
});
