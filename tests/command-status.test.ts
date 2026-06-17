import { describe, expect, it } from "vitest";
import { COMMAND_STATUS_PROBE_TYPE, registerCommandStatusTool } from "../command-status.ts";
import { COMMAND_STATUS_TYPE } from "../history.ts";
import type { CommandStatusPayload } from "../types.ts";
import { freshState, lifecycleFor, recordingPi } from "./helpers.ts";

type StatusParams = {
	status: CommandStatusPayload["status"];
	summary: string;
	user_prompt?: string;
	next_steps?: CommandStatusPayload["next_steps"];
	recommended_next_step?: number;
};

function toolFor(state = freshState()) {
	const { pi, tools, handlers, emit } = recordingPi();
	registerCommandStatusTool(pi, state);
	const tool = tools.find((t) => t.name === "report_scramjet_command_status");
	if (!tool) throw new Error("report_scramjet_command_status tool not registered");
	const execute = (params: StatusParams) =>
		tool.execute("call-id", params, undefined, undefined, undefined) as Promise<any>;
	return { state, pi, tools, handlers, emit, tool, execute };
}

describe("registerCommandStatusTool — registration", () => {
	it("registers exactly the report_scramjet_command_status tool", () => {
		const { tools } = toolFor();
		expect(tools).toHaveLength(1);
		expect(tools[0].name).toBe("report_scramjet_command_status");
	});

	it("registers no event handlers (counter resets on terminal status, not per-turn)", () => {
		const { handlers } = toolFor();
		expect(handlers.get("before_agent_start")).toBeUndefined();
	});

	it("exposes the unified message-based next-step schema and recommended-index fields", () => {
		const { tool } = toolFor();
		const params = tool.parameters;
		expect(params.properties.recommended_next_step.type).toBe("integer");
		expect(params.properties.recommended_next_step.minimum).toBe(0);
		expect(params.properties.recommended_next_step.description).toContain("Zero-based index");
		const nextStepSchema = params.properties.next_steps.items;
		// One flat shape — no discriminated union, no type field, no label.
		expect(nextStepSchema.anyOf).toBeUndefined();
		expect(nextStepSchema.properties.message.type).toBe("string");
		expect(nextStepSchema.properties.fresh_session.type).toBe("boolean");
		expect(nextStepSchema.properties.reason.type).toBe("string");
		expect(nextStepSchema.properties.type).toBeUndefined();
		expect(nextStepSchema.properties.name).toBeUndefined();
		expect(nextStepSchema.properties.label).toBeUndefined();
		expect(nextStepSchema.required).toEqual(["message"]);
	});
});

describe("registerCommandStatusTool — phase gate", () => {
	it.each(["idle", "running", "reported", "waiting"] as const)(
		"rejects with a helpful error and no terminate when phase is %s",
		async (phase) => {
			const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor(phase) }));
			const result = await execute({ status: "completed", summary: "done" });

			expect(result.terminate).toBeUndefined();
			expect(result.details.error).toBe("out-of-phase");
			expect(result.details.phase).toBe(state.lifecycle.phase);
			expect(String(result.content[0].text)).toContain("only when");
			expect(state.lifecycle.phase).not.toBe("probing");
		},
	);

	it("records the status, advances to reported, and terminates when phase is probing", async () => {
		const { state, pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing", "mach12:pr-create") }));
		const result = await execute({ status: "completed", summary: "all green" });

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.phase).toBe("reported");
		expect(state.lifecycle).toMatchObject({
			phase: "reported",
			command: "mach12:pr-create",
			status: {
				status: "completed",
				summary: "all green",
			},
		});
		expect(String(result.content[0].text)).toContain("completed");
		expect(pi.appended).toContainEqual({
			customType: COMMAND_STATUS_TYPE,
			data: { commandName: "mach12:pr-create", status: "completed" },
		});
	});

	it("journals the report under the active command name (issue 88)", async () => {
		const { pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing", "mach12:pr-create") }));
		await execute({ status: "waiting_for_user", summary: "awaiting approval" });
		expect(pi.appended).toContainEqual({
			customType: COMMAND_STATUS_TYPE,
			data: { commandName: "mach12:pr-create", status: "waiting_for_user" },
		});
	});

	it("journals a status using the lifecycle command (getActiveCommand)", async () => {
		const { pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing", "mach12:test") }));
		await execute({ status: "completed", summary: "done" });
		expect(pi.appended).toContainEqual({
			customType: COMMAND_STATUS_TYPE,
			data: { commandName: "mach12:test", status: "completed" },
		});
	});

	it("stores command-message next_steps and renders the first as a forward pointer for completed", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));
		const result = await execute({
			status: "completed",
			summary: "stage 2 done",
			next_steps: [{ message: "/mach12:issue-implement 84 3", fresh_session: true }],
			recommended_next_step: 0,
		});

		expect(result.terminate).toBe(true);
		if (state.lifecycle.phase === "reported") {
			expect(state.lifecycle.status.next_steps).toEqual([
				{ message: "/mach12:issue-implement 84 3", fresh_session: true },
			]);
			expect(state.lifecycle.status.recommended_next_step).toBe(0);
		}
		expect(String(result.content[0].text)).toBe("→ /mach12:issue-implement 84 3");
		expect(result.details).toMatchObject({
			status: "completed",
			message: "/mach12:issue-implement 84 3",
			recommended_next_step: 0,
		});
	});

	it("renders the recommended command pointer when it is not the first next_step", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));
		const result = await execute({
			status: "completed",
			summary: "stage done",
			next_steps: [
				{ message: "/mach12:issue-review" },
				{ message: "/mach12:issue-implement 92 3", fresh_session: true },
			],
			recommended_next_step: 1,
		});

		expect(String(result.content[0].text)).toBe("→ /mach12:issue-implement 92 3");
		expect(result.details).toMatchObject({
			status: "completed",
			message: "/mach12:issue-implement 92 3",
			recommended_next_step: 1,
		});
	});

	it("stores non-command messages without rendering a bogus command pointer", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));
		const result = await execute({
			status: "completed",
			summary: "needs user choice",
			next_steps: [{ message: "Ask the user which branch to use", reason: "No branch was specified" }],
			recommended_next_step: 0,
		});

		expect(result.terminate).toBe(true);
		if (state.lifecycle.phase === "reported") {
			expect(state.lifecycle.status.next_steps).toEqual([
				{ message: "Ask the user which branch to use", reason: "No branch was specified" },
			]);
		}
		expect(String(result.content[0].text)).toBe("status: completed");
		expect(result.details).toMatchObject({
			status: "completed",
			message: "Ask the user which branch to use",
			recommended_next_step: 0,
		});
	});

	it("rejects a duplicate report: the second call lands at phase 'reported' and is refused", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));

		const first = await execute({ status: "completed", summary: "done" });
		expect(first.terminate).toBe(true);
		expect(state.lifecycle.phase).toBe("reported");

		const second = await execute({ status: "completed", summary: "done again" });
		expect(second.terminate).toBeUndefined();
		expect(second.details.error).toBe("out-of-phase");
		expect(second.details.phase).toBe("reported");
		// The first report is preserved in the lifecycle variant.
		if (state.lifecycle.phase === "reported") {
			expect(state.lifecycle.status.summary).toBe("done");
		}
	});

	it("renders a plain status line for non-completed reports", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));
		const result = await execute({
			status: "waiting_for_user",
			summary: "asked the user",
			user_prompt: "which branch?",
		});

		expect(result.terminate).toBe(true);
		expect(String(result.content[0].text)).toBe("status: waiting_for_user");
	});
});

describe("registerCommandStatusTool — continuing status", () => {
	it("transitions probing → running and does not terminate", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));
		const result = await execute({ status: "continuing", summary: "more work to do" });

		expect(result.terminate).toBeUndefined();
		expect(result.details.status).toBe("continuing");
		expect(state.lifecycle.phase).toBe("running");
	});

	it("does not journal the continuing status", async () => {
		const { pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing", "a:cmd") }));
		await execute({ status: "continuing", summary: "still working" });
		expect(pi.appended).toHaveLength(0);
	});

	it("allows up to 3 consecutive continues then returns limit error", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));

		for (let i = 0; i < 3; i++) {
			state.lifecycle = { phase: "probing", command: "test:cmd", continueCount: i };
			const result = await execute({ status: "continuing", summary: "working" });
			expect(result.details.status).toBe("continuing");
		}

		state.lifecycle = { phase: "probing", command: "test:cmd", continueCount: 3 };
		const limited = await execute({ status: "continuing", summary: "still going" });
		expect(limited.details.error).toBe("continue-limit");
		expect(limited.content[0].text).toContain("completed");
		expect(limited.terminate).toBeUndefined();
		expect(state.lifecycle.phase).toBe("probing");
	});

	it("increments continueCount structurally on each continue", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));

		await execute({ status: "continuing", summary: "first" });
		expect(state.lifecycle).toMatchObject({ phase: "running", continueCount: 1 });

		// Simulate agent-end → probing (preserves count)
		state.lifecycle = { phase: "probing", command: "test:cmd", continueCount: 1 };
		await execute({ status: "continuing", summary: "second" });
		expect(state.lifecycle).toMatchObject({ phase: "running", continueCount: 2 });
	});

	it("resets the counter structurally on new command start", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));

		// Exhaust continues
		state.lifecycle = { phase: "probing", command: "test:cmd", continueCount: 3 };
		const limited = await execute({ status: "continuing", summary: "too many" });
		expect(limited.details.error).toBe("continue-limit");

		// A new command start structurally resets continueCount to 0
		state.lifecycle = { phase: "probing", command: "test:new-cmd", continueCount: 0 };

		const fresh = await execute({ status: "continuing", summary: "fresh start" });
		expect(fresh.details.status).toBe("continuing");
		expect(state.lifecycle).toMatchObject({ phase: "running", continueCount: 1 });
	});
});

describe("COMMAND_STATUS_PROBE_TYPE", () => {
	it("is a stable custom-message type string", () => {
		expect(COMMAND_STATUS_PROBE_TYPE).toBe("scramjet-command-status");
	});
});
