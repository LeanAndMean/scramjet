import { describe, expect, it } from "vitest";
import { parseArgs } from "../src/cli/args.js";
import { validateInvocationModelDefinition } from "../src/core/model-registry.js";
import { resolveInvocationModelDefinition } from "../src/main.js";

const minimal = {
	api: "openai-completions",
	baseUrl: "http://localhost:8080/deployment/v1",
	id: "Vendor/Model:high",
	apiKeyEnv: "INVOCATION_KEY",
	contextWindow: 1000.5,
	maxTokens: 2000,
};
const env = { INVOCATION_KEY: "  !synthetic-secret  " };
const resolve = (args: string[], environment: NodeJS.ProcessEnv = env, tty = true) =>
	resolveInvocationModelDefinition(parseArgs(args), tty, environment);
const inline = (definition: unknown) => ["-p", "--model-definition", JSON.stringify(definition)];

describe("invocation definition selectors", () => {
	it.each(["--model-definition", "--model-definition-env"])("accepts separate and equals %s values", (flag) => {
		const field: "modelDefinition" | "modelDefinitionEnv" =
			flag === "--model-definition" ? "modelDefinition" : "modelDefinitionEnv";
		for (const args of [[flag, "value"], [`${flag}=value`]]) {
			expect(parseArgs(args)[field]).toBe("value");
			expect(parseArgs(args).diagnostics).toEqual([]);
		}
	});
	it.each(
		[
			["--model-definition"],
			["--model-definition="],
			["--model-definition-env", "--print"],
			["--model-definition", "-p"],
			["--model-definition", "{}", "--model-definition={} "],
			["--model-definition-env", "DEF", "--model-definition-env=DEF"],
			["--model-definition", "{}", "--model-definition-env", "DEF"],
			...["--provider", "--model", "--models", "--api-key"].flatMap((flag) => [
				["--model-definition", "{}", flag, "synthetic-secret"],
				["--model-definition-env=DEF", `${flag}=synthetic-secret`],
				[flag, "--model-definition", "{}"],
			]),
		].map((args) => ({ args })),
	)("rejects incomplete/repeated/conflicting selectors $args", ({ args }) => {
		const diagnostics = parseArgs(args).diagnostics;
		expect(diagnostics.some((d) => d.type === "error")).toBe(true);
		expect(JSON.stringify(diagnostics)).not.toContain("synthetic-secret");
	});
	it("preserves unrelated options and extension flags", () => {
		const parsed = parseArgs([...inline(minimal), "--thinking", "high", "--custom=value"]);
		expect(parsed.thinking).toBe("high");
		expect(parsed.unknownFlags.get("custom")).toBe("value");
		expect(parsed.unknownFlags.has("model-definition")).toBe(false);
	});
});

describe("registry-owned invocation validation", () => {
	it.each(["openai-completions", "openai-responses", "anthropic-messages", "google-generative-ai"])(
		"accepts %s with literal ID and explicit limits",
		(api) => {
			const definition = validateInvocationModelDefinition({ ...minimal, api });
			expect(definition).toMatchObject({ ...minimal, api, name: minimal.id, reasoning: false, input: ["text"] });
			expect(definition.cost).toEqual({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0 });
		},
	);
	it("preserves declared metadata", () => {
		const definition = {
			...minimal,
			name: "Display",
			reasoning: true,
			input: ["text", "image"],
			thinkingLevelMap: { off: null, high: "effort" },
			maxInputTokens: 900.5,
			requestLimits: [{ maxTotalTokens: 950, maxInputTokens: 900, maxOutputTokens: 50, supportsTools: true }],
			cost: { input: 1, output: 2, cacheRead: 3, cacheWrite: 4 },
			compat: {
				supportsDeveloperRole: false,
				openRouterRouting: { sort: { by: "price" }, max_price: { prompt: 1 } },
			},
		};
		expect(validateInvocationModelDefinition(definition)).toEqual(definition);
	});
	it.each([
		null,
		[],
		"synthetic-secret",
		1,
		{},
		...["api", "baseUrl", "id", "apiKeyEnv", "contextWindow", "maxTokens"].map((field) =>
			Object.fromEntries(Object.entries(minimal).filter(([key]) => key !== field)),
		),
		...[
			"apiKey",
			"headers",
			"provider",
			"oauth",
			"streamSimple",
			"providers",
			"modelOverrides",
			"file",
			"discovery",
			"contextWindowBudget",
			"synthetic-secret",
		].map((key) => ({ ...minimal, [key]: "synthetic-secret" })),
		...["openai-codex-responses", "google-vertex", "azure-openai-responses", "synthetic-secret"].map((api) => ({
			...minimal,
			api,
		})),
		...["", " \t"].map((id) => ({ ...minimal, id })),
		...[0, -1, Infinity, NaN].map((contextWindow) => ({ ...minimal, contextWindow })),
		...[0, -1, 1.5, Infinity, Number.MAX_SAFE_INTEGER + 1].map((maxTokens) => ({ ...minimal, maxTokens })),
		{ ...minimal, maxInputTokens: Infinity },
		{ ...minimal, requestLimits: [] },
		{ ...minimal, requestLimits: [{ maxTotalTokens: Infinity, supportsTools: true }] },
		{ ...minimal, requestLimits: [{ maxTotalTokens: 10, supportsTools: true, synthetic: true }] },
		{ ...minimal, input: ["audio"] },
		{ ...minimal, thinkingLevelMap: { max: "unsupported" } },
		{ ...minimal, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, synthetic: 1 } },
		{ ...minimal, cost: { input: Infinity, output: 0, cacheRead: 0, cacheWrite: 0 } },
		{ ...minimal, compat: { supportsDeveloperRole: "false" } },
		{ ...minimal, compat: { synthetic: true } },
		{ ...minimal, api: "openai-responses", compat: { supportsDeveloperRole: false } },
		{ ...minimal, api: "google-generative-ai", compat: { sendSessionIdHeader: true } },
		{ ...minimal, compat: { openRouterRouting: { synthetic: true } } },
		{ ...minimal, compat: { openRouterRouting: { sort: { synthetic: true } } } },
		{ ...minimal, compat: { openRouterRouting: { preferred_max_latency: { synthetic: 1 } } } },
		{ ...minimal, compat: { vercelGatewayRouting: { synthetic: true } } },
	])("rejects invalid definition %# without echoing data", (definition) => {
		expect(() => validateInvocationModelDefinition(definition)).toThrow();
		try {
			validateInvocationModelDefinition(definition);
		} catch (error) {
			expect(String(error)).not.toContain("synthetic-secret");
		}
	});
	it.each([
		"/relative",
		"ftp://localhost/path",
		"http://",
		" http://localhost",
		"http://localhost ",
		"http://synthetic-secret@localhost",
		"http://@localhost",
		"http://localhost?",
		"http://localhost#",
		"http://localhost?key=synthetic-secret",
		"http://localhost#synthetic-secret",
		"http://local\nhost",
		"http://localhost/\u0000",
		"http://localhost/\u007f",
		"http://localhost/\u0085",
		"http://localhost/path\u00a0segment",
		"http://\\@localhost",
		"http://localhost\\path",
	])("rejects unsafe URL %# without echoing it", (baseUrl) => {
		expect(() => validateInvocationModelDefinition({ ...minimal, baseUrl })).toThrow(/baseUrl/);
	});
});

describe("CLI source resolution", () => {
	it("only reads explicitly selected environment JSON", () => {
		const environment = { ...env, DEF: JSON.stringify(minimal) };
		expect(resolve(["-p"], environment)).toBeUndefined();
		expect(resolve(["-p", "--model-definition-env", "DEF"], environment)).toEqual(resolve(inline(minimal)));
	});
	it("snapshots credential bytes without evaluation or recursive lookup", () => {
		const environment = { INVOCATION_KEY: "!synthetic-secret" };
		const result = resolve(inline(minimal), environment);
		environment.INVOCATION_KEY = "changed";
		expect(result?.apiKey).toBe("!synthetic-secret");
		expect(result?.definition).not.toHaveProperty("apiKey");
	});
	it.each(["{synthetic-secret", "null", "[]", "{} // synthetic-secret", '{"id":"synthetic-secret",}'])(
		"rejects strict JSON %# with controlled diagnostics",
		(json) => {
			expect(() => resolve(["-p", "--model-definition", json])).toThrow(/--model-definition/);
			try {
				resolve(["-p", "--model-definition", json]);
			} catch (error) {
				expect(String(error)).not.toContain("synthetic-secret");
			}
		},
	);
	it("rejects JSON numeric overflow", () => {
		expect(() => resolve(["-p", "--model-definition", JSON.stringify(minimal).replace("1000.5", "1e400")])).toThrow();
	});
	it.each([undefined, "", "  \n"])("rejects absent/blank environment values %#", (value) => {
		expect(() => resolve(["-p", "--model-definition-env=DEF"], { DEF: value })).toThrow(/environment/);
		expect(() => resolve(inline(minimal), { INVOCATION_KEY: value })).toThrow(/apiKeyEnv/);
	});
	it.each(["!synthetic-secret", "bad-name", "1KEY", "", " KEY "])("rejects invalid environment names %#", (name) => {
		expect(() => resolve(["-p", `--model-definition-env=${name}`])).toThrow();
		expect(() => resolve(inline({ ...minimal, apiKeyEnv: name }))).toThrow(/apiKeyEnv/);
	});
	it("rejects interactive generation but accepts all existing headless modes and redirected stdin", () => {
		expect(() => resolve(["--model-definition", JSON.stringify(minimal)])).toThrow(/headless/);
		for (const args of [["-p"], ["--mode", "text", "-p"], ["--mode", "json"], ["--mode", "rpc"]]) {
			expect(resolve([...args, "--model-definition", JSON.stringify(minimal)])?.definition.id).toBe(minimal.id);
		}
		expect(resolve(["--model-definition", JSON.stringify(minimal)], env, false)?.definition.id).toBe(minimal.id);
	});
	it("keeps help/version credential-free and listing validation non-generating", () => {
		for (const flag of ["--help", "--version"]) {
			expect(resolve([flag, "--model-definition-env=UNSET"], {})).toBeUndefined();
		}
		expect(resolve(["--list-models", "--model-definition", JSON.stringify(minimal)], {})?.definition.id).toBe(
			minimal.id,
		);
		expect(() => resolve(["--list-models", "--model-definition", "{}"], {})).toThrow();
	});
});
