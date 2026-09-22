import { Text } from "@leanandmean/tui";
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

it.each(["widget", "footer"])("requires the current approval paint after a same-size %s change", async (change) => {
	const h = await createProductionInteractiveHarness(
		60,
		16,
		undefined,
		true,
		SettingsManager.inMemory({
			theme: "pi-dark",
			quietStartup: true,
			dockEditor: true,
		}),
	);
	const write = h.terminal.write.bind(h.terminal);
	const flush = h.terminal.flush.bind(h.terminal);
	const buffered: string[] = [];
	const gates: { batch: string[]; released: boolean; release(): Promise<void> }[] = [];
	const activate = vi.fn();
	const context = new Text(Array.from({ length: 40 }, (_, index) => `CONTEXT-${index}`).join("\n"), 0, 0);
	let finish: ((value: string) => void) | undefined;
	let outcome: Promise<unknown> | undefined;
	let newerPaint: Promise<void> | undefined;
	let settled = false;
	let writeSpy: ReturnType<typeof vi.spyOn> | undefined;
	let flushSpy: ReturnType<typeof vi.spyOn> | undefined;
	try {
		h.extensionUI.setWidget("above", () => new Text("WIDGET-A", 0, 0));
		h.extensionUI.setWidget("below", () => new Text("BELOW", 0, 0), { placement: "belowEditor" });
		h.extensionUI.setFooter(() => new Text("FOOTER-A", 0, 0));
		h.extensionUI.setEditorText("DRAFT");
		await h.emit({ type: "tool_execution_start", toolCallId: "approval", toolName: "unknown", args: {} });
		const tool = h.internals.chatContainer.children[0];
		await h.frame();
		expect(h.internals.ui.getViewportState()!.height).toBeLessThan(16);
		writeSpy = vi.spyOn(h.terminal, "write").mockImplementation((data) => {
			buffered.push(data);
		});
		flushSpy = vi.spyOn(h.terminal, "flush").mockImplementation(() => {
			const batch = buffered.splice(0);
			return new Promise<void>((resolve) => {
				const gate = {
					batch,
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
						handleInput(data) {
							if (data === "\r") {
								activate();
								done("approved");
							}
						},
					};
				},
				{ toolAttachedContext: { toolCallId: "approval", render: () => context } },
			)
			.then(
				(value) => {
					settled = true;
					return value;
				},
				(error: Error) => {
					settled = true;
					return error;
				},
			);
		await vi.waitFor(() => expect(gates.length).toBeGreaterThan(0));
		const first = gates[0];
		expect(first.batch.join("")).toContain("APPROVE");
		expect(first.batch.join("")).toContain("WIDGET-A");
		expect(first.batch.join("")).toContain("FOOTER-A");
		expect(h.internals.editorContainer.children).toHaveLength(0);
		expect(h.internals.ui.getViewportState()!.height).toBe(16);
		if (change === "widget") h.extensionUI.setWidget("above", () => new Text("WIDGET-B\nWIDGET-C\nWIDGET-D", 0, 0));
		else h.extensionUI.setFooter(() => new Text("FOOTER-B\nFOOTER-C\nFOOTER-D", 0, 0));
		newerPaint = h.internals.ui.renderNow({ requireFlush: true });
		const second = gates.at(-1)!;
		expect(second).not.toBe(first);
		expect(second.batch.join("")).toContain(change === "widget" ? "WIDGET-D" : "FOOTER-D");
		expect(second.batch).not.toEqual(first.batch);
		expect([h.terminal.columns, h.terminal.rows]).toEqual([60, 16]);
		await first.release();
		await new Promise((resolve) => setImmediate(resolve));
		expect(h.terminal.visibleLines()[11].slice(0, 59).trimEnd()).toBe("APPROVE");
		expect(second.released).toBe(false);
		expect(h.internals.ui.hasOverlay()).toBe(false);
		expect(h.internals.ui.isComponentVisible(tool)).toBe(true);
		expect(h.internals.ui.isComponentRenderComplete(tool)).toBe(true);
		expect(h.internals.ui.isComponentRenderComplete(context)).toBe(true);
		expect(settled).toBe(false);
		h.terminal.sendInput("\r");
		await new Promise((resolve) => setImmediate(resolve));
		expect(activate).not.toHaveBeenCalled();
		expect(settled).toBe(false);
		expect(h.internals.ui.isViewportFrameFlushed()).toBe(false);
		for (const gate of gates) await gate.release();
		await newerPaint;
		flushSpy.mockRestore();
		writeSpy.mockRestore();
		await h.frame();
		await vi.waitFor(() => expect(h.internals.ui.isComponentFocused(tool)).toBe(true));
		expect(h.terminal.visibleLines()[9].slice(0, 59).trimEnd()).toBe("APPROVE");
		expect(h.internals.ui.isViewportFrameFlushed()).toBe(true);
		expect([h.terminal.columns, h.terminal.rows]).toEqual([60, 16]);
		h.terminal.sendInput("\r");
		expect(await outcome).toBe("approved");
		expect(activate).toHaveBeenCalledOnce();
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
	} finally {
		finish?.("cancelled");
		for (const gate of gates) await gate.release();
		await newerPaint;
		await outcome;
		flushSpy?.mockRestore();
		writeSpy?.mockRestore();
		await h.dispose();
		vi.restoreAllMocks();
	}
});

async function attachedInput(handleInput: (data: string) => void, text: () => string) {
	const h = await createProductionInteractiveHarness(
		60,
		16,
		undefined,
		true,
		SettingsManager.inMemory({
			theme: "pi-dark",
			quietStartup: true,
			...{ dockEditor: false },
		}),
	);
	await h.emit({ type: "tool_execution_start", toolCallId: "approval", toolName: "unknown", args: {} });
	const tool = h.internals.chatContainer.children[0];
	let finish!: (value: string) => void;
	const outcome = h.extensionUI
		.custom<string>(
			(_ui, _theme, _keys, done) => {
				finish = done;
				return { render: () => [text()], invalidate() {}, handleInput };
			},
			{
				toolAttachedContext: {
					toolCallId: "approval",
					render: () => ({ render: () => ["COMPLETE CONTEXT"], invalidate() {} }),
				},
			},
		)
		.catch((error: Error) => error);
	await vi.waitFor(() => expect(h.internals.ui.isComponentFocused(tool)).toBe(true));
	await h.frame();
	return {
		h,
		async close() {
			finish("cancelled");
			await outcome;
			await h.dispose();
		},
	};
}

it("keeps approval gated until an overlay dismissal reaches the terminal", async () => {
	const activate = vi.fn();
	const attached = await attachedInput(
		(data) => {
			if (data === "\r") activate();
		},
		() => "APPROVE",
	);
	const { h } = attached;
	const write = h.terminal.write.bind(h.terminal);
	const flush = h.terminal.flush.bind(h.terminal);
	const buffered: string[] = [];
	const releases: (() => Promise<void>)[] = [];
	let writeSpy: ReturnType<typeof vi.spyOn> | undefined;
	let flushSpy: ReturnType<typeof vi.spyOn> | undefined;
	try {
		const overlay = h.internals.ui.showOverlay(
			{
				render: () => Array(16).fill("OVERLAY STILL ON TERMINAL"),
				invalidate() {},
				handleInput() {},
			},
			{ row: 0, col: 0, width: 59, maxHeight: 16 },
		);
		await h.frame();
		expect(h.terminal.visibleLines().join("\n")).toContain("OVERLAY STILL ON TERMINAL");
		writeSpy = vi.spyOn(h.terminal, "write").mockImplementation((data) => {
			buffered.push(data);
		});
		flushSpy = vi.spyOn(h.terminal, "flush").mockImplementation(() => {
			const batch = buffered.splice(0);
			return new Promise<void>((resolve) =>
				releases.push(async () => {
					for (const data of batch) write(data);
					await flush();
					resolve();
				}),
			);
		});
		overlay.hide();
		await vi.waitFor(() =>
			expect(writeSpy!.mock.calls.some(([data]) => String(data).includes("\x1b[?2026h"))).toBe(true),
		);
		expect(h.terminal.visibleLines().join("\n")).toContain("OVERLAY STILL ON TERMINAL");
		h.terminal.sendInput("\r");
		expect(activate).not.toHaveBeenCalled();
	} finally {
		for (const release of releases) await release();
		for (const data of buffered) write(data);
		await flush();
		flushSpy?.mockRestore();
		writeSpy?.mockRestore();
		await attached.close();
		vi.restoreAllMocks();
	}
});

it("delivers successive attached-control edits after ordinary scheduled paints", async () => {
	let value = "";
	const attached = await attachedInput(
		(data) => {
			value += data;
		},
		() => `TEXT: ${value}`,
	);
	try {
		for (const key of "abc") {
			attached.h.terminal.sendInput(key);
			expect(value).toBe("abc".slice(0, "abc".indexOf(key) + 1));
			await vi.waitFor(() => expect(attached.h.terminal.visibleLines().join("\n")).toContain(`TEXT: ${value}`));
		}
	} finally {
		await attached.close();
		vi.restoreAllMocks();
	}
});
