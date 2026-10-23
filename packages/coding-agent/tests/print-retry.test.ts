import { type AssistantMessage, createAssistantMessageEventStream, getModel } from "@leanandmean/ai";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentSession, PromptOptions, RunSettlement } from "../src/core/agent-session.js";
import type { AgentSessionRuntime } from "../src/core/agent-session-runtime.js";
import { SettingsManager } from "../src/core/settings-manager.js";
import { runPrintMode } from "../src/modes/print-mode.js";
import { createProductionInteractiveHarness } from "./helpers/interactive-harness.js";

const output = vi.hoisted(() => ({ write: vi.fn(), flush: vi.fn(async () => {}) }));
vi.mock("../src/core/output-guard.js", () => ({ writeRawStdout: output.write, flushRawStdout: output.flush }));
vi.mock("../src/utils/tools-manager.js", () => ({ ensureTool: vi.fn(async () => undefined) }));
afterEach(() => {
	vi.restoreAllMocks();
	output.write.mockClear();
});
function message(text: string): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text }],
		api: "openai-chat",
		provider: "openai",
		model: "test",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: "stop",
		timestamp: 0,
	};
}
function fixture(outcomes: (RunSettlement | undefined | Error)[]) {
	const session = {
		state: { messages: [message("STALE OR HARNESS")] },
		bindExtensions: vi.fn(async () => {}),
		subscribe: vi.fn(() => () => {}),
		prompt: vi.fn(async (_text: string, options?: PromptOptions) => {
			const result = outcomes.shift();
			if (result)
				options?.onRunSettlement?.(result instanceof Error ? Promise.reject(result) : Promise.resolve(result));
		}),
	} as unknown as AgentSession;
	const runtime = {
		session,
		setRebindSession: vi.fn(),
		dispose: vi.fn(async () => {}),
	} as unknown as AgentSessionRuntime;
	return { runtime, session };
}

describe("captured print outcomes", () => {
	it("prints the last invocation's result from real offline recovery", async () => {
		let calls = 0;
		const h = await createProductionInteractiveHarness(
			60,
			24,
			undefined,
			true,
			SettingsManager.inMemory({
				theme: "pi-dark",
				quietStartup: true,
				compaction: { enabled: false },
				retry: { enabled: true, maxRetries: 2, baseDelayMs: 1 },
			}),
			undefined,
			{
				agent: {
					initialState: { model: getModel("openai", "gpt-4o") },
					getApiKey: async () => "offline",
					streamFn: () => {
						const failed = calls++ === 1;
						const response = {
							...message(`RESULT-${calls}`),
							model: "gpt-4o",
							...(failed ? { stopReason: "error" as const, errorMessage: "503 server error" } : {}),
						};
						const stream = createAssistantMessageEventStream();
						stream.push({ type: "start", partial: response });
						stream.push({ type: "done", reason: failed ? "error" : "stop", message: response });
						return stream;
					},
				},
			},
		);
		try {
			expect(await runPrintMode(h.runtime, { mode: "text", initialMessage: "first", messages: ["second"] })).toBe(0);
			expect(calls).toBe(3);
			expect(output.write).toHaveBeenCalledExactlyOnceWith("RESULT-3\n");
		} finally {
			await h.dispose();
		}
	});

	it("waits for captured settlement after prompt return", async () => {
		const { runtime, session } = fixture([]);
		let resolve!: (value: RunSettlement) => void;
		const settlement = new Promise<RunSettlement>((done) => {
			resolve = done;
		});
		vi.mocked(session.prompt).mockImplementation(async (_text, options) => {
			options?.onRunSettlement?.(settlement);
		});
		const running = runPrintMode(runtime, { mode: "text", initialMessage: "delayed" });
		await vi.waitFor(() => expect(session.prompt).toHaveBeenCalled());
		expect(output.write).not.toHaveBeenCalled();
		resolve({ status: "completed", assistantMessage: message("DELAYED") });
		expect(await running).toBe(0);
		expect(output.write).toHaveBeenCalledExactlyOnceWith("DELAYED\n");
	});

	it("does not substitute a replacement session's output", async () => {
		const { runtime, session } = fixture([]);
		vi.mocked(session.prompt).mockImplementation(async (_text, options) => {
			options?.onRunSettlement?.(Promise.resolve({ status: "cancelled" }));
			Object.assign(runtime, { session: { state: { messages: [message("REPLACEMENT")] } } });
		});
		vi.spyOn(console, "error").mockImplementation(() => {});
		expect(await runPrintMode(runtime, { mode: "text", initialMessage: "old" })).toBe(1);
		expect(output.write).not.toHaveBeenCalled();
	});

	it("prints recovered provider text instead of the later live artifact", async () => {
		const { runtime } = fixture([{ status: "completed", assistantMessage: message("RECOVERED") }]);
		expect(await runPrintMode(runtime, { mode: "text", initialMessage: "work" })).toBe(0);
		expect(output.write).toHaveBeenCalledExactlyOnceWith("RECOVERED\n");
	});
	it("does not borrow the previous result for a handled final invocation", async () => {
		const { runtime } = fixture([{ status: "completed", assistantMessage: message("PRIOR") }, undefined]);
		expect(await runPrintMode(runtime, { mode: "text", initialMessage: "work", messages: ["/handled"] })).toBe(0);
		expect(output.write).not.toHaveBeenCalled();
	});
	it.each([
		{ status: "failed", errorMessage: "503 server error" } as RunSettlement,
		{ status: "cancelled" } as RunSettlement,
		new Error("persistence failed"),
	])("returns a nonzero actionable outcome without stale stdout: %j", async (result) => {
		const error = vi.spyOn(console, "error").mockImplementation(() => {});
		const { runtime } = fixture([result]);
		expect(await runPrintMode(runtime, { mode: "text", initialMessage: "work" })).toBe(1);
		expect(output.write).not.toHaveBeenCalled();
		expect(error.mock.calls.flat().join(" ")).toMatch(/review|try|submit/i);
	});
});
