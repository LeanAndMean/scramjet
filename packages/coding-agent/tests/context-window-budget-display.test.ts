import type { Model } from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
import { formatModelContext } from "../src/cli/list-models.js";
import { formatContextUsage } from "../src/modes/interactive/components/footer.js";

const model = (contextWindow: number, contextWindowBudget?: number) =>
	({ contextWindow, contextWindowBudget }) as Model<"openai-responses">;

describe("context window budget display", () => {
	it("labels split capacity and operational budget in model listings", () => {
		expect(formatModelContext(model(1_050_000, 272_000))).toBe("1.1M capacity (272K budget)");
	});

	it("keeps ordinary model listings concise", () => {
		expect(formatModelContext(model(272_000))).toBe("272K");
	});

	it("aligns footer percentage, denominator, and warning state to the budget", () => {
		expect(formatContextUsage(204_000, 75, 272_000, 1_050_000, true)).toEqual({
			display: "204k/272k budget (75.0%, auto; 1.1M capacity)",
			severity: "warning",
		});
	});

	it("shows unknown usage cleanly after compaction on split-budget models", () => {
		expect(formatContextUsage(null, null, 272_000, 1_050_000, true)).toEqual({
			display: "?/272k budget (?, auto; 1.1M capacity)",
			severity: "normal",
		});
	});

	it("keeps ordinary footer context concise", () => {
		expect(formatContextUsage(136_000, 50, 272_000, 272_000, false)).toEqual({
			display: "136k/272k (50.0%)",
			severity: "normal",
		});
	});
});
