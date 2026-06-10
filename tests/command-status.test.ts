import { describe, expect, it } from "vitest";
import { COMMAND_STATUS_PROBE_TYPE, registerCommandStatusTool } from "../command-status.ts";
import { COMMAND_STATUS_TYPE } from "../history.ts";
import type { CommandStatusPayload } from "../types.ts";
import { freshState, recordingPi } from "./helpers.ts";

type StatusParams = {
	status: CommandStatusPayload["status"];
	summary: string;
	user_prompt?: string;
	next_steps?: CommandStatusPayload["next_steps"];
	recommended_next_step?: number;
};

function toolFor(state = freshState()) {
	const { pi, tools, handlers } = recordingPi();
	registerCommandStatusTool(pi, state);
	const tool = tools.find((t) => t.name === "scramjet_command_status");
	if (!tool) throw new Error("scramjet_command_status tool not registered");
	const execute = (params: StatusParams) =>
		tool.execute("call-id", params, undefined, undefined, undefined) as Promise<any>;
	return { state, pi, tools, handlers, tool, execute };
}

describe("registerCommandStatusTool — registration", () => {
	it("registers exactly the scramjet_command_status tool", () => {
		const { tools } = toolFor();
		expect(tools).toHaveLength(1);
		expect(tools[0].name).toBe("scramjet_command_status");
	});

	it("registers no before_agent_start handler (the answer turn injects nothing about completion)", () => {
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
			const { state, execute } = toolFor(freshState({ commandPhase: phase }));
			const result = await execute({ status: "completed", summary: "done" });

			expect(result.terminate).toBeUndefined();
			expect(result.details.error).toBe("out-of-phase");
			expect(result.details.phase).toBe(phase);
			expect(String(result.content[0].text)).toContain("only when");
			// State is untouched: no status stored, phase unchanged.
			expect(state.latestCommandStatus).toBeNull();
			expect(state.commandPhase).toBe(phase);
		},
	);

	it("records the status, advances to reported, and terminates when phase is probing", async () => {
		const { state, pi, execute } = toolFor(
			freshState({ commandPhase: "probing", activeTopLevelCommand: "mach12:pr-create" }),
		);
		const result = await execute({ status: "completed", summary: "all green" });

		expect(result.terminate).toBe(true);
		expect(state.commandPhase).toBe("reported");
		expect(state.latestCommandStatus).toEqual({
			status: "completed",
			summary: "all green",
			user_prompt: undefined,
			next_steps: undefined,
			recommended_next_step: undefined,
		});
		expect(String(result.content[0].text)).toContain("completed");
		// issue 88: the report is journaled so a rewind/resume can reconstruct the
		// resting phase (idle here; waiting for a waiting_for_user report).
		expect(pi.appended).toContainEqual({
			customType: COMMAND_STATUS_TYPE,
			data: { commandName: "mach12:pr-create", status: "completed" },
		});
	});

	it("journals the report under the active command name (issue 88)", async () => {
		const { pi, execute } = toolFor(
			freshState({ commandPhase: "probing", activeTopLevelCommand: "mach12:pr-create" }),
		);
		await execute({ status: "waiting_for_user", summary: "awaiting approval" });
		expect(pi.appended).toContainEqual({
			customType: COMMAND_STATUS_TYPE,
			data: { commandName: "mach12:pr-create", status: "waiting_for_user" },
		});
	});

	it("does not journal a status when there is no active command (guarded)", async () => {
		const { pi, execute } = toolFor(freshState({ commandPhase: "probing", activeTopLevelCommand: null }));
		await execute({ status: "completed", summary: "no active command" });
		expect(pi.appended.some((e: { customType: string }) => e.customType === COMMAND_STATUS_TYPE)).toBe(false);
	});

	it("stores command-message next_steps and renders the first as a forward pointer for completed", async () => {
		const { state, execute } = toolFor(freshState({ commandPhase: "probing" }));
		const result = await execute({
			status: "completed",
			summary: "stage 2 done",
			next_steps: [{ message: "/mach12:issue-implement 84 3", fresh_session: true }],
			recommended_next_step: 0,
		});

		expect(result.terminate).toBe(true);
		expect(state.latestCommandStatus?.next_steps).toEqual([
			{ message: "/mach12:issue-implement 84 3", fresh_session: true },
		]);
		expect(state.latestCommandStatus?.recommended_next_step).toBe(0);
		expect(String(result.content[0].text)).toBe("→ /mach12:issue-implement 84 3");
		expect(result.details).toMatchObject({
			status: "completed",
			message: "/mach12:issue-implement 84 3",
			recommended_next_step: 0,
		});
	});

	it("renders the recommended command pointer when it is not the first next_step", async () => {
		const { execute } = toolFor(freshState({ commandPhase: "probing" }));
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
		const { state, execute } = toolFor(freshState({ commandPhase: "probing" }));
		const result = await execute({
			status: "completed",
			summary: "needs user choice",
			next_steps: [{ message: "Ask the user which branch to use", reason: "No branch was specified" }],
			recommended_next_step: 0,
		});

		expect(result.terminate).toBe(true);
		expect(state.latestCommandStatus?.next_steps).toEqual([
			{ message: "Ask the user which branch to use", reason: "No branch was specified" },
		]);
		expect(String(result.content[0].text)).toBe("status: completed");
		expect(result.details).toMatchObject({
			status: "completed",
			message: "Ask the user which branch to use",
			recommended_next_step: 0,
		});
	});

	it("rejects a duplicate report: the second call lands at phase 'reported' and is refused", async () => {
		const { state, execute } = toolFor(freshState({ commandPhase: "probing" }));

		const first = await execute({ status: "completed", summary: "done" });
		expect(first.terminate).toBe(true);
		expect(state.commandPhase).toBe("reported");

		// A second report in the same probe lands at phase "reported", which the
		// phase gate rejects (covered indirectly by the it.each above; named here
		// to document the duplicate-report contract explicitly).
		const second = await execute({ status: "completed", summary: "done again" });
		expect(second.terminate).toBeUndefined();
		expect(second.details.error).toBe("out-of-phase");
		expect(second.details.phase).toBe("reported");
		// The first report is preserved; the duplicate did not overwrite it.
		expect(state.latestCommandStatus?.summary).toBe("done");
	});

	it("renders a plain status line for non-completed reports", async () => {
		const { execute } = toolFor(freshState({ commandPhase: "probing" }));
		const result = await execute({
			status: "waiting_for_user",
			summary: "asked the user",
			user_prompt: "which branch?",
		});

		expect(result.terminate).toBe(true);
		expect(String(result.content[0].text)).toBe("status: waiting_for_user");
	});
});

describe("COMMAND_STATUS_PROBE_TYPE", () => {
	it("is a stable custom-message type string", () => {
		expect(COMMAND_STATUS_PROBE_TYPE).toBe("scramjet-command-status");
	});
});
