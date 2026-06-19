import { describe, expect, it } from "vitest";
import { createLogger } from "../logger.ts";
import { registerSubagentOutputAdvisor } from "../subagent-output-advisor.ts";
import { freshState, logMessages, recordingPi } from "./helpers.ts";

function freshRecordingState(pi: any, overrides = {}) {
	return freshState({ logger: createLogger(pi), ...overrides });
}

describe("registerSubagentOutputAdvisor — registration", () => {
	it("registers exactly one tool_result handler", () => {
		const { pi, handlers } = recordingPi();
		registerSubagentOutputAdvisor(pi, freshState());
		expect(handlers.has("tool_result")).toBe(true);
		expect(handlers.size).toBe(1);
	});
});

describe("registerSubagentOutputAdvisor — (no output) detection", () => {
	function makeResult(overrides: Record<string, unknown> = {}) {
		return {
			type: "tool_result",
			toolCallId: "x",
			toolName: "subagent",
			input: { agent: "mach12:code-reviewer", task: "review this" },
			content: [{ type: "text", text: "(no output)" }],
			isError: false,
			details: undefined,
			...overrides,
		};
	}

	it("warns and pushes a sidebar entry on a literal (no output) result from subagent", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi);
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(makeResult());

		expect(logMessages(pi)).toHaveLength(1);
		const message = logMessages(pi)[0];
		expect(message).toContain("advisory");
		expect(message).toContain("mach12:code-reviewer");
		expect(state.sidebarLog).toHaveLength(1);
		expect(state.sidebarLog[0].command).toContain("mach12:code-reviewer");
		expect(state.sidebarLog[0].command).toContain("no output");
		expect(state.sidebarLog[0].origin).toBe("agent");
	});

	it("does not fire when the subagent returned real content", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi);
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(makeResult({ content: [{ type: "text", text: "Findings: nothing suspicious" }] }));

		expect(logMessages(pi)).toEqual([]);
		expect(state.sidebarLog).toEqual([]);
	});

	it("does not fire on (no output) from a different tool", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi);
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(makeResult({ toolName: "bash" }));

		expect(logMessages(pi)).toEqual([]);
		expect(state.sidebarLog).toEqual([]);
	});

	it("does not fire when the error path embeds (no output) in a longer message", async () => {
		// Upstream's error path emits e.g. "Agent failed: (no output)" — that
		// already surfaces an error in the calling agent's context. The advisor
		// targets only the silent success path (exact match on "(no output)").
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi);
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(makeResult({ content: [{ type: "text", text: "Agent failed: (no output)" }], isError: true }));

		expect(logMessages(pi)).toEqual([]);
		expect(state.sidebarLog).toEqual([]);
	});

	it("extracts the agent name from chain-mode input", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi);
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(
			makeResult({
				input: {
					chain: [
						{ agent: "mach12:scout", task: "step 1" },
						{ agent: "mach12:planner", task: "step 2" },
					],
				},
			}),
		);

		expect(logMessages(pi)).toHaveLength(1);
		const message = logMessages(pi)[0];
		expect(message).toContain("chain ending in mach12:planner");
		expect(state.sidebarLog).toHaveLength(1);
		expect(state.sidebarLog[0].command).toContain("chain ending in mach12:planner");
	});

	it("falls back to <unknown> when the input shape lacks a recognizable agent name", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi);
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(makeResult({ input: { tasks: [{ agent: "scout", task: "x" }] } }));

		expect(logMessages(pi)).toHaveLength(1);
		expect(logMessages(pi)[0]).toContain("<unknown>");
	});

	it("records depth from the active delegate stack", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi, {
			delegateStack: [
				{ commandName: "outer", depth: 0 },
				{ commandName: "inner", depth: 1 },
			],
		});
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(makeResult());

		expect(state.sidebarLog[0].depth).toBe(2);
	});
});
