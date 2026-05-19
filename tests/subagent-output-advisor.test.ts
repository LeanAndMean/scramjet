import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerSubagentOutputAdvisor } from "../subagent-output-advisor.ts";
import { freshState, recordingPi } from "./helpers.ts";

describe("registerSubagentOutputAdvisor — registration", () => {
	it("registers exactly one tool_result handler", () => {
		const { pi, handlers } = recordingPi();
		registerSubagentOutputAdvisor(pi, freshState());
		expect(handlers.has("tool_result")).toBe(true);
		expect(handlers.size).toBe(1);
	});
});

describe("registerSubagentOutputAdvisor — (no output) detection", () => {
	let warnSpy: ReturnType<typeof vi.spyOn>;

	beforeEach(() => {
		warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
	});

	afterEach(() => {
		warnSpy.mockRestore();
	});

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
		const state = freshState();
		const { pi, handlers } = recordingPi();
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(makeResult());

		expect(warnSpy).toHaveBeenCalledTimes(1);
		const message = String(warnSpy.mock.calls[0][0]);
		expect(message).toContain("[scramjet]");
		expect(message).toContain("advisory");
		expect(message).toContain("mach12:code-reviewer");
		expect(state.sidebarLog).toHaveLength(1);
		expect(state.sidebarLog[0].command).toContain("mach12:code-reviewer");
		expect(state.sidebarLog[0].command).toContain("no output");
		expect(state.sidebarLog[0].origin).toBe("agent");
	});

	it("does not fire when the subagent returned real content", async () => {
		const state = freshState();
		const { pi, handlers } = recordingPi();
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(makeResult({ content: [{ type: "text", text: "Findings: nothing suspicious" }] }));

		expect(warnSpy).not.toHaveBeenCalled();
		expect(state.sidebarLog).toEqual([]);
	});

	it("does not fire on (no output) from a different tool", async () => {
		const state = freshState();
		const { pi, handlers } = recordingPi();
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(makeResult({ toolName: "bash" }));

		expect(warnSpy).not.toHaveBeenCalled();
		expect(state.sidebarLog).toEqual([]);
	});

	it("does not fire when the error path embeds (no output) in a longer message", async () => {
		// Upstream's error path emits e.g. "Agent failed: (no output)" — that
		// already surfaces an error in the calling agent's context. The advisor
		// targets only the silent success path (exact match on "(no output)").
		const state = freshState();
		const { pi, handlers } = recordingPi();
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(makeResult({ content: [{ type: "text", text: "Agent failed: (no output)" }], isError: true }));

		expect(warnSpy).not.toHaveBeenCalled();
		expect(state.sidebarLog).toEqual([]);
	});

	it("extracts the agent name from chain-mode input", async () => {
		const state = freshState();
		const { pi, handlers } = recordingPi();
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

		expect(warnSpy).toHaveBeenCalledTimes(1);
		const message = String(warnSpy.mock.calls[0][0]);
		expect(message).toContain("chain ending in mach12:planner");
		expect(state.sidebarLog).toHaveLength(1);
		expect(state.sidebarLog[0].command).toContain("chain ending in mach12:planner");
	});

	it("falls back to <unknown> when the input shape lacks a recognizable agent name", async () => {
		const state = freshState();
		const { pi, handlers } = recordingPi();
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(makeResult({ input: { tasks: [{ agent: "scout", task: "x" }] } }));

		expect(warnSpy).toHaveBeenCalledTimes(1);
		expect(String(warnSpy.mock.calls[0][0])).toContain("<unknown>");
	});

	it("records depth from the active delegate stack", async () => {
		const state = freshState({
			delegateStack: [
				{ commandName: "outer", depth: 0 },
				{ commandName: "inner", depth: 1 },
			],
		});
		const { pi, handlers } = recordingPi();
		registerSubagentOutputAdvisor(pi, state);
		const handler = handlers.get("tool_result")![0] as any;

		await handler(makeResult());

		expect(state.sidebarLog[0].depth).toBe(2);
	});
});
