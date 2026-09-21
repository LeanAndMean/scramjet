import { expect, it, vi } from "vitest";
import { SettingsManager } from "../src/core/settings-manager.js";
import { createProductionInteractiveHarness } from "./helpers/interactive-harness.js";

vi.mock("../src/utils/tools-manager.js", () => ({ ensureTool: vi.fn(async () => undefined) }));

it("does not authorize a newer layout using an older frame's flush", async () => {
	const h = await createProductionInteractiveHarness(
		13,
		16,
		undefined,
		true,
		SettingsManager.inMemory({
			theme: "pi-dark",
			quietStartup: true,
			...{ dockEditor: false },
		}),
	);
	const write = h.terminal.write.bind(h.terminal);
	const flush = h.terminal.flush.bind(h.terminal);
	const buffered: string[] = [];
	const gates: { released: boolean; release(): Promise<void> }[] = [];
	const activate = vi.fn();
	let finish: ((value: string) => void) | undefined;
	let second: Promise<void> | undefined;
	let outcome: Promise<unknown> | undefined;
	let writeSpy: ReturnType<typeof vi.spyOn> | undefined;
	let flushSpy: ReturnType<typeof vi.spyOn> | undefined;
	try {
		await h.emit({ type: "tool_execution_start", toolCallId: "approval", toolName: "unknown", args: {} });
		await h.frame();
		writeSpy = vi.spyOn(h.terminal, "write").mockImplementation((data) => {
			buffered.push(data);
		});
		flushSpy = vi.spyOn(h.terminal, "flush").mockImplementation(() => {
			const batch = buffered.splice(0);
			return new Promise<void>((resolve) => {
				const gate = {
					released: false,
					async release() {
						if (gate.released) return;
						gate.released = true;
						for (const data of batch) write(data);
						await flush();
						resolve();
					},
				};
				gates.push(gate);
			});
		});
		outcome = h.extensionUI
			.custom<string>(
				(_ui, _theme, _keys, done) => {
					finish = done;
					return {
						render: () => ["APPROVE"],
						invalidate() {},
						handleInput() {
							activate();
							done("approved");
						},
					};
				},
				{
					toolAttachedContext: {
						toolCallId: "approval",
						render: () => ({
							render: () => [
								"REQUIRES FULL APPROVAL CONTEXT",
								...Array.from({ length: 50 }, (_, i) => `ROW-${i}`),
							],
							invalidate() {},
						}),
					},
				},
			)
			.catch((error: Error) => error);
		await vi.waitFor(() => expect(gates).toHaveLength(1));
		h.terminal.sendInput("\r");
		expect(activate).not.toHaveBeenCalled();
		h.terminal.resize(61, 16);
		second = h.internals.ui.renderNow({ requireFlush: true });
		expect(gates).toHaveLength(2);
		await gates[0].release();
		await new Promise((resolve) => setImmediate(resolve));
		expect(gates[1].released).toBe(false);
		h.terminal.sendInput("\r");
		await new Promise((resolve) => setImmediate(resolve));
		expect(activate).not.toHaveBeenCalled();
	} finally {
		finish?.("cancelled");
		for (const gate of gates) await gate.release();
		await second;
		await outcome;
		flushSpy?.mockRestore();
		writeSpy?.mockRestore();
		await h.dispose();
		vi.restoreAllMocks();
	}
});
