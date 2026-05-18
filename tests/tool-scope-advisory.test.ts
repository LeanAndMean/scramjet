import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerToolCallAdvisor } from "../tool-scope-advisory.ts";
import type { ScramjetState } from "../types.ts";

function freshState(overrides: Partial<ScramjetState> = {}): ScramjetState {
	return {
		enabled: false,
		registry: new Map(),
		activeTopLevelCommand: null,
		sidebarLog: [],
		delegateStack: [],
		...overrides,
	};
}

function recordingPi() {
	const handlers = new Map<string, (event: unknown) => unknown>();
	const pi: any = {
		on(event: string, handler: (event: unknown) => unknown) {
			handlers.set(event, handler);
		},
	};
	return { pi, handlers };
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
	let warnSpy: ReturnType<typeof vi.spyOn>;

	beforeEach(() => {
		warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
	});

	afterEach(() => {
		warnSpy.mockRestore();
	});

	it("does not warn and does not block when the stack is empty", async () => {
		const { pi, handlers } = recordingPi();
		registerToolCallAdvisor(pi, freshState());
		const handler = handlers.get("tool_call") as any;
		const result = await handler({ type: "tool_call", toolCallId: "x", toolName: "bash", input: {} });
		expect(warnSpy).not.toHaveBeenCalled();
		expect(result).toBeUndefined();
	});

	it("does not warn when the active frame is unrestricted (effectiveAllowedTools undefined)", async () => {
		const state = freshState({
			delegateStack: [{ commandName: "mach12:push", depth: 0 }],
		});
		const { pi, handlers } = recordingPi();
		registerToolCallAdvisor(pi, state);
		const handler = handlers.get("tool_call") as any;
		await handler({ type: "tool_call", toolCallId: "x", toolName: "anything", input: {} });
		expect(warnSpy).not.toHaveBeenCalled();
	});

	it("does not warn when the tool is in the active frame's allowed-tools", async () => {
		const state = freshState({
			delegateStack: [{ commandName: "mach12:push", depth: 0, effectiveAllowedTools: ["Read", "Bash"] }],
		});
		const { pi, handlers } = recordingPi();
		registerToolCallAdvisor(pi, state);
		const handler = handlers.get("tool_call") as any;
		await handler({ type: "tool_call", toolCallId: "x", toolName: "Read", input: {} });
		await handler({ type: "tool_call", toolCallId: "y", toolName: "Bash", input: {} });
		expect(warnSpy).not.toHaveBeenCalled();
	});

	it("warns when the tool is not in the active frame's allowed-tools and includes context", async () => {
		const state = freshState({
			delegateStack: [{ commandName: "mach12:push", depth: 2, effectiveAllowedTools: ["Read"] }],
		});
		const { pi, handlers } = recordingPi();
		registerToolCallAdvisor(pi, state);
		const handler = handlers.get("tool_call") as any;
		const result = await handler({ type: "tool_call", toolCallId: "x", toolName: "Bash", input: {} });
		expect(warnSpy).toHaveBeenCalledTimes(1);
		const message = String(warnSpy.mock.calls[0][0]);
		expect(message).toContain("[scramjet]");
		expect(message).toContain("advisory");
		expect(message).toContain("Bash");
		expect(message).toContain("mach12:push");
		expect(message).toContain("depth=2");
		expect(message).toContain("Read");
		// Advisory only; never blocks.
		expect(result).toBeUndefined();
	});

	it("checks against the top of stack (active frame) when nested frames are present", async () => {
		const state = freshState({
			delegateStack: [
				{ commandName: "outer", depth: 0, effectiveAllowedTools: ["Read", "Bash", "Edit"] },
				{ commandName: "inner", depth: 1, effectiveAllowedTools: ["Read"] },
			],
		});
		const { pi, handlers } = recordingPi();
		registerToolCallAdvisor(pi, state);
		const handler = handlers.get("tool_call") as any;
		await handler({ type: "tool_call", toolCallId: "x", toolName: "Bash", input: {} });
		expect(warnSpy).toHaveBeenCalledTimes(1);
		const message = String(warnSpy.mock.calls[0][0]);
		expect(message).toContain("inner");
		expect(message).not.toContain("outer");
	});

	it("exempts the delegate tool itself from advisory warnings", async () => {
		const state = freshState({
			delegateStack: [{ commandName: "mach12:push", depth: 0, effectiveAllowedTools: ["Read"] }],
		});
		const { pi, handlers } = recordingPi();
		registerToolCallAdvisor(pi, state);
		const handler = handlers.get("tool_call") as any;
		await handler({ type: "tool_call", toolCallId: "x", toolName: "delegate", input: {} });
		expect(warnSpy).not.toHaveBeenCalled();
	});

	it("warns for every tool when the frame's effectiveAllowedTools is the empty set", async () => {
		// Empty array means the intersection was empty -- nothing is allowed.
		// Distinct from undefined ("unrestricted"). Every tool call warns.
		const state = freshState({
			delegateStack: [{ commandName: "mach12:push", depth: 0, effectiveAllowedTools: [] }],
		});
		const { pi, handlers } = recordingPi();
		registerToolCallAdvisor(pi, state);
		const handler = handlers.get("tool_call") as any;
		await handler({ type: "tool_call", toolCallId: "x", toolName: "Read", input: {} });
		expect(warnSpy).toHaveBeenCalledTimes(1);
	});
});
