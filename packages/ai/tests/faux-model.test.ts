import { describe, expect, expectTypeOf, it } from "vitest";
import { registerFauxProvider } from "../src/index.js";
import type { ImagesModel, Model } from "../src/types.js";

describe("Faux model context", () => {
	it("preserves total context and an independent input constraint", () => {
		const faux = registerFauxProvider({
			models: [{ id: "constrained", contextWindow: 1_050_000, maxInputTokens: 900_000 }],
		});
		try {
			expect(faux.getModel()).toMatchObject({ contextWindow: 1_050_000, maxInputTokens: 900_000 });
			expect(faux.getModel()).not.toHaveProperty("contextWindowBudget");
		} finally {
			faux.unregister();
		}
	});

	it("does not invent an input constraint", () => {
		const faux = registerFauxProvider({ models: [{ id: "default", contextWindow: 64_000 }] });
		try {
			expect(faux.getModel().contextWindow).toBe(64_000);
			expect(faux.getModel().maxInputTokens).toBeUndefined();
		} finally {
			faux.unregister();
		}
	});

	it("rejects the obsolete field instead of silently ignoring it", () => {
		const definition = { id: "old", contextWindow: 1000, contextWindowBudget: 1000 };
		expect(() => registerFauxProvider({ provider: "test", models: [definition] })).toThrow(
			/test\/old.*contextWindowBudget was removed/,
		);
	});

	it("removes the budget contract and excludes text input constraints from image models", () => {
		expectTypeOf<
			"contextWindowBudget" extends keyof Model<"openai-responses"> ? true : false
		>().toEqualTypeOf<false>();
		expectTypeOf<"maxInputTokens" extends keyof ImagesModel<"openai-images"> ? true : false>().toEqualTypeOf<false>();
	});
});
