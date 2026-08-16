import { describe, expect, it } from "vitest";
import { Markdown, type MarkdownTheme } from "../src/components/markdown.js";
import { resetCapabilitiesCache, setCapabilities } from "../src/terminal-image.js";

const identity = (text: string) => text;
const theme: MarkdownTheme = {
	heading: identity,
	link: identity,
	linkUrl: identity,
	code: identity,
	codeBlock: identity,
	codeBlockBorder: identity,
	quote: identity,
	quoteBorder: identity,
	hr: identity,
	listBullet: identity,
	bold: identity,
	italic: identity,
	strikethrough: identity,
	underline: identity,
};

describe("untrusted Markdown", () => {
	it("renders Markdown naturally, hides HTML comments, and keeps unsupported HTML inert", () => {
		const input = [
			"<!-- mach12-review -->",
			"# Review",
			"",
			"**bold** and [docs](https://example.com/docs)",
			"",
			"- first",
			"- second",
			"",
			"| A | B |",
			"| - | - |",
			"| 1 | 2 |",
			"",
			"<details>unsupported</details>",
			"",
			"```html",
			"<!-- visible in code -->",
			"```",
		].join("\n");

		const output = new Markdown(input, 0, 0, theme, undefined, { contentMode: "untrusted" }).render(100).join("\n");

		expect(output).toContain("Review");
		expect(output).toContain("bold");
		expect(output).toContain("docs (https://example.com/docs)");
		expect(output).toContain("- first");
		expect(output).toContain("│ A");
		expect(output).toContain("<details>unsupported</details>");
		expect(output).not.toContain("mach12-review");
		expect(output).toContain("<!-- visible in code -->");
		expect(output).not.toContain("⟦");
	});

	it("keeps links visible but inactive on hyperlink-capable terminals", () => {
		setCapabilities({ images: undefined, trueColor: true, hyperlinks: true });
		try {
			const output = new Markdown("[docs](https://example.com/docs)", 0, 0, theme, undefined, {
				contentMode: "untrusted",
			})
				.render(100)
				.join("\n");
			expect(output).toContain("docs (https://example.com/docs)");
			expect(output).not.toContain("\u001b]8;;");
		} finally {
			resetCapabilitiesCache();
		}
	});

	it("neutralizes terminal controls in nested Markdown contexts", () => {
		const hostile = "\u001b]8;;https://evil.example\u0007PAYLOAD\u001b]8;;\u0007\u202e\u2066\uffff";
		const input = [
			`# heading ${hostile}`,
			`- item ${hostile}`,
			`| cell |`,
			`| - |`,
			`| ${hostile} |`,
			`[link ${hostile}](https://example.com/${hostile})`,
			`\`${hostile}\``,
			"```",
			hostile,
			"```",
		].join("\n");

		const output = new Markdown(input, 0, 0, theme, undefined, { contentMode: "untrusted" }).render(120).join("\n");

		expect(output).toContain("PAYLOAD");
		expect(output).not.toMatch(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069\ufffe\uffff]/u);
		expect(output).not.toContain("\u001b]8;;");
	});
});
