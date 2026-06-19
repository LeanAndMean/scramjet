import { describe, expect, it } from "vitest";
import { createLogger } from "../logger.ts";
import { registerToolCallAdvisor } from "../tool-scope-advisory.ts";
import { freshState, logMessages, recordingPi } from "./helpers.ts";

function freshRecordingState(pi: any, overrides = {}) {
	return freshState({ logger: createLogger(pi), ...overrides });
}

describe("registerToolCallAdvisor — registration", () => {
	it("registers exactly one tool_call handler", () => {
		const { pi, handlers } = recordingPi();
		registerToolCallAdvisor(pi, freshState());
		expect(handlers.has("tool_call")).toBe(true);
		expect(handlers.size).toBe(1);
	});
});

describe("registerToolCallAdvisor — advisory warnings", () => {
	it("does not warn and does not block when the stack is empty", async () => {
		const { pi, handlers } = recordingPi();
		registerToolCallAdvisor(pi, freshRecordingState(pi));
		const handler = handlers.get("tool_call")![0] as any;
		const result = await handler({ type: "tool_call", toolCallId: "x", toolName: "bash", input: {} });
		expect(logMessages(pi)).toEqual([]);
		expect(result).toBeUndefined();
	});

	it("does not warn when the active frame is unrestricted (effectiveAllowedTools undefined)", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi, {
			delegateStack: [{ commandName: "mach12:push", depth: 0 }],
		});
		registerToolCallAdvisor(pi, state);
		const handler = handlers.get("tool_call")![0] as any;
		await handler({ type: "tool_call", toolCallId: "x", toolName: "anything", input: {} });
		expect(logMessages(pi)).toEqual([]);
	});

	it("does not warn when the tool is in the active frame's allowed-tools", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi, {
			delegateStack: [{ commandName: "mach12:push", depth: 0, effectiveAllowedTools: ["Read", "Bash"] }],
		});
		registerToolCallAdvisor(pi, state);
		const handler = handlers.get("tool_call")![0] as any;
		await handler({ type: "tool_call", toolCallId: "x", toolName: "Read", input: {} });
		await handler({ type: "tool_call", toolCallId: "y", toolName: "Bash", input: {} });
		expect(logMessages(pi)).toEqual([]);
	});

	it("warns when the tool is not in the active frame's allowed-tools and includes context", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi, {
			delegateStack: [{ commandName: "mach12:push", depth: 2, effectiveAllowedTools: ["Read"] }],
		});
		registerToolCallAdvisor(pi, state);
		const handler = handlers.get("tool_call")![0] as any;
		const result = await handler({ type: "tool_call", toolCallId: "x", toolName: "Bash", input: {} });
		expect(logMessages(pi)).toHaveLength(1);
		const message = logMessages(pi)[0];
		expect(message).toContain("advisory");
		expect(message).toContain("Bash");
		expect(message).toContain("mach12:push");
		expect(message).toContain("depth=2");
		expect(message).toContain("Read");
		// Advisory only; never blocks.
		expect(result).toBeUndefined();
	});

	it("checks against the top of stack (active frame) when nested frames are present", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi, {
			delegateStack: [
				{ commandName: "outer", depth: 0, effectiveAllowedTools: ["Read", "Bash", "Edit"] },
				{ commandName: "inner", depth: 1, effectiveAllowedTools: ["Read"] },
			],
		});
		registerToolCallAdvisor(pi, state);
		const handler = handlers.get("tool_call")![0] as any;
		await handler({ type: "tool_call", toolCallId: "x", toolName: "Bash", input: {} });
		expect(logMessages(pi)).toHaveLength(1);
		const message = logMessages(pi)[0];
		expect(message).toContain("inner");
		expect(message).not.toContain("outer");
	});

	it("exempts the delegate tool itself from advisory warnings", async () => {
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi, {
			delegateStack: [{ commandName: "mach12:push", depth: 0, effectiveAllowedTools: ["Read"] }],
		});
		registerToolCallAdvisor(pi, state);
		const handler = handlers.get("tool_call")![0] as any;
		await handler({ type: "tool_call", toolCallId: "x", toolName: "delegate", input: {} });
		expect(logMessages(pi)).toEqual([]);
	});

	it("warns for every tool when the frame's effectiveAllowedTools is the empty set", async () => {
		// Empty array means the intersection was empty -- nothing is allowed.
		// Distinct from undefined ("unrestricted"). Every tool call warns.
		const { pi, handlers } = recordingPi();
		const state = freshRecordingState(pi, {
			delegateStack: [{ commandName: "mach12:push", depth: 0, effectiveAllowedTools: [] }],
		});
		registerToolCallAdvisor(pi, state);
		const handler = handlers.get("tool_call")![0] as any;
		await handler({ type: "tool_call", toolCallId: "x", toolName: "Read", input: {} });
		expect(logMessages(pi)).toHaveLength(1);
	});
});
