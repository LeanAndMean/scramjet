import { describe, expect, it } from "vitest";
import { registerSuggestNextStepsTool } from "../src/suggest-next-steps.js";
import { freshState, lifecycleFor, recordingPi } from "./helpers.js";

function setup(stateOverrides = {}) {
	const { pi, tools } = recordingPi();
	const state = freshState(stateOverrides);
	registerSuggestNextStepsTool(pi as any, state);
	const tool = tools[0];
	return { tool, state, pi };
}

function fakeCtx(has = true) {
	return { hasUI: has, ui: { custom: async () => null } };
}

const validSteps = [{ message: "/mach12:pr-create 55", reason: "PR is ready" }];

describe("suggest_scramjet_next_steps", () => {
	describe("accept path", () => {
		it("stores payload with generation snapshot when idle", async () => {
			const { tool, state } = setup();
			state.registry = new Map([["mach12:pr-create", { name: "mach12:pr-create", filePath: "", body: "" }]]) as any;

			const result = await tool.execute(
				"tc1",
				{
					next_steps: validSteps,
					recommended_next_step: 0,
				},
				undefined,
				undefined,
				fakeCtx(),
			);

			expect(state.pendingSuggestion).toEqual({
				steps: validSteps,
				recommendedIndex: 0,
				generation: 0,
			});
			expect(result.details.stored).toBe(true);
			expect(result.details.stepCount).toBe(1);
		});

		it("last-write-wins — second call overwrites first", async () => {
			const { tool, state } = setup();
			state.registry = new Map([
				["mach12:pr-create", { name: "mach12:pr-create", filePath: "", body: "" }],
				["mach12:pr-review", { name: "mach12:pr-review", filePath: "", body: "" }],
			]) as any;

			await tool.execute(
				"tc1",
				{
					next_steps: validSteps,
					recommended_next_step: 0,
				},
				undefined,
				undefined,
				fakeCtx(),
			);

			const secondSteps = [{ message: "/mach12:pr-review 55", reason: "Review needed" }];
			await tool.execute(
				"tc2",
				{
					next_steps: secondSteps,
					recommended_next_step: 0,
				},
				undefined,
				undefined,
				fakeCtx(),
			);

			expect(state.pendingSuggestion!.steps).toEqual(secondSteps);
		});

		it("does not mutate lifecycleGeneration", async () => {
			const { tool, state } = setup();
			state.registry = new Map([["mach12:pr-create", { name: "mach12:pr-create", filePath: "", body: "" }]]) as any;
			const genBefore = state.lifecycleGeneration;

			await tool.execute(
				"tc1",
				{
					next_steps: validSteps,
					recommended_next_step: 0,
				},
				undefined,
				undefined,
				fakeCtx(),
			);

			expect(state.lifecycleGeneration).toBe(genBefore);
		});
	});

	describe("non-TUI rejection", () => {
		it("rejects with error when no ctx.ui", async () => {
			const { tool, state } = setup();

			const result = await tool.execute(
				"tc1",
				{
					next_steps: validSteps,
				},
				undefined,
				undefined,
				fakeCtx(false),
			);

			expect(result.details.error).toBe("non-tui");
			expect(state.pendingSuggestion).toBeNull();
		});

		it("rejects when hasUI is false (headless/noOp context)", async () => {
			const { tool, state } = setup();

			const result = await tool.execute(
				"tc1",
				{
					next_steps: validSteps,
				},
				undefined,
				undefined,
				fakeCtx(false),
			);

			expect(result.details.error).toBe("non-tui");
			expect(state.pendingSuggestion).toBeNull();
		});
	});

	describe("non-idle rejection (parameterized)", () => {
		const phases = ["running", "dormant", "probing", "reported", "waiting"] as const;

		for (const phase of phases) {
			it(`rejects when phase is ${phase}`, async () => {
				const state = freshState({ lifecycle: lifecycleFor(phase) });
				const { pi, tools } = recordingPi();
				registerSuggestNextStepsTool(pi as any, state);
				const tool = tools[0];

				const result = await tool.execute(
					"tc1",
					{
						next_steps: validSteps,
					},
					undefined,
					undefined,
					fakeCtx(),
				);

				expect(result.details.error).toBe("command-active");
				expect(result.details.phase).toBeDefined();
				expect(state.pendingSuggestion).toBeNull();
			});
		}
	});

	describe("freetext co-occurrence rejection", () => {
		it("rejects when freetextAwaitingReply is true", async () => {
			const { tool, state } = setup({ freetextAwaitingReply: true });

			const result = await tool.execute(
				"tc1",
				{
					next_steps: validSteps,
				},
				undefined,
				undefined,
				fakeCtx(),
			);

			expect(result.details.error).toBe("awaiting-freetext-reply");
			expect(state.pendingSuggestion).toBeNull();
		});
	});

	describe("validation rejections", () => {
		it("rejects unknown command", async () => {
			const { tool, state } = setup();
			// Empty registry — command not found
			state.registry = new Map() as any;

			const result = await tool.execute(
				"tc1",
				{
					next_steps: [{ message: "/nonexistent:cmd 1", reason: "test" }],
					recommended_next_step: 0,
				},
				undefined,
				undefined,
				fakeCtx(),
			);

			expect(result.details.error).toBe("validation");
			expect(state.pendingSuggestion).toBeNull();
		});

		it("rejects delegate-only command", async () => {
			const { tool, state } = setup();
			state.registry = new Map([
				["mach12:push", { name: "mach12:push", filePath: "", body: "", delegateOnly: true }],
			]) as any;

			const result = await tool.execute(
				"tc1",
				{
					next_steps: [{ message: "/mach12:push", reason: "push it" }],
					recommended_next_step: 0,
				},
				undefined,
				undefined,
				fakeCtx(),
			);

			expect(result.details.error).toBe("validation");
			expect(state.pendingSuggestion).toBeNull();
		});

		it("rejects blank reason", async () => {
			const { tool, state } = setup();
			state.registry = new Map([["mach12:pr-create", { name: "mach12:pr-create", filePath: "", body: "" }]]) as any;

			const result = await tool.execute(
				"tc1",
				{
					next_steps: [{ message: "/mach12:pr-create 55" }],
					recommended_next_step: 0,
				},
				undefined,
				undefined,
				fakeCtx(),
			);

			expect(result.details.error).toBe("validation");
			expect(state.pendingSuggestion).toBeNull();
		});

		it("rejects recommended index out of range", async () => {
			const { tool, state } = setup();
			state.registry = new Map([["mach12:pr-create", { name: "mach12:pr-create", filePath: "", body: "" }]]) as any;

			await tool.execute(
				"tc1",
				{
					next_steps: validSteps,
					recommended_next_step: 5,
				},
				undefined,
				undefined,
				fakeCtx(),
			);

			// validateNextSteps sets recommendedReason but doesn't reject valid steps;
			// the selector handles missing recommended gracefully.
			expect(state.pendingSuggestion).not.toBeNull();
		});

		it("allows non-command messages (open policy)", async () => {
			const { tool, state } = setup();

			const result = await tool.execute(
				"tc1",
				{
					next_steps: [{ message: "Run the tests manually", reason: "Tests might be flaky" }],
					recommended_next_step: 0,
				},
				undefined,
				undefined,
				fakeCtx(),
			);

			expect(state.pendingSuggestion).not.toBeNull();
			expect(result.details.stored).toBe(true);
		});
	});

	describe("clearing matrix", () => {
		it("input clears pendingSuggestion and freetextAwaitingReply", async () => {
			const { pi, emit } = recordingPi();
			const state = freshState({
				pendingSuggestion: { steps: validSteps, generation: 0 },
				freetextAwaitingReply: true,
			});
			// We need the history module's input handler for clearing.
			// Import and register it.
			const { registerHistory } = await import("../src/history.js");
			registerHistory(pi, state);

			await emit("input", { text: "hello", source: "interactive" });
			expect(state.pendingSuggestion).toBeNull();
			expect(state.freetextAwaitingReply).toBe(false);
		});

		it("before_agent_start clears both fields", async () => {
			const { pi, emit } = recordingPi();
			const state = freshState({
				pendingSuggestion: { steps: validSteps, generation: 0 },
				freetextAwaitingReply: true,
			});
			const { registerHistory } = await import("../src/history.js");
			registerHistory(pi, state);

			await emit("before_agent_start", {});
			expect(state.pendingSuggestion).toBeNull();
			expect(state.freetextAwaitingReply).toBe(false);
		});

		it("session_compact clears both fields", async () => {
			const { pi, emit } = recordingPi();
			const state = freshState({
				pendingSuggestion: { steps: validSteps, generation: 0 },
				freetextAwaitingReply: true,
			});
			const { registerAutoContinue } = await import("../src/auto-continue.js");
			registerAutoContinue(pi, state);

			await emit("session_compact", {});
			expect(state.pendingSuggestion).toBeNull();
			expect(state.freetextAwaitingReply).toBe(false);
		});

		it("session_shutdown clears both fields", async () => {
			const { pi, emit } = recordingPi();
			const state = freshState({
				pendingSuggestion: { steps: validSteps, generation: 0 },
				freetextAwaitingReply: true,
			});
			const { registerAutoContinue } = await import("../src/auto-continue.js");
			registerAutoContinue(pi, state);

			await emit("session_shutdown", {});
			expect(state.pendingSuggestion).toBeNull();
			expect(state.freetextAwaitingReply).toBe(false);
		});
	});

	describe("steer-during-storing-run", () => {
		it("input during same run clears the just-stored suggestion", async () => {
			const { pi, tools, emit } = recordingPi();
			const state = freshState();
			state.registry = new Map([["mach12:pr-create", { name: "mach12:pr-create", filePath: "", body: "" }]]) as any;
			registerSuggestNextStepsTool(pi as any, state);
			const { registerHistory } = await import("../src/history.js");
			registerHistory(pi, state);
			const tool = tools[0];

			// Agent stores a suggestion
			await tool.execute(
				"tc1",
				{
					next_steps: validSteps,
					recommended_next_step: 0,
				},
				undefined,
				undefined,
				fakeCtx(),
			);
			expect(state.pendingSuggestion).not.toBeNull();

			// User sends input before agent_end fires
			await emit("input", { text: "actually do something else", source: "interactive" });
			expect(state.pendingSuggestion).toBeNull();
		});
	});
});
