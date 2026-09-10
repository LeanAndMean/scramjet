import { describe, expect, it } from "vitest";
import { inspectProviderRequestToolInventory } from "../src/provider-request-tool-inventory.js";
import type { Api } from "../src/types.js";

describe("inspectProviderRequestToolInventory", () => {
	it.each([
		{
			api: "openai-responses",
			payload: { tools: [{ name: "zeta" }, { name: "alpha" }, { name: "zeta" }] },
		},
		{
			api: "azure-openai-responses",
			payload: { tools: [{ name: "zeta" }, { name: "alpha" }, { name: "zeta" }] },
		},
		{
			api: "openai-codex-responses",
			payload: { tools: [{ name: "zeta" }, { name: "alpha" }, { name: "zeta" }] },
		},
		{
			api: "anthropic-messages",
			payload: { tools: [{ name: "zeta" }, { name: "alpha" }, { name: "zeta" }] },
		},
		{
			api: "openai-completions",
			payload: {
				tools: [{ function: { name: "zeta" } }, { function: { name: "alpha" } }, { function: { name: "zeta" } }],
			},
		},
		{
			api: "mistral-conversations",
			payload: {
				tools: [{ function: { name: "zeta" } }, { function: { name: "alpha" } }, { function: { name: "zeta" } }],
			},
		},
		{
			api: "google-generative-ai",
			payload: {
				config: {
					tools: [
						{ functionDeclarations: [{ name: "zeta" }, { name: "alpha" }] },
						{ functionDeclarations: [{ name: "zeta" }] },
					],
				},
			},
		},
		{
			api: "google-vertex",
			payload: {
				config: {
					tools: [
						{ functionDeclarations: [{ name: "zeta" }, { name: "alpha" }] },
						{ functionDeclarations: [{ name: "zeta" }] },
					],
				},
			},
		},
		{
			api: "bedrock-converse-stream",
			payload: {
				toolConfig: {
					tools: [{ toolSpec: { name: "zeta" } }, { toolSpec: { name: "alpha" } }, { toolSpec: { name: "zeta" } }],
				},
			},
		},
	] satisfies Array<{ api: Api; payload: unknown }>)(
		"observes sorted deduplicated names for $api",
		({ api, payload }) => {
			expect(inspectProviderRequestToolInventory(api, payload)).toEqual({
				status: "observed",
				toolNames: ["alpha", "zeta"],
			});
		},
	);

	it.each([
		["openai-responses", {}],
		["azure-openai-responses", { tools: [] }],
		["openai-codex-responses", {}],
		["anthropic-messages", { tools: [] }],
		["openai-completions", {}],
		["mistral-conversations", { tools: [] }],
		["google-generative-ai", { contents: [] }],
		["google-vertex", { config: { tools: [] } }],
		["bedrock-converse-stream", { messages: [] }],
	] satisfies Array<[Api, unknown]>)(
		"observes an empty inventory for an absent or empty tool container on %s",
		(api, payload) => {
			expect(inspectProviderRequestToolInventory(api, payload)).toEqual({ status: "observed", toolNames: [] });
		},
	);

	it.each([
		["openai-responses", null, "payload-not-object"],
		["anthropic-messages", { tools: {} }, "tool-list-not-array"],
		["openai-codex-responses", { tools: [null] }, "tool-entry-not-object"],
		["openai-responses", { tools: [{}] }, "tool-name-missing"],
		["anthropic-messages", { tools: [{ name: 1 }] }, "tool-name-not-string"],
		["openai-responses", { tools: [{ name: "  " }] }, "tool-name-empty"],
		["openai-completions", { tools: [{}] }, "tool-container-not-object"],
		["mistral-conversations", { tools: [{ function: { name: "" } }] }, "tool-name-empty"],
		["google-generative-ai", { config: [] }, "tool-container-not-object"],
		["google-vertex", { config: { tools: [{}] } }, "tool-list-not-array"],
		[
			"google-generative-ai",
			{ config: { tools: [{ functionDeclarations: [{ name: "valid" }, { name: false }] }] } },
			"tool-name-not-string",
		],
		["bedrock-converse-stream", { toolConfig: [] }, "tool-container-not-object"],
		["bedrock-converse-stream", { toolConfig: {} }, "tool-list-not-array"],
		[
			"bedrock-converse-stream",
			{ toolConfig: { tools: [{ toolSpec: { name: "valid" } }, { toolSpec: null }] } },
			"tool-container-not-object",
		],
	] satisfies Array<[Api, unknown, string]>)("reports bounded malformed reason %s", (api, payload, reason) => {
		expect(inspectProviderRequestToolInventory(api, payload)).toEqual({ status: "malformed", reason });
	});

	it("reports unreadable supported payloads with a bounded malformed reason", () => {
		const payload = new Proxy(
			{},
			{
				get() {
					throw new Error("secret getter failure");
				},
			},
		);

		expect(inspectProviderRequestToolInventory("openai-responses", payload)).toEqual({
			status: "malformed",
			reason: "payload-unreadable",
		});
	});

	it("reports custom APIs as unsupported without inspecting the payload", () => {
		const payload = new Proxy(
			{},
			{
				get() {
					throw new Error("payload must not be inspected");
				},
			},
		);

		expect(inspectProviderRequestToolInventory("custom-private-api", payload)).toEqual({
			status: "unsupported",
		});
	});
});
