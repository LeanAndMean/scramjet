import { getModel, type Model } from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
import { formatModelContext } from "../src/cli/list-models.js";
import { formatContextUsage } from "../src/modes/interactive/components/footer.js";

describe("one context display", () => {
	it("shows the same generated Copilot total in the listing and footer", () => {
		const model = getModel("github-copilot", "gpt-6-astra");
		expect(formatModelContext(model)).toBe("1M");
		expect(formatContextUsage(500_000, 50, model.contextWindow, true)).toEqual({
			display: "500k/1.0M (50.0%, auto)",
			severity: "normal",
		});
	});

	it("does not display an independent input constraint as a context budget", () => {
		const model = { contextWindow: 1_050_000, maxInputTokens: 900_000 } as Model<"openai-responses">;
		expect(formatModelContext(model)).toBe("1.1M");
	});

	it.each([
		[70, "normal"],
		[75, "warning"],
		[90, "warning"],
		[91, "error"],
	] as const)("uses total-context percentage %s for severity", (percent, severity) => {
		expect(formatContextUsage(percent * 10_000, percent, 1_000_000, true).severity).toBe(severity);
	});

	it("keeps unknown usage unknown", () => {
		expect(formatContextUsage(null, null, 1_050_000, true)).toEqual({
			display: "?/1.1M (?, auto)",
			severity: "normal",
		});
	});

	it("preserves disabled auto-compaction presentation", () => {
		expect(formatContextUsage(136_000, 50, 272_000, false)).toEqual({
			display: "136k/272k (50.0%)",
			severity: "normal",
		});
	});
});
