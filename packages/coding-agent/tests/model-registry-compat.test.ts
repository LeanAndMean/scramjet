import { Type } from "typebox";
import { Compile } from "typebox/compile";
import { describe, expect, it } from "vitest";

const AnthropicMessagesCompatSchema = Type.Object({
	supportsEagerToolInputStreaming: Type.Optional(Type.Boolean()),
	supportsLongCacheRetention: Type.Optional(Type.Boolean()),
	supportsTemperature: Type.Optional(Type.Boolean()),
	forceAdaptiveThinking: Type.Optional(Type.Boolean()),
});

const ProviderCompatSchema = Type.Union([
	Type.Object({
		openRouterRouting: Type.Optional(Type.Any()),
		vercelGatewayRouting: Type.Optional(Type.Any()),
	}),
	Type.Object({}),
	AnthropicMessagesCompatSchema,
]);

const ProviderConfigSchema = Type.Object({
	providers: Type.Record(
		Type.String(),
		Type.Object({
			compat: Type.Optional(ProviderCompatSchema),
			models: Type.Optional(
				Type.Array(
					Type.Object({
						id: Type.String({ minLength: 1 }),
						compat: Type.Optional(ProviderCompatSchema),
					}),
				),
			),
			modelOverrides: Type.Optional(
				Type.Record(Type.String(), Type.Object({ compat: Type.Optional(ProviderCompatSchema) })),
			),
		}),
	),
});

const validate = Compile(ProviderConfigSchema);
const validateAnthropicCompat = Compile(AnthropicMessagesCompatSchema);

describe("AnthropicMessagesCompat schema validation", () => {
	it("accepts supportsTemperature and forceAdaptiveThinking on provider-level compat", () => {
		const config = {
			providers: {
				anthropic: {
					compat: {
						supportsTemperature: false,
						forceAdaptiveThinking: true,
					},
				},
			},
		};
		expect(validate.Check(config)).toBe(true);
	});

	it("accepts both fields on a custom model compat", () => {
		const config = {
			providers: {
				anthropic: {
					models: [
						{
							id: "claude-opus-4-8",
							compat: {
								supportsTemperature: false,
								forceAdaptiveThinking: true,
							},
						},
					],
				},
			},
		};
		expect(validate.Check(config)).toBe(true);
	});

	it("accepts both fields on per-model override compat", () => {
		const config = {
			providers: {
				anthropic: {
					modelOverrides: {
						"claude-opus-4-8": {
							compat: {
								supportsTemperature: false,
								forceAdaptiveThinking: true,
								supportsEagerToolInputStreaming: true,
							},
						},
					},
				},
			},
		};
		expect(validate.Check(config)).toBe(true);
	});

	it("rejects non-boolean supportsTemperature", () => {
		expect(validateAnthropicCompat.Check({ supportsTemperature: "false" })).toBe(false);
	});

	it("rejects non-boolean forceAdaptiveThinking", () => {
		expect(validateAnthropicCompat.Check({ forceAdaptiveThinking: 1 })).toBe(false);
	});

	it("preserves unrelated compat fields when both new fields are present", () => {
		const config = {
			providers: {
				anthropic: {
					models: [
						{
							id: "claude-opus-4-8",
							compat: {
								supportsEagerToolInputStreaming: true,
								supportsLongCacheRetention: false,
								supportsTemperature: false,
								forceAdaptiveThinking: true,
							},
						},
					],
				},
			},
		};
		expect(validate.Check(config)).toBe(true);
	});
});
