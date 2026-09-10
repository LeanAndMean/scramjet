import { runInNewContext } from "node:vm";
import ts from "typescript";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Model } from "../src/types.js";

const initialExitCode = process.exitCode;
const { writeFileSync } = vi.hoisted(() => ({ writeFileSync: vi.fn() }));
vi.mock("fs", async (importOriginal) => ({ ...(await importOriginal<typeof import("fs")>()), writeFileSync }));

function feedModel(id: string, context: number) {
	return {
		id,
		name: `Feed ${id}`,
		tool_call: true,
		reasoning: true,
		limit: { context, output: 128000 },
		provider: { npm: id.startsWith("claude") ? "@ai-sdk/anthropic" : "@ai-sdk/openai" },
	};
}

async function generate(
	present: boolean,
	options: {
		invalidContext?: number;
		endpointError?: boolean;
		failedCatalog?: string;
		expectFailure?: boolean;
		openRouterEndpoints?: unknown;
		openRouterEndpointError?: boolean;
		vercelEndpoints?: unknown;
		unsettledEndpoint?: string;
		unsettledEndpointBody?: string;
		expectedError?: string;
	} = {},
) {
	vi.resetModules();
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
			return {
				ok: !options.openRouterEndpointError,
				status: options.openRouterEndpointError ? 503 : 200,
				json: endpointJson({
					data: {
						endpoints: options.openRouterEndpoints ?? [
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
			return {
				ok: true,
				json: async () => ({
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
					groq: {
						models: {
							example: feedModel("example", "invalidContext" in options ? options.invalidContext! : 500000),
						},
					},
					together: { models: { "zai-org/GLM-5.2": feedModel("zai-org/GLM-5.2", 262144) } },
					xai: { models: present ? { "grok-code-fast-1": feedModel("grok-code-fast-1", 32768) } : {} },
					anthropic: { models },
					openai: { models: present ? models : {} },
					opencode: { models },
					"opencode-go": { models },
					"github-copilot": {
						models: {
							"gpt-5.2-codex": feedModel("gpt-5.2-codex", 400000),
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
				}),
			};
		}
		if (url === "https://openrouter.ai/api/v1/models") {
			return {
				ok: true,
				json: async () => ({
					data: [
						{ id: "x-ai/grok-code-fast-1", context_length: 32768, supported_parameters: ["tools"] },
						{ id: "openai/gpt-5.4", name: "First", context_length: 1500000, supported_parameters: ["tools"] },
						{ id: "openai/gpt-5.4", name: "Second", context_length: 2000000, supported_parameters: ["tools"] },
					],
				}),
			};
		}
		if (url === "https://ai-gateway.vercel.sh/v1/models") {
			return {
				ok: true,
				json: async () => ({ data: [{ id: "openai/gpt-5.4", context_window: 1600000, tags: ["tool-use"] }] }),
			};
		}
		throw new Error(`Unexpected fetch: ${url}`);
	});
	vi.stubGlobal("fetch", fetch);
	await import("../scripts/generate-models.js");
	await vi.waitFor(() => expect(writeFileSync.mock.calls.length + errors.mock.calls.length).toBeGreaterThan(0));
	if (options.expectFailure) {
		expect(errors).toHaveBeenCalled();
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
	expect(writeFileSync).toHaveBeenCalledTimes(1);
	expect(errors).not.toHaveBeenCalled();
	expect(fetch).toHaveBeenCalledTimes(7);
	const output = writeFileSync.mock.calls[0][1] as string;
	const compiled = ts.transpileModule(output, { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText;
	const exports: { MODELS?: Record<string, Record<string, Model<any>>> } = {};
	runInNewContext(compiled, { exports });
	return exports.MODELS!;
}

afterEach(() => {
	process.exitCode = initialExitCode;
	vi.restoreAllMocks();
	vi.unstubAllGlobals();
});

describe("real generator context corrections", () => {
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
		expect(models["azure-openai-responses"]["gpt-6-astra"]).toBeUndefined();
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
			"gpt-5.4": 1000000,
			"gpt-5.4-mini": 272000,
			"gpt-5.5": 272000,
			"gpt-5.6-sol": 872000,
			"gpt-5.6-terra": 872000,
			"gpt-5.6-luna": 872000,
			"gpt-6-astra": 872000,
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

	it.each([
		{ openRouterEndpoints: [] },
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
