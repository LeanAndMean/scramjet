import { describe, expect, it } from "vitest";
import { buildRecordText, NEXT_STEP_RECORD_TOOL, registerNextStepRecord } from "../src/next-step-record.js";
import { recordingPi } from "./helpers.js";

function renderLines(component: any): string {
	return component.render(120).join("\n");
}

describe("buildRecordText", () => {
	it("marks the selected option with → and pads the rest", () => {
		const text = buildRecordText({
			outcome: "selected",
			options: [
				{ message: "/b:ok", reason: "primary" },
				{ message: "/c:alt", reason: "alternate" },
			],
			selected: "/c:alt",
			sourceCommand: "a:cmd",
			source: "completion",
		});
		expect(text).toBe(["Next step (from a:cmd):", "  /b:ok — primary", "→ /c:alt — alternate"].join("\n"));
	});

	it("renders dismissed outcomes as bullets followed by Cancelled", () => {
		const text = buildRecordText({
			outcome: "dismissed",
			options: [{ message: "/b:ok", reason: "primary" }, { message: "/c:alt" }],
			selected: null,
			sourceCommand: "a:cmd",
			source: "completion",
		});
		expect(text).toBe(["Next step (from a:cmd):", "- /b:ok — primary", "- /c:alt", "Cancelled"].join("\n"));
	});

	it("uses the suggestion heading for suggestion source", () => {
		const text = buildRecordText({
			outcome: "selected",
			options: [{ message: "/b:ok" }],
			selected: "/b:ok",
			sourceCommand: null,
			source: "suggestion",
		});
		expect(text.startsWith("Agent-suggested next step:")).toBe(true);
	});

	it("falls back to a plain heading without a source command", () => {
		const text = buildRecordText({
			outcome: "selected",
			options: [{ message: "/b:ok" }],
			selected: "/b:ok",
			sourceCommand: null,
			source: "completion",
		});
		expect(text.startsWith("Next step:")).toBe(true);
	});

	it("omits the reason suffix when absent", () => {
		const text = buildRecordText({
			outcome: "selected",
			options: [{ message: "/b:ok" }],
			selected: "/b:ok",
			sourceCommand: null,
			source: "completion",
		});
		expect(text).toContain("→ /b:ok");
		expect(text).not.toContain("—");
	});
});

describe("registerNextStepRecord", () => {
	it("registers a harness-only tool", () => {
		const bag = recordingPi();
		registerNextStepRecord(bag.pi);
		const tool = bag.tools.find((t) => t.name === NEXT_STEP_RECORD_TOOL);
		expect(tool).toBeDefined();
		expect(tool.activation).toBe("harness-only");
		expect(tool.promptSnippet).toBeUndefined();
	});

	it("execute returns the record text and echoes params in details", async () => {
		const bag = recordingPi();
		registerNextStepRecord(bag.pi);
		const tool = bag.tools.find((t) => t.name === NEXT_STEP_RECORD_TOOL);
		const params = {
			outcome: "selected",
			options: [{ message: "/b:ok", reason: "primary" }],
			selected: "/b:ok",
			sourceCommand: "a:cmd",
			source: "completion",
		};
		const result = await tool.execute("id", params);
		expect(result.content[0].text).toContain("→ /b:ok — primary");
		expect(result.details).toEqual(params);
	});

	it("renderResult renders the selected outcome with → prefix", async () => {
		const bag = recordingPi();
		registerNextStepRecord(bag.pi);
		const tool = bag.tools.find((t) => t.name === NEXT_STEP_RECORD_TOOL);
		const result = await tool.execute("id", {
			outcome: "selected",
			options: [{ message: "/b:ok" }, { message: "/c:alt" }],
			selected: "/b:ok",
			sourceCommand: "a:cmd",
			source: "completion",
		});
		const rendered = renderLines(tool.renderResult(result));
		expect(rendered).toContain("→ /b:ok");
		expect(rendered).toContain("  /c:alt");
	});

	it("renderResult renders Cancelled on dismiss", async () => {
		const bag = recordingPi();
		registerNextStepRecord(bag.pi);
		const tool = bag.tools.find((t) => t.name === NEXT_STEP_RECORD_TOOL);
		const result = await tool.execute("id", {
			outcome: "dismissed",
			options: [{ message: "/b:ok" }],
			selected: null,
			sourceCommand: null,
			source: "completion",
		});
		const rendered = renderLines(tool.renderResult(result));
		expect(rendered).toContain("- /b:ok");
		expect(rendered).toContain("Cancelled");
	});

	it("renderResult handles malformed details without throwing", () => {
		const bag = recordingPi();
		registerNextStepRecord(bag.pi);
		const tool = bag.tools.find((t) => t.name === NEXT_STEP_RECORD_TOOL);
		expect(() => tool.renderResult({ content: [], details: { bogus: true } })).not.toThrow();
		expect(() => tool.renderResult({ content: [], details: undefined })).not.toThrow();
	});
});
