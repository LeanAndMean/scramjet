import { expect, it } from "vitest";
import { Editor } from "../src/components/editor.js";
import { Text } from "../src/components/text.js";
import { TUI } from "../src/tui.js";
import { HeadlessTerminal } from "./helpers/headless-terminal.js";

it("observes curly underline and explicit resets in terminal cells", async () => {
	const terminal = new HeadlessTerminal();
	terminal.write("\x1b[4:3mA\x1b[24mB\x1b[4:3mC\x1b[0mD");
	await terminal.flush();
	expect([0, 1, 2, 3].map((col) => terminal.cell(0, col).underline)).toEqual([true, false, true, false]);
});

it("contains wrapped curly underlines to text, not padding", async () => {
	const terminal = new HeadlessTerminal(8, 4);
	const tui = new TUI(terminal);
	const text = new Text("\x1b[4:3mabc def\x1b[24m", 1, 0);
	tui.configureViewport({ getBlocks: () => [{ component: text }] });
	tui.start();
	try {
		await tui.renderNow({ requireFlush: true });
		expect(terminal.cell(0, 1)).toMatchObject({ text: "a", underline: true });
		expect.soft(terminal.cell(0, 4)).toMatchObject({ text: " ", underline: false });
		expect.soft(terminal.cell(1, 1)).toMatchObject({ text: "d", underline: true });
	} finally {
		tui.stop();
	}
});

it.each([true, false])(
	"clears spellcheck underlines after text replacement and scrolling with dock=%s",
	async (dock) => {
		const terminal = new HeadlessTerminal(40, 10);
		const tui = new TUI(terminal);
		const editor = new Editor(tui, {
			borderColor: (s) => s,
			selectList: {
				selectedPrefix: (s) => s,
				selectedText: (s) => s,
				description: (s) => s,
				scrollInfo: (s) => s,
				noMatch: (s) => s,
			},
			spellcheckError: (s) => s,
		});
		let misspelled = true;
		editor.setSpellcheckProvider({
			onUpdate: null,
			textChanged() {},
			getMisspelledRanges: () => (misspelled ? [{ start: 0, end: 5 }] : []),
		});
		const history = new Text(Array.from({ length: 40 }, (_, i) => `plain row ${i}`).join("\n"), 0, 0);
		tui.configureViewport({ getBlocks: () => [{ component: history }, { component: editor, dock }] });
		tui.setFocus(editor);
		editor.setText("wrold");
		tui.start();
		const underlined = () =>
			Array.from({ length: terminal.rows }, (_, row) =>
				Array.from({ length: terminal.columns }, (_, col) => terminal.cell(row, col).underline),
			).flat();
		try {
			await tui.renderNow({ requireFlush: true });
			expect(underlined().some(Boolean)).toBe(true);
			misspelled = false;
			editor.setText("world");
			await tui.renderNow({ requireFlush: true });
			expect(underlined().some(Boolean)).toBe(false);
			tui.scrollViewportTo(0);
			await tui.renderNow({ requireFlush: true });
			expect(terminal.visibleLines().join("\n")).toContain("plain row 0");
			expect(underlined().some(Boolean)).toBe(false);
			tui.followViewport();
			await tui.renderNow({ requireFlush: true });
			expect(terminal.visibleLines().join("\n")).toContain("world");
			expect(underlined().some(Boolean)).toBe(false);
		} finally {
			tui.stop();
		}
	},
);
