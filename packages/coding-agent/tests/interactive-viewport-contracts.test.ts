import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AssistantMessage } from "@leanandmean/ai";
import { getKeybindings, Text } from "@leanandmean/tui";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type Settings, SettingsManager } from "../src/core/settings-manager.js";
import * as clipboard from "../src/utils/clipboard.js";
import { createProductionInteractiveHarness } from "./helpers/interactive-harness.js";

vi.mock("../src/utils/tools-manager.js", () => ({ ensureTool: vi.fn(async () => undefined) }));

const harnesses: Awaited<ReturnType<typeof createProductionInteractiveHarness>>[] = [];
const directories: string[] = [];
const mouse = (button: number, x = 2, y = 2) => `\x1b[<${button};${x};${y}M`;

function settings(values: Record<string, unknown> = {}) {
	return SettingsManager.inMemory({
		theme: "pi-dark",
		quietStartup: true,
		compaction: { enabled: false },
		retry: { enabled: false },
		...values,
	});
}

async function setup(rows = 24, manager = settings(), columns = 60) {
	const h = await createProductionInteractiveHarness(columns, rows, undefined, true, manager);
	harnesses.push(h);
	h.extensionUI.setFooter(() => new Text("FIXTURE-FOOTER", 0, 0));
	return h;
}

async function history(h: Awaited<ReturnType<typeof setup>>) {
	h.internals.committedChatContainer.addChild(
		new Text(Array.from({ length: 100 }, (_, i) => `HISTORY-${String(i).padStart(3, "0")}`).join("\n"), 0, 0),
	);
	h.extensionUI.setEditorText("DRAFT");
	await h.frame();
}

async function openSettings(h: Awaited<ReturnType<typeof setup>>, query: string) {
	h.extensionUI.setEditorText("/settings");
	h.terminal.sendInput("\r");
	await h.frame();
	for (const char of query) h.terminal.sendInput(char);
	await h.frame();
}

afterEach(async () => {
	for (const h of harnesses.splice(0)) await h.dispose();
	for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true });
	vi.restoreAllMocks();
});

describe("retained interactive contracts", () => {
	it("keeps the default dock and draft visible while browsing and typing", async () => {
		const h = await setup();
		await history(h);
		h.internals.ui.scrollViewportTo(0);
		let frame = await h.frame();
		expect(frame.join("\n")).toContain("HISTORY-000");
		expect(frame.join("\n")).toContain("DRAFT");
		expect(frame.at(-1)).toContain("FIXTURE-FOOTER");
		h.terminal.sendInput("x");
		frame = await h.frame();
		expect(h.extensionUI.getEditorText()).toBe("DRAFTx");
		expect(frame.join("\n")).toContain("HISTORY-000");
		expect(h.internals.ui.getViewportState()!.followingTail).toBe(false);
	});

	it("keeps a transcript selection from crossing into the dock", async () => {
		const copy = vi.spyOn(clipboard, "copyToClipboard").mockResolvedValue();
		const h = await setup();
		await history(h);
		h.internals.ui.scrollViewportTo(0);
		const frame = await h.frame();
		expect(frame.at(-1)).toContain("FIXTURE-FOOTER");
		const height = h.internals.ui.getViewportState()!.height;
		expect(height).toBeLessThan(h.terminal.rows);
		h.terminal.sendInput("\x1b[<0;1;1M");
		h.terminal.sendInput(`\x1b[<32;20;${h.terminal.rows}M`);
		h.terminal.sendInput(`\x1b[<0;20;${h.terminal.rows}m`);
		await h.frame();
		h.terminal.sendInput("\x03");
		await h.frame();
		expect(copy).toHaveBeenCalledOnce();
		const text = copy.mock.calls[0][0];
		expect(text).toBe(
			frame
				.slice(0, height)
				.map((line, index) => line.slice(0, index === height - 1 ? 19 : h.terminal.columns - 1))
				.join("\n"),
		);
		expect(text).not.toContain("DRAFT");
		expect(text).not.toContain("FIXTURE-FOOTER");
	});

	it("reclaims a safe scrolling layout for an oversized dock without losing draft text", async () => {
		const h = await setup();
		await history(h);
		h.extensionUI.setWidget(
			"large",
			() => new Text(Array.from({ length: 40 }, (_, i) => `WIDGET-${i}`).join("\n"), 0, 0),
		);
		const frame = await h.frame();
		expect(frame.join("\n")).toMatch(/dock.*(space|fit|suspend)/i);
		expect(frame.join("\n")).toContain("DRAFT");
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
		const seen = new Set<number>();
		for (let step = 0; step < 80; step++) {
			for (const line of await h.frame()) {
				const match = /WIDGET-(\d+)/.exec(line);
				if (match) seen.add(Number(match[1]));
			}
			if (h.internals.ui.getViewportState()!.offset === 0) break;
			h.terminal.sendInput(mouse(64));
		}
		expect([...seen].sort((a, b) => a - b)).toEqual(Array.from({ length: 40 }, (_, index) => index));
		h.extensionUI.setWidget("large", undefined);
		h.internals.ui.scrollViewportTo(0);
		const restored = await h.frame();
		expect(restored.join("\n")).toContain("HISTORY-000");
		expect(restored.at(-1)).toContain("FIXTURE-FOOTER");
		expect(restored.join("\n")).toContain("DRAFT");
	});

	it("enters transcript browsing from the tail using only the configured keyboard default", async () => {
		const h = await setup();
		await history(h);
		const before = h.internals.ui.getViewportState()!;
		expect(before.followingTail).toBe(true);
		h.terminal.sendInput("\x1b[5;3~");
		await h.frame();
		expect(h.internals.ui.getViewportState()!.offset).toBeLessThan(before.offset);
		expect(h.internals.ui.getViewportState()!.followingTail).toBe(false);
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
	});

	it.each(["\x0f", "\x14"])("preserves the reading anchor through presentation input %j", async (key) => {
		const h = await setup(24, settings({ dockEditor: false }));
		await history(h);
		if (key === "\x0f") {
			await h.emit({
				type: "tool_execution_start",
				toolCallId: "expand",
				toolName: "bash",
				args: { command: "synthetic" },
			});
			await h.emit({
				type: "tool_execution_end",
				toolCallId: "expand",
				isError: false,
				result: {
					content: [{ type: "text", text: Array.from({ length: 40 }, (_, i) => `DETAIL-${i}`).join("\n") }],
				},
			});
		} else {
			const message: AssistantMessage = {
				role: "assistant",
				content: [
					{ type: "thinking", thinking: "THINKING-CONTENT" },
					{ type: "text", text: "ANSWER" },
				],
				api: "anthropic-messages",
				provider: "anthropic",
				model: "synthetic",
				stopReason: "stop",
				timestamp: 0,
				usage: {
					input: 0,
					output: 0,
					cacheRead: 0,
					cacheWrite: 0,
					totalTokens: 0,
					cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
				},
			};
			await h.emit({ type: "message_start", message });
			await h.emit({ type: "message_end", message });
		}
		const component = h.internals.committedChatContainer.children.at(-1)!;
		const presentation = component.render(59);
		h.internals.ui.scrollViewportTo(10);
		const before = await h.frame();
		h.terminal.sendInput(key);
		const after = await h.frame();
		expect(component.render(59)).not.toEqual(presentation);
		expect(after[0]).toBe(before[0]);
		expect(h.internals.ui.getViewportState()!.followingTail).toBe(false);
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
	});

	it("uses the selected wheel step without modifying editor input", async () => {
		const h = await setup(24, settings({ dockEditor: false, scrollWheelStep: 7 }));
		await history(h);
		h.internals.ui.scrollViewportTo(30);
		await h.frame();
		h.terminal.sendInput(mouse(64));
		await h.frame();
		expect(h.internals.ui.getViewportState()!.offset).toBe(23);
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
	});

	it.each([
		{ rows: 40, percent: 10, count: 4 },
		{ rows: 40, percent: 50, count: 20 },
		{ rows: 10, percent: 30, count: 3 },
		{ rows: 40, percent: 0, count: 4 },
		{ rows: 40, percent: 100, count: 20 },
	])("bounds input rows at $percent percent of $rows terminal rows", async ({ rows, percent, count }) => {
		const h = await setup(rows, settings({ dockEditor: false, editorMaxHeightPercent: percent }));
		const draft = Array.from({ length: 50 }, (_, i) => `INPUT-${String(i).padStart(3, "0")}`).join("\n");
		h.extensionUI.setEditorText(draft);
		const frame = await h.frame();
		expect(frame.filter((line) => line.includes("INPUT-")).length).toBe(count);
		expect(h.extensionUI.getEditorText()).toBe(draft);
		h.terminal.sendInput("\x1b[5~");
		await h.frame();
		h.terminal.sendInput("X");
		const changed = h.extensionUI.getEditorText().split("\n");
		expect(changed.findIndex((line) => line.includes("X"))).toBe(49 - count);
	});

	it("selects committed mode at real startup without alternate-screen entry", async () => {
		const h = await setup(24, settings({ tuiMode: "committed" }));
		expect(h.internals.ui.getViewportState()).toBeUndefined();
		expect(h.terminal.writes.join("")).not.toContain("\x1b[?1049h");
		h.internals.committedChatContainer.addChild(new Text("COMMITTED-ROW", 0, 0));
		h.internals.ui.commit();
		await h.frame();
		expect(h.terminal.bufferLines().join("\n")).toContain("COMMITTED-ROW");
	});

	it("exposes live docking in the real settings selector and persists the choice", async () => {
		const directory = mkdtempSync(join(tmpdir(), "scramjet-viewport-settings-"));
		directories.push(directory);
		const agentDir = join(directory, "agent");
		mkdirSync(agentDir);
		const file = join(agentDir, "settings.json");
		writeFileSync(file, JSON.stringify({ theme: "pi-dark", quietStartup: true }));
		const manager = SettingsManager.create(directory, agentDir);
		const h = await setup(24, manager);
		await history(h);
		await openSettings(h, "dock");
		expect(h.terminal.visibleLines().join("\n")).toContain("Dock input area");
		h.terminal.sendInput("\r");
		await h.frame();
		await manager.flush();
		expect(JSON.parse(readFileSync(file, "utf8"))).toMatchObject({ dockEditor: false, theme: "pi-dark" });
		h.terminal.sendInput("\x1b");
		await h.frame();
		h.internals.ui.scrollViewportTo(0);
		expect((await h.frame()).join("\n")).not.toContain("FIXTURE-FOOTER");
	});

	it.each(["context", "controls"])("rejects incomplete approval %s without authorizing", async (part) => {
		const h = await setup(12, settings({ dockEditor: false }));
		await h.emit({ type: "tool_execution_start", toolCallId: "approval", toolName: "unknown", args: {} });
		const tool = h.internals.chatContainer.children[0];
		let finish!: (value: string) => void;
		let state: "pending" | "approved" | "rejected" | "cancelled" = "pending";
		const activate = vi.fn();
		const outcome = h.extensionUI
			.custom<string>(
				(_tui, _theme, _keys, done) => {
					finish = done;
					return {
						render: () => [part === "controls" ? "CONTROL-".repeat(20) : "APPROVE OR CANCEL"],
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
							invalidate() {},
							render: () => [
								part === "context" ? "PRIVATE-CONTEXT-".repeat(20) : "COMPLETE CONTEXT",
								...Array.from({ length: 50 }, (_, i) => `PAYLOAD-${i}`),
							],
						}),
					},
				},
			)
			.then(
				(value) => {
					state = value as typeof state;
				},
				() => {
					state = "rejected";
				},
			);
		try {
			await vi.waitFor(() => expect(state === "rejected" || h.internals.ui.isComponentFocused(tool)).toBe(true));
			await h.frame();
			h.terminal.sendInput("\r");
			await vi.waitFor(() => expect(state).not.toBe("pending"));
			expect(activate).not.toHaveBeenCalled();
			expect(state).toBe("rejected");
		} finally {
			finish?.("cancelled");
			await outcome;
		}
	});

	it("keeps autocomplete usable across repeated oversized-dock fallback renders", async () => {
		const h = await setup();
		for (const character of "/hot") h.terminal.sendInput(character);
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("→ hotkeys"));
		h.extensionUI.setWidget(
			"oversized",
			() => new Text(Array.from({ length: 40 }, (_, i) => `LARGE-${i}`).join("\n"), 0, 0),
		);
		for (let pass = 0; pass < 3; pass++) {
			const frame = await h.frame();
			expect(frame.join("\n")).toContain("→ hotkeys");
			expect(frame.join("\n")).toMatch(/dock.*suspended/i);
		}
		h.terminal.sendInput("\t");
		await h.frame();
		expect(h.extensionUI.getEditorText()).toMatch(/^\/hotkeys/);
	});

	it("shrinks the editor before suspending a dock that can still fit", async () => {
		const h = await setup(24, settings({ editorMaxHeightPercent: 50 }));
		await history(h);
		const draft = Array.from({ length: 30 }, (_, index) => `INPUT-${index}`).join("\n");
		h.extensionUI.setEditorText(draft);
		h.extensionUI.setWidget(
			"adjacent",
			() => new Text(Array.from({ length: 10 }, (_, index) => `BAND-${index}`).join("\n"), 0, 0),
			{ placement: "belowEditor" },
		);
		const frame = await h.frame();
		expect(frame.join("\n")).not.toMatch(/dock.*suspended/i);
		expect(frame.filter((line) => line.includes("INPUT-")).length).toBe(8);
		for (let index = 0; index < 10; index++) expect(frame.join("\n")).toContain(`BAND-${index}`);
		expect(frame.at(-1)).toContain("FIXTURE-FOOTER");
		expect(h.internals.ui.getViewportState()!.height).toBe(2);
		expect(h.extensionUI.getEditorText()).toBe(draft);
		h.extensionUI.setWidget("adjacent", undefined);
		expect((await h.frame()).filter((line) => line.includes("INPUT-")).length).toBe(12);
	});

	it("gives focused settings navigation precedence over a viewport paging remap", async () => {
		const h = await setup();
		await history(h);
		getKeybindings().setUserBindings({ "tui.viewport.pageUp": "up" });
		await openSettings(h, "");
		const before = h.internals.ui.getViewportState()!.offset;
		h.terminal.sendInput("\x1b[A");
		const frame = await h.frame();
		expect(h.internals.ui.getViewportState()!.offset).toBe(before);
		expect(frame.find((line) => line.includes("Wheel scroll lines"))).toContain("→ Wheel scroll lines");
	});

	it.each([
		{ query: "wheel", key: "scrollWheelStep", initial: 3 },
		{ query: "height", key: "editorMaxHeightPercent", initial: 30 },
		{ query: "dock", key: "dockEditor", initial: true },
	])("shows an unsaved warning when a live $key write fails", async ({ query, key, initial }) => {
		let durable = JSON.stringify({ theme: "pi-dark", quietStartup: true, [key]: initial });
		let failWrites = false;
		const manager = SettingsManager.fromStorage({
			withLock(scope, update) {
				const next = update(scope === "global" ? durable : undefined);
				if (next !== undefined && scope === "global") {
					if (failWrites)
						throw Object.assign(new Error("permission denied (synthetic EACCES)"), { code: "EACCES" });
					durable = next;
				}
			},
		});
		const h = await setup(24, manager);
		await manager.flush();
		failWrites = true;
		await openSettings(h, query);
		h.terminal.sendInput("\r");
		await manager.flush();
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).toMatch(/not saved|unsaved/i));
		expect(JSON.parse(durable)[key]).toBe(initial);
		expect(manager.getGlobalSettings()[key as keyof Settings]).not.toBe(initial);
	});

	it("changes wheel scrolling through settings and restores the saved value on restart", async () => {
		const directory = mkdtempSync(join(tmpdir(), "scramjet-wheel-settings-"));
		directories.push(directory);
		const agentDir = join(directory, "agent");
		mkdirSync(agentDir);
		writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ theme: "pi-dark", quietStartup: true }));
		const manager = SettingsManager.create(directory, agentDir);
		const h = await setup(24, manager);
		await history(h);
		await openSettings(h, "wheel");
		expect(h.terminal.visibleLines().join("\n")).toContain("Wheel scroll lines");
		for (let step = 0; step < 4; step++) {
			h.terminal.sendInput("\r");
			await h.frame();
		}
		await manager.flush();
		expect(JSON.parse(readFileSync(join(agentDir, "settings.json"), "utf8")).scrollWheelStep).toBe(7);
		expect((await h.frame()).join("\n")).not.toMatch(/not saved|unsaved/i);
		h.terminal.sendInput("\x1b");
		await h.frame();
		h.internals.ui.scrollViewportTo(30);
		await h.frame();
		h.terminal.sendInput(mouse(64));
		await h.frame();
		expect(h.internals.ui.getViewportState()!.offset).toBe(23);
		const restarted = await setup(24, SettingsManager.create(directory, agentDir));
		await history(restarted);
		restarted.internals.ui.scrollViewportTo(30);
		await restarted.frame();
		restarted.terminal.sendInput(mouse(64));
		await restarted.frame();
		expect(restarted.internals.ui.getViewportState()!.offset).toBe(23);
	});

	it("keeps project precedence explicit when a live editor-height edit saves globally", async () => {
		const directory = mkdtempSync(join(tmpdir(), "scramjet-project-layout-"));
		directories.push(directory);
		const agentDir = join(directory, "agent");
		mkdirSync(agentDir);
		mkdirSync(join(directory, ".scramjet"));
		const projectFile = join(directory, ".scramjet", "settings.json");
		const project = JSON.stringify({ editorMaxHeightPercent: 20 });
		writeFileSync(projectFile, project);
		writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ theme: "pi-dark", quietStartup: true }));
		const manager = SettingsManager.create(directory, agentDir);
		const h = await setup(40, manager);
		await openSettings(h, "height");
		expect(h.terminal.visibleLines().join("\n")).toContain("Project settings override");
		h.terminal.sendInput("\r");
		const frame = await h.frame();
		await manager.flush();
		expect(frame.join("\n")).toContain("20%");
		expect(JSON.parse(readFileSync(join(agentDir, "settings.json"), "utf8")).editorMaxHeightPercent).toBe(25);
		expect(readFileSync(projectFile, "utf8")).toBe(project);
		h.terminal.sendInput("\x1b");
		h.extensionUI.setEditorText(Array.from({ length: 30 }, (_, i) => `INPUT-${i}`).join("\n"));
		expect((await h.frame()).filter((line) => line.includes("INPUT-")).length).toBe(8);
	});

	it("keeps the selected setting visible when undocking while reading", async () => {
		const h = await setup();
		await history(h);
		h.extensionUI.setWidget("above", ["ABOVE"]);
		h.extensionUI.setWidget("below", ["BELOW"], { placement: "belowEditor" });
		h.internals.ui.scrollViewportTo(0);
		await h.frame();
		await openSettings(h, "dock");
		h.terminal.sendInput("\r");
		const frame = await h.frame();
		expect(frame.join("\n")).toContain("Dock input area");
		expect(frame.join("\n")).toContain("false");
	});

	it.each(["\x1b", "\x1b[27u"])(
		"cancels focused settings with the first Escape %j after undocking",
		async (cancelKey) => {
			const h = await setup();
			await history(h);
			h.extensionUI.setWidget("above", ["ABOVE"]);
			h.extensionUI.setWidget("below", ["BELOW"], { placement: "belowEditor" });
			h.internals.ui.scrollViewportTo(0);
			await h.frame();
			await openSettings(h, "dock");
			h.terminal.sendInput("\r");
			await h.frame();
			h.terminal.sendInput(cancelKey);
			await h.frame();
			h.terminal.sendInput("x");
			expect(h.extensionUI.getEditorText()).toBe("x");
		},
	);

	it.each(["\x1b", "\x1b[27u"])(
		"cancels a focused selector while independently detached using %j",
		async (cancelKey) => {
			const h = await setup(24, settings({ dockEditor: false }));
			await history(h);
			await openSettings(h, "dock");
			h.internals.ui.scrollViewportTo(0);
			await h.frame();
			expect(h.internals.ui.getViewportState()!.followingTail).toBe(false);
			h.terminal.sendInput(cancelKey);
			await h.frame();
			h.terminal.sendInput("x");
			expect(h.extensionUI.getEditorText()).toBe("x");
		},
	);

	it("keeps settings usable in a narrow terminal while toggling the dock", async () => {
		const h = await setup(12, settings(), 24);
		await openSettings(h, "dock");
		let frame = await h.frame();
		expect(frame.join("\n")).toContain("Dock input area");
		h.terminal.sendInput("\r");
		frame = await h.frame();
		expect(frame.join("\n")).toContain("Dock input area");
		expect(frame.join("\n")).toContain("false");
		h.terminal.sendInput("\x1b");
		await h.frame();
		h.terminal.sendInput("x");
		expect(h.extensionUI.getEditorText()).toBe("x");
	});

	it("keeps the docked editor cursor on its painted row while reading history", async () => {
		const h = await setup();
		await history(h);
		h.internals.ui.scrollViewportTo(0);
		await h.frame();
		h.terminal.sendInput("x");
		const frame = await h.frame();
		expect(h.terminal.cursorPosition().row).toBe(frame.findIndex((line) => line.includes("DRAFTx")));
		expect(frame[0]).toContain("HISTORY-000");
	});

	it("preserves invalid settings files while exposing their load error", async () => {
		const directory = mkdtempSync(join(tmpdir(), "scramjet-invalid-layout-"));
		directories.push(directory);
		const agentDir = join(directory, "agent");
		mkdirSync(agentDir);
		const file = join(agentDir, "settings.json");
		writeFileSync(file, "{broken-json");
		const manager = SettingsManager.create(directory, agentDir);
		expect(manager.drainErrors()).toEqual([expect.objectContaining({ scope: "global", error: expect.any(Error) })]);
		const h = await setup(24, manager);
		await openSettings(h, "wheel");
		h.terminal.sendInput("\r");
		await h.frame();
		await manager.flush();
		expect(readFileSync(file, "utf8")).toBe("{broken-json");
	});
});
