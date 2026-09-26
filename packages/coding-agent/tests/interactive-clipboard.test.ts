import { type EditorComponent, Text } from "@leanandmean/tui";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { SettingsManager } from "../src/core/settings-manager.js";
import * as clipboard from "../src/utils/clipboard.js";
import { createProductionInteractiveHarness } from "./helpers/interactive-harness.js";

vi.mock("../src/utils/tools-manager.js", () => ({ ensureTool: vi.fn(async () => undefined) }));

let h: Awaited<ReturnType<typeof createProductionInteractiveHarness>>;

beforeEach(async () => {
	h = await createProductionInteractiveHarness(
		60,
		24,
		undefined,
		true,
		SettingsManager.inMemory({ theme: "pi-dark", quietStartup: true, dockEditor: true }),
	);
	h.extensionUI.setFooter(() => new Text("FOOTER", 0, 0));
	h.extensionUI.setEditorText("DRAFT");
});

afterEach(async () => {
	await h.dispose();
	vi.restoreAllMocks();
});

async function requestPaste() {
	const row = (await h.frame()).findIndex((line) => line.includes("DRAFT"));
	expect(row).toBeGreaterThanOrEqual(0);
	h.terminal.sendInput(`\x1b[<2;3;${row + 1}M`);
	h.terminal.sendInput(`\x1b[<2;3;${row + 1}m`);
}

it.each(["edit and revert", "selector round trip", "reset", "overlay", "unrelated right-click"])(
	"cancels all shared paste requests after %s",
	async (action) => {
		let resolve!: (text: string) => void;
		const read = vi.spyOn(clipboard, "readClipboardText").mockImplementationOnce(
			() =>
				new Promise((done) => {
					resolve = done;
				}),
		);
		await requestPaste();
		await requestPaste();
		expect(read).toHaveBeenCalledOnce();
		let close: (() => void) | undefined;
		if (action === "edit and revert") {
			h.extensionUI.setEditorText("edited");
			h.extensionUI.setEditorText("DRAFT");
		} else if (action === "selector round trip") {
			const answer = h.extensionUI.confirm("Wait", "Temporary");
			await h.frame();
			h.terminal.sendInput("\x1b");
			await answer;
		} else if (action === "reset") h.internals.clearTranscript();
		else if (action === "overlay") {
			const overlay = h.internals.ui.showOverlay(new Text("OVERLAY"));
			close = () => overlay.hide();
		} else h.terminal.sendInput(`\x1b[<2;2;${h.terminal.rows}M`);
		try {
			await h.frame();
			resolve("STALE");
			await h.frame();
			expect(h.extensionUI.getEditorText()).toBe("DRAFT");
		} finally {
			close?.();
		}
		read.mockResolvedValueOnce("FRESH");
		await requestPaste();
		await vi.waitFor(() => expect(h.extensionUI.getEditorText()).toBe("DRAFTFRESH"));
		expect(read).toHaveBeenCalledTimes(2);
	},
);

it("reports a shared clipboard failure once and permits a fresh read afterward", async () => {
	let reject!: (error: Error) => void;
	const read = vi.spyOn(clipboard, "readClipboardText").mockImplementationOnce(
		() =>
			new Promise((_resolve, fail) => {
				reject = fail;
			}),
	);
	const editor = h.internals.editorContainer.children[0] as EditorComponent;
	const submit = vi.spyOn(editor, "onSubmit");
	await requestPaste();
	await requestPaste();
	await requestPaste();
	expect(read).toHaveBeenCalledOnce();
	reject(new Error("synthetic clipboard unavailable"));
	await h.frame();
	const text = (await h.frame()).join("\n");
	expect(text.match(/Paste failed: synthetic clipboard unavailable/g)).toHaveLength(1);
	expect(h.extensionUI.getEditorText()).toBe("DRAFT");
	read.mockResolvedValueOnce("FRESH\nsecond line");
	await requestPaste();
	await vi.waitFor(() => expect(h.extensionUI.getEditorText()).toBe("DRAFTFRESH\nsecond line"));
	expect(read).toHaveBeenCalledTimes(2);
	expect(submit).not.toHaveBeenCalled();
});
