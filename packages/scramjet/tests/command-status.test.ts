import { describe, expect, it } from "vitest";
import {
	buildDormantCommandNotice,
	COMMAND_STATUS_PROBE_TYPE,
	registerCommandStatusTool,
	registerDormantCommandNotice,
} from "../src/command-status.js";
import { COMMAND_STATUS_TYPE } from "../src/history.js";
import { isProbeDue, isProbeInFlight } from "../src/lifecycle.js";
import type { CommandStatusPayload } from "../src/types.js";
import { freshState, lifecycleFor, recordingPi } from "./helpers.js";

type StatusParams = {
	status: CommandStatusPayload["status"];
	summary: string;
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

	it("does not register the dormant notice handler", () => {
		const { handlers } = toolFor();
		expect(handlers.get("before_agent_start")).toBeUndefined();
	});

	it("advertises inline reporting via description and promptSnippet", () => {
		const { tool } = toolFor();
		expect(tool.description).toContain("inline");
		expect(tool.promptSnippet).toContain("report_scramjet_command_status");
		expect(tool.promptSnippet).toContain("answer first");
	});

	it("orders summary before status in the schema (evidence before assessment)", () => {
		const { tool } = toolFor();
		const keys = Object.keys(tool.parameters.properties);
		expect(keys.indexOf("summary")).toBeGreaterThanOrEqual(0);
		expect(keys.indexOf("summary")).toBeLessThan(keys.indexOf("status"));
	});

	it("requires a non-empty summary and describes incremental semantics", () => {
		const { tool } = toolFor();
		expect(tool.parameters.required).toContain("summary");
		expect(tool.parameters.properties.summary.minLength).toBe(1);
		expect(tool.parameters.properties.summary.pattern).toBe("\\S");
		expect(tool.parameters.properties.summary.description.toLowerCase()).toContain("since");
	});

	it("exposes the unified message-based next-step schema and recommended-index fields", () => {
		const { tool } = toolFor();
		const params = tool.parameters;
		expect(params.properties.recommended_next_step.type).toBe("integer");
		expect(params.properties.recommended_next_step.minimum).toBe(0);
		expect(params.properties.recommended_next_step.description).toContain("Zero-based index");
		const nextStepSchema = params.properties.next_steps.items;
		expect(nextStepSchema.anyOf).toBeUndefined();
		expect(nextStepSchema.properties.message.type).toBe("string");
		expect(nextStepSchema.properties.fresh_session.type).toBe("boolean");
		expect(nextStepSchema.properties.reason.type).toBe("string");
		expect(nextStepSchema.properties.type).toBeUndefined();
		expect(nextStepSchema.properties.name).toBeUndefined();
		expect(nextStepSchema.properties.label).toBeUndefined();
		expect(nextStepSchema.required).toEqual(["message", "fresh_session"]);
	});
});

describe("registerCommandStatusTool — gate", () => {
	it("rejects with no active command (idle)", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("idle") }));
		const result = await execute({ status: "completed", summary: "done" });

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("out-of-phase");
		expect(String(result.content[0].text)).toContain("not active right now");
	});

	it("leaves lifecycle facts, generation, and journal unchanged on a rejected call", async () => {
		const { state, pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("waiting", "mach12:test") }));
		const gen = state.lifecycleGeneration;
		const before = { ...state.lifecycle };
		const result = await execute({ status: "completed", summary: "done" });

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("out-of-phase");
		expect(state.lifecycleGeneration).toBe(gen);
		expect(state.lifecycle).toEqual(before);
		expect(pi.appended.filter((e: any) => e.customType === COMMAND_STATUS_TYPE)).toHaveLength(0);
	});

	it("accepts terminal status when probe armed (running) — inline reporting", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ status: "completed", summary: "done" });

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.lastReport?.status).toBe("completed");
		expect(state.lifecycle.probeArmed).toBe(false);
		expect(isProbeDue(state.lifecycle)).toBe(false);
	});

	it("journals the inline report under the active command name", async () => {
		const { pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("running", "mach12:pr-create") }));
		await execute({ status: "completed", summary: "done" });
		expect(pi.appended).toContainEqual({
			customType: COMMAND_STATUS_TYPE,
			data: { commandName: "mach12:pr-create", status: "completed", summary: "done" },
		});
	});

	it("accepts blocked inline when probe armed (running)", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ status: "blocked", summary: "missing dep" });

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.lastReport).toMatchObject({ status: "blocked", summary: "missing dep" });
		expect(state.lifecycle.probeArmed).toBe(false);
	});

	it("accepts incomplete inline when probe armed (running)", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ status: "incomplete", summary: "stopped" });

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.lastReport).toMatchObject({ status: "incomplete", summary: "stopped" });
		expect(state.lifecycle.probeArmed).toBe(false);
	});

	it("rejects continuing when probe armed (running)", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("running") }));
		const result = await execute({ status: "continuing", summary: "more work" });

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("out-of-phase");
		expect(state.lifecycle.probeArmed).toBe(true);
	});

	it("rejects terminal status from reported (has lastReport)", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("reported", "mach12:pr-create") }));
		const result = await execute({ status: "completed", summary: "done" });

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("out-of-phase");
		expect(String(result.content[0].text)).toContain("already");
	});

	it("rejects terminal status when parked for input (waiting)", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("waiting") }));
		const result = await execute({ status: "completed", summary: "done" });

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("out-of-phase");
		expect(String(result.content[0].text)).toContain("parked");
	});

	it("accepts terminal status from dormant", async () => {
		const { state, pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("dormant", "mach12:test") }));
		const result = await execute({ status: "completed", summary: "done" });

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.lastReport).toMatchObject({ status: "completed", summary: "done" });
		const statusEntries = pi.appended.filter((e: any) => e.customType === COMMAND_STATUS_TYPE);
		expect(statusEntries).toHaveLength(1);
	});

	it("accepts blocked from dormant", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("dormant", "mach12:test") }));
		const result = await execute({ status: "blocked", summary: "missing dep" });

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.lastReport).toMatchObject({ status: "blocked", summary: "missing dep" });
	});

	it("accepts incomplete from dormant", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("dormant", "mach12:test") }));
		const result = await execute({ status: "incomplete", summary: "stopped" });

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.lastReport).toMatchObject({ status: "incomplete", summary: "stopped" });
	});

	it("records the status, sets lastReport, and terminates when probe is in flight", async () => {
		const { state, pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing", "mach12:pr-create") }));
		const result = await execute({ status: "completed", summary: "all green" });

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.lastReport).not.toBeNull();
		expect(state.lifecycle.lastReport).toMatchObject({
			status: "completed",
			summary: "all green",
		});
		expect(state.lifecycle.activeCommand).toBe("mach12:pr-create");
		expect(String(result.content[0].text)).toContain("completed");
		expect(pi.appended).toContainEqual({
			customType: COMMAND_STATUS_TYPE,
			data: { commandName: "mach12:pr-create", status: "completed", summary: "all green" },
		});
	});

	it("journals the report under the active command name", async () => {
		const { pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing", "mach12:pr-create") }));
		await execute({ status: "blocked", summary: "awaiting approval" });
		expect(pi.appended).toContainEqual({
			customType: COMMAND_STATUS_TYPE,
			data: { commandName: "mach12:pr-create", status: "blocked", summary: "awaiting approval" },
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
		expect(state.lifecycle.lastReport).toMatchObject({
			next_steps: [{ message: "/mach12:issue-implement 84 3", fresh_session: true }],
			recommended_next_step: 0,
		});
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
				{ message: "/mach12:issue-review", fresh_session: false },
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
			next_steps: [
				{ message: "Ask the user which branch to use", fresh_session: false, reason: "No branch was specified" },
			],
			recommended_next_step: 0,
		});

		expect(result.terminate).toBe(true);
		expect(state.lifecycle.lastReport?.next_steps).toEqual([
			{ message: "Ask the user which branch to use", fresh_session: false, reason: "No branch was specified" },
		]);
		expect(String(result.content[0].text)).toBe("status: completed");
	});

	it("rejects a duplicate report: the second call has lastReport set and is refused", async () => {
		const { state, pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));

		const first = await execute({ status: "completed", summary: "done" });
		expect(first.terminate).toBe(true);
		expect(state.lifecycle.lastReport).not.toBeNull();

		const second = await execute({ status: "completed", summary: "done again" });
		expect(second.terminate).toBeUndefined();
		expect(second.details.error).toBe("out-of-phase");
		expect(state.lifecycle.lastReport!.summary).toBe("done");
		// The rejected duplicate produces no additional status artifact.
		expect(pi.appended.filter((e: any) => e.customType === COMMAND_STATUS_TYPE)).toHaveLength(1);
	});

	it("renders a plain status line for non-completed reports", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));
		const result = await execute({
			status: "blocked",
			summary: "missing dependency",
		});

		expect(result.terminate).toBe(true);
		expect(String(result.content[0].text)).toBe("status: blocked");
	});
});

describe("registerCommandStatusTool — probe continuing", () => {
	it("re-arms probe and does not terminate", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));
		const result = await execute({ status: "continuing", summary: "more work to do" });

		expect(result.terminate).toBeUndefined();
		expect(result.details.status).toBe("continuing");
		expect(isProbeDue(state.lifecycle)).toBe(true);
	});

	it("journals the continuing status with its incremental summary", async () => {
		const { pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing", "a:cmd") }));
		await execute({ status: "continuing", summary: "still working" });
		expect(pi.appended).toContainEqual({
			customType: COMMAND_STATUS_TYPE,
			data: { commandName: "a:cmd", status: "continuing", summary: "still working" },
		});
	});

	it("does not journal a continue-limit rejection", async () => {
		const { state, pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing", "a:cmd") }));
		state.lifecycle = lifecycleFor("probing", "a:cmd", { continueCount: 3 });
		const gen = state.lifecycleGeneration;
		const limited = await execute({ status: "continuing", summary: "still going" });
		expect(limited.details.error).toBe("continue-limit");
		expect(pi.appended.filter((e: any) => e.customType === COMMAND_STATUS_TYPE)).toHaveLength(0);
		expect(state.lifecycleGeneration).toBe(gen);
	});

	it("allows up to 3 consecutive continues then returns limit error", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));

		for (let i = 0; i < 3; i++) {
			state.lifecycle = lifecycleFor("probing", "test:cmd", { continueCount: i });
			const result = await execute({ status: "continuing", summary: "working" });
			expect(result.details.status).toBe("continuing");
		}

		state.lifecycle = lifecycleFor("probing", "test:cmd", { continueCount: 3 });
		const limited = await execute({ status: "continuing", summary: "still going" });
		expect(limited.details.error).toBe("continue-limit");
		expect(limited.content[0].text).toContain("completed");
		expect(limited.terminate).toBeUndefined();
		expect(isProbeInFlight(state.lifecycle)).toBe(true);
	});

	it("increments continueCount structurally on each continue", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));

		await execute({ status: "continuing", summary: "first" });
		expect(state.lifecycle.probeArmed).toBe(true);
		expect(state.lifecycle.continueCount).toBe(1);

		// Simulate re-entering probe (preserves count)
		state.lifecycle = lifecycleFor("probing", "test:cmd", { continueCount: 1 });
		await execute({ status: "continuing", summary: "second" });
		expect(state.lifecycle.probeArmed).toBe(true);
		expect(state.lifecycle.continueCount).toBe(2);
	});

	it("resets the counter structurally on new command start", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("probing") }));

		state.lifecycle = lifecycleFor("probing", "test:cmd", { continueCount: 3 });
		const limited = await execute({ status: "continuing", summary: "too many" });
		expect(limited.details.error).toBe("continue-limit");

		state.lifecycle = lifecycleFor("probing", "test:new-cmd", { continueCount: 0 });
		const fresh = await execute({ status: "continuing", summary: "fresh start" });
		expect(fresh.details.status).toBe("continuing");
		expect(state.lifecycle.probeArmed).toBe(true);
		expect(state.lifecycle.continueCount).toBe(1);
	});
});

describe("registerCommandStatusTool — dormant continuing", () => {
	it("accepts continuing from dormant and re-arms probe with counter at 0", async () => {
		const { state, execute } = toolFor(freshState({ lifecycle: lifecycleFor("dormant", "mach12:test") }));
		const result = await execute({ status: "continuing", summary: "resuming work" });

		expect(result.terminate).toBeUndefined();
		expect(result.details.status).toBe("continuing");
		expect(isProbeDue(state.lifecycle)).toBe(true);
		expect(state.lifecycle.continueCount).toBe(0);
		expect(state.lifecycle.activeCommand).toBe("mach12:test");
	});

	it("journals dormant continuing with its incremental summary", async () => {
		const { pi, execute } = toolFor(freshState({ lifecycle: lifecycleFor("dormant", "mach12:test") }));
		await execute({ status: "continuing", summary: "resuming" });
		expect(pi.appended).toContainEqual({
			customType: COMMAND_STATUS_TYPE,
			data: { commandName: "mach12:test", status: "continuing", summary: "resuming" },
		});
	});

	it("allows resumption even after a prior continue-limit exhaustion", async () => {
		const state = freshState({ lifecycle: lifecycleFor("dormant", "mach12:test") });
		const { execute } = toolFor(state);

		const result = await execute({ status: "continuing", summary: "fresh engagement" });
		expect(result.details.status).toBe("continuing");
		expect(state.lifecycle.continueCount).toBe(0);
		expect(isProbeDue(state.lifecycle)).toBe(true);
	});

	it("rejects continuing from waiting (parked for input)", async () => {
		const { execute } = toolFor(freshState({ lifecycle: lifecycleFor("waiting", "mach12:test") }));
		const result = await execute({ status: "continuing", summary: "trying to resume" });

		expect(result.terminate).toBeUndefined();
		expect(result.details.error).toBe("out-of-phase");
	});
});

describe("registerDormantCommandNotice", () => {
	it("emits a dormant notice when command is dormant", async () => {
		const state = freshState({ lifecycle: lifecycleFor("dormant", "mach12:test") });
		const { pi, handlers } = toolFor(state);
		registerDormantCommandNotice(pi, state);

		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({})) as any;

		expect(result).toBeDefined();
		expect(result.systemPromptSection).toBeDefined();
		expect(result.systemPromptSection.id).toBe("scramjet:dormant-command");
		expect(result.systemPromptSection.cacheRetention).toBe("none");
		expect(typeof result.systemPromptSection.text).toBe("string");
		expect(result.systemPromptSection.text).toContain("mach12:test");
		expect(result.systemPromptSection.text).toContain("dormant");
		expect(result.systemPromptSection.text).toContain("continuing");
	});

	it("does not emit when lifecycle is idle", async () => {
		const state = freshState({ lifecycle: lifecycleFor("idle") });
		const { pi, handlers } = toolFor(state);
		registerDormantCommandNotice(pi, state);

		const handler = handlers.get("before_agent_start")![0];
		const result = await handler({});

		expect(result).toBeUndefined();
	});

	it("does not emit when command is actively running", async () => {
		const state = freshState({ lifecycle: lifecycleFor("running", "mach12:test") });
		const { pi, handlers } = toolFor(state);
		registerDormantCommandNotice(pi, state);

		const handler = handlers.get("before_agent_start")![0];
		const result = await handler({});

		expect(result).toBeUndefined();
	});

	it("does not emit when probe is in flight", async () => {
		const state = freshState({ lifecycle: lifecycleFor("probing", "mach12:test") });
		const { pi, handlers } = toolFor(state);
		registerDormantCommandNotice(pi, state);

		const handler = handlers.get("before_agent_start")![0];
		const result = await handler({});

		expect(result).toBeUndefined();
	});

	it("does not have content or message properties", async () => {
		const state = freshState({ lifecycle: lifecycleFor("dormant", "mach12:test") });
		const { pi, handlers } = toolFor(state);
		registerDormantCommandNotice(pi, state);

		const handler = handlers.get("before_agent_start")![0];
		const result = (await handler({})) as any;

		expect(result.content).toBeUndefined();
		expect(result.message).toBeUndefined();
		expect(result.systemPrompt).toBeUndefined();
	});
});

describe("buildDormantCommandNotice", () => {
	it("names the dormant command", () => {
		const notice = buildDormantCommandNotice("mach12:issue-plan");
		expect(notice).toContain("mach12:issue-plan");
	});

	it("explains that ordinary replies do not auto-resume", () => {
		const notice = buildDormantCommandNotice("test:cmd");
		expect(notice).toContain("do NOT auto-resume");
	});

	it("explains that continuing is the resume path", () => {
		const notice = buildDormantCommandNotice("test:cmd");
		expect(notice).toContain("continuing");
	});

	it("explains both paths are accepted from dormant", () => {
		const notice = buildDormantCommandNotice("test:cmd");
		expect(notice).toContain("terminal status directly");
		expect(notice).toContain("Both paths are accepted");
	});
});

describe("COMMAND_STATUS_PROBE_TYPE", () => {
	it("is a stable custom-message type string", () => {
		expect(COMMAND_STATUS_PROBE_TYPE).toBe("scramjet-command-status");
	});
});
