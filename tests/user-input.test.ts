import { describe, expect, it } from "vitest";
import { registerUserInputTool } from "../user-input.ts";
import { freshState, recordingPi } from "./helpers.ts";

type UserInputParams = {
	type: "confirm" | "select" | "freetext";
	message: string;
	options?: { value: string; label: string; description?: string }[];
	recommended?: number;
	placeholder?: string;
};

function toolFor(state = freshState()) {
	const { pi, tools, handlers } = recordingPi();
	registerUserInputTool(pi, state);
	const tool = tools.find((t: any) => t.name === "scramjet_user_input");
	if (!tool) throw new Error("scramjet_user_input tool not registered");
	const execute = (params: UserInputParams, ctx?: unknown) =>
		tool.execute("call-id", params, undefined, undefined, ctx) as Promise<any>;
	return { state, pi, tools, handlers, tool, execute };
}

describe("registerUserInputTool — registration", () => {
	it("registers exactly the scramjet_user_input tool", () => {
		const { tools } = toolFor();
		expect(tools).toHaveLength(1);
		expect(tools[0].name).toBe("scramjet_user_input");
	});

	it("has a promptSnippet for system prompt visibility", () => {
		const { tool } = toolFor();
		expect(tool.promptSnippet).toBeDefined();
		expect(tool.promptSnippet).toContain("scramjet_user_input");
	});

	it("has the expected schema shape with type enum and flat optional fields", () => {
		const { tool } = toolFor();
		const params = tool.parameters;
		expect(params.required).toContain("type");
		expect(params.required).toContain("message");
		expect(params.properties.type.anyOf).toBeDefined();
		expect(params.properties.message.type).toBe("string");
		expect(params.properties.options).toBeDefined();
		expect(params.properties.recommended).toBeDefined();
		expect(params.properties.placeholder).toBeDefined();
	});
});

describe("registerUserInputTool — phase gate", () => {
	it.each(["idle", "reported", "waiting"] as const)(
		"rejects with a helpful error and no terminate when phase is %s",
		async (phase) => {
			const { execute } = toolFor(freshState({ commandPhase: phase }));
			const ctx = { ui: {} };
			const result = await execute({ type: "confirm", message: "Proceed?" }, ctx);

			expect(result.terminate).toBeUndefined();
			expect(result.details.error).toBe("out-of-phase");
			expect(result.details.phase).toBe(phase);
			expect(String(result.content[0].text)).toContain("not available right now");
		},
	);

	it.each(["running", "probing"] as const)("accepts calls when phase is %s", async (phase) => {
		const { execute } = toolFor(freshState({ commandPhase: phase }));
		const ctx = { ui: {} };
		const result = await execute({ type: "confirm", message: "Proceed?" }, ctx);

		expect(result.details.error).not.toBe("out-of-phase");
	});
});

describe("registerUserInputTool — non-TUI guard", () => {
	it("returns a helpful error when ctx.ui is absent", async () => {
		const { execute } = toolFor(freshState({ commandPhase: "running" }));
		const result = await execute({ type: "confirm", message: "Proceed?" }, {});

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("non-tui");
		expect(String(result.content[0].text)).toContain("TUI environment");
	});

	it("returns a helpful error when ctx is undefined", async () => {
		const { execute } = toolFor(freshState({ commandPhase: "running" }));
		const result = await execute({ type: "confirm", message: "Proceed?" }, undefined);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("non-tui");
	});
});

describe("registerUserInputTool — runtime validation", () => {
	const ctx = { ui: {} };

	it("rejects select without options", async () => {
		const { execute } = toolFor(freshState({ commandPhase: "running" }));
		const result = await execute({ type: "select", message: "Pick one" }, ctx);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("validation");
		expect(String(result.content[0].text)).toContain("options");
		expect(String(result.content[0].text)).toContain("non-empty array");
	});

	it("rejects select with empty options array", async () => {
		const { execute } = toolFor(freshState({ commandPhase: "running" }));
		const result = await execute({ type: "select", message: "Pick one", options: [] }, ctx);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("validation");
		expect(String(result.content[0].text)).toContain("non-empty array");
	});

	it("rejects select with recommended out of range", async () => {
		const { execute } = toolFor(freshState({ commandPhase: "running" }));
		const result = await execute(
			{
				type: "select",
				message: "Pick one",
				options: [{ value: "a", label: "A" }],
				recommended: 5,
			},
			ctx,
		);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("validation");
		expect(String(result.content[0].text)).toContain("out of range");
	});

	it("accepts select with valid recommended index", async () => {
		const { execute } = toolFor(freshState({ commandPhase: "running" }));
		const result = await execute(
			{
				type: "select",
				message: "Pick one",
				options: [
					{ value: "a", label: "A" },
					{ value: "b", label: "B" },
				],
				recommended: 1,
			},
			ctx,
		);

		expect(result.details.error).not.toBe("validation");
	});

	it("rejects empty message", async () => {
		const { execute } = toolFor(freshState({ commandPhase: "running" }));
		const result = await execute({ type: "confirm", message: "" }, ctx);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("validation");
		expect(String(result.content[0].text)).toContain("message");
	});

	it("rejects whitespace-only message", async () => {
		const { execute } = toolFor(freshState({ commandPhase: "running" }));
		const result = await execute({ type: "confirm", message: "   " }, ctx);

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("validation");
	});
});

describe("registerUserInputTool — non-terminating results", () => {
	it("never returns terminate: true on any error path", async () => {
		const { execute } = toolFor(freshState({ commandPhase: "running" }));
		const ctx = { ui: {} };

		const validationResult = await execute({ type: "select", message: "Pick", options: [] }, ctx);
		expect(validationResult.terminate).toBeUndefined();

		const nonTuiResult = await execute({ type: "confirm", message: "Ok?" });
		expect(nonTuiResult.terminate).toBeUndefined();
	});

	it("never returns terminate: true on the stub success path", async () => {
		const { execute } = toolFor(freshState({ commandPhase: "running" }));
		const ctx = { ui: {} };
		const result = await execute({ type: "confirm", message: "Proceed?" }, ctx);
		expect(result.terminate).toBeUndefined();
	});
});
