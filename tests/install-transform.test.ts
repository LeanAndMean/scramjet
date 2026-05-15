import { describe, expect, it } from "vitest";
// @ts-expect-error -- .mjs has no .d.ts; vitest resolves it at runtime.
import { TransformError, transformAgentSource } from "../src/install/transform.mjs";

describe("transformAgentSource — happy paths", () => {
	it("strips `model: inherit` and rewrites inline tools array", () => {
		const input = "---\nname: a\nmodel: inherit\ntools: [Read, Bash]\n---\nbody\n";
		const output = transformAgentSource(input, "fixture") as string;
		expect(output).toBe("---\nname: a\ntools: Read, Bash\n---\nbody\n");
	});

	it("preserves model values other than `inherit` unchanged", () => {
		const inputSonnet = "---\nname: a\ntools: [Read]\nmodel: sonnet\n---\nbody\n";
		const inputOpus = "---\nname: a\ntools: [Read]\nmodel: opus\n---\nbody\n";
		expect(transformAgentSource(inputSonnet, "fixture")).toContain("model: sonnet");
		expect(transformAgentSource(inputOpus, "fixture")).toContain("model: opus");
	});

	it("rewrites a block-sequence tools list to comma form", () => {
		const input = "---\nname: a\ntools:\n  - Read\n  - Write\n---\nbody\n";
		const output = transformAgentSource(input, "fixture") as string;
		expect(output).toBe("---\nname: a\ntools: Read, Write\n---\nbody\n");
	});

	it("paren-aware splitter preserves Claude Code's `Bash(npm:*, git:*)` syntax", () => {
		const input = '---\nname: a\ntools: [Read, "Bash(npm:*, git:*)", Write]\n---\nbody\n';
		const output = transformAgentSource(input, "fixture") as string;
		expect(output).toContain("tools: Read, Bash(npm:*, git:*), Write");
	});

	it("paren-aware splitter handles mixed quotes around paren-syntax items", () => {
		// Locks the combined contract: a single-quoted Bash(...) entry next to
		// an unquoted bare name must round-trip without internal comma-mangling.
		// A future regression that splits on commas before walking parens would
		// emit `Bash(npm:*` `*)` and turn this case into invalid YAML.
		const input = "---\nname: a\ntools: ['Bash(npm:*)', Read]\n---\nbody\n";
		const output = transformAgentSource(input, "fixture") as string;
		expect(output).toContain("tools: Bash(npm:*), Read");
	});

	it("strips surrounding quotes from inline items", () => {
		const input = `---\nname: a\ntools: ['Read', "Bash"]\n---\nbody\n`;
		const output = transformAgentSource(input, "fixture") as string;
		expect(output).toContain("tools: Read, Bash");
	});

	it("strips surrounding quotes from block-sequence items", () => {
		const input = `---\nname: a\ntools:\n  - 'Read'\n  - "Bash"\n---\nbody\n`;
		const output = transformAgentSource(input, "fixture") as string;
		expect(output).toContain("tools: Read, Bash");
	});

	it("passes through files with no frontmatter, normalizing CRLF to LF", () => {
		const input = "no frontmatter here\r\nsecond line\r\n";
		const output = transformAgentSource(input, "fixture") as string;
		expect(output).toBe("no frontmatter here\nsecond line\n");
	});

	it("normalizes CRLF in the body of files that have frontmatter", () => {
		const input = "---\nname: a\ntools: [Read]\n---\nline one\r\nline two\r\n";
		const output = transformAgentSource(input, "fixture") as string;
		expect(output).toBe("---\nname: a\ntools: Read\n---\nline one\nline two\n");
	});

	it("preserves keys other than `model:` and `tools:` verbatim", () => {
		const input = "---\nname: a\ndescription: hello\ncolor: blue\n---\nbody\n";
		const output = transformAgentSource(input, "fixture") as string;
		expect(output).toContain("name: a");
		expect(output).toContain("description: hello");
		expect(output).toContain("color: blue");
	});
});

describe("transformAgentSource — idempotency", () => {
	it("running the transform twice produces byte-identical output", () => {
		const inputs = [
			"---\nname: a\nmodel: inherit\ntools: [Read, Bash]\n---\nbody\n",
			"---\nname: b\ntools:\n  - Read\n  - Write\nmodel: sonnet\n---\nbody\n",
			'---\nname: c\ntools: [Read, "Bash(npm:*, git:*)", Write]\n---\nbody\n',
			"---\nname: d\ndescription: no tools key\n---\nbody\n",
			"no frontmatter here\nsecond line\n",
		];
		for (const input of inputs) {
			const once = transformAgentSource(input, "fixture") as string;
			const twice = transformAgentSource(once, "fixture") as string;
			expect(twice).toBe(once);
		}
	});
});

describe("transformAgentSource — frontmatter detection edges", () => {
	it("file ending exactly `tools: [Read]\\n---` (no trailing newline) is not treated as frontmatter and passes through unchanged", () => {
		// The frontmatter regex requires a newline before the closing `---` and
		// optionally one after. A file that ends right at the closing `---`
		// with no trailing newline (and that has no leading `---\n` opener) is
		// not a real frontmatter block. Locking this in stops a future regex
		// loosening from accidentally accepting half-frontmatter and emitting
		// `tools:` (null) on a body that happens to look like a key.
		const input = "tools: [Read]\n---";
		const output = transformAgentSource(input, "fixture") as string;
		// No frontmatter opener -> body path: CRLF normalization only.
		expect(output).toBe("tools: [Read]\n---");
	});

	it("rewrites every `tools:` array in a malformed file with multiple tools keys", () => {
		// Locks the current line-by-line behavior. The transform was never
		// designed to handle two tools: keys in the same frontmatter block
		// (that itself violates YAML, since the second silently overrides
		// the first), but the line walker rewrites each occurrence
		// independently. If a future refactor swaps to a global single-shot
		// match, this test should fail and force an explicit decision about
		// whether to fail loud on the duplicate-key case (probably the
		// right move) or preserve the current "rewrite all" behavior.
		const input = "---\nname: a\ntools: [Read]\ntools: [Bash]\n---\nbody\n";
		const output = transformAgentSource(input, "fixture") as string;
		expect(output).toContain("tools: Read");
		expect(output).toContain("tools: Bash");
		// Neither line still carries the YAML-array brackets.
		expect(output).not.toMatch(/tools: \[/);
	});
});

describe("transformAgentSource — failure modes", () => {
	it("rejects `tools: []` (Pi cannot express 'no tools allowed')", () => {
		const input = "---\nname: a\ntools: []\n---\nbody\n";
		expect(() => transformAgentSource(input, "fixture-path")).toThrow(TransformError);
		expect(() => transformAgentSource(input, "fixture-path")).toThrow(/fixture-path/);
		expect(() => transformAgentSource(input, "fixture-path")).toThrow(/tools: \[\] cannot be represented/);
	});

	it("rejects unsupported flow-map `tools: {...}` shape", () => {
		const input = "---\nname: a\ntools: {a: 1}\n---\nbody\n";
		expect(() => transformAgentSource(input, "fixture-path")).toThrow(/unsupported tools array shape/);
	});

	it("rejects nested-array inline form", () => {
		const input = "---\nname: a\ntools: [[Read], Bash]\n---\nbody\n";
		expect(() => transformAgentSource(input, "fixture-path")).toThrow(/unsupported tools array shape/);
	});

	it("rejects a comment line inside a block sequence", () => {
		const input = "---\nname: a\ntools:\n  # forbidden\n  - Read\n---\nbody\n";
		expect(() => transformAgentSource(input, "fixture-path")).toThrow(/comment line inside tools: block sequence/);
	});

	it("rejects an empty block sequence", () => {
		const input = "---\nname: a\ntools:\n\nother: x\n---\nbody\n";
		expect(() => transformAgentSource(input, "fixture-path")).toThrow(
			/tools: block sequence at line \d+ has no items/,
		);
	});

	it("rejects a block-sequence item that contains a nested bracket", () => {
		const input = "---\nname: a\ntools:\n  - Read\n  - [nested]\n---\nbody\n";
		expect(() => transformAgentSource(input, "fixture-path")).toThrow(/unsupported nested block-sequence item/);
	});

	it("error messages always include the source label", () => {
		const cases = [
			"---\nname: a\ntools: []\n---\nbody\n",
			"---\nname: a\ntools: {a: 1}\n---\nbody\n",
			"---\nname: a\ntools:\n  # bad\n  - Read\n---\nbody\n",
		];
		for (const input of cases) {
			expect(() => transformAgentSource(input, "tagged-path/x.md")).toThrow(/tagged-path\/x\.md/);
		}
	});
});
