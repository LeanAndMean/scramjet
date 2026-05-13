import { describe, expect, it } from "vitest";
import { resolveProviderBridgeConfig, stripEagerInputStreaming } from "../provider-bridge.ts";

describe("resolveProviderBridgeConfig", () => {
	it("returns null when ANTHROPIC_BASE_URL is unset", () => {
		expect(resolveProviderBridgeConfig({})).toBeNull();
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

	it("returns baseUrl only when no auth token or api key is set", () => {
		expect(
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
			}),
		).toEqual({ baseUrl: "http://127.0.0.1:18080" });
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
		expect(
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
				ANTHROPIC_AUTH_TOKEN: "",
				ANTHROPIC_API_KEY: "",
			}),
		).toEqual({ baseUrl: "http://127.0.0.1:18080" });
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
		expect(
			resolveProviderBridgeConfig({
				SCRAMJET_PROVIDER_BRIDGE: "false",
				ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
			}),
		).toEqual({ baseUrl: "http://127.0.0.1:18080" });
	});

	it("does not disable the bridge when SCRAMJET_PROVIDER_BRIDGE is '1'", () => {
		expect(
			resolveProviderBridgeConfig({
				SCRAMJET_PROVIDER_BRIDGE: "1",
				ANTHROPIC_BASE_URL: "http://127.0.0.1:18080",
			}),
		).toEqual({ baseUrl: "http://127.0.0.1:18080" });
	});

	it("throws on a malformed ANTHROPIC_BASE_URL", () => {
		expect(() =>
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "not-a-url",
			}),
		).toThrow();
	});

	it("throws on a URL that parses but has a degenerate hostname", () => {
		expect(() =>
			resolveProviderBridgeConfig({
				ANTHROPIC_BASE_URL: "https://.",
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
});
