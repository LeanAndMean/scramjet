import { describe, expect, expectTypeOf, it } from "vitest";
import { getContextWindowBudget, registerFauxProvider } from "../src/index.js";
import type { ImagesModel } from "../src/types.js";

describe("Faux model context window budget", () => {
	it("preserves an explicit budget", () => {
		const faux = registerFauxProvider({
			models: [{ id: "split", contextWindow: 1_050_000, contextWindowBudget: 272_000 }],
		});

		try {
			expect(faux.getModel().contextWindowBudget).toBe(272_000);
			expect(getContextWindowBudget(faux.getModel())).toBe(272_000);
		} finally {
			faux.unregister();
		}
	});

	it("falls back to capacity when no budget is supplied", () => {
		const faux = registerFauxProvider({ models: [{ id: "default", contextWindow: 64_000 }] });

		try {
			expect(faux.getModel().contextWindowBudget).toBeUndefined();
			expect(getContextWindowBudget(faux.getModel())).toBe(64_000);
		} finally {
			faux.unregister();
		}
	});

	it("keeps text context budgets out of image models", () => {
		expectTypeOf<
			"contextWindowBudget" extends keyof ImagesModel<"openai-images"> ? true : false
		>().toEqualTypeOf<false>();
	});
});
