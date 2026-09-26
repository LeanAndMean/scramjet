import { afterEach, describe, expect, it, vi } from "vitest";
import { CombinedAutocompleteProvider } from "../src/autocomplete.js";
import { Editor } from "../src/components/editor.js";
import { getKeybindings } from "../src/keybindings.js";
import { StdinBuffer } from "../src/stdin-buffer.js";
import type { TUI } from "../src/tui.js";

const identity = (text: string) => text;
const fixtures: { editor: Editor; input: StdinBuffer }[] = [];

function setup() {
	const editor = new Editor({ requestRender: vi.fn() } as unknown as TUI, {
		borderColor: identity,
		selectList: {
			selectedPrefix: identity,
			selectedText: identity,
			description: identity,
			scrollInfo: identity,
			noMatch: identity,
		},
	});
	const provider = new CombinedAutocompleteProvider([{ name: "settings" }], "/tmp");
	const apply = vi.spyOn(provider, "applyCompletion");
	editor.setAutocompleteProvider(provider);
	const submit = vi.fn();
	editor.onSubmit = submit;
	const input = new StdinBuffer();
	input.on("data", (data) => editor.handleInput(data));
	fixtures.push({ editor, input });
	return { editor, input, submit, apply, provider };
}

afterEach(() => {
	for (const { editor, input } of fixtures.splice(0)) {
		editor.setText("");
		input.destroy();
	}
	getKeybindings().setUserBindings({});
	vi.restoreAllMocks();
});

describe("autocomplete acceptance currentness", () => {
	it.each(["\r", "\x1b[13u"])("submits the typed command instead of applying a stale prefix on %j", async (enter) => {
		const f = setup();
		f.input.process("/settin");
		await vi.waitFor(() => expect(f.editor.isShowingAutocomplete()).toBe(true));
		f.input.process(`gs${enter}`);
		expect(f.submit).toHaveBeenCalledExactlyOnceWith("/settings");
		expect(f.apply).not.toHaveBeenCalled();
		expect(f.editor.getText()).toBe("");
	});

	it.each(["/set", "/settings"])("still accepts a current command completion for %s", async (prefix) => {
		const f = setup();
		f.input.process(prefix);
		await vi.waitFor(() => expect(f.editor.isShowingAutocomplete()).toBe(true));
		f.input.process("\r");
		expect(f.submit).toHaveBeenCalledExactlyOnceWith("/settings");
		expect(f.apply).toHaveBeenCalledOnce();
	});

	it("submits normally if no autocomplete response has installed a menu", () => {
		const f = setup();
		f.input.process("/settings\r");
		expect(f.submit).toHaveBeenCalledExactlyOnceWith("/settings");
	});

	it("refreshes stale Tab completion against the current draft before accepting it", async () => {
		const f = setup();
		f.input.process("/settin");
		await vi.waitFor(() => expect(f.editor.isShowingAutocomplete()).toBe(true));
		f.input.process("gs\t");
		expect(f.apply).not.toHaveBeenCalled();
		expect(f.editor.getText()).toBe("/settings");
		await vi.waitFor(() => expect(f.editor.isShowingAutocomplete()).toBe(true));
		f.input.process("\t");
		expect(f.editor.getText()).toBe("/settings ");
		expect(f.apply).toHaveBeenCalledOnce();
		expect(f.submit).not.toHaveBeenCalled();
	});

	it("does not fall through a stale remapped confirmation to an unrelated editor action", async () => {
		const f = setup();
		getKeybindings().setUserBindings({ "tui.select.confirm": "ctrl+y" });
		f.editor.setText("KILLED");
		f.input.process("\x15");
		f.input.process("/settin");
		await vi.waitFor(() => expect(f.editor.isShowingAutocomplete()).toBe(true));
		f.input.process("gs\x19");
		expect(f.editor.getText()).toBe("/settings");
		expect(f.apply).not.toHaveBeenCalled();
		expect(f.submit).not.toHaveBeenCalled();
	});

	it.each([
		{ input: "\x1b[D", expected: "/settin" },
		{ input: "\x7fX", expected: "/settiX" },
	])("rejects completion after cursor-only or same-length edits: $expected", async ({ input, expected }) => {
		const f = setup();
		f.input.process("/settin");
		await vi.waitFor(() => expect(f.editor.isShowingAutocomplete()).toBe(true));
		f.input.process(`${input}\r`);
		expect(f.submit).toHaveBeenCalledExactlyOnceWith(expected);
		expect(f.apply).not.toHaveBeenCalled();
	});

	it("does not reinstall a late response after stale-menu submission", async () => {
		const f = setup();
		f.input.process("/settin");
		await vi.waitFor(() => expect(f.editor.isShowingAutocomplete()).toBe(true));
		let resolve!: (value: { prefix: string; items: { value: string; label: string }[] }) => void;
		const response = new Promise<{ prefix: string; items: { value: string; label: string }[] }>((done) => {
			resolve = done;
		});
		const suggestions = vi.spyOn(f.provider, "getSuggestions").mockReturnValue(response);
		f.input.process("gs");
		await vi.waitFor(() => expect(suggestions).toHaveBeenCalledOnce());
		f.input.process("\r");
		resolve({ prefix: "/settings", items: [{ value: "settings", label: "settings" }] });
		await response;
		await Promise.resolve();
		expect(f.submit).toHaveBeenCalledExactlyOnceWith("/settings");
		expect(f.editor.isShowingAutocomplete()).toBe(false);
		expect(f.editor.getText()).toBe("");
	});

	it.each([1, 2])("preserves current file completion with %s results", async (count) => {
		const f = setup();
		vi.spyOn(f.provider, "getSuggestions").mockResolvedValue({
			prefix: "fi",
			items: Array.from({ length: count }, (_, i) => ({ value: `file-${i}.ts`, label: `file-${i}.ts` })),
		});
		f.editor.setText("fi");
		f.input.process("\t");
		if (count === 2) {
			await vi.waitFor(() => expect(f.editor.isShowingAutocomplete()).toBe(true));
			f.input.process("\r");
		} else await vi.waitFor(() => expect(f.apply).toHaveBeenCalledOnce());
		expect(f.editor.getText()).toBe("file-0.ts");
		expect(f.apply).toHaveBeenCalledOnce();
		expect(f.submit).not.toHaveBeenCalled();
	});

	it("honors a confirmation key that is also explicitly bound to submit", async () => {
		const f = setup();
		getKeybindings().setUserBindings({ "tui.select.confirm": "f6", "tui.input.submit": "f6" });
		f.input.process("/settin");
		await vi.waitFor(() => expect(f.editor.isShowingAutocomplete()).toBe(true));
		f.input.process("gs\x1b[17~");
		expect(f.submit).toHaveBeenCalledExactlyOnceWith("/settings");
		expect(f.apply).not.toHaveBeenCalled();
	});

	it("does not bypass disabled submission when discarding stale completion", async () => {
		const f = setup();
		f.editor.disableSubmit = true;
		f.input.process("/settin");
		await vi.waitFor(() => expect(f.editor.isShowingAutocomplete()).toBe(true));
		f.input.process("gs\r");
		expect(f.editor.getText()).toBe("/settings");
		expect(f.submit).not.toHaveBeenCalled();
		expect(f.apply).not.toHaveBeenCalled();
	});
});
