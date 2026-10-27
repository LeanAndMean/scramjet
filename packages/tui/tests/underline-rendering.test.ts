import { afterEach, describe, expect, it, vi } from "vitest";
import { Editor } from "../src/components/editor.js";
import { Text } from "../src/components/text.js";
import { TUI } from "../src/tui.js";
import { HeadlessTerminal } from "./helpers/headless-terminal.js";

describe("Editor spelling underline selection", () => {
	afterEach(() => vi.unstubAllEnvs());

	const identities: Array<[string, Record<string, string>, "single" | "curly"]> = [
		["Apple Terminal", { TERM_PROGRAM: "Apple_Terminal" }, "single"],
		["XTerm 390", { TERM: "xterm", XTERM_VERSION: "XTerm(390)" }, "single"],
		["XTerm 412", { TERM: "xterm-256color", XTERM_VERSION: "XTerm(412)" }, "single"],
		["generic TERM", { TERM: "xterm-256color" }, "curly"],
		["missing identity", {}, "curly"],
		["empty identity", { TERM_PROGRAM: "", XTERM_VERSION: "" }, "curly"],
		["malformed XTerm marker", { TERM: "xterm", XTERM_VERSION: "XTerm(390)stale" }, "curly"],
		["nonnumeric XTerm version", { TERM: "xterm", XTERM_VERSION: "XTerm(next)" }, "curly"],
		["missing XTerm TERM", { XTERM_VERSION: "XTerm(390)" }, "curly"],
		["nonmatching TERM", { TERM: "vt100", XTERM_VERSION: "XTerm(390)" }, "curly"],
		["other TERM_PROGRAM", { TERM: "xterm", TERM_PROGRAM: "WezTerm", XTERM_VERSION: "XTerm(390)" }, "curly"],
		["Kitty", { TERM: "xterm-kitty", KITTY_WINDOW_ID: "1" }, "curly"],
		["Kitty with stale XTerm marker", { TERM: "xterm-kitty", XTERM_VERSION: "XTerm(390)" }, "curly"],
		["Apple precedence", { TERM_PROGRAM: "Apple_Terminal", XTERM_VERSION: "XTerm(390)" }, "single"],
		["empty mux variables", { TERM_PROGRAM: "Apple_Terminal", TMUX: "", STY: "" }, "single"],
		...["TMUX", "STY"].flatMap((key): typeof identities => [
			[`${key} excludes Apple`, { TERM_PROGRAM: "Apple_Terminal", [key]: "active" }, "curly"],
			[`${key} excludes XTerm`, { TERM: "xterm", XTERM_VERSION: "XTerm(390)", [key]: "active" }, "curly"],
		]),
		...["tmux", "tmux-256color", "screen", "screen-256color"].map((TERM): (typeof identities)[number] => [
			TERM,
			{ TERM, TERM_PROGRAM: "Apple_Terminal", XTERM_VERSION: "XTerm(390)" },
			"curly",
		]),
	];

	function makeEditor(env: Record<string, string>) {
		for (const key of ["TERM", "TERM_PROGRAM", "XTERM_VERSION", "TMUX", "STY", "KITTY_WINDOW_ID"]) {
			vi.stubEnv(key, env[key]);
		}
		const editor = new Editor(new TUI(new HeadlessTerminal(40, 10)), {
			borderColor: (s) => s,
			selectList: {
				selectedPrefix: (s) => s,
				selectedText: (s) => s,
				description: (s) => s,
				scrollInfo: (s) => s,
				noMatch: (s) => s,
			},
			spellcheckError: (s) => `\x1b[31m${s}\x1b[39m`,
		});
		editor.setSpellcheckProvider({
			onUpdate: null,
			textChanged() {},
			getMisspelledRanges: (line) => (line === 0 ? [{ start: 0, end: 3 }] : []),
		});
		return editor;
	}

	it.each(identities)("selects the expected opener for %s", (_name, env, style) => {
		const editor = makeEditor(env);
		editor.setText("teh correct text");
		const output = editor.render(40).join("\n");
		const opener = style === "single" ? "\x1b[4m" : "\x1b[4:3m";
		expect(output).toContain(`${opener}\x1b[31mteh\x1b[39m\x1b[24m correct text`);
		expect(output).not.toContain(style === "single" ? "\x1b[4:3m" : "\x1b[4m");
		expect(output).not.toContain("\x1b[49m");
	});

	it.each(["start", "inside", "end", "noncursor"])("passes single style through the %s render path", (cursor) => {
		const editor = makeEditor({ TERM_PROGRAM: "Apple_Terminal" });
		editor.setText(cursor === "noncursor" ? "teh correct text\nnext line" : "teh correct text");
		if (cursor === "start" || cursor === "inside") {
			editor.handleInput("\x01");
			if (cursor === "inside") editor.handleInput("\x1b[C");
		}
		const output = editor.render(40).join("\n");
		expect(output).not.toContain("\x1b[4:3m");
		if (cursor === "start") {
			expect(output).toContain("\x1b[7mt\x1b[0m\x1b[4m\x1b[31meh\x1b[39m\x1b[24m correct text");
		} else if (cursor === "inside") {
			expect(output).toContain(
				"\x1b[4m\x1b[31mt\x1b[39m\x1b[24m\x1b[7me\x1b[0m\x1b[4m\x1b[31mh\x1b[39m\x1b[24m correct text",
			);
		} else {
			expect(output).toContain("\x1b[4m\x1b[31mteh\x1b[39m\x1b[24m correct text");
		}
	});
});

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
			spellcheckError: (s) => `\x1b[31m${s}\x1b[39m`,
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
		editor.setText("wrold correct text");
		tui.start();
		const underlined = () =>
			Array.from({ length: terminal.rows }, (_, row) =>
				Array.from({ length: terminal.columns }, (_, col) => terminal.cell(row, col).underline),
			).flat();
		try {
			await tui.renderNow({ requireFlush: true });
			expect(underlined().some(Boolean)).toBe(true);
			const editorRow = terminal.visibleLines().findIndex((line) => line.startsWith("wrold correct text"));
			expect(editorRow).toBeGreaterThanOrEqual(0);
			for (let col = 0; col < terminal.columns; col++) {
				expect(terminal.cell(editorRow, col)).toMatchObject({ underline: col < 5, background: undefined });
			}
			misspelled = false;
			editor.setText("world correct text");
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
