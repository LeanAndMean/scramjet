import { type AssistantMessage, createAssistantMessageEventStream, getModel } from "@leanandmean/ai";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SettingsManager } from "../src/core/settings-manager.js";
import { createProductionInteractiveHarness } from "./helpers/interactive-harness.js";

vi.mock("../src/utils/tools-manager.js", () => ({ ensureTool: vi.fn(async () => undefined) }));
const harnesses: Awaited<ReturnType<typeof createProductionInteractiveHarness>>[] = [];
afterEach(async () => {
	for (const h of harnesses.splice(0)) await h.dispose();
	vi.unstubAllEnvs();
	vi.restoreAllMocks();
});

async function setup(
	initialMessage?: string,
	extension?: Parameters<typeof createProductionInteractiveHarness>[2],
	failures = 2,
) {
	vi.stubEnv("SCRAMJET_OFFLINE", "1");
	let calls = 0;
	const h = await createProductionInteractiveHarness(
		90,
		30,
		extension,
		true,
		SettingsManager.inMemory({
			theme: "pi-dark",
			quietStartup: true,
			compaction: { enabled: false },
			retry: { enabled: true, maxRetries: 2, baseDelayMs: 300 },
		}),
		undefined,
		{
			initialMessage,
			agent: {
				initialState: { model: getModel("openai", "gpt-4o") },
				getApiKey: async () => "offline",
				streamFn: () => {
					const failed = calls++ < failures;
					const message: AssistantMessage = {
						role: "assistant",
						content: [{ type: "text", text: failed ? "" : "RECOVERED" }],
						api: "openai-chat",
						provider: "openai",
						model: "gpt-4o",
						usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
						stopReason: failed ? "error" : "stop",
						...(failed ? { errorMessage: "503 server error" } : {}),
						timestamp: Date.now(),
					};
					const stream = createAssistantMessageEventStream();
					stream.push({ type: "start", partial: message });
					stream.push({ type: "done", reason: failed ? "error" : "stop", message });
					return stream;
				},
			},
		},
	);
	harnesses.push(h);
	void h.startLoop();
	return { ...h, calls: () => calls };
}

describe("production input loop during recovery", () => {
	it("labels exhausted recovery separately from cancellation and leaves input usable", async () => {
		const h = await setup("initial", undefined, 3);
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("Recovery exhausted"));
		expect(h.session.isRetrying).toBe(false);
		h.extensionUI.setEditorText("new work");
		h.terminal.sendInput("\r");
		await vi.waitFor(() => expect(h.calls()).toBe(4));
	});

	it("Escape cancels the pre-classification gap without starting a retry", async () => {
		let release!: () => void;
		const gate = new Promise<void>((resolve) => {
			release = resolve;
		});
		let entered = false;
		const h = await setup("initial", (pi) => {
			pi.on("agent_end", async () => {
				entered = true;
				await gate;
			});
		});
		await vi.waitFor(() => expect(entered).toBe(true));
		expect(h.session.isRetrying).toBe(false);
		h.terminal.sendInput("\x1b");
		release();
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("Execution cancelled"));
		expect(h.calls()).toBe(1);
	});

	it("preserves a newer draft after asynchronous submission refusal", async () => {
		const h = await setup();
		let reject!: (error: Error) => void;
		vi.spyOn(h.session, "prompt").mockImplementationOnce(
			() =>
				new Promise((_resolve, fail) => {
					reject = fail;
				}),
		);
		h.extensionUI.setEditorText("rejected submission");
		h.terminal.sendInput("\r");
		await vi.waitFor(() => expect(reject).toBeDefined());
		h.extensionUI.setEditorText("NEWER DRAFT");
		reject(new Error("Automatic retry is in progress"));
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("Automatic retry"));
		expect(h.extensionUI.getEditorText()).toBe("NEWER DRAFT");
	});

	it("restores a refused submission when no newer draft exists", async () => {
		const h = await setup();
		vi.spyOn(h.session, "prompt").mockRejectedValueOnce(new Error("Automatic retry is in progress"));
		h.extensionUI.setEditorText("REJECTED");
		h.terminal.sendInput("\r");
		await vi.waitFor(() => expect(h.extensionUI.getEditorText()).toBe("REJECTED"));
	});

	it("drops old retry controls on session replacement and accepts new work", async () => {
		const h = await setup("initial");
		await vi.waitFor(() => expect(h.session.isRetrying).toBe(true));
		await h.runtime.newSession();
		expect(h.runtime.session).not.toBe(h.session);
		h.extensionUI.setEditorText("new session work");
		h.terminal.sendInput("\r");
		await vi.waitFor(() => expect(h.calls()).toBeGreaterThan(1));
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("RECOVERED"));
	});

	it.each(["\r", "\x1b\r"])(
		"retains %j submission during initial-message recovery, cancels and accepts new work",
		async (key) => {
			const h = await setup("initial");
			await vi.waitFor(() => expect(h.session.isRetrying).toBe(true));
			h.extensionUI.setEditorText("RETAIN ME");
			h.terminal.sendInput(key);
			await h.frame();
			expect(h.extensionUI.getEditorText()).toBe("RETAIN ME");
			expect(h.session.getSteeringMessages()).toEqual([]);
			h.terminal.sendInput("\x1b");
			await vi.waitFor(() => expect(h.session.isRetrying).toBe(false));
			await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("Execution cancelled"));
			expect((await h.frame()).join("\n")).not.toContain("Retry failed");
			h.extensionUI.setEditorText("next");
			h.terminal.sendInput("\r");
			await vi.waitFor(() => expect(h.calls()).toBe(3));
			await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("RECOVERED"));
		},
	);

	it("retains normal-loop backoff input across multiple attempts and restores controls after recovery", async () => {
		const h = await setup();
		h.extensionUI.setEditorText("first");
		h.terminal.sendInput("\r");
		await vi.waitFor(() => expect(h.session.isRetrying).toBe(true));
		h.extensionUI.setEditorText("DRAFT");
		h.terminal.sendInput("\r");
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
		await vi.waitFor(() => expect(h.calls()).toBe(3));
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("RECOVERED"));
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
		h.extensionUI.setEditorText("next");
		h.terminal.sendInput("\r");
		await vi.waitFor(() => expect(h.calls()).toBe(4));
	});
});
