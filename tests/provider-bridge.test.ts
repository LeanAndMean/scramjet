import type { ExtensionAPI, ProviderConfig } from "@earendil-works/pi-coding-agent";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { registerProviderBridge, resolveProviderBridgeConfig, stripEagerInputStreaming } from "../provider-bridge.ts";

describe("resolveProviderBridgeConfig", () => {
	it("returns null when ANTHROPIC_BASE_URL is unset", () => {
		expect(resolveProviderBridgeConfig({})).toBeNull();
	});

	it("returns null when ANTHROPIC_BASE_URL is an empty string", () => {
		expect(resolveProviderBridgeConfig({ ANTHROPIC_BASE_URL: "" })).toBeNull();
	});

	it("returns null for the stock Anthropic host", () => {
		expect(
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "https://api.anthropic.com",
			}),
		).toBeNull();
	});

	it("returns null for the stock Anthropic host with a path", () => {
		expect(
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "https://api.anthropic.com/v1",
			}),
		).toBeNull();
	});

	it("returns null for the stock Anthropic host with a trailing dot (FQDN form)", () => {
		expect(
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "https://api.anthropic.com.",
			}),
		).toBeNull();
	});

	it("returns null for the stock Anthropic host with a trailing dot and a path", () => {
		expect(
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "https://api.anthropic.com./v1",
			}),
		).toBeNull();
	});

	it("returns baseUrl only when no auth token or api key is set", () => {
		const result = resolveProviderBridgeConfig({
			ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
		});
		expect(result).toEqual({ baseUrl: "http://127.0.0.1:18080" });
		expect(Object.hasOwn(result as object, "apiKey")).toBe(false);
	});

	it("uses ANTHROPIC_AUTH_TOKEN as the apiKey when set", () => {
		expect(
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
				ANTHROPIC_AUTH_TOKEN: "managed-by-tux",
			}),
		).toEqual({ baseUrl: "http://127.0.0.1:18080", apiKey: "managed-by-tux" });
	});

	it("uses ANTHROPIC_API_KEY as the apiKey when ANTHROPIC_AUTH_TOKEN is unset", () => {
		expect(
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
				ANTHROPIC_API_KEY: "sk-test-key",
			}),
		).toEqual({ baseUrl: "http://127.0.0.1:18080", apiKey: "sk-test-key" });
	});

	it("prefers ANTHROPIC_AUTH_TOKEN over ANTHROPIC_API_KEY when both are set", () => {
		expect(
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
				ANTHROPIC_AUTH_TOKEN: "auth-wins",
				ANTHROPIC_API_KEY: "api-loses",
			}),
		).toEqual({ baseUrl: "http://127.0.0.1:18080", apiKey: "auth-wins" });
	});

	it("falls through to ANTHROPIC_API_KEY when ANTHROPIC_AUTH_TOKEN is an empty string", () => {
		expect(
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
				ANTHROPIC_AUTH_TOKEN: "",
				ANTHROPIC_API_KEY: "sk-test-key",
			}),
		).toEqual({ baseUrl: "http://127.0.0.1:18080", apiKey: "sk-test-key" });
	});

	it("omits apiKey when both auth vars are empty strings", () => {
		const result = resolveProviderBridgeConfig({
			ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
			ANTHROPIC_AUTH_TOKEN: "",
			ANTHROPIC_API_KEY: "",
		});
		expect(result).toEqual({ baseUrl: "http://127.0.0.1:18080" });
		expect(Object.hasOwn(result as object, "apiKey")).toBe(false);
	});

	it("returns null when SCRAMJET_PROVIDER_BRIDGE is '0' even with valid proxy config", () => {
		expect(
			resolveProviderBridgeConfig({
				SCRAMJET_PROVIDER_BRIDGE: "0",
				ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
				ANTHROPIC_AUTH_TOKEN: "managed-by-tux",
			}),
		).toBeNull();
	});

	it("does not disable the bridge when SCRAMJET_PROVIDER_BRIDGE is 'false'", () => {
		const result = resolveProviderBridgeConfig({
			SCRAMJET_PROVIDER_BRIDGE: "false",
			ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
		});
		expect(result).toEqual({ baseUrl: "http://127.0.0.1:18080" });
		expect(Object.hasOwn(result as object, "apiKey")).toBe(false);
	});

	it("does not disable the bridge when SCRAMJET_PROVIDER_BRIDGE is '1'", () => {
		const result = resolveProviderBridgeConfig({
			SCRAMJET_PROVIDER_BRIDGE: "1",
			ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
		});
		expect(result).toEqual({ baseUrl: "http://127.0.0.1:18080" });
		expect(Object.hasOwn(result as object, "apiKey")).toBe(false);
	});

	it("throws on a malformed ANTHROPIC_BASE_URL", () => {
		expect(() =>
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "not-a-url",
			}),
		).toThrow();
	});

	it("throws on a URL that parses but has a degenerate hostname (single dot)", () => {
		expect(() =>
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "https://.",
			}),
		).toThrow();
	});

	it("throws on a URL that parses but has a degenerate hostname (double dot)", () => {
		expect(() =>
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "https://..",
			}),
		).toThrow();
	});

	it("throws on a URL with empty authority (scheme followed by triple slash)", () => {
		expect(() =>
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "https:///path",
			}),
		).toThrow();
	});
});

describe("stripEagerInputStreaming", () => {
	it("removes eager_input_streaming from each tool and returns the mutated payload", () => {
		const payload = {
			model: "claude-opus-4-6",
			tools: [
				{ name: "read", eager_input_streaming: true },
				{ name: "write", eager_input_streaming: true },
			],
		};
		const result = stripEagerInputStreaming(payload);
		expect(result).toBe(payload);
		expect(payload.tools[0]).toEqual({ name: "read" });
		expect(payload.tools[1]).toEqual({ name: "write" });
	});

	it("removes the field only from tools that have it, leaving others untouched", () => {
		const payload = {
			tools: [{ name: "read", eager_input_streaming: true }, { name: "write" }],
		};
		const result = stripEagerInputStreaming(payload);
		expect(result).toBe(payload);
		expect(payload.tools).toEqual([{ name: "read" }, { name: "write" }]);
	});

	it("preserves all other tool fields when stripping (rich tool shape)", () => {
		const originalTool = {
			name: "read",
			description: "Read a file from disk",
			input_schema: {
				type: "object",
				properties: { path: { type: "string" } },
				required: ["path"],
			},
			cache_control: { type: "ephemeral" },
			eager_input_streaming: true,
		};
		const payload = { model: "claude-opus-4-6", tools: [originalTool] };
		const originalToolsRef = payload.tools;

		const result = stripEagerInputStreaming(payload);

		expect(result).toBe(payload);
		expect(payload.tools).toBe(originalToolsRef);
		expect(payload.tools[0]).toBe(originalTool);
		expect(payload.tools[0]).toEqual({
			name: "read",
			description: "Read a file from disk",
			input_schema: {
				type: "object",
				properties: { path: { type: "string" } },
				required: ["path"],
			},
			cache_control: { type: "ephemeral" },
		});
		expect("eager_input_streaming" in payload.tools[0]).toBe(false);
	});

	it("returns undefined when no tool carries the field (no change)", () => {
		const payload = { tools: [{ name: "read" }, { name: "write" }] };
		expect(stripEagerInputStreaming(payload)).toBeUndefined();
	});

	it("returns undefined when the payload has no tools array", () => {
		expect(stripEagerInputStreaming({ model: "claude-opus-4-6" })).toBeUndefined();
	});

	it("returns undefined when tools is not an array", () => {
		expect(stripEagerInputStreaming({ tools: "not-an-array" })).toBeUndefined();
	});

	it("returns undefined for null, undefined, or non-object payloads", () => {
		expect(stripEagerInputStreaming(null)).toBeUndefined();
		expect(stripEagerInputStreaming(undefined)).toBeUndefined();
		expect(stripEagerInputStreaming("string")).toBeUndefined();
		expect(stripEagerInputStreaming(42)).toBeUndefined();
	});

	it("ignores non-object tool entries", () => {
		const payload = { tools: [null, "string", 42, { name: "ok", eager_input_streaming: true }] };
		const result = stripEagerInputStreaming(payload);
		expect(result).toBe(payload);
		expect(payload.tools[3]).toEqual({ name: "ok" });
	});

	it("logs and returns undefined when the strip throws (defensive)", () => {
		// Frozen tools array makes `delete` throw in strict mode (which ES
		// modules use by default). Simulates a future Pi version sending a
		// frozen payload.
		const payload = {
			tools: Object.freeze([Object.freeze({ name: "read", eager_input_streaming: true })]),
		};
		const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

		expect(stripEagerInputStreaming(payload)).toBeUndefined();
		expect(warnSpy).toHaveBeenCalledWith(expect.stringMatching(/^scramjet: stripEagerInputStreaming failed:/));

		warnSpy.mockRestore();
	});
});

type RegisterCall = ["registerProvider", string, ProviderConfig];
type OnCall = [
	"on",
	"before_provider_request",
	(event: { type: "before_provider_request"; payload: unknown }) => Promise<unknown>,
];
type Call = RegisterCall | OnCall;

function makeMockPi() {
	const calls: Call[] = [];
	const pi = {
		registerProvider: (name: string, config: ProviderConfig) => {
			calls.push(["registerProvider", name, config]);
		},
		on: (
			event: string,
			handler: (event: { type: "before_provider_request"; payload: unknown }) => Promise<unknown>,
		) => {
			calls.push(["on", event as "before_provider_request", handler]);
		},
	} as unknown as ExtensionAPI;
	return { pi, calls };
}

describe("registerProviderBridge", () => {
	let warnSpy: ReturnType<typeof vi.spyOn>;

	beforeEach(() => {
		warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
	});

	it("makes no calls and does not warn when env yields no config (unset)", () => {
		const { pi, calls } = makeMockPi();
		registerProviderBridge(pi, {});
		expect(calls).toEqual([]);
		expect(warnSpy).not.toHaveBeenCalled();
	});

	it("makes no calls and does not warn when env yields no config (stock Anthropic)", () => {
		const { pi, calls } = makeMockPi();
		registerProviderBridge(pi, { ANTHROPIC_BASE_URL: "https://api.anthropic.com" });
		expect(calls).toEqual([]);
		expect(warnSpy).not.toHaveBeenCalled();
	});

	it("makes no calls and does not warn when SCRAMJET_PROVIDER_BRIDGE=0 (silent kill-switch)", () => {
		const { pi, calls } = makeMockPi();
		registerProviderBridge(pi, {
			SCRAMJET_PROVIDER_BRIDGE: "0",
			ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
		});
		expect(calls).toEqual([]);
		expect(warnSpy).not.toHaveBeenCalled();
	});

	it("registers provider and installs hook when config is active", () => {
		const { pi, calls } = makeMockPi();
		registerProviderBridge(pi, {
			ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
			ANTHROPIC_AUTH_TOKEN: "managed-by-tux",
		});

		expect(calls).toHaveLength(2);
		expect(calls[0]).toEqual([
			"registerProvider",
			"anthropic",
			{ baseUrl: "http://127.0.0.1:18080", apiKey: "managed-by-tux" },
		]);
		expect(calls[1][0]).toBe("on");
		expect(calls[1][1]).toBe("before_provider_request");
		expect(typeof calls[1][2]).toBe("function");
	});

	it("omits apiKey on the registered ProviderConfig when no auth token is set", () => {
		// F3: the resolver-level Object.hasOwn assertion does not cover the
		// integration with pi.registerProvider. A refactor that introduced
		// `{ ...config, apiKey: config.apiKey }` (adding explicit undefined)
		// would pass every other test. Pin the wrapper-layer contract here.
		const { pi, calls } = makeMockPi();
		registerProviderBridge(pi, { ANTHROPIC_BASE_URL: "http://127.0.0.1:18080" });

		expect(calls).toHaveLength(2);
		const registerCall = calls[0] as RegisterCall;
		expect(registerCall[0]).toBe("registerProvider");
		expect(registerCall[2]).toEqual({ baseUrl: "http://127.0.0.1:18080" });
		expect(Object.hasOwn(registerCall[2], "apiKey")).toBe(false);
	});

	it("logs activation with `apiKey present` when an auth token is set", () => {
		const { pi } = makeMockPi();
		registerProviderBridge(pi, {
			ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
			ANTHROPIC_AUTH_TOKEN: "managed-by-tux",
		});
		expect(warnSpy).toHaveBeenCalledWith(
			expect.stringMatching(/^scramjet: bridging anthropic provider to 127\.0\.0\.1 \(apiKey present\)$/),
		);
	});

	it("logs activation with `apiKey absent` when no auth token is set", () => {
		const { pi } = makeMockPi();
		registerProviderBridge(pi, { ANTHROPIC_BASE_URL: "http://127.0.0.1:18080" });
		expect(warnSpy).toHaveBeenCalledWith(
			expect.stringMatching(/^scramjet: bridging anthropic provider to 127\.0\.0\.1 \(apiKey absent\)$/),
		);
	});

	it("does not throw on malformed URL — logs and skips registration", () => {
		const { pi, calls } = makeMockPi();
		expect(() => registerProviderBridge(pi, { ANTHROPIC_BASE_URL: "not-a-url" })).not.toThrow();
		expect(calls).toEqual([]);
		expect(warnSpy).toHaveBeenCalledWith(expect.stringMatching(/^scramjet: provider bridge disabled:/));
	});

	it("does not throw on degenerate hostname — logs and skips registration", () => {
		const { pi, calls } = makeMockPi();
		expect(() => registerProviderBridge(pi, { ANTHROPIC_BASE_URL: "https://.." })).not.toThrow();
		expect(calls).toEqual([]);
		expect(warnSpy).toHaveBeenCalledWith(expect.stringMatching(/^scramjet: provider bridge disabled:/));
	});

	it("logs `partially installed` and swallows when pi.on throws after registerProvider succeeded", () => {
		// F1: registerProvider has already rewired the Anthropic provider when
		// `on` throws — the bridge is half-installed (proxy active, but the
		// eager_input_streaming strip is missing, which would 400 at Foundry).
		// The hook-failure path must NOT escape into Pi's loadExtension catch
		// (which would tear down the rest of the extension) AND must be
		// distinguishable from the "fully disabled" path so operators can grep
		// for the half-install state.
		const registerCalls: Array<["registerProvider", string, ProviderConfig]> = [];
		const pi = {
			registerProvider: (name: string, config: ProviderConfig) => {
				registerCalls.push(["registerProvider", name, config]);
			},
			on: () => {
				throw new Error("pi.on exploded");
			},
		} as unknown as ExtensionAPI;

		expect(() => registerProviderBridge(pi, { ANTHROPIC_BASE_URL: "http://127.0.0.1:18080" })).not.toThrow();
		expect(registerCalls).toHaveLength(1);
		expect(warnSpy).toHaveBeenCalledWith(
			expect.stringMatching(
				/^scramjet: provider bridge partially installed \(hook registration failed\): pi\.on exploded$/,
			),
		);
	});

	it("logs `disabled` and swallows when pi.registerProvider throws", () => {
		// F1: registerProvider throwing means the provider is NOT rewired —
		// stock Anthropic stays in effect. This is the "fully disabled" path
		// and must use the regular `disabled` log line, not `partially
		// installed`. Must also swallow so the rest of the extension survives.
		const pi = {
			registerProvider: () => {
				throw new Error("registerProvider exploded");
			},
			on: () => {
				throw new Error("on should not be called when registerProvider fails");
			},
		} as unknown as ExtensionAPI;

		expect(() => registerProviderBridge(pi, { ANTHROPIC_BASE_URL: "http://127.0.0.1:18080" })).not.toThrow();
		expect(warnSpy).toHaveBeenCalledWith(
			expect.stringMatching(/^scramjet: provider bridge disabled: registerProvider exploded$/),
		);
	});

	it("registered hook returns the strip result for a payload with eager_input_streaming", async () => {
		const { pi, calls } = makeMockPi();
		registerProviderBridge(pi, { ANTHROPIC_BASE_URL: "http://127.0.0.1:18080" });

		const onCall = calls.find((c) => c[0] === "on") as OnCall;
		const handler = onCall[2];

		const payload = { tools: [{ name: "read", eager_input_streaming: true }] };
		const result = await handler({ type: "before_provider_request", payload });

		expect(result).toBe(payload);
		expect(payload.tools[0]).toEqual({ name: "read" });
	});

	it("registered hook returns undefined when there's nothing to strip", async () => {
		const { pi, calls } = makeMockPi();
		registerProviderBridge(pi, { ANTHROPIC_BASE_URL: "http://127.0.0.1:18080" });

		const onCall = calls.find((c) => c[0] === "on") as OnCall;
		const handler = onCall[2];

		const result = await handler({
			type: "before_provider_request",
			payload: { tools: [{ name: "read" }] },
		});

		expect(result).toBeUndefined();
	});
});
